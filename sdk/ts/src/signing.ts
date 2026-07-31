/**
 * Callback verification and receipt chain verification (protocol §15, signing.md).
 *
 * Two independent schemes, answering different questions:
 *
 * - **Callback signatures** — HMAC-SHA-256 over a canonical string, verified by the receiver with
 *   a shared secret. Implemented here in full, on WebCrypto alone — no dependency, and no Node
 *   built-in, so the same code runs on Node, Deno, Bun, and Workers. Hashing is therefore async.
 * - **Receipt integrity** — a per-tenant hash chain (the MUST) with an optional detached Ed25519
 *   signature (a MAY). The chain is implemented here.
 *
 * ## The claim a signature makes, and the one it does not
 *
 * A valid signature proves the **sender**. It never proves the **tenant**. Resolve tenancy from
 * your own stored state — keyed on the endpoint that received the callback, or on the secret that
 * verified it — and never from a field in the body. Trusting the body would let anyone holding one
 * valid key target an arbitrary tenant (§15, I13). This module gives you no way to read tenancy
 * from a callback body, on purpose.
 */

import { asBuffer, canonicalBytes, constantTimeEquals, sha256Hex, toHex, type JsonObject } from "./document.ts";
import { CallbackSignatureError, NonConformingDocument } from "./errors.ts";
import { Signal } from "./models.ts";

/** signing.md §1.3 step 2. Receiver-enforced, and not negotiable downward by the sender. */
export const FRESHNESS_WINDOW_SECONDS = 300;
export const SIGNATURE_VERSION = "1";

/** The predecessor of the first receipt in a tenant's chain (signing.md §2.2). */
const ZERO_DIGEST = "sha256:" + "0".repeat(64);

export type HeaderBag = Record<string, string | string[] | undefined> | Headers | Map<string, string>;

/**
 * A callback that passed every check in signing.md §1.3.
 *
 * `deliveryId` is the deduplication key: the same signal may legitimately arrive more than once,
 * and applying it twice is the caller's bug, not the sender's. Returning `2xx` does **not** consume
 * the signal — consumption is the ack (§8.3), which is what `Waiter.ack` does.
 */
export interface VerifiedCallback {
  signal: Signal;
  deliveryId: string;
  signalId: string;
  sequence: number;
  timestamp: number;
}

/**
 * `version LF timestamp LF delivery_id LF sha256_hex(body)` — exactly three line feeds, no
 * trailing newline (signing.md §1.2).
 *
 * The body's **hash** is signed rather than the body, so a receiver can verify before buffering,
 * and so a body that begins with a digit cannot be confused with the timestamp before it.
 * `deliveryId` is inside the signed string so a valid signature cannot be lifted onto a different
 * delivery of the same payload.
 */
export async function callbackCanonicalString(
  version: string,
  timestamp: number | string,
  deliveryId: string,
  body: Uint8Array,
): Promise<string> {
  const bodyHash = await sha256Hex(body);
  return `${version}\n${timestamp}\n${deliveryId}\n${bodyHash}`;
}

const encoder = new TextEncoder();

async function hmacSha256Hex(secret: string, message: string): Promise<string> {
  const key = await crypto.subtle.importKey(
    "raw",
    asBuffer(encoder.encode(secret)),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  const signature = await crypto.subtle.sign(
    "HMAC",
    key,
    asBuffer(encoder.encode(message)),
  );
  return toHex(new Uint8Array(signature));
}

/** Produce the lowercase-hex `v1` value for one secret. Present so tests can build valid and
 *  deliberately invalid vectors; a client verifies, it does not sign. */
export async function signCallback(
  secret: string,
  version: string,
  timestamp: number,
  deliveryId: string,
  body: Uint8Array,
): Promise<string> {
  return hmacSha256Hex(secret, await callbackCanonicalString(version, timestamp, deliveryId, body));
}

/** Case-insensitive header lookup. HTTP header names are case-insensitive and frameworks disagree
 *  about which case they hand you. */
function header(headers: HeaderBag, name: string): string | undefined {
  const lowered = name.toLowerCase();
  if (typeof Headers !== "undefined" && headers instanceof Headers) {
    return headers.get(lowered) ?? undefined;
  }
  if (headers instanceof Map) {
    for (const [key, value] of headers) if (key.toLowerCase() === lowered) return value;
    return undefined;
  }
  for (const key of Object.keys(headers)) {
    if (key.toLowerCase() === lowered) {
      const value = (headers as Record<string, string | string[] | undefined>)[key];
      return Array.isArray(value) ? value[0] : value;
    }
  }
  return undefined;
}

function parseSignatureHeader(raw: string): { timestamp: number; signatures: string[] } {
  let timestamp: number | undefined;
  const signatures: string[] = [];
  for (const rawPart of raw.split(",")) {
    const part = rawPart.trim();
    const index = part.indexOf("=");
    if (index < 0) throw new CallbackSignatureError("malformed Handoff-Signature: element without '='");
    const key = part.slice(0, index);
    const value = part.slice(index + 1);
    if (key === "t") {
      if (timestamp !== undefined) {
        throw new CallbackSignatureError("malformed Handoff-Signature: more than one 't' element");
      }
      if (!/^\d+$/.test(value)) {
        throw new CallbackSignatureError("malformed Handoff-Signature: 't' is not an integer");
      }
      timestamp = Number(value);
    } else if (key === "v1") {
      if (!value) throw new CallbackSignatureError("malformed Handoff-Signature: empty 'v1' element");
      signatures.push(value);
    }
  }
  if (timestamp === undefined) throw new CallbackSignatureError("malformed Handoff-Signature: no 't' element");
  if (signatures.length === 0) throw new CallbackSignatureError("malformed Handoff-Signature: no 'v1' element");
  return { timestamp, signatures };
}

/**
 * Verify an inbound callback and return its typed signal, or throw.
 *
 * `body` must be the raw bytes as received, before any parsing or re-serialization: the signature
 * covers the bytes on the wire, and re-encoding a parsed object produces a different hash for the
 * same document (signing.md §3). Passing a string is refused rather than silently encoded, because
 * the encoding you would get is not necessarily the one that arrived.
 *
 * `secrets` is the **active** set. A retired secret must not be in it — a receiver that keeps
 * retired secrets active has made rotation meaningless. During a rotation overlap both secrets are
 * active and either one verifying is a pass (signing.md §1.4).
 *
 * Every check of signing.md §1.3 runs in order and the first failure rejects. Rejection messages
 * name the check and never include a secret or any value derived from one.
 */
export async function verifyCallback(
  headers: HeaderBag,
  body: Uint8Array,
  secrets: string | string[],
  options: { window?: number; now?: number } = {},
): Promise<VerifiedCallback> {
  if (typeof body === "string") {
    throw new TypeError(
      "verifyCallback() needs the raw request body as bytes: the signature covers the bytes as " +
        "transmitted, and re-encoding a decoded body changes the hash",
    );
  }
  const active = (Array.isArray(secrets) ? secrets : [secrets]).filter(Boolean);
  if (active.length === 0) throw new CallbackSignatureError("no active callback secrets configured");

  const rawSignature = header(headers, "Handoff-Signature");
  if (!rawSignature) throw new CallbackSignatureError("missing Handoff-Signature header");
  const { timestamp, signatures } = parseSignatureHeader(rawSignature);

  // 2. Freshness, before any cryptography: a replayed callback is cheap to reject.
  const window = options.window ?? FRESHNESS_WINDOW_SECONDS;
  const now = options.now ?? Date.now() / 1000;
  if (Math.abs(now - timestamp) > window) {
    throw new CallbackSignatureError(`timestamp outside the ${window}s freshness window`);
  }

  const version = header(headers, "Handoff-Version");
  if (!version) throw new CallbackSignatureError("missing Handoff-Version header");
  const deliveryId = header(headers, "Handoff-Delivery");
  if (!deliveryId) throw new CallbackSignatureError("missing Handoff-Delivery header");

  // 3 & 4. Hash the received bytes; rebuild the canonical string from headers only. Reading any of
  // these from the body would let the body attest to its own authenticity.
  const canonical = await callbackCanonicalString(version, timestamp, deliveryId, body);

  let matched = false;
  for (const secret of active) {
    const expected = await hmacSha256Hex(secret, canonical);
    for (const candidate of signatures) {
      if (constantTimeEquals(expected, candidate)) matched = true;
    }
  }
  if (!matched) throw new CallbackSignatureError("signature did not match any active secret");

  let signal: Signal;
  try {
    signal = Signal.from(body);
  } catch (cause) {
    throw new CallbackSignatureError(`body is not a JSON object: ${(cause as Error).message}`);
  }

  // 6. The header is a convenience mirror; the body field is authoritative because the body hash
  // covers it. A disagreement means one of the two was tampered with.
  const headerSequence = header(headers, "Handoff-Sequence");
  if (headerSequence !== undefined) {
    if (!/^\d+$/.test(headerSequence)) {
      throw new CallbackSignatureError("malformed Handoff-Sequence header");
    }
    if (Number(headerSequence) !== signal.sequence) {
      throw new CallbackSignatureError("Handoff-Sequence disagrees with the body's sequence");
    }
  }

  return {
    signal,
    deliveryId,
    signalId: header(headers, "Handoff-Signal") ?? signal.id,
    sequence: signal.sequence,
    timestamp,
  };
}

// -- receipts -----------------------------------------------------------------------------------

/** `sha256` of the receipt with its `chain` member removed, canonicalized (signing.md §2.2). */
export async function receiptCoreHash(receipt: JsonObject): Promise<string> {
  const core: JsonObject = {};
  for (const key of Object.keys(receipt)) if (key !== "chain") core[key] = receipt[key];
  return sha256Hex(canonicalBytes(core));
}

/** `height LF prev_digest LF core_hash`, hashed and prefixed. Height is inside the input so an
 *  entry cannot be excised and the rest re-linked without detection. */
export async function chainDigest(
  height: number,
  prevDigest: string,
  coreHash: string,
): Promise<string> {
  return "sha256:" + (await sha256Hex(encoder.encode(`${height}\n${prevDigest}\n${coreHash}`)));
}

/**
 * Recompute one receipt's `chain.digest` from its own content and position.
 *
 * This is the base tamper-evidence mechanism and it needs no key management at all, which is why
 * the protocol makes it a MUST and detached signatures only a MAY (§9.4).
 */
export async function verifyReceiptChain(receipt: JsonObject): Promise<boolean> {
  const chain = receipt?.chain;
  if (!chain || typeof chain !== "object") return false;
  try {
    const expected = await chainDigest(chain.height, chain.prev_digest, await receiptCoreHash(receipt));
    return constantTimeEquals(expected, String(chain.digest ?? ""));
  } catch (error) {
    // A receipt with no canonical form is a different finding from one whose digest does not
    // recompute, and this function must not report them identically: the first says whatever
    // minted it is broken, the second says someone changed a sealed record.
    if (error instanceof NonConformingDocument) throw error;
    return false;
  }
}

/**
 * Verify a whole tenant chain in order: each digest recomputes, and each links to the last.
 *
 * Altering any historical receipt changes its core hash, which changes its digest, which
 * invalidates every digest after it and therefore the exported head (§9.4, C-15).
 */
export async function verifyChain(
  receipts: JsonObject[],
  genesis: string = ZERO_DIGEST,
): Promise<boolean> {
  let previous = genesis;
  for (const receipt of receipts) {
    const chain = receipt?.chain;
    if (!chain || typeof chain !== "object") return false;
    if (String(chain.prev_digest) !== previous) return false;
    if (!(await verifyReceiptChain(receipt))) return false;
    previous = String(chain.digest);
  }
  return true;
}

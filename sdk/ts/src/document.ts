/**
 * Ordered JSON documents and the two serializations the protocol defines.
 *
 * `encodeDocument` — two-space indent, member order preserved as received, one trailing newline.
 * This is how the canonical fixtures in `spec/fixtures` are stored, and the form this SDK is
 * asserted byte-identical against.
 *
 * `canonicalBytes` — RFC 8785 (JCS): members sorted by the UTF-16 code units of their names, no
 * insignificant whitespace, no trailing newline. Every digest in the protocol (§1.4) is taken
 * over this.
 *
 * Member order is preserved rather than normalized for two reasons. Protocol §19 makes new
 * response fields additive, so a client that drops members it does not recognize corrupts
 * anything it forwards or re-hashes. And the canonical receipt fixtures carry the same members in
 * two different orders — `08-receipt-decision.json` in JCS order because it is the signed core
 * plus its chain entry, `09-receipt-policy.json` in declaration order — so no fixed field order
 * can reproduce both.
 */

import { NonConformingDocument } from "./errors.ts";

export type JsonValue = string | number | boolean | null | JsonValue[] | { [key: string]: JsonValue };
export type JsonObject = { [key: string]: any };

/**
 * Member names whose values are capability-adjacent and must never be rendered into a log line,
 * an error message, or a `toString` (§11.1, §12, I8, I18). Redaction is display-only.
 */
const REDACTED_MEMBERS = new Set([
  "resume_token",
  "resume_payload",
  "url",
  "transport",
  "secret",
  "secrets",
  "password",
  "api_key",
  "authorization",
]);

/**
 * Drop members whose value is `undefined` or `null`, preserving declaration order.
 *
 * Omitting a member is not the same as sending `null`: several protocol members are "nullable and
 * required" while others are simply optional, and sending an explicit null for the second kind
 * asserts something the caller did not mean.
 */
export function compact<T extends JsonObject>(input: T): JsonObject {
  const out: JsonObject = {};
  for (const key of Object.keys(input)) {
    const value = input[key];
    if (value !== undefined && value !== null) out[key] = value;
  }
  return out;
}

/** Encode in the canonical fixture form: 2-space indent, source order, trailing newline. */
export function encodeDocument(value: unknown): Uint8Array {
  return new TextEncoder().encode(JSON.stringify(unwrap(value), null, 2) + "\n");
}

/** Parse JSON, preserving member order. */
export function decodeDocument(raw: Uint8Array | string): any {
  return JSON.parse(typeof raw === "string" ? raw : new TextDecoder().decode(raw));
}

/**
 * Serialize to RFC 8785 (JCS) — the input to every digest in the protocol.
 *
 * Non-integer numbers are rejected rather than serialized. JCS specifies a number format that a
 * naive float rendering does not reproduce, so a float here yields a digest that is stable in one
 * implementation and wrong across two. The protocol's canonicalized objects carry no non-integer
 * numbers; failing loudly keeps it that way.
 */
export function canonicalBytes(value: unknown): Uint8Array {
  return new TextEncoder().encode(JSON.stringify(canonicalize(unwrap(value), "")));
}

/** Lowercase hex of a byte sequence. */
export function toHex(bytes: Uint8Array): string {
  let out = "";
  for (const byte of bytes) out += byte.toString(16).padStart(2, "0");
  return out;
}

/**
 * SHA-256 over raw bytes, as lowercase hex.
 *
 * WebCrypto rather than a Node built-in, so this SDK runs unmodified on Node, Deno, Bun, and
 * Workers with no dependency and no runtime detection. The cost is that every hashing operation is
 * async; the benefit is that there is nothing to install and nothing to shim.
 */
export async function sha256Hex(bytes: Uint8Array): Promise<string> {
  const buffer = await crypto.subtle.digest("SHA-256", bytes as unknown as ArrayBufferView);
  return toHex(new Uint8Array(buffer));
}

/** The `sha256:<hex>` digest of an object's canonical form (§1.4). */
export async function digest(value: unknown): Promise<string> {
  return "sha256:" + (await sha256Hex(canonicalBytes(value)));
}

/**
 * Compare two strings without leaking where they diverge.
 *
 * A length mismatch is answered without an early return so that the comparison costs the same
 * either way; the values compared here are fixed-width hex digests, so lengths agree in practice.
 */
export function constantTimeEquals(a: string, b: string): boolean {
  const left = new TextEncoder().encode(a);
  const right = new TextEncoder().encode(b);
  let difference = left.length ^ right.length;
  const length = Math.max(left.length, right.length);
  for (let i = 0; i < length; i += 1) {
    difference |= (left[i] ?? 0) ^ (right[i] ?? 0);
  }
  return difference === 0;
}

function canonicalize(value: any, path: string): any {
  if (typeof value === "number") {
    if (!Number.isInteger(value)) {
      throw new NonConformingDocument(
        `${path || "<root>"} carries the non-integer number ${value}, and digest-covered content ` +
          `carries integers only (§1.4). This document has no canonical form and therefore no ` +
          `digest, so it cannot have been produced by a conforming Server — §1.4 requires every ` +
          `digest-covered number to be stored and served in the form the canonicalizer emits. ` +
          `That is a defect in whatever minted this, and it is not evidence that anyone tampered ` +
          `with it.`,
      );
    }
    return value;
  }
  if (Array.isArray(value)) return value.map((item, index) => canonicalize(item, `${path}[${index}]`));
  if (value && typeof value === "object") {
    const out: JsonObject = {};
    // JCS orders members by the UTF-16 code units of their names, which is exactly what the
    // default `Array.prototype.sort` comparator does — it compares UTF-16 code units, not code
    // points. The two orders diverge for any non-BMP name against any BMP name above U+D7FF, and
    // a `document` field puts caller-chosen keys into the receipt core, so the distinction is
    // load-bearing rather than theoretical.
    for (const key of Object.keys(value).sort()) {
      out[key] = canonicalize(value[key], path ? `${path}.${key}` : key);
    }
    return out;
  }
  return value;
}

function unwrap(value: any): any {
  if (value instanceof Doc) return value.toJSON();
  if (Array.isArray(value)) return value.map(unwrap);
  if (value && typeof value === "object") {
    const out: JsonObject = {};
    for (const key of Object.keys(value)) out[key] = unwrap(value[key]);
    return out;
  }
  return value;
}

function redact(value: any, member = ""): any {
  if (REDACTED_MEMBERS.has(member) && value !== null && value !== undefined) return "<redacted>";
  if (Array.isArray(value)) return value.map((item) => redact(item, member));
  if (value && typeof value === "object") {
    const out: JsonObject = {};
    for (const key of Object.keys(value)) out[key] = redact(value[key], key);
    return out;
  }
  return value;
}

/**
 * A typed view over one protocol object, preserving its wire member order.
 *
 * Subclasses add named accessors. Members a subclass does not name are still readable by key and
 * are re-serialized untouched, which is what §19's additive-compatibility rule requires.
 */
export class Doc {
  readonly data: JsonObject;

  constructor(data: JsonObject = {}) {
    this.data = data;
  }

  static from<T extends Doc>(this: new (data: JsonObject) => T, raw: Uint8Array | string | JsonObject): T {
    const parsed =
      typeof raw === "string" || raw instanceof Uint8Array ? decodeDocument(raw) : raw;
    if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
      throw new Error(`expected a JSON object, got ${Array.isArray(parsed) ? "array" : typeof parsed}`);
    }
    return new this(parsed);
  }

  get<T = any>(key: string): T | undefined {
    return this.data[key] as T | undefined;
  }

  /** The underlying mapping, in wire order. */
  toJSON(): JsonObject {
    return this.data;
  }

  /** Fixture form: 2-space indent, source order, trailing newline. */
  encode(): Uint8Array {
    return encodeDocument(this.data);
  }

  /** RFC 8785 canonical form — what digests are taken over. */
  canonical(): Uint8Array {
    return canonicalBytes(this.data);
  }

  digest(): Promise<string> {
    return digest(this.data);
  }

  toString(): string {
    let body = JSON.stringify(redact(this.data));
    if (body.length > 400) body = body.slice(0, 397) + "...";
    return `${this.constructor.name}(${body})`;
  }

  /** Node prints this for `console.log`, so it must redact too. */
  [Symbol.for("nodejs.util.inspect.custom")](): string {
    return this.toString();
  }
}

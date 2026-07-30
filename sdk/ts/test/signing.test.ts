/**
 * Callback and receipt verification against the worked vectors in `spec/signing.md`.
 *
 * Every constant below is copied from that document. An implementation is expected to reproduce
 * each one exactly, and all four negative callback vectors must be rejected.
 */

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

import {
  CallbackSignatureError,
  callbackCanonicalString,
  chainDigest,
  receiptCoreHash,
  sha256Hex,
  signCallback,
  verifyCallback,
  verifyChain,
  verifyReceiptChain,
} from "../src/index.ts";

const HERE = dirname(fileURLToPath(import.meta.url));
const FIXTURES = join(HERE, "..", "..", "..", "spec", "fixtures");

const SECRET_A = "whsec_2f8a91c4e7b3d05a6c1e9f47b28d3a05";
const SECRET_B = "whsec_9d41c07be5a2f36819b4d0e7c5a81f62";
const RETIRED = "whsec_00000000000000000000000000000000";

const TIMESTAMP = 1785592064;
const DELIVERY = "dlv_01K3MB2R6C8ZC4YRXB2N6VD9FT";
const OTHER_DELIVERY = "dlv_01K3MB2R6D8ZC4YRXB2N6VD9FT";
const SIGNAL = "sig_01K3MB2R4X8ZC4YRXB2N6VD9FT";

const SIG_A = "cae13126f8dcd1e918376aa373be2757db7281a3e5aaed2d83d716537e03de80";
const SIG_B = "d86b3740bad654e46c1349614523a476be0eb7d6a30a798b2d475374f36c57eb";
const SIG_TAMPERED_A = "621af1622c79ccb0d444ae046dae7db4a8e5b96c6ae0d9bd574ff8bc0be26a66";
const SIG_OTHER_DELIVERY = "9a674a003d0507ad13369a6bd82713769116a276ec57f26eb2637b2af00f8e68";

const BODY = new Uint8Array(readFileSync(join(FIXTURES, "signing", "callback-body.json")));

function headers(
  signature: string,
  overrides: { delivery?: string; sequence?: string; version?: string } = {},
): Record<string, string> {
  const delivery = overrides.delivery ?? DELIVERY;
  return {
    "Handoff-Signature": signature,
    "Handoff-Delivery": delivery,
    "Handoff-Signal": SIGNAL,
    "Handoff-Version": overrides.version ?? "1",
    "Handoff-Sequence": overrides.sequence ?? "1",
    "Handoff-Idempotency-Key": delivery,
    "Content-Type": "application/json",
  };
}

async function rejects(fn: () => Promise<unknown>, pattern: RegExp): Promise<void> {
  await assert.rejects(fn, (error: Error) => {
    assert.ok(
      error instanceof CallbackSignatureError,
      `expected CallbackSignatureError, got ${error.name}`,
    );
    assert.match(error.message, pattern);
    return true;
  });
}

// -- the construction itself -------------------------------------------------------------------

test("canonical string matches the document", async () => {
  // signing.md §1.2: exactly three line feeds, no trailing newline.
  const canonical = await callbackCanonicalString("1", TIMESTAMP, DELIVERY, BODY);
  const bodyHash = "fbd6ec4cacc7cb9c9371d2791f946535e3d391a0594a92b5a3a27dd34f5e94fa";
  assert.equal(canonical, `1\n${TIMESTAMP}\n${DELIVERY}\n${bodyHash}`);
  assert.equal(canonical.split("\n").length - 1, 3);
  assert.ok(!canonical.endsWith("\n"));
});

test("signatures reproduce both worked vectors", async () => {
  assert.equal(await signCallback(SECRET_A, "1", TIMESTAMP, DELIVERY, BODY), SIG_A);
  assert.equal(await signCallback(SECRET_B, "1", TIMESTAMP, DELIVERY, BODY), SIG_B);
});

// -- positive vectors --------------------------------------------------------------------------

test("verifies under secret A", async () => {
  const result = await verifyCallback(headers(`t=${TIMESTAMP},v1=${SIG_A}`), BODY, [SECRET_A], {
    now: TIMESTAMP,
  });
  assert.equal(result.deliveryId, DELIVERY);
  assert.equal(result.signalId, SIGNAL);
  assert.equal(result.sequence, 1);
  assert.equal(result.signal.type, "answered");
  assert.equal(result.signal.decision?.values.decision, "approve");
});

test("rotation overlap verifies under either secret", async () => {
  // signing.md §1.4: while two secrets are active the server signs with both and the receiver
  // accepts either, so there is no window in which valid callbacks fail.
  const both = `t=${TIMESTAMP},v1=${SIG_A},v1=${SIG_B}`;
  for (const active of [[SECRET_A], [SECRET_B], [SECRET_A, SECRET_B], [SECRET_B, SECRET_A]]) {
    const result = await verifyCallback(headers(both), BODY, active, { now: TIMESTAMP });
    assert.equal(result.sequence, 1);
  }
});

// -- the four negative vectors -----------------------------------------------------------------

test("negative: a tampered body is rejected", async () => {
  const tampered = new TextEncoder().encode(
    new TextDecoder().decode(BODY).replace('"approve"', '"reject"'),
  );
  assert.equal(
    await sha256Hex(tampered),
    "8d1b25a370b6de9d1a504ca1acfe97dc7abe10d4c12b0d33dfaf74f5114eb019",
  );
  await rejects(
    () => verifyCallback(headers(`t=${TIMESTAMP},v1=${SIG_A}`), tampered, [SECRET_A], { now: TIMESTAMP }),
    /did not match/,
  );
  // The vector notes what an attacker *holding the secret* would produce, to make the point that
  // one who does not hold it cannot.
  assert.equal(await signCallback(SECRET_A, "1", TIMESTAMP, DELIVERY, tampered), SIG_TAMPERED_A);
});

test("negative: replay onto another delivery is rejected", async () => {
  // The delivery id is inside the signed string, so a valid signature cannot be lifted onto a
  // different delivery of the same payload.
  await rejects(
    () =>
      verifyCallback(headers(`t=${TIMESTAMP},v1=${SIG_A}`, { delivery: OTHER_DELIVERY }), BODY, [SECRET_A], {
        now: TIMESTAMP,
      }),
    /did not match/,
  );
  assert.equal(await signCallback(SECRET_A, "1", TIMESTAMP, OTHER_DELIVERY, BODY), SIG_OTHER_DELIVERY);
});

test("negative: a stale timestamp is rejected", async () => {
  // 301 seconds earlier, signature recomputed and cryptographically valid — and still refused,
  // because freshness is receiver-enforced.
  const stale = TIMESTAMP - 301;
  const valid = await signCallback(SECRET_A, "1", stale, DELIVERY, BODY);
  await rejects(
    () => verifyCallback(headers(`t=${stale},v1=${valid}`), BODY, [SECRET_A], { now: TIMESTAMP }),
    /freshness window/,
  );
});

test("negative: a retired secret is rejected", async () => {
  const signed = await signCallback(RETIRED, "1", TIMESTAMP, DELIVERY, BODY);
  await rejects(
    () => verifyCallback(headers(`t=${TIMESTAMP},v1=${signed}`), BODY, [SECRET_A, SECRET_B], { now: TIMESTAMP }),
    /did not match/,
  );
});

// -- boundary and hygiene ----------------------------------------------------------------------

test("the freshness boundary is inclusive at 300 seconds", async () => {
  const signed = await signCallback(SECRET_A, "1", TIMESTAMP, DELIVERY, BODY);
  for (const offset of [300, -300]) {
    assert.ok(
      await verifyCallback(headers(`t=${TIMESTAMP},v1=${signed}`), BODY, [SECRET_A], {
        now: TIMESTAMP + offset,
      }),
    );
  }
  await rejects(
    () => verifyCallback(headers(`t=${TIMESTAMP},v1=${signed}`), BODY, [SECRET_A], { now: TIMESTAMP + 301 }),
    /freshness window/,
  );
});

test("a sequence header disagreeing with the body is rejected", async () => {
  await rejects(
    () =>
      verifyCallback(headers(`t=${TIMESTAMP},v1=${SIG_A}`, { sequence: "7" }), BODY, [SECRET_A], {
        now: TIMESTAMP,
      }),
    /Sequence/,
  );
});

for (const header of ["", `v1=${SIG_A}`, `t=${TIMESTAMP}`, `t=nan,v1=${SIG_A}`, `t=${TIMESTAMP},v1=`, "garbage"]) {
  test(`a malformed signature header is rejected: ${JSON.stringify(header)}`, async () => {
    await rejects(
      () => verifyCallback(headers(header), BODY, [SECRET_A], { now: TIMESTAMP }),
      /Handoff-Signature/,
    );
  });
}

test("a re-encoded body does not verify", async () => {
  // signing.md §3's first trap. Note what makes it a trap rather than an obvious bug: this
  // particular body is already canonical, so a compact JSON.stringify of it round-trips to the
  // identical bytes and a receiver that re-encodes would appear to work. It is the moment anything
  // differs — whitespace here, member order elsewhere, a framework's own body parser — that the
  // hash diverges. Verify the bytes that arrived, never a re-encoding of them.
  const parsed = JSON.parse(new TextDecoder().decode(BODY));
  assert.deepEqual(
    new TextEncoder().encode(JSON.stringify(parsed)),
    BODY,
    "the fixture is canonical, so a compact re-encode coincides — which is exactly the trap",
  );

  const prettyPrinted = new TextEncoder().encode(JSON.stringify(parsed, null, 2));
  assert.notDeepEqual(prettyPrinted, BODY);
  await rejects(
    () => verifyCallback(headers(`t=${TIMESTAMP},v1=${SIG_A}`), prettyPrinted, [SECRET_A], { now: TIMESTAMP }),
    /did not match/,
  );
});

test("headers are matched case-insensitively", async () => {
  const lowered: Record<string, string> = {};
  for (const [key, value] of Object.entries(headers(`t=${TIMESTAMP},v1=${SIG_A}`))) {
    lowered[key.toLowerCase()] = value;
  }
  const result = await verifyCallback(lowered, BODY, [SECRET_A], { now: TIMESTAMP });
  assert.equal(result.deliveryId, DELIVERY);
});

test("a Headers instance works too", async () => {
  const bag = new Headers(headers(`t=${TIMESTAMP},v1=${SIG_A}`));
  assert.equal((await verifyCallback(bag, BODY, [SECRET_A], { now: TIMESTAMP })).deliveryId, DELIVERY);
});

test("rejection messages never contain a secret", async () => {
  await assert.rejects(
    () => verifyCallback(headers(`t=${TIMESTAMP},v1=${SIG_A}`), BODY, [SECRET_B], { now: TIMESTAMP }),
    (error: Error) => {
      assert.ok(!error.message.includes(SECRET_A));
      assert.ok(!error.message.includes(SECRET_B));
      assert.ok(!error.message.includes("whsec_"));
      return true;
    },
  );
});

test("an empty active secret set is refused", async () => {
  await rejects(
    () => verifyCallback(headers(`t=${TIMESTAMP},v1=${SIG_A}`), BODY, [], { now: TIMESTAMP }),
    /no active callback secrets/,
  );
});

// -- receipts ------------------------------------------------------------------------------------

const readJson = (file: string) => JSON.parse(readFileSync(join(FIXTURES, file), "utf8"));

test("receipt core hash and chain digest match the worked vector", async () => {
  const receipt = readJson("08-receipt-decision.json");
  assert.equal(
    await receiptCoreHash(receipt),
    "2763f39ef8a61d493106d3db302ec36cae5c024ca3da3a019d483ccc29704ad1",
  );
  assert.equal(
    await chainDigest(4211, "sha256:" + "0".repeat(64), "2763f39ef8a61d493106d3db302ec36cae5c024ca3da3a019d483ccc29704ad1"),
    "sha256:919f8870391849de4e7b1d5b249ccbaaa7d5a7d3f500f5571c5a92dd0c3909db",
  );
  assert.ok(await verifyReceiptChain(receipt));
});

/**
 * `09-receipt-policy.json` presents itself as the next entry after `08` — its `prev_digest` is
 * `08`'s digest and its height is one higher — but its stored `chain.digest` does not recompute
 * from its own content (see the note at the end of this file). The chain mechanism is therefore
 * asserted over a recomputed second entry, so that this test states something true about the
 * implementation rather than about a fixture.
 */
async function relinkedPolicyReceipt(): Promise<Record<string, any>> {
  const policy = readJson("09-receipt-policy.json");
  policy.chain.digest = await chainDigest(
    policy.chain.height,
    policy.chain.prev_digest,
    await receiptCoreHash(policy),
  );
  return policy;
}

test("a two-receipt chain verifies end to end", async () => {
  const decision = readJson("08-receipt-decision.json");
  const policy = await relinkedPolicyReceipt();
  assert.equal(policy.chain.prev_digest, decision.chain.digest);
  assert.equal(policy.chain.height, decision.chain.height + 1);
  assert.ok(await verifyChain([decision, policy]));
});

test("altering a historical receipt invalidates the rest of the chain", async () => {
  // §9.4, C-15: the property the chain exists for.
  const decision = readJson("08-receipt-decision.json");
  const policy = await relinkedPolicyReceipt();
  assert.ok(await verifyChain([decision, policy]));

  decision.decision.values.note = "tampered";
  assert.equal(await verifyChain([decision, policy]), false);
});

test("receipt negative vectors are rejected", async () => {
  // signing.md §2.5. Any of these changes the chain digest, which invalidates the head.
  for (const mutate of [
    (r: any) => (r.decision.values.decision = "reject"),
    (r: any) => (r.chain.height = 4210),
    (r: any) => (r.chain.prev_digest = "sha256:" + "1".repeat(64)),
  ]) {
    const receipt = readJson("08-receipt-decision.json");
    mutate(receipt);
    assert.equal(await verifyReceiptChain(receipt), false);
  }
});

// NOTE — fixture defect, reported upstream, not worked around here:
// `spec/fixtures/09-receipt-policy.json` carries
//   chain.digest = sha256:c1a4f0bb7d2e6935481acdf20e7b3c56d9084e1fa27bc3d5608e94af1236b7d0
// but recomputing it from that receipt's own core (a4070dc2…) at height 4212 with its stated
// prev_digest yields
//   sha256:1c4738c06a55a1ecc2217b55ac20fa6ba65319e81fc3b7ac49a726536afeb669
// `08-receipt-decision.json` recomputes exactly, matching signing.md §2.5, so the canonicalization
// is right and the fixture's digest is a placeholder.

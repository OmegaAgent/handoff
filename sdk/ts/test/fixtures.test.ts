/**
 * Byte-identical round trips against every canonical fixture.
 *
 * `spec/fixtures/README.md` states the contract: an SDK's serialization is asserted byte-identical
 * after re-encoding, and the two files under `signing/` are the byte sequences the worked signature
 * vectors are computed over. These tests assert exactly that, and nothing weaker — comparing parsed
 * objects instead of bytes would pass while the digests silently disagreed.
 */

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join, relative } from "node:path";

import { Doc, canonicalBytes, Receipt, Request, Signal, sha256Hex } from "../src/index.ts";

const HERE = dirname(fileURLToPath(import.meta.url));
const FIXTURES = join(HERE, "..", "..", "..", "spec", "fixtures");

const documentFixtures = [
  ...readdirSync(FIXTURES).filter((f) => f.endsWith(".json")).map((f) => join(FIXTURES, f)),
  ...readdirSync(join(FIXTURES, "use-cases")).map((f) => join(FIXTURES, "use-cases", f)),
].sort();

const signingFixtures = readdirSync(join(FIXTURES, "signing"))
  .map((f) => join(FIXTURES, "signing", f))
  .sort();

const read = (path: string) => new Uint8Array(readFileSync(path));
const name = (path: string) => relative(FIXTURES, path);

test("the fixture set is present", () => {
  assert.equal(documentFixtures.length, 27);
  assert.equal(signingFixtures.length, 2);
});

for (const path of documentFixtures) {
  test(`${name(path)} round-trips byte-identically`, () => {
    const raw = read(path);
    assert.deepEqual(Doc.from(raw).encode(), raw);
  });
}

for (const path of signingFixtures) {
  test(`${name(path)} round-trips byte-identically under JCS`, () => {
    // Stored canonicalized, so parsing and re-canonicalizing must reproduce them exactly. An
    // implementation that cannot has a canonicalization bug regardless of what its own tests say.
    const raw = read(path);
    assert.deepEqual(canonicalBytes(JSON.parse(new TextDecoder().decode(raw))), raw);
  });
}

test("the round-trip assertion is byte-level, not parse-level", () => {
  // The guard on the guard. A reformatted document parses to an equal object but is a different
  // byte sequence, so a suite that compared parsed objects would pass here — and the fixtures would
  // quietly stop being a cross-language contract. `deepEqual` on two Uint8Arrays compares elements,
  // which is to say bytes; this test proves that distinction is load-bearing rather than assumed.
  const raw = read(join(FIXTURES, "05-signal-answered.json"));
  const parsed = JSON.parse(new TextDecoder().decode(raw));
  const reformatted = new TextEncoder().encode(JSON.stringify(parsed, null, 4) + "\n");

  assert.deepEqual(
    JSON.parse(new TextDecoder().decode(reformatted)),
    parsed,
    "a parse-level comparison cannot tell these two apart",
  );
  assert.notDeepEqual(reformatted, raw, "a byte-level comparison can, and that is what we assert");
});

test("signing fixture hashes match the worked vectors", async () => {
  // signing.md §1.6 and §2.5 publish these lengths and hashes. They are the check on whether this
  // implementation canonicalizes correctly.
  const body = read(join(FIXTURES, "signing", "callback-body.json"));
  const core = read(join(FIXTURES, "signing", "receipt-core.json"));
  assert.equal(body.length, 493);
  assert.equal(
    await sha256Hex(body),
    "fbd6ec4cacc7cb9c9371d2791f946535e3d391a0594a92b5a3a27dd34f5e94fa",
  );
  assert.equal(core.length, 1125);
  assert.equal(
    await sha256Hex(core),
    "2763f39ef8a61d493106d3db302ec36cae5c024ca3da3a019d483ccc29704ad1",
  );
});

test("the decision receipt's core is exactly the signing fixture", () => {
  // fixtures/README.md guarantee 1.
  const receipt = Receipt.from(read(join(FIXTURES, "08-receipt-decision.json")));
  assert.deepEqual(canonicalBytes(receipt.core()), read(join(FIXTURES, "signing", "receipt-core.json")));
});

test("two receipts carry the same members in different orders", () => {
  // Which is why the SDK preserves wire order instead of imposing a field order. `08` is stored in
  // JCS order because it is the signed core plus its chain entry; `09` is in declaration order. No
  // fixed field order reproduces both, so member order is data.
  const decision = Doc.from(read(join(FIXTURES, "08-receipt-decision.json")));
  const policy = Doc.from(read(join(FIXTURES, "09-receipt-policy.json")));
  assert.deepEqual(Object.keys(decision.toJSON()).sort(), Object.keys(policy.toJSON()).sort());
  assert.notDeepEqual(Object.keys(decision.toJSON()), Object.keys(policy.toJSON()));
});

test("unknown members survive a round trip", () => {
  // §19: new response fields are additive and a client must ignore what it does not know — which
  // means carrying it, not dropping it.
  const raw = JSON.parse(readFileSync(join(FIXTURES, "05-signal-answered.json"), "utf8"));
  raw["x-vendor-annotation"] = { seen: true };
  const signal = new Signal(raw);
  assert.equal(signal.type, "answered");
  const encoded = JSON.parse(new TextDecoder().decode(signal.encode()));
  assert.deepEqual(encoded["x-vendor-annotation"], { seen: true });
});

test("typed accessors read the canonical fixtures", () => {
  const request = Request.from(read(join(FIXTURES, "02-request-created.json")));
  assert.equal(request.id, "req_01K3M7QW8ZC4YRXB2N6VD9FTHE");
  assert.ok(request.isPending);
  assert.equal(request.prompt.title, "Refund $2,400 to Acme Corp?");

  const signal = Signal.from(read(join(FIXTURES, "05-signal-answered.json")));
  assert.ok(signal.isTerminal);
  assert.equal(signal.decision?.values.decision, "approve");
  assert.ok(signal.decision?.decidedByHuman);

  const lapsed = Signal.from(read(join(FIXTURES, "07-signal-attempt-lapsed.json")));
  assert.equal(lapsed.isTerminal, false);
  assert.equal(lapsed.decision, null);

  const policy = Receipt.from(read(join(FIXTURES, "09-receipt-policy.json")));
  assert.equal(policy.actorType, "policy");
  assert.equal(policy.decidedByHuman, false);
});

test("canonicalization refuses non-integer numbers", () => {
  // signing.md §3 trap 2: a naive float format produces a digest that is stable in one
  // implementation and wrong across two.
  assert.deepEqual(canonicalBytes({ n: 4211 }), new TextEncoder().encode('{"n":4211}'));
  assert.throws(() => canonicalBytes({ amount: 2400.5 }), /non-integer number/);
});

test("a signal's string form redacts its resume token", () => {
  // The resume token authorizes the ack. It is not an identifier and must not be logged.
  const signal = Signal.from(read(join(FIXTURES, "05-signal-answered.json")));
  assert.ok(!String(signal).includes("rt_01K3MB2R55"));
  assert.ok(String(signal).includes("<redacted>"));
  assert.equal(signal.resumeToken, "rt_01K3MB2R558ZC4YRXB2N6VD9FT", "redaction is display-only");
});

test("a grant session's string form redacts its transport url", () => {
  // §11.2: the resolved transport URL is the only resolvable address in a conforming system, and
  // it must not be persisted, logged, or echoed.
  const session = Doc.from(read(join(FIXTURES, "13-grant-session.json")));
  assert.ok(!String(session).includes("wss://relay.example.com"));
});

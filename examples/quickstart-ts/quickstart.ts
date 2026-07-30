/**
 * Handoff quickstart: raise → a person answers → typed outcome → redeem.
 *
 * Run it against a real server:
 *
 *     HANDOFF_URL=https://handoff.example.com HANDOFF_API_KEY=sk_... node quickstart.ts
 *
 * Run it with no server at all, which is what happens by default:
 *
 *     node quickstart.ts
 *
 * ## What the offline run proves and does not prove
 *
 * With no `HANDOFF_URL` set, this starts the SDK's **test double** — a small in-memory server that
 * implements the handful of operations this script calls and none of the guarantees that make a
 * server conformant. There is no tenancy, no authority evaluation, no receipt chain, no storage
 * immutability, no delivery ladder.
 *
 * So the offline run proves the *client* half end to end: that a raise produces a durable wait, that
 * a long poll is satisfied by a person's answer, that the answer arrives as typed data, that the ack
 * is what consumes it, and that redemption is idempotent per effect key. It proves nothing about any
 * server. For that, run `conformance/` against a real one.
 *
 * Needs Node >= 22.18 (TypeScript is run directly, with types stripped).
 */

import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
const SDK = join(HERE, "..", "..", "sdk", "ts", "src", "index.ts");

const handoff = await import(SDK);
const { Client, authority, capability, evidence, fields, prompt, requires, ttlPolicy } = handoff;

let baseUrl = process.env.HANDOFF_URL;
let server: any;

if (!baseUrl) {
  // A test double, not a conforming server.
  const { FakeServer } = await import(join(HERE, "..", "..", "sdk", "ts", "test", "fake-server.ts"));
  server = await new FakeServer().start();
  baseUrl = server.baseUrl;
  console.log(`no HANDOFF_URL set — using the bundled test double at ${baseUrl}\n`);
}

const client = new Client({ baseUrl, apiKey: process.env.HANDOFF_API_KEY ?? "demo-key" });

// 1. Declare what you need. Not what kind of thing this is — what the answer must look like, what
//    the person must be handed, and who is entitled to give it.
const pending = await client.raiseRequest({
  waiterRef: "run:quickstart-0198f2a1",
  prompt: prompt(
    "Refund $2,400 to Acme Corp?",
    "Invoice INV-8821 was double-charged on 2026-07-28.",
    [evidence.link("Invoice INV-8821", "https://billing.internal/inv/8821")],
  ),
  requires: requires(
    [
      fields.choice("decision", "Decision", [
        ["approve", "Refund it"],
        ["reject", "Don't refund"],
      ]),
      fields.text("note", "Add a note", { required: false, maxLen: 500 }),
    ],
    { authority: authority("editor", "session", { reason: "the refund leaves our account" }) },
  ),
  ttl: "PT4H",
  ttlPolicy: ttlPolicy("expire_and_deny"),
  mode: "gated", // do not perform the effect without redeeming first
});

console.log(`raised   ${pending.id}`);
console.log(`created  ${pending.wasCreated}   (201 means 'I asked'; 200 means 'I already asked')`);
console.log(`surface  ${pending.surfaceUrl}`);
console.log(`waiter   ${pending.waiterRef}   <- the only thing that has to survive a crash\n`);

if (server) {
  // Stands in for a human opening the surface and clicking. In a real deployment this is a person
  // authenticated in your tenant. It cannot be the agent: a service-account principal calling
  // /answer gets 403 requester_may_not_answer, by principal type and under no configuration (§4.2).
  setTimeout(() => {
    console.log("(a person opens the surface and approves)");
    void new Client({ baseUrl, apiKey: "a-persons-session" }).answer(pending.id, {
      decision: "approve",
      note: "Confirmed with Acme on the phone.",
    });
  }, 1000);
}

// 2. Wait. The wait is a durable row on the server, not this loop — killing this process here loses
//    nothing, and `handoff.resume(waiterRef)` picks it up from anywhere.
console.log("waiting for a person...");

await pending.receive(
  async (received: any) => {
    const outcome = received.outcome;

    // 3. The answer is typed data. No prose to interpret, no regex, no model in the path.
    console.log(`\noutcome  ${outcome.outcome}`);
    console.log(`source   ${outcome.source}   (human, policy, or runtime_inference — never guessed)`);
    console.log(`values   ${JSON.stringify(outcome.values)}`);
    console.log(`receipt  ${outcome.receiptId}`);

    if (!outcome.decidedByHuman) {
      console.log("\nnobody decided this — a policy did. Not treating it as consent.");
      return;
    }
    if (outcome.value("decision") !== "approve") {
      console.log("\nrejected. Nothing to spend.");
      return;
    }

    // 4. One answer authorizes exactly one effect. Redeem immediately before doing it, and act only
    //    when firstRedemption is true — that is what stops a retried turn from refunding twice.
    const spend = await outcome.redeem("stripe:refund:ch_1B");
    console.log(`\nredeem   firstRedemption=${spend.firstRedemption}`);
    console.log(
      spend.firstRedemption
        ? "         -> performing the refund now"
        : "         -> already refunded on an earlier attempt; doing nothing",
    );

    const replay = await outcome.redeem("stripe:refund:ch_1B");
    console.log(`replay   firstRedemption=${replay.firstRedemption}   (the same effect, refused twice)`);
  },
  { timeoutMs: 120_000 },
);

// The ack was sent when that callback resolved, and not before. Had it rejected, the signal would
// still be queued for the next process to reattach and find.
console.log("\nacked. The signal is consumed exactly once.");

if (server) await server.close();

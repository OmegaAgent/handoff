# @handoffproto/sdk

The TypeScript client for [Handoff](https://github.com/OmegaAgent/handoff), the protocol for human
intervention in automated work. No runtime dependencies.

```ts
import * as handoff from "@handoffproto/sdk";

handoff.configure({ baseUrl: "https://handoff.example.com", apiKey });   // or $HANDOFF_URL

// Ask a person. Resolves when they answer.
const address = await handoff.ask("Which shipping address should I use?");

// Ask for a decision, then spend it on exactly one effect.
const outcome = await handoff.approve("Refund $2,400 to Acme Corp?", { mode: "gated" });
if (outcome.approved && (await outcome.redeem("stripe:refund:ch_1B")).firstRedemption) {
  await stripe.refunds.create({ charge: "ch_1B" });
}
```

## What this gives you

**One human answer, delivered to your agent exactly once, as typed data, authorizing exactly one
effect.**

That sentence is the whole claim, and every word in it is load-bearing:

- *typed data* — the decision arrives structured according to the request's own declaration, never
  as prose a model or a regex has to interpret;
- *exactly once* — delivery is at-least-once and the ack is idempotent, which together give
  effectively-once **application** if you ack after applying (the SDK makes that the easy path);
- *one effect* — one answer mints one authorization, and redemption is idempotent per effect key,
  so a retried turn cannot refund the customer twice.

It does **not** resume your execution, and neither does any other implementation of this protocol.
Whether your program can pick up where it stopped is a property of your program. What Handoff
guarantees is that the answer is there, typed and unconsumed, whenever it does.

## The wait is not in your process

This is the difference between a protocol and a loop. The wait is a durable row on the server keyed
by `waiterRef`, so the client holding it is disposable:

```ts
const pending = await handoff.raiseRequest({
  waiterRef: "run:0198f2a1",
  prompt: handoff.prompt("Refund $2,400 to Acme Corp?", "Invoice INV-8821 was double-charged."),
  requires: handoff.requires(
    [handoff.fields.choice("decision", "Decision", ["approve", "reject"])],
    { authority: handoff.authority("editor", "session") },
  ),
});
```

Your process can die here — crash, redeploy, `SIGKILL`, anything. Later, from anywhere:

```ts
const waiter = await handoff.resume("run:0198f2a1");   // the only thing that had to survive
await waiter.receive(async (received) => {
  await apply(received.values);                        // your work
});                                                    // the ack is sent here
```

The ordering is the point. **The ack is what consumes a signal — not reading it, and not returning
2xx to a callback.** `receive()` acks when the callback resolves; if it rejects, nothing is acked
and the signal stays queued for the next process to find. Acking first and applying second would
turn at-least-once delivery into at-most-once application, which is the exact bug this protocol
exists to make impossible.

To record that a decision arrived and could not be acted on — not an error, and worth keeping:

```ts
await waiter.receive(async (received) => received.unable("the refund API was down"));
```

## Declarations, not kinds

A request declares *what it needs*. There is no `kind` on the wire and no branch behind it, which is
why all eight interaction patterns in the specification are eight declarations over one shape:

```ts
handoff.requires(
  [
    handoff.fields.text("email", "Email"),
    handoff.fields.secret("password", "Password", { sinkRef: "snk_..." }),
  ],
  {
    capabilities: [handoff.capability("interactive_surface", { scope: "drive", optional: true })],
    authority: handoff.authority("admin", "session"),
  },
);
```

A `secret` field never carries its value here. The answer carries `{ provided: true }` and the value
itself goes to a sink your runtime owns and can audit.

## Verifying callbacks

```ts
const result = await handoff.verifyCallback(request.headers, rawBodyBytes, activeSecrets);
// result.deliveryId is your deduplication key
```

Pass the **raw bytes as received**. The signature covers the bytes on the wire, so re-encoding a
parsed body produces a different hash for the same document. A valid signature proves the
**sender**, never the tenant — resolve tenancy from your own stored state, keyed on the endpoint or
the secret, and never from a field in the body.

Receipt integrity is `verifyReceiptChain()` / `verifyChain()`.

## Errors

Every error carries a stable machine-readable `code` and throws a class that mirrors it:
`AlreadyAnswered` (with `.receiptId`), `RequesterMayNotAnswer`, `InsufficientAuthority`,
`AuthorizationSpent`, `AnswerValidationFailed` (with per-field `.fields`), and the rest of §13. A
code this version does not recognize throws `HandoffProtocolError` with the code intact rather than
being coerced into the nearest familiar class.

Never branch on `.message`. It is written for people and may change at any time.

## Runtime and build

Zero dependencies, and no Node built-ins: hashing, HMAC, and randomness all go through **WebCrypto**,
so the same source runs on Node, Deno, Bun, and Workers. Hashing is therefore async — `verifyCallback`,
`verifyChain`, and `digest` all return promises.

The package ships TypeScript source and is consumed directly by any runtime that strips types
(Node ≥ 22.18, Deno, Bun) or any bundler. To emit JavaScript for older consumers:

```bash
npx tsc -p tsconfig.json --noEmit false --outDir dist --declaration \
  --rewriteRelativeImportExtensions
```

`--rewriteRelativeImportExtensions` needs TypeScript ≥ 5.7 and rewrites the `.ts` import specifiers
to `.js` on the way out. That step is not run here because this checkout has no npm registry access;
`npm run typecheck` (`tsc --noEmit`) is, and it passes under `--strict`.

## Tests

```bash
npm test          # node --test test/*.test.ts
npm run typecheck # tsc --noEmit, strict
```

The suite asserts byte-identical serialization against every fixture in `spec/fixtures/`, the worked
signature vectors in `spec/signing.md` including all four negative cases, and — by `SIGKILL`ing a
real subprocess in the middle of a real long poll — that a client can die at any point without
losing an answer.

## Licence

MIT — see `LICENSE`. The rest of the Handoff repository is Apache-2.0; the SDKs are MIT so they
vendor cleanly into anything.

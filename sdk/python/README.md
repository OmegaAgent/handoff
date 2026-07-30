# handoff-human

The Python client for [Handoff](https://github.com/OmegaAgent/handoff), the protocol for human
intervention in automated work. Standard library only, no dependencies.

```bash
pip install handoff-human
```

```python
import handoff

handoff.configure(base_url="https://handoff.example.com", api_key=...)   # or $HANDOFF_URL

# Ask a person. Blocks until they answer.
address = handoff.ask("Which shipping address should I use?")

# Ask for a decision, then spend it on exactly one effect.
outcome = handoff.approve("Refund $2,400 to Acme Corp?", mode="gated")
if outcome and outcome.redeem("stripe:refund:ch_1B").first_redemption:
    stripe.refund("ch_1B")
```

## What this gives you

**One human answer, delivered to your agent exactly once, as typed data, authorizing exactly one
effect.**

That sentence is the whole claim, and every word in it is load-bearing:

- *typed data* — the decision arrives structured according to the request's own declaration,
  never as prose a model or a regex has to interpret;
- *exactly once* — delivery is at-least-once and the ack is idempotent, which together give
  effectively-once **application** if you ack after applying (the SDK makes that the easy path);
- *one effect* — one answer mints one authorization, and redemption is idempotent per effect key,
  so a retried turn cannot refund the customer twice.

It does **not** resume your execution, and neither does any other implementation of this
protocol. Whether your program can pick up where it stopped is a property of your program. What
Handoff guarantees is that the answer is there, typed and unconsumed, whenever it does.

## The wait is not in your process

This is the difference between a protocol and a loop. The wait is a durable row on the server
keyed by `waiter_ref`, so the client holding it is disposable:

```python
pending = handoff.raise_request(
    waiter_ref="run:0198f2a1",
    prompt=handoff.prompt("Refund $2,400 to Acme Corp?", "Invoice INV-8821 was double-charged."),
    requires=handoff.requires(
        [handoff.fields.choice("decision", "Decision", ["approve", "reject"])],
        authority=handoff.authority("editor", "session"),
    ),
)
```

Your process can die here — crash, redeploy, `kill -9`, anything. Later, from anywhere:

```python
waiter = handoff.resume("run:0198f2a1")     # the only thing that had to survive
with waiter.receive() as received:
    apply(received.values)                  # your work
# the ack is sent here, after the block completed
```

The ordering is the point. **The ack is what consumes a signal — not reading it, and not
returning 2xx to a callback.** `receive()` acks when the block completes; if the block raises,
nothing is acked and the signal stays queued for the next process to find. Acking first and
applying second would turn at-least-once delivery into at-most-once application, which is the
exact bug this protocol exists to make impossible.

To record that a decision arrived and could not be acted on — not an error, and worth keeping:

```python
with waiter.receive() as received:
    received.unable("the refund API was down")
```

## Declarations, not kinds

A request declares *what it needs*. There is no `kind` on the wire and no branch behind it, which
is why all eight interaction patterns in the specification are eight declarations over one shape:

```python
handoff.requires(
    [
        handoff.fields.text("email", "Email"),
        handoff.fields.secret("password", "Password", sink_ref="snk_..."),
    ],
    capabilities=[handoff.capability("interactive_surface", scope="drive", optional=True)],
    authority=handoff.authority("admin", "session"),
)
```

A `secret` field never carries its value here. The answer carries `{"provided": true}` and the
value itself goes to a sink your runtime owns and can audit. Declaring one also raises the
authority floor server-side, as a consequence of the request's shape rather than a hand-written
branch.

## Defaults are declared, not guessed

`ask(default=…)` returns the default if nobody answers — but it declares it **at raise time**, as
`ttl_policy: {on_expiry: "default", default_answer: …}`. When it fires, the server mints a policy
receipt with `actor.type = "policy"`, so no audit can mistake it for consent. Guessing the same
value client-side afterwards produces identical behaviour and no record at all.

## Verifying callbacks

```python
from handoff import verify_callback

result = verify_callback(request.headers, request.body_bytes, active_secrets=[...])
# result.delivery_id is your deduplication key
```

Pass the **raw bytes as received**. The signature covers the bytes on the wire, so re-encoding a
parsed body produces a different hash for the same document; passing a `str` is refused rather
than silently encoded. A valid signature proves the **sender**, never the tenant — resolve
tenancy from your own stored state, keyed on the endpoint or the secret, and never from a field
in the body.

Receipt integrity is `verify_receipt_chain()` / `verify_chain()`, both standard library. The
optional detached Ed25519 layer (`verify_receipt_signature`) needs `cryptography`; the hash chain
that the protocol actually requires does not.

## Errors

Every error carries a stable machine-readable `code` and raises a class that mirrors it:
`AlreadyAnswered` (with `.receipt_id`), `RequesterMayNotAnswer`, `InsufficientAuthority`,
`AuthorizationSpent`, `AnswerValidationFailed` (with per-field `.fields`), and the rest of §13.
A code this version does not recognize raises `HandoffProtocolError` with the code intact rather
than being coerced into the nearest familiar class.

Never branch on `.message`. It is written for people and may change at any time.

## Tests

```bash
cd sdk/python && python3 -m pytest tests/ -q
```

The suite asserts byte-identical serialization against every fixture in `spec/fixtures/`, the
worked signature vectors in `spec/signing.md` including all four negative cases, and — by
`SIGKILL`ing a real subprocess in the middle of a real long poll — that a client can die at any
point without losing an answer.

## Licence

MIT — see `LICENSE` in this directory. The rest of the Handoff repository is Apache-2.0; the SDKs
are MIT so they vendor cleanly into anything.

## Module rename in 0.2.0

The module is now `handoff`. The old `human` module still imports and forwards to it with a
`DeprecationWarning`, and is **removed in 0.3.0**:

```python
import human      # works in 0.2.x, warns, gone in 0.3.0
import handoff    # do this
```

Two names from 0.1.x could not be carried forward. `create_request(kind=…)` took an interaction
kind, which the protocol does not have. `clear_wall(live_view_url=…)` took a resolvable URL, and
the protocol never carries a resolvable address by value — a live surface is now an opaque
capability handle that the answerer's own client resolves. Both still import and both explain the
replacement when called.

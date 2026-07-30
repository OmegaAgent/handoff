# Examples

One runnable quickstart per language. Both show the same four steps: declare what you need, wait
for a person, read the typed outcome, spend it on exactly one effect.

| Example | Run it |
|---|---|
| [`quickstart-python/`](quickstart-python/) | `python3 quickstart-python/quickstart.py` (Python ≥ 3.9, no dependencies) |
| [`quickstart-ts/`](quickstart-ts/) | `node quickstart-ts/quickstart.ts` (Node ≥ 22.18, no dependencies) |

Point either at a real deployment with `HANDOFF_URL` and `HANDOFF_API_KEY`.

## What the offline run proves, and what it does not

With no `HANDOFF_URL` set, each quickstart starts the SDK's **test double** — a small in-memory
server that implements the handful of operations the script calls and none of the guarantees that
make a server conformant. No tenancy, no authority evaluation, no receipt chain, no storage-level
immutability, no delivery ladder.

So the offline run proves the *client* half end to end:

- a raise produces a durable server-side wait, and the `201`/`200` distinction is visible;
- a long poll is satisfied the moment a person answers;
- the answer arrives as typed data, with its source recorded as `human` rather than assumed;
- the **ack** is what consumes the signal, and it is sent after the outcome has been applied;
- redemption is idempotent per effect key — the same effect is refused a second time.

It proves nothing about any server. For that, run the conformance suite in `conformance/` against a
real one.

## `night-hack/`

Preserved prior art from the hackathon that this protocol grew out of. It predates the specification
and does not speak it — it is kept for the record, not as an example to copy. Two of its ideas
survive into the SDKs and one does not:

- `ask(default=…)` returning a fallback instead of raising survives, but the default is now declared
  **at raise time** so that an unanswered request mints a policy receipt rather than a silent guess;
- blocking until a person acts survives, and is now a durable server-side wait rather than a loop
  inside the agent process;
- `clear_wall(live_view_url=…)` does **not** survive. It handed the agent a resolvable live-view URL,
  and the protocol never carries a resolvable address by value (§11.1, I8). A live surface is now an
  opaque capability handle that the answerer's own client exchanges for a session.

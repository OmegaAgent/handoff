# Conformance suite

Declarative cases that decide whether an implementation is Handoff-compatible. The runner is
[`core/crates/handoff-conformance`](../core/crates/handoff-conformance); it takes a base URL and does
not care what is listening there.

```
handoff-conformance --base-url https://your-deployment.example.com/v1 --profile your-profile.yaml
```

**Status: 25 Level 1 cases, all passing against the reference server. The one Level 2 case does
not pass, and cannot.** They were written before any conforming
server existed, and the last thing done to them was to run them against a server that answers `501`
to everything and confirm the report reads `0/25 passing` with twenty-five individually named
failures and a non-zero exit. A suite that cannot fail loudly is not a suite. Against the reference implementation the Level 1 cases are green;
`conformance/GATE.md` records both directions.

**C-17 is deliberately unpassable here.** It requires `GET /meta` to advertise Level 2 with the
`continuation` extension. This build refuses a raise carrying `resume_payload`, because it has no
encryption at rest to protect one with, and it advertises Level 1 — derived from that same
capability rather than declared, so the two cannot drift. A server that has not implemented an
optional extension failing its optional case is the suite working, not a defect. See `BACKLOG.md`.

The runner checks the case set against `spec/conformance-map.json` before running anything, so a
case §18 defines with no file, or a file §18 does not define, stops the run rather than quietly
changing the total.

Two things a deployment must supply beyond a base URL, both documented in
`core/crates/handoff-conformance/profile.example.yaml`: credentials for the principal aliases the
cases name, and the handful of commands that reach below the HTTP API — C-15 must attempt a receipt
mutation at the **storage** layer, because §9.4 puts the application inside the threat model, and
C-7 must grep the logs. A case whose requirements are missing **fails**; it is never skipped.

The case format is documented in `core/crates/handoff-conformance/CASE-FORMAT.md`.

## A hook must show its work

The hooks are where a conformance claim is easiest to fake, because they are the only part of the
suite the deployment writes. A hostile review demonstrated the whole of it: the eight assertions
covering the properties an implementer *cannot* verify from the HTTP surface — storage-level
immutability, transactional atomicity, channel-content non-authority, secret absence from logs —
were all satisfied by four hooks stubbed with `true` and `false`. Every one of those hooks exits the
way its case wanted when it does nothing at all.

So an exit code is never the whole assertion. Each hook step also requires **evidence in the output
that the hook did the thing**, and where the evidence can be tied to something the suite read
independently over HTTP, it is:

| hook | must print |
|---|---|
| `storage_update_receipt`, `storage_delete_receipt` | the receipt id it attempted, and the engine's own refusal |
| `receipt_chain_verify` | `chain_verified head=<digest> height=<n>` — and it must be the head `GET /receipts/chain-head` returned |
| `chain_tamper_is_detected` | `tamper_detected altered=<id> head_before=<digest> head_after=did-not-verify`, `head_before` likewise |
| `channel_inbound` | `channel_message=<id> request=<id> channel=<name> request_state=<state>` |
| `observe_page_state_change` | `observation=<id> request=<id> request_state=<state> receipts=<n>` |
| `crash_between_state_and_event` | `crash_point_reached=<name> instance_exit=<non-zero> … agree=yes` |
| `logs` | anything, but it must exit 0 and it must not be empty |

Two of these deserve their reason stated. **C-23** asks whether a state change and its event are one
transaction, and "both are present and they agree" is equally true of an answer that committed
normally — so the hook has to show the process was actually interrupted at the seam, and left by a
fault rather than by exiting 0. A deployment that cannot induce the crash must **fail** the case
rather than report agreement it never tested. And **C-7**'s `logs` source is not one contribution
among several: a hook that fails, or that prints nothing, fails the case. §12.3 makes "no secret in
a log line" normative, and there is no version of "we could not show you" that is a pass.

Print more than the required line if it helps you debug. The cases match; they do not compare whole
outputs.

## Why this directory is not a test folder

This is the project's governance instrument, and three rules in
[`GOVERNANCE.md`](../GOVERNANCE.md) hang off it:

- **A `spec/` change must arrive with a case here.** CI blocks a pull request that edits `spec/` and
  nothing under `conformance/`. A specification whose executable half is optional stops being a
  specification within about two releases.
- **A core behaviour change without a case here is not merged.** This is what stops the suite
  lagging the implementation, which is the normal way a conformance suite dies.
- **The suite gates deploys, including the maintainer's own hosted service.** A service that cannot
  pass the open suite has a red build. That turns "we did not quietly fork the core" into a check
  anyone can rerun.

## Claiming conformance

"Handoff-compatible" is claimable for a specific protocol version only against a **published passing
run** for that version: the case list, the result per case, the suite version, and the date, posted
somewhere a skeptic can find them and rerun them against your endpoint. See
[`TRADEMARKS.md`](../TRADEMARKS.md).

## Three ways a check passes without measuring anything

Every one of these was found in this repository, in a check written by someone trying to prevent
exactly the thing it failed to catch. They are listed because the pattern recurs and the failure is
always silent: a check that measures nothing looks identical to one that measures everything, and
the only signal is that it has never been seen to fail.

**A verifier that shares code with the producer proves only self-consistency.** The receipt chain
was verified by calling the same canonicalization the server used to build it, so any
self-consistent construction passed — including one that disagreed with the specification and with
both published SDKs, which could verify none of the receipts the server minted. A verifier must
implement the spec independently, or assert against published vectors. Ours now does both.

**An anti-vacuity guard must be scoped to the same unit as the assertion it protects.** The
row-level-security test asserts, per table, that a query without a tenant predicate returns only the
caller's rows — a comparison that can only fail when *another* tenant owns a row in that table. One
guard checked that a second tenant existed at all, which proves the loop ran and not that any
iteration could have failed. Eight of nineteen tables were asserting nothing. A guard covering a
loop is not a guard covering its iterations.

**A readiness probe must confirm it is talking to the server it started.** The conformance harness
probed a port, got an answer, and proceeded — against an orphaned server from a previous run, with a
stale database. The same tree scored 17/24, 19/24, 22/24, 23/24 and 24/24 in one evening and not one
of those failures was about the protocol. A run against a stranger is not a measurement, whichever
way it lands.

The habit that catches all three: **break the thing on purpose and watch the check fail.** If you
have never seen a check go red, you have not tested the check — you have tested the code, using an
instrument of unknown sensitivity. Every guard in this suite has been shown failing at least once,
and the demonstration belongs in the pull request that adds it.

## Contributing a case

Cases are declarative YAML under `cases/`, so a case is readable by someone who does not write Rust.
The format is `core/crates/handoff-conformance/CASE-FORMAT.md`; every case-specific fact lives in the
YAML, and the runner is an interpreter that knows nothing about any individual case.

The most useful contribution is an **issue describing an edge you hit in production** and what the
correct behaviour should be. Edges found in the field are worth more than edges imagined at a desk,
and the four the design already calls hardest are: double-resolve conflict, attempt lapse, callback
retry, and receipt verification.

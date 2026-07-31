# Conformance suite

Declarative cases that decide whether an implementation is Handoff-compatible. The runner is
[`core/crates/handoff-conformance`](../core/crates/handoff-conformance); it takes a base URL and does
not care what is listening there.

```
handoff-conformance --base-url https://your-deployment.example.com/v1 --profile your-profile.yaml
```

**Status: 26 Level 1 cases, all passing against the reference server. The one Level 2 case does
not pass, and cannot.** They were written before any conforming
server existed, and two things are done to them regularly to keep the total meaning something: they
are run against a server that answers `501` to everything, where the report must read
`0/26 passing` with every case individually named and a non-zero exit; and they are run
against a profile whose hooks implement nothing, where C-15 must be red. A suite that cannot fail
loudly is not a suite. Against the reference implementation the Level 1 cases are green;
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

Among the credentials, note **two machine principals in tenant A**. C-5 needs a machine that did not
raise the request it tries to answer: with one machine, "refused because it is a machine" and
"refused because it is the requester" are the same observation, and the rule §4.2 calls load-bearing
is not the one being tested.

The case format is documented in `core/crates/handoff-conformance/CASE-FORMAT.md`.

## What the suite measures, and what it takes on trust

The hooks are where a conformance claim is easiest to fake, because they are the only part of the
suite the deployment writes. Two hostile reviews proved it in a row.

The first replaced every hook with `true` and `false` and got a green run: eight assertions covering
the properties an implementer *cannot* verify from the HTTP surface were satisfied by four stubs.
The answer then was to require **evidence in the output** — the receipt id, the chain head. The
second review defeated that in about fifteen lines of shell, and the reason generalizes past this
one case:

> Any requirement a hook can satisfy as a function of its own inputs is defeatable, because the
> claimant writes the hook. A nonce will be echoed. A digest will be read out of a column. The one
> value that was neither handed over nor readable — a chain height — was brute-forced by printing
> four hundred lines, because the matcher is a search.

So the bar did not move a third time. The **number of hooks went down** instead.

**The chain is computed here.** `verify_chain` reads the receipts from the deployment's own HTTP
surface and walks them inside the runner, in an implementation of `signing.md` §2.2 that shares no
code with any server and is checked against the published vectors in `spec/fixtures/signing/`. It
requires the exported head to be the head that walk arrives at, and then rewrites each receipt in
turn and requires the walk to break. `receipt_chain_verify` and `chain_tamper_is_detected` are gone:
there is no longer anything to narrate, and a deployment with no chain cannot serve receipts whose
stored digests are those hashes.

**A storage mutation is judged by what HTTP shows afterwards.** `storage_mutation` reads the object
before the attempt and again after it. A refusal that was really a mutation that landed fails on the
byte comparison. And the same command, aimed at a row the engine *permits* writing, must land and
become visible over HTTP — a hook that touches no storage cannot put a nonce into a request the
suite then reads back.

### The hooks that remain, and what decides each case

| hook | what decides the case |
|---|---|
| `storage_mutate` | the object, re-read over HTTP after the attempt: byte-identical for a refusal, changed for the control |
| `channel_inbound` | `channel_message=<id> request=<id> channel=<name> request_state=<state>` — then everything C-21 asserts afterwards is HTTP |
| `observe_page_state_change` | `observation=<id> request=<id> request_state=<state> receipts=<n>` |
| `crash_between_state_and_event` | `crash_point_reached=<name> instance_exit=<non-zero> … agree=yes` |
| `events` | one event per line, each naming its type |
| `canonicalize` | the published byte lengths and digests, and exact canonical strings for inline documents |
| `logs` | anything, but it must exit 0 and it must not be empty |

**C-23** asks whether a state change and its event are one transaction, and "both are present and
they agree" is equally true of an answer that committed normally — so the hook has to show the
process was actually interrupted at the seam, and left by a fault rather than by exiting 0. A
deployment that cannot induce the crash must **fail** the case rather than report agreement it never
tested. **C-7**'s `logs` source is not one contribution among several: a hook that fails, or that
prints nothing, fails the case. §12.3 makes "no secret in a log line" normative, and there is no
version of "we could not show you" that is a pass.

Print more than the required line if it helps you debug. The cases match; they do not compare whole
outputs.

### The part that is attested rather than measured, named rather than implied

Three cases can still be satisfied by a deployment willing to write a hook that lies rather than one
that is merely missing:

- **C-7** — a fabricated log contains no secret. The suite can tell an absent log from a shown one;
  it cannot tell a shown one from an invented one.
- **C-21, C-22** — an injection or an observation that never happened, reported as happening.
- **C-15, in one narrow respect** — `storage_mutate` is required to be one parameterized command, so
  the honest implementation cannot be honest for `target: request` and absent for `target: receipt`.
  A deployment that *branches* on the target to fake the refusal has written a deliberate lie, and
  this suite would not catch it. Closing that needs the suite to perform the mutation itself against
  a supplied store credential, which needs a database driver per storage engine; `BACKLOG.md` holds
  it.

That list is not editorial. It is produced by running the suite against
`core/crates/handoff-conformance/dev/lying-hooks/`, a profile whose every hook performs no work and
prints whatever its case matches on. `dev/run-lying-hooks.sh` runs it, asserts C-15 goes red, and
prints the cases the liar survives — which is where the list above comes from. `conformance/GATE.md`
records the run.

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
exactly the thing it failed to catch. They are written in the past tense on purpose: each is a
defect that has since been closed, kept here because the *pattern* recurs and the failure is always
silent. A check that measures nothing looks identical to one that measures everything, and the only
signal is that it has never been seen to fail. None of these is a statement about the tree as it
stands today; a lesson that has to be re-verified every time the code moves stops being a lesson.

**A verifier that shares code with the producer proved only self-consistency.** The receipt chain
was verified by calling the same canonicalization the server used to build it, so any
self-consistent construction passed — including one that disagreed with the specification and with
both published SDKs, which could verify none of the receipts the server minted. What closed it: a
verifier must implement the standard independently, or assert against published vectors. This suite
now does both — `core/crates/handoff-conformance/src/chain.rs` implements RFC 8785 and
`signing.md` §2.2 with no dependency on any protocol crate, checked against the vectors in
`spec/fixtures/signing/` — and C-26 is the case that spends it. Its sibling half,
`scripts/verify-minted-receipts.sh`, hands the same server-minted receipts to the two published
SDKs, because a Rust verifier written by the same people from the same reading can still share a
misreading with the server.

**An anti-vacuity guard must be scoped to the same unit as the assertion it protects.** The
row-level-security test asserted, per table, that a query without a tenant predicate returns only
the caller's rows — a comparison that can only fail when *another* tenant owns a row in that table.
One guard checked that a second tenant existed at all, which proves the loop ran and not that any
iteration could have failed, and a large fraction of the tables were asserting nothing while the
test reported full coverage. Two things closed it: the guard moved inside the iteration, and the
table list stopped being maintained by hand — a cross-check against the database's own policy
catalogue now fails when a table gains a policy and no assertion. The same shape appears in this
suite's own chain walk, which rewrites *each* receipt in turn rather than proving once that some
rewrite would be caught.

**A readiness probe must confirm it is talking to the server it started.** The conformance harness
probed a port, got an answer, and proceeded — against an orphaned server from a previous run, with a
stale database. One tree produced five different scores in one evening and not one of those failures
was about the protocol. What closed it: the runner checks its own process is alive before it trusts
an HTTP answer, and every resource a run owns — database, ports, scratch directory — derives from a
per-run token. A run against a stranger is not a measurement, whichever way it lands.

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

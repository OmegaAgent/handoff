# Running the conformance suite against `handoffd`

```
core/dev/run-conformance.sh            # create a database, run all 24 cases, tear down
core/dev/run-conformance.sh --case C-8 # one case
KEEP=1 core/dev/run-conformance.sh     # leave the database up for inspection
```

The script creates a **disposable database per run**, seeds `bootstrap.json`, starts `handoffd`,
and points the suite at it. It refuses to touch a database called `omega` or `omega_e2e`.

## Why the database is disposable

Several cases raise fixture requests that never expire, and a `dedupe_key` collapses onto a
`pending` request from an earlier run (§3.3 rule 3) — so a second run against the same store would
see `200` where the case requires `201`. A clean store is what makes a run measure this build
rather than the residue of the last one.

## What is below the HTTP API, and why

Five requirements cannot be asserted over HTTP by construction, and the suite expresses each as a
hook the deployment supplies (`conformance-profile.yaml`):

| Hook | Case | Why it cannot be HTTP |
|---|---|---|
| `storage_update_receipt`, `storage_delete_receipt` | C-15 | §9.4 puts the application inside the threat model, so immutability must be attempted **as the application's own database role** and refused by the engine. |
| `receipt_chain_verify`, `chain_tamper_is_detected` | C-15 | Re-walking the chain, and proving a rewrite invalidates the head. The tamper check operates on a **disposable copy** and never touches the live store. |
| `channel_inbound` | C-21 | The protocol defines the outbound delivery model but deliberately **no inbound channel-adapter surface**. |
| `observe_page_state_change` | C-22 | A runtime observation is not an API call, and §9.7 requires it to produce no receipt. |
| `events`, `crash_between_state_and_event` | C-23 | The protocol publishes no endpoint listing events, and killing a process between two writes is not an HTTP operation. |
| `canonicalize` | C-24 | RFC 8785 canonicalization is a pure function over bytes, checked against the published signing fixtures. |

Each hook is a `handoffd` subcommand, not a test fixture. An operator who wants to know whether
their receipts still verify runs `handoffd verify-chain` — the same command the suite does.

## The crash probe

`crash-between-writes.sh` starts a second `handoffd` with `HANDOFF_CRASH_POINT=answer_after_state_write`,
raises and answers a request through it, and the process aborts **inside the open transaction**,
after the state row is written and before the event row is. It then restarts and asserts that either
both writes are present or neither is. §18 calls C-23 the case an implementation is most tempted to
skip, because emitting the event just after the commit passes every happy-path test; this is what
makes it fail when it should.

`HANDOFF_CRASH_POINT` is unset in every deployment that is not running this suite, and changes
nothing when unset.

## Row-level security needs a role that cannot bypass it

Every `handoff_*` table has RLS enabled and forced, and each request-scoped transaction names its
tenant before it reads anything — so a query that lost its `WHERE tenant_ref = …` still cannot see
another tenant's rows. **A superuser, or any role with `BYPASSRLS`, ignores every policy**, which
leaves this defence inert while every test still passes. Run `handoffd` as a least-privilege role
with `SELECT, INSERT, UPDATE, DELETE` on its own tables and nothing more.

The integration test `row_level_security_holds_on_every_tenant_scoped_table` creates such a role,
asserts the property per table on **length and identity**, and then asserts that the role it used
does not bypass RLS — so the test cannot pass vacuously the way a superuser run would.

The tenant predicate in every query is the primary defence; RLS is the one that catches the day
somebody forgets it.

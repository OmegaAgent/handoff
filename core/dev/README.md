# Running the conformance suite against `handoffd`

```
core/dev/run-conformance.sh            # create a database, run every Level 1 case, tear down
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

Ten requirements cannot be asserted over HTTP by construction, and the suite expresses each as a
hook the deployment supplies (`conformance-profile.yaml`). Ten hooks, and the table below lists all
ten — the count and the table disagreeing is how `logs` went unlisted here for a while:

| Hook | Case | Why it cannot be HTTP |
|---|---|---|
| `logs` | C-7, C-17 | §12.3 makes "no secret in a log line" normative, and a log line is not a response body. A hook that fails, or that prints nothing, fails the case: there is no version of "we could not show you" that is a pass. |
| `storage_update_receipt`, `storage_delete_receipt` | C-15 | §9.4 puts the application inside the threat model, so immutability must be attempted **as the application's own database role** and refused by the engine. |
| `receipt_chain_verify`, `chain_tamper_is_detected` | C-15 | Re-walking the chain, and proving a rewrite invalidates the head. The tamper check operates on a **disposable copy** and never touches the live store. |
| `channel_inbound` | C-21 | The protocol defines the outbound delivery model but deliberately **no inbound channel-adapter surface**. |
| `observe_page_state_change` | C-22 | A runtime observation is not an API call, and §9.7 requires it to produce no receipt. |
| `events`, `crash_between_state_and_event` | C-23 | The protocol publishes no endpoint listing events, and killing a process between two writes is not an HTTP operation. |
| `canonicalize` | C-24 | RFC 8785 canonicalization is a pure function over bytes, checked against the published signing fixtures. |

Each is a real operator tool rather than a test fixture: `logs` is however you already read your
logs, and the rest are `handoffd` subcommands. An operator who wants to know whether their receipts
still verify runs `handoffd verify-chain` — the same command the suite does.

A hook is also never satisfied by its exit code alone. Each one has a one-word shell command that
exits the way its case wants while doing nothing, so every hook step additionally requires evidence
in the output that the hook did the thing — and where that evidence can be tied to something the
suite read independently over HTTP, it is. The required lines are in `conformance/README.md`.

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

Twenty of the twenty-one `handoff_*` tables have RLS enabled and forced — every one that carries a
`tenant_ref`. The exception is `handoff_migrations`, which holds applied migration numbers and no
tenant's data. **A superuser, or any role with `BYPASSRLS`, ignores every policy**, which leaves
this defence inert while every test still passes.

Run `handoffd` as a role that **owns its own schema and has neither `SUPERUSER` nor `BYPASSRLS`**:
`CREATE DATABASE handoff OWNER handoff_app`. `FORCE ROW LEVEL SECURITY` keeps the owner subject to
the policies, and ownership is needed rather than optional, because `handoffd` applies migrations
on every start — a role with only `SELECT, INSERT, UPDATE, DELETE` cannot start it, and fails with
`permission denied for schema public`.

The policy passes when **no** tenant has been named, and that half cannot be removed:
authentication resolves a credential to a tenant (§4.1), so the query that discovers the tenant is
unable to name it, and the cross-tenant sweeps have no tenant to name either. RLS therefore catches
a query that named its tenant and then lost its `WHERE tenant_ref = …`, and does not catch one that
named no tenant at all.

Two integration tests hold this down, and both refuse to pass vacuously:

| Test | What it asserts |
|---|---|
| `row_level_security_holds_on_every_tenant_scoped_table` | Per table, on **length and identity**, that a query without the predicate returns exactly the caller's rows. It first asserts that the *other* tenant owns a row in that table, because otherwise the comparison is between two empty sets; and that the role it used does not bypass RLS. |
| `every_request_scoped_path_names_its_tenant` | Tightens the policy to fail closed on all but `handoff_principals` and drives the API, so "each request-scoped transaction names its tenant" is measured rather than asserted. The paths that do not are listed in the test, and the list may shrink but not grow. |

The tenant predicate in every query is the primary defence; RLS is the one that catches the day
somebody forgets it.

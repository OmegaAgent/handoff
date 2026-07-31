# handoff-cloud — the managed Ωmegas deployment

**CLOSED SOURCE.** `license = "UNLICENSED"`, `publish = false`, workspace-wide. Nothing here may be
relicensed, published, or moved into `core/`.

This directory is the future contents of the private `OmegaAgent/handoff-cloud` repository. It lives
in this tree during development so a change to the open core and the adapter that consumes it can be
compiled together. Extracting it is a `git mv` plus one edit: the path dependencies in `Cargo.toml`
become tag pins.

It builds into its own `target/`, and deliberately does not share the core's. Sharing one build
directory saves several gigabytes and costs something worse: two workspaces with different feature
unification relink each other's artifacts, including the `handoffd` binary that the conformance
suite is at that moment executing. That produces test failures that look like protocol regressions
and are not.

## What the managed service is

The **open** `handoff-server`, plus one adapter crate.

```
handoff-omegas-server  =  handoff-server (open, pinned by tag)
                       +  handoff-omegas (this crate, ~2k LOC of adapters)
```

There is no private variant of the protocol, no managed-only state machine, and no second
implementation of anything the spec describes. Every type this crate exports implements a port
declared in `handoff_core::seam`, and `src/main.rs` wires them into the same server a self-hoster
runs. If `main.rs` ever grows a route handler, the boundary has moved.

`GET /v1/meta` reports, in `core_version`, the `handoff-core` version this service is running. If it drifts more than
one minor release behind the latest published tag, the open-core strategy is failing — visibly, to
everyone, including us.

## What works, and what refuses

Most of the control plane this consumes **does not exist yet**. Nothing here simulates it.

| Adapter | State |
|---|---|
| `auth` — `CallerAuthenticator` | **Real.** Full offline ES256 JWT verification against a JWKS, tested end to end against a locally signed token. Refuses every credential until an issuer is configured, because `POST /api/token` is M5 and has not landed. |
| `tenancy` — `TenantResolver` | **Real.** Tenant is the org; a Space id is refused. |
| `directory` — `RecipientDirectory` | **Real over a fake control plane.** Reads `GET /api/orgs/{id}/members`. Can route to the in-app surface only: no per-person contact record exists in any of the control plane's 74 migrations, and asking to page someone is refused rather than silently dropped. |
| `meter` — `MeterSink` | **Real over a fake.** Org-scoped dedupe keys, checked before the batch leaves the process. |
| `events` — `EventSink` | **Real over a fake.** `handoff.*` namespace only; a payload carrying an answer or a prompt is refused. |
| `outbox` + `reconciler` | **Real, and tested against a real Postgres.** Durability is not a property a fake can demonstrate. |
| `delivery` — `DeliveryChannel` | **Structure real, transports absent.** Refuses to construct a channel with a single global destination, so per-tenant paging cannot ship on the shared number. |
| `signer` — `ReceiptSigner` | **Refuses.** There is no attestation key, no custody decision, and no service. A stub would produce receipts that look attested. |
| `takeover` — `TakeoverBroker` | **Refuses.** The revocable viewer token is post-M0 and unbuilt; it does **not** fall back to today's broadcast URL. |

`handoff-omegas-server preflight` prints every absent dependency, each naming its surface, its
milestone, and where the decision is recorded.

## Two things an owner has to decide

1. **Machine auth cannot serve an out-of-repo Handoff as specified.** See the module docs in
   `src/auth.rs`. Handoff's public API is blocked on this.
2. **"Do not build a second audit table" cannot hold literally.** See `src/events.rs` for the
   argument and `src/outbox.rs` + `src/reconciler.rs` for the shape.

## Running the tests

```
cargo test --workspace      # 76 tests; five of them need a local Postgres
```

The Postgres-backed tests create and drop disposable `handoff_managed_*` databases, using
`HANDOFF_TEST_ADMIN_URL` (default `postgres://omega:omega@localhost:5432/postgres`) — the same
convention as the open server's own suite.

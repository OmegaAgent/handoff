# Sizing the handoff-v1 Postgres tier — 2026-08-01

## What failed, and the measurement behind it

During the in-network conformance run the node went into `error` state with 3 critical checks and
recovered only on restart. `max_connections` was 300, so the obvious reading — "it ran out of
connection slots" — was wrong. The machine was 256 MB, and measured at idle, with no application
load and effectively one connection:

    total 207 MB   used 157 MB   available 49 MB

Roughly 49 MB of headroom. A Postgres backend costs several MB of RSS, so the node could serve on
the order of ten more connections before exhausting memory, against a configured ceiling of 300 —
about thirty times what it could honour. It accepted connections it had no memory to feed.

The conformance run put `handoffd`'s pool (16) beside the C-23 crash instance's pool (another 16 by
default) plus psql hooks and repmgr's own connections. That is comfortably past ten, which is why
the node fell over there and not during ordinary serving.

## What changed

Memory 256 MB → 1024 MB. Measured after:

    total 962 MB   used 280 MB   available 681 MB

Headroom 49 MB → 681 MB, about fourteen times. At several MB per backend that supports on the order
of a hundred concurrent backends, against `handoffd`'s pool of 16. Verified afterwards: node
`primary` with 3/3 checks passing, `handoff-v1.fly.dev` and `handoff.omegas.dev` both 200.

Cost: shared-cpu-1x 256 MB → 1 GB on Fly, a difference of a few dollars a month on an account that
already runs the app. Not a spending decision anyone needs to weigh.

## What is NOT fixed, stated rather than left to be discovered

`max_connections` is still 300, which remains above what even 1 GB can serve. The right change is a
ceiling the machine can honour — around 100 — so that exhaustion arrives as a refused connection
with a clear error instead of an out-of-memory death that takes the primary with it. It is not done
because `fly pg config update` fails on a flyctl bug that mis-parses the IPv6 address of an
unmanaged Postgres:

    Error: parse "http://fdaa:51:ef82:...:5500/commands/admin/settings/view/postgres": invalid port

The same bug broke `fly postgres attach` earlier in this deployment. Forcing it with `ALTER SYSTEM`
would reach around the path postgres-flex manages its own configuration through, on a live primary,
to gain defence in depth against a failure the memory change already covers by fourteen times. That
trade is not worth taking on a running service; it wants either a fixed flyctl or the flex admin API.

## The wider point about this tier

This is an unmanaged single-node Postgres, which Fly itself says it does not support and whose
operation, management and disaster recovery are the operator's problem. It has no replica, and its
volume is 1 GB. For a service holding receipts that are meant to be permanent and independently
verifiable, single-node with no replica is the more serious exposure — larger than the sizing
question this note answers. `SECURITY.md` already says an exported chain head is the only defence
against tail truncation, and nothing exports one yet.

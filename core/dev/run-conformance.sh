#!/bin/sh
# Run the Handoff conformance suite against a freshly migrated handoffd.
#
# The database is disposable and created per run, for one reason worth stating: several cases raise
# fixture requests that never expire, and a `dedupe_key` collapses onto a `pending` request from an
# earlier run (§3.3 rule 3). A clean store is what makes a run measure this build rather than the
# residue of the last one.
#
#   core/dev/run-conformance.sh              # create, run, tear down
#   KEEP=1 core/dev/run-conformance.sh       # leave the database and server up for inspection
#   PROFILE=… core/dev/run-conformance.sh    # run against another profile, same server
#
# `PROFILE` exists for one caller: the lying-hooks gate in
# `core/crates/handoff-conformance/dev/`, which stands up this same deployment and points the suite
# at a profile whose hooks implement nothing. A suite that cannot be run against a hostile profile
# cannot be shown to reject one.
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)

# One token per run, and every resource this script owns is derived from it.
#
# This is the fix for a whole class of failure rather than a convenience. The database name was
# already per-run, but the port, the crash port and the scratch directory were fixed defaults. Two
# runs therefore did not fail cleanly against each other — they interleaved. One run's exit trap
# dropped the database out from under the other's running server, and the suite reported plausible
# partial failures that read exactly like protocol regressions. The same tree scored 17/24, 19/24,
# 22/24, 23/24 and 24/24 in one evening, and not one of the failures was an assertion about the
# protocol.
#
# Defaults must be safe, because the alternative is every caller remembering to pass three
# variables and the one who forgets silently corrupting somebody else's measurement.
TOKEN="${HANDOFF_RUN_TOKEN:-$(date +%s)_$$}"

# Scratch for this run: the server log, and the private copy of the binaries the hooks exec.
# Two runs sharing one directory overwrite each other's `handoffd` mid-suite, and a hook that
# execs a half-written binary reports a conformance failure that is really a race.
RUN_DIR="${HANDOFF_RUN_DIR:-$HERE/.run/$TOKEN}"
mkdir -p "$RUN_DIR"
export HANDOFF_RUN_DIR="$RUN_DIR"

# Ask the kernel for two free ports rather than guessing. A hardcoded default is what made
# concurrent runs collide in the first place, and an arithmetic offset from a timestamp only makes
# the collision rarer, not impossible.
free_port() {
  python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()'
}

PG_HOST="${PGHOST:-localhost}"
PG_PORT="${PGPORT:-5432}"
PG_USER="${PGUSER:-omega}"
PG_PASSWORD="${PGPASSWORD:-omega}"
export PGPASSWORD="$PG_PASSWORD"

DB="${HANDOFF_DB:-handoff_conf_$TOKEN}"
export PG_ADMIN_URL="postgres://$PG_USER@$PG_HOST:$PG_PORT/postgres"
export HANDOFF_DATABASE_URL="postgres://$PG_USER@$PG_HOST:$PG_PORT/$DB"
export HANDOFF_BOOTSTRAP="$HERE/bootstrap.json"
export HANDOFF_BIND="${HANDOFF_BIND:-127.0.0.1:$(free_port)}"
export HANDOFF_PUBLIC_BASE="http://$HANDOFF_BIND"
# Overridable so a caller can contradict the profile on purpose -- that is what
# mutation-pass.sh does, and a script that hardcodes its own environment cannot be reused
# by the thing testing whether the suite notices when the environment is wrong.
export HANDOFF_LINK_ONLY_PERMITTED="${HANDOFF_LINK_ONLY_PERMITTED:-false}"
export HANDOFF_CALLBACK_SECRETS="${HANDOFF_CALLBACK_SECRETS:-whsec_2f8a91c4e7b3d05a6c1e9f47b28d3a05,whsec_9d41c07be5a2f36819b4d0e7c5a81f62}"
export HANDOFF_SWEEP_INTERVAL_MS=250
export HANDOFF_CRASH_PORT="${HANDOFF_CRASH_PORT:-$(free_port)}"
export CARGO_INCREMENTAL=0

# A guard, not a courtesy: these are somebody else's databases and this script creates and drops.
case "$DB" in
  omega|omega_e2e|postgres|template*) echo "refusing to touch the database named $DB" >&2; exit 1;;
esac

echo "building…"
# Scoped with -p, not just --bin. In a virtual workspace a bare `cargo build --bin` still builds
# every member, so an unrelated crate that is mid-edit stops this suite from producing any number
# at all. The two packages named here are the only ones a conformance run needs.
( cd "$ROOT/core" \
  && cargo build --quiet -p handoff-server --bin handoffd \
  && cargo build --quiet -p handoff-conformance --bin handoff-conformance )

# Point at the built binary directly, NOT at a copy.
#
# An earlier revision copied it here to survive a concurrent `cargo build` relinking it. That made
# things worse: on macOS a freshly linked binary carries an ad-hoc code signature, and copying it
# while the linker is still writing produces a file whose signature does not verify — the kernel
# then SIGKILLs it on exec. The hooks reported `exited -1` with no output, which looks like
# anything except a code-signing failure. The real fix for build races is not to run cargo against
# this target directory while a suite is in flight.
export HANDOFFD="$ROOT/core/target/debug/handoffd"

cleanup() {
  # Wait for it to actually go. A kill that is not waited on leaves the port held for long enough
  # that the next run trips the pre-flight check above — or, before that check existed, silently
  # measured the corpse.
  if [ -n "${SERVER:-}" ]; then
    kill "$SERVER" 2>/dev/null || true
    wait "$SERVER" 2>/dev/null || true
  fi
  if [ "${KEEP:-0}" != "1" ]; then
    psql "$PG_ADMIN_URL" -q -c "drop database if exists \"$DB\" with (force)" >/dev/null 2>&1 || true
  else
    echo "kept: $HANDOFF_DATABASE_URL"
  fi
}
trap cleanup EXIT

# Refuse to run against a server this script did not start.
#
# This is not hypothetical tidiness. An orphaned handoffd from an earlier run keeps the port, our
# own server then fails to bind and exits, and the readiness probe below happily gets a 200 from
# the stranger — so the suite measures a different build against a different database and reports a
# number that means nothing. Whichever way that lands, green or red, it is not a measurement.
if lsof -nP -iTCP:"${HANDOFF_BIND##*:}" -sTCP:LISTEN >/dev/null 2>&1; then
  echo "something is already listening on $HANDOFF_BIND:" >&2
  lsof -nP -iTCP:"${HANDOFF_BIND##*:}" -sTCP:LISTEN >&2
  echo "refusing to run, because the suite would measure that server and not this build" >&2
  exit 1
fi

psql "$PG_ADMIN_URL" -q -c "drop database if exists \"$DB\" with (force)" >/dev/null 2>&1 || true
psql "$PG_ADMIN_URL" -q -c "create database \"$DB\"" >/dev/null
echo "database: $DB"

: > "$RUN_DIR/handoffd.log"
"$HANDOFFD" serve >>"$RUN_DIR/handoffd.log" 2>&1 &
SERVER=$!

# Our own process first, the HTTP probe second. The other order is how a stale server gets
# mistaken for a healthy start: it answers, we break out of the loop, and nobody notices that the
# server we launched is already dead.
for _ in $(seq 1 80); do
  if ! kill -0 "$SERVER" 2>/dev/null; then
    echo "handoffd exited during startup:" >&2
    tail -n 40 "$RUN_DIR/handoffd.log" >&2
    exit 1
  fi
  if curl -sf "http://$HANDOFF_BIND/v1/meta" >/dev/null 2>&1; then break; fi
  sleep 0.25
done
echo "handoffd: $($HANDOFFD --version)"

cd "$ROOT"
set +e
"$ROOT/core/target/debug/handoff-conformance" \
  --base-url "http://$HANDOFF_BIND/v1" \
  --profile "${PROFILE:-$HERE/conformance-profile.yaml}" \
  "$@"
STATUS=$?
set -e
echo "conformance exit code: $STATUS"
exit $STATUS

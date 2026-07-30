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
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
RUN_DIR="$HERE/.run"
mkdir -p "$RUN_DIR"

PG_HOST="${PGHOST:-localhost}"
PG_PORT="${PGPORT:-5432}"
PG_USER="${PGUSER:-omega}"
PG_PASSWORD="${PGPASSWORD:-omega}"
export PGPASSWORD="$PG_PASSWORD"

DB="${HANDOFF_DB:-handoff_conf_$(date +%s)}"
export PG_ADMIN_URL="postgres://$PG_USER@$PG_HOST:$PG_PORT/postgres"
export HANDOFF_DATABASE_URL="postgres://$PG_USER@$PG_HOST:$PG_PORT/$DB"
export HANDOFF_BOOTSTRAP="$HERE/bootstrap.json"
export HANDOFF_BIND="${HANDOFF_BIND:-127.0.0.1:8080}"
export HANDOFF_PUBLIC_BASE="http://$HANDOFF_BIND"
export HANDOFF_LINK_ONLY_PERMITTED=false
export HANDOFF_CALLBACK_SECRETS="whsec_2f8a91c4e7b3d05a6c1e9f47b28d3a05,whsec_9d41c07be5a2f36819b4d0e7c5a81f62"
export HANDOFF_SWEEP_INTERVAL_MS=250
export HANDOFF_CRASH_PORT="${HANDOFF_CRASH_PORT:-8091}"
export CARGO_INCREMENTAL=0

# A guard, not a courtesy: these are somebody else's databases and this script creates and drops.
case "$DB" in
  omega|omega_e2e|postgres|template*) echo "refusing to touch the database named $DB" >&2; exit 1;;
esac

echo "building…"
( cd "$ROOT/core" && cargo build --quiet --bin handoffd --bin handoff-conformance )
export HANDOFFD="$ROOT/core/target/debug/handoffd"

cleanup() {
  [ -n "${SERVER:-}" ] && kill "$SERVER" 2>/dev/null || true
  if [ "${KEEP:-0}" != "1" ]; then
    psql "$PG_ADMIN_URL" -q -c "drop database if exists \"$DB\" with (force)" >/dev/null 2>&1 || true
  else
    echo "kept: $HANDOFF_DATABASE_URL"
  fi
}
trap cleanup EXIT

psql "$PG_ADMIN_URL" -q -c "drop database if exists \"$DB\" with (force)" >/dev/null 2>&1 || true
psql "$PG_ADMIN_URL" -q -c "create database \"$DB\"" >/dev/null
echo "database: $DB"

: > "$RUN_DIR/handoffd.log"
"$HANDOFFD" serve >>"$RUN_DIR/handoffd.log" 2>&1 &
SERVER=$!

for _ in $(seq 1 80); do
  if curl -sf "http://$HANDOFF_BIND/v1/meta" >/dev/null 2>&1; then break; fi
  if ! kill -0 "$SERVER" 2>/dev/null; then
    echo "handoffd exited during startup:" >&2
    tail -n 40 "$RUN_DIR/handoffd.log" >&2
    exit 1
  fi
  sleep 0.25
done
echo "handoffd: $($HANDOFFD --version)"

cd "$ROOT"
set +e
"$ROOT/core/target/debug/handoff-conformance" \
  --base-url "http://$HANDOFF_BIND/v1" \
  --profile "$HERE/conformance-profile.yaml" \
  "$@"
STATUS=$?
set -e
echo "conformance exit code: $STATUS"
exit $STATUS

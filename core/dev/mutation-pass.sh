#!/bin/sh
# Break the server on purpose; assert the suite notices, and notices for the right reason.
#
# The stub gate in conformance/GATE.md proves the suite CAN fail. It does not prove the suite fails
# for the right reason — a suite that failed everything would satisfy it identically. This does:
# each mutation removes exactly one property, and the run must go red in exactly the case that owns
# that property, with every other case still passing. A mutation that reddens the whole suite is as
# uninformative as one that reddens nothing.
#
# Two of the four mutations are configuration and two are source. The source ones are reverted with
# `git checkout --`, never with a stash, and the script refuses to start on a dirty tree so it can
# never discard work that was not its own.
#
# Run: sh core/dev/mutation-pass.sh
set -eu

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
cd "$ROOT"

if [ -n "$(git status --porcelain)" ]; then
  echo "refusing to run on a dirty tree: this script reverts source mutations with git checkout --" >&2
  git status --porcelain >&2
  exit 1
fi

PGHOST="${PGHOST:-localhost}"; PGPORT="${PGPORT:-5432}"
PGUSER="${PGUSER:-omega}"; PGPASSWORD="${PGPASSWORD:-omega}"
export PGHOST PGPORT PGUSER PGPASSWORD
FAILURES=0

free_port () { python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()'; }

# Run the suite against a server started with the environment this function is given, and print the
# case ids that failed. Deliberately does NOT use run-conformance.sh: that script pins the very
# environment variables two of these mutations need to change.
run_with () {  # $1 = extra env, as "K=V K=V"
  db="handoff_mut_$$_$(date +%s)"
  run_dir=$(mktemp -d)
  createdb "$db" >/dev/null 2>&1
  port=$(free_port)
  env $1 \
    HANDOFF_DATABASE_URL="postgres://$PGUSER@$PGHOST:$PGPORT/$db" \
    PG_ADMIN_URL="postgres://$PGUSER@$PGHOST:$PGPORT/postgres" \
    HANDOFF_DB="$db" \
    HANDOFF_RUN_DIR="$run_dir" \
    HANDOFF_BOOTSTRAP="$ROOT/core/dev/bootstrap.json" \
    HANDOFF_BIND="127.0.0.1:$port" \
    HANDOFF_PUBLIC_BASE="http://127.0.0.1:$port" \
    HANDOFF_SWEEP_INTERVAL_MS=250 \
    HANDOFF_CRASH_PORT="$(free_port)" \
    HANDOFFD="$ROOT/core/target/debug/handoffd" \
    "$ROOT/core/target/debug/handoffd" serve >"$run_dir/handoffd.log" 2>&1 &
  server=$!
  i=0
  while [ $i -lt 80 ]; do
    if ! kill -0 "$server" 2>/dev/null; then
      echo "handoffd exited during startup:" >&2; tail -n 20 "$run_dir/handoffd.log" >&2
      dropdb "$db" >/dev/null 2>&1 || true; return 1
    fi
    curl -sf "http://127.0.0.1:$port/v1/meta" >/dev/null 2>&1 && break
    sleep 0.25; i=$((i+1))
  done
  "$ROOT/core/target/debug/handoff-conformance" \
      --base-url "http://127.0.0.1:$port/v1" \
      --profile "$ROOT/core/dev/conformance-profile.yaml" >"$run_dir/out.txt" 2>&1 || true
  kill "$server" 2>/dev/null || true; wait "$server" 2>/dev/null || true
  dropdb "$db" >/dev/null 2>&1 || true
  SUMMARY=$(grep -E '[0-9]+/[0-9]+ passing' "$run_dir/out.txt" | tail -1)
  FAILED=$(grep -E '^FAIL' "$run_dir/out.txt" | awk '{print $2}' | tr '\n' ' ' | sed 's/ *$//')
  rm -rf "$run_dir"
}

# $1 = label, $2 = expected sole failing case ("" = expect all green), $3 = env for the server
expect () {
  run_with "$3" || { echo "  $1: the server would not start"; FAILURES=$((FAILURES+1)); return; }
  printf '%-52s %s  failed: %s\n' "$1" "${SUMMARY:-no summary}" "${FAILED:-none}"
  if [ -z "$2" ]; then
    [ -z "$FAILED" ] || { echo "  expected every case to pass"; FAILURES=$((FAILURES+1)); }
  elif [ "$FAILED" != "$2" ]; then
    # Both halves matter. Nothing failing means the suite cannot see this property at all; more than
    # the owning case failing means the mutation was too broad to attribute.
    echo "  expected exactly $2 to fail, got '${FAILED:-none}'"
    FAILURES=$((FAILURES+1))
  fi
}

echo "building…"
( cd core && cargo build --quiet -p handoff-server --bin handoffd \
           && cargo build --quiet -p handoff-conformance --bin handoff-conformance )

echo
echo "baseline"
expect "unmutated" "" ""

echo
echo "configuration mutations — the deployment contradicts the profile it publishes"
expect "link_only permitted, profile says forbidden" "C-6b" "HANDOFF_LINK_ONLY_PERMITTED=true HANDOFF_CALLBACK_SECRETS=whsec_2f8a91c4e7b3d05a6c1e9f47b28d3a05,whsec_9d41c07be5a2f36819b4d0e7c5a81f62"
expect "callbacks signed with unknown secrets" "C-18" "HANDOFF_LINK_ONLY_PERMITTED=false HANDOFF_CALLBACK_SECRETS=whsec_ffffffffffffffffffffffffffffffff,whsec_eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"

echo
echo "source mutations — the invariant is removed from the code that enforces it"
BASE_ENV="HANDOFF_LINK_ONLY_PERMITTED=false HANDOFF_CALLBACK_SECRETS=whsec_2f8a91c4e7b3d05a6c1e9f47b28d3a05,whsec_9d41c07be5a2f36819b4d0e7c5a81f62"

python3 - <<'PY'
import pathlib
p = pathlib.Path("core/crates/handoff-server/src/routes.rs"); s = p.read_text()
old = "    if principal.kind == PrincipalKind::Machine {"
assert s.count(old) == 1, "require_person no longer looks like this — re-derive the mutation"
p.write_text(s.replace(old, "    if false && principal.kind == PrincipalKind::Machine {", 1))
PY
( cd core && cargo build --quiet -p handoff-server --bin handoffd )
expect "any principal may answer (I15 removed)" "C-5" "$BASE_ENV"
git checkout -- core/crates/handoff-server/src/routes.rs

python3 - <<'PY'
import pathlib, re
p = pathlib.Path("core/crates/handoff-server/src/routes.rs"); s = p.read_text()
m = re.search(r'let slot = slot\(\s*&principal,\s*"answer",\s*&id\.to_string\(\),', s)
assert m, "the answer route's slot() call no longer looks like this — re-derive the mutation"
p.write_text(s[:m.start()] + m.group(0).replace("&id.to_string()", '"any"') + s[m.end():])
PY
( cd core && cargo build --quiet -p handoff-server --bin handoffd )
expect "idempotency slot forgets the object (I20)" "C-25" "$BASE_ENV"
git checkout -- core/crates/handoff-server/src/routes.rs

( cd core && cargo build --quiet -p handoff-server --bin handoffd )
echo
echo "restored"
expect "unmutated again" "" ""

echo
if [ -n "$(git status --porcelain)" ]; then
  echo "the tree is dirty after the pass; a mutation was not reverted:" >&2
  git status --porcelain >&2
  FAILURES=$((FAILURES+1))
fi

if [ "$FAILURES" -ne 0 ]; then
  echo "$FAILURES mutation(s) did not behave as expected — the suite does not measure what it claims"
  exit 1
fi
echo "every mutation was caught by exactly the case that owns it"

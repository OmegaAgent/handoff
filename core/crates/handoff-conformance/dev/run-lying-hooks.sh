#!/bin/sh
# The anti-vacuity gate: run the suite against a profile whose hooks implement nothing.
#
#   core/crates/handoff-conformance/dev/run-lying-hooks.sh
#
# This is the check on the check. A hostile review defeated C-15 twice — first with hooks stubbed
# `true` and `false`, then with hooks that printed the evidence the case asked for — and both times
# the suite reported 25/25 and exit 0 against a deployment with mutable receipts and no chain
# verifier. Nothing in the repository would have noticed, because a check that measures nothing
# looks exactly like one that measures everything.
#
# So the attack is now a gate. `lying-hooks/profile.yaml` is the same reference deployment, the same
# credentials, and the same honest server — with every below-HTTP hook replaced by a script that
# performs no work and prints whatever its case matches on. C-15 MUST be red. If it is ever green
# again, the suite has stopped measuring the one property it exists to measure and this script says
# so with a non-zero exit.
#
# It also prints which other hook cases the liar survives. That list is not a failure of this gate:
# it is the honest surface of what a claimant could fake, and `conformance/GATE.md` publishes it.
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../../.." && pwd)
OUT="${TMPDIR:-/tmp}/handoff-lying-hooks.$$.txt"
trap 'rm -f "$OUT"' EXIT

# Cases whose whole property is below the HTTP API, and which therefore MUST NOT survive a profile
# that implements none of it.
MUST_FAIL="C-15"

echo "running the suite against lying hooks…"
set +e
PROFILE="$HERE/lying-hooks/profile.yaml" "$ROOT/core/dev/run-conformance.sh" >"$OUT" 2>&1
STATUS=$?
set -e

grep -E "^(PASS|FAIL) |passing$" "$OUT" || true

if [ "$STATUS" -eq 0 ]; then
  echo
  echo "GATE FAILED: the suite passed against hooks that implement nothing." >&2
  echo "Every property below the HTTP API is currently claimable by printing a sentence." >&2
  exit 1
fi

FAILED=0
for CASE in $MUST_FAIL; do
  if grep -q "^FAIL  $CASE " "$OUT"; then
    echo "  $CASE is red against the liar, as it must be"
  else
    echo "GATE FAILED: $CASE survived a profile that implements nothing." >&2
    FAILED=1
  fi
done
[ "$FAILED" -eq 0 ] || exit 1

echo
echo "hook cases the liar SURVIVES — attested, not measured (conformance/GATE.md publishes this):"
# Derived from the case files rather than listed here. A hand-maintained list of which cases use
# hooks is exactly the kind of number this project has now watched go stale three times.
for FILE in "$ROOT"/conformance/cases/*.yaml; do
  grep -q "^  hooks:" "$FILE" || continue
  ID=$(sed -n 's/^id: *//p' "$FILE" | head -n 1)
  grep -q "^PASS  $ID " "$OUT" && echo "  $ID"
done
echo
echo "gate passed: the suite rejects hooks that implement nothing"

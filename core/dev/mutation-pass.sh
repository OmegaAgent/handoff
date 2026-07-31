#!/bin/sh
# Break the server on purpose; check the case that owns the property notices.
#
# The stub gate in conformance/GATE.md proves the suite CAN fail. It does not prove the suite fails
# for the right REASON -- a suite that failed everything would satisfy it identically, and so would
# one whose cases all fail for a shared reason like a dead port or a missing hook. This removes one
# property at a time and requires the run to go red in exactly the case that owns it, with every
# other case still passing. Both halves are the assertion: nothing failing means the suite cannot
# see that property, and more than one case failing means the mutation was too broad to attribute.
#
# It drives run-conformance.sh rather than standing up its own deployment. An earlier version of
# this script reimplemented that environment and got it wrong -- the hook-backed cases failed at
# baseline, so every mutation was being measured against a broken run and the numbers meant nothing.
# The lesson is worth the comment: a harness that duplicates the setup it is testing measures the
# duplicate. run-conformance.sh takes the two values these mutations need from the environment for
# exactly this reason.
#
# Source mutations are reverted with `git checkout --`, never a stash. The script refuses to start on
# a dirty tree so it can never discard work that was not its own, and it checks the tree is clean at
# the end, because a mutation pass that leaves a mutation behind is worse than one that never ran.
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

FAILURES=0

# $1 = label, $2 = the case expected to fail ("" = expect every case to pass), $3 = env overrides
expect () {
  out=$(env $3 sh "$ROOT/core/dev/run-conformance.sh" 2>&1 || true)
  summary=$(printf '%s\n' "$out" | grep -E '[0-9]+/[0-9]+ passing' | tail -1)
  failed=$(printf '%s\n' "$out" | grep -E '^FAIL' | awk '{print $2}' | tr '\n' ' ' | sed 's/ *$//')
  printf '%-46s %-16s failed: %s\n' "$1" "${summary:-NO SUMMARY}" "${failed:-none}"
  if [ -z "$2" ]; then
    [ -z "$failed" ] || { echo "  expected every case to pass"; FAILURES=$((FAILURES+1)); }
  elif [ "$failed" != "$2" ]; then
    echo "  expected exactly $2 to fail, got '${failed:-none}'"
    FAILURES=$((FAILURES+1))
  fi
}

echo "baseline"
expect "unmutated" "" ""

echo
echo "configuration mutations -- the deployment contradicts the profile it publishes"
expect "link_only permitted, profile forbids it" "C-6b" "HANDOFF_LINK_ONLY_PERMITTED=true"
expect "callbacks signed with unknown secrets" "C-18" "HANDOFF_CALLBACK_SECRETS=whsec_ffffffffffffffffffffffffffffffff,whsec_eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"

echo
echo "source mutations -- the invariant is removed from the code that enforces it"

python3 - <<'PY'
import pathlib
p = pathlib.Path("core/crates/handoff-server/src/routes.rs"); s = p.read_text()
old = "    if principal.kind == PrincipalKind::Machine {"
assert s.count(old) == 1, "require_person no longer looks like this -- re-derive the mutation"
p.write_text(s.replace(old, "    if false && principal.kind == PrincipalKind::Machine {", 1))
PY
expect "any principal may answer (I15 removed)" "C-5" ""
git checkout -- core/crates/handoff-server/src/routes.rs

python3 - <<'PY'
import pathlib, re
p = pathlib.Path("core/crates/handoff-server/src/routes.rs"); s = p.read_text()
m = re.search(r'let slot = slot\(\s*&principal,\s*"answer",\s*&id\.to_string\(\),', s)
assert m, "the answer route's slot() call no longer looks like this -- re-derive the mutation"
p.write_text(s[:m.start()] + m.group(0).replace("&id.to_string()", '"any"') + s[m.end():])
PY
expect "idempotency slot forgets the object (I20)" "C-25" ""
git checkout -- core/crates/handoff-server/src/routes.rs

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
  echo "$FAILURES mutation(s) did not behave as expected -- the suite does not measure what it claims"
  exit 1
fi
echo "every mutation was caught by exactly the case that owns it"

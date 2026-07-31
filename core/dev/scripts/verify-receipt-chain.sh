#!/bin/sh
# C-15. Re-walk every receipt chain and exit 0 only if every link agrees with the stored head.
#
# The verifier is the open one in handoff-protocol: a pure function over stored receipts,
# recomputing each digest from height, the predecessor's digest, and the receipt's own core hash.
#
# Exit status alone is not what the case asserts, because `true` exits 0 as well. So this reduces
# the verifier's own report to the evidence line C-15 matches against:
#
#     chain_verified head=<digest> height=<n>
#
# one per tenant, and the case requires that pair to be the one `GET /receipts/chain-head` returned
# over HTTP a few steps earlier. A hook that did not walk the chain cannot produce it, and a hook
# that walked some other store produces the wrong one.
set -eu

if OUT=$("${HANDOFFD:?}" verify-chain 2>&1); then STATUS=0; else STATUS=$?; fi
printf '%s\n' "$OUT"

EVIDENCE=$(printf '%s\n' "$OUT" | awk '
  match($0, /head [^ ]+ at height [0-9]+/) {
    split(substr($0, RSTART, RLENGTH), p, " ")
    printf "chain_verified head=%s height=%s\n", p[2], p[5]
  }')

if [ "$STATUS" -eq 0 ] && [ -z "$EVIDENCE" ]; then
  echo "the verifier reported no chain head, so there is nothing to check against the HTTP" >&2
  echo "surface: a green exit code over an empty walk proves nothing" >&2
  exit 1
fi

printf '%s\n' "$EVIDENCE"
exit "$STATUS"

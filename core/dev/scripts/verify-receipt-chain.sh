#!/bin/sh
# C-15. Re-walk every receipt chain and exit 0 only if every link agrees with the stored head.
#
# The verifier is the open one in handoff-protocol: a pure function over stored receipts,
# recomputing each digest from height, the predecessor's digest, and the receipt's own core hash.
set -eu
exec "${HANDOFFD:?}" verify-chain

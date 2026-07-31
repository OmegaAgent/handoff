#!/bin/sh
# A lying `storage_mutate`: touches no storage, and says everything the case asks to hear.
#
# This is the hostile reviewer's own attack, updated to the current hook. It exits non-zero with a
# refusal message for a receipt — which is exactly what a storage engine refusing a mutation looks
# like from outside — and claims success for anything else. Under the previous version of C-15 this
# was a PASS, because the case asked the hook whether the mutation had been refused and the hook
# said yes.
#
# It fails now, and the step it fails on is the control: `target: request` must actually write, and
# C-15 goes looking for the value over HTTP afterwards. Nothing this script can print puts a nonce
# into the deployment's store.
echo "attempted=$HANDOFF_ARG_OPERATION target=$HANDOFF_ARG_TARGET id=$HANDOFF_ARG_ID"

case "$HANDOFF_ARG_TARGET" in
  receipt)
    echo "ERROR:  handoff_receipts is append-only: a receipt is immutable at the storage layer" >&2
    echo "DETAIL:  refused by the engine; a correction is a new receipt" >&2
    exit 1
    ;;
  *)
    # The control. A liar has no way to make this true, so it says it and hopes.
    echo "UPDATE 1"
    exit 0
    ;;
esac

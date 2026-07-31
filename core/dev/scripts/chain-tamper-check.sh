#!/bin/sh
# C-15. Prove the hash chain is load-bearing rather than decorative.
#
# Copies the receipts into a DISPOSABLE database, alters one historical entry there, and re-runs the
# same verifier. Exit 0 only when the head no longer verifies. This must never touch the live store,
# so every write below names the copy.
set -eu

: "${HANDOFF_DATABASE_URL:?}" "${HANDOFFD:?}" "${PG_ADMIN_URL:?}"

COPY="handoff_tamper_$$"
CLEAN_URL=$(printf '%s' "$HANDOFF_DATABASE_URL" | sed "s#/[^/?]*\(?.*\)\{0,1\}\$#/$COPY#")

cleanup() {
  psql "$PG_ADMIN_URL" -q -c "drop database if exists \"$COPY\" with (force)" >/dev/null 2>&1 || true
}
trap cleanup EXIT

psql "$PG_ADMIN_URL" -q -c "create database \"$COPY\"" >/dev/null

# The copy gets the real schema, triggers included, from the same migrations the live store ran.
HANDOFF_DATABASE_URL="$CLEAN_URL" "$HANDOFFD" migrate >/dev/null

COLUMNS="id,tenant_ref,request_id,kind,height,prev_digest,digest,decided_at,decision,body"
psql "$HANDOFF_DATABASE_URL" -q -c "\\copy (select $COLUMNS from handoff_receipts order by tenant_ref, height) to stdout" \
  | psql "$CLEAN_URL" -q -c "\\copy handoff_receipts($COLUMNS) from stdin" >/dev/null

# Which chain to rewrite: the one that owns the receipt the case named.
#
# Earlier this picked "the tenant with the most receipts", which worked but left the hook's report
# unattached to anything the case had independently observed. Taking the tenant from
# HANDOFF_ARG_RECEIPT_ID means `head_before` below is the same head the case read from
# `GET /receipts/chain-head`, so C-15 can require the two to agree rather than take this script's
# word that it tampered with something.
TENANT=$(psql "$CLEAN_URL" -qtA -c \
  "select tenant_ref from handoff_receipts where id = '${HANDOFF_ARG_RECEIPT_ID:?}'")
if [ -z "$TENANT" ]; then
  echo "no receipt named ${HANDOFF_ARG_RECEIPT_ID} is in the copy; there is nothing to tamper with" >&2
  exit 1
fi

# The verifier must agree with the live store before anything is altered. A copy that was already
# broken would make the rest of this script prove nothing. Its report is also where `head_before`
# comes from: the head this chain had while it was still intact.
if ! BEFORE=$(HANDOFF_DATABASE_URL="$CLEAN_URL" "$HANDOFFD" verify-chain --tenant "$TENANT" 2>&1); then
  echo "the untouched copy does not verify; the tamper check would prove nothing" >&2
  printf '%s\n' "$BEFORE" >&2
  exit 1
fi
HEAD_BEFORE=$(printf '%s\n' "$BEFORE" | awk 'match($0, /head [^ ]+ at height/) {
  split(substr($0, RSTART, RLENGTH), p, " "); print p[2] }')
if [ -z "$HEAD_BEFORE" ]; then
  echo "the untouched copy reported no head, so an invalidated head would mean nothing" >&2
  exit 1
fi

# Rewrite history. The trigger that makes this impossible in the live store is dropped *here*,
# in the copy, precisely because the point is to see what a successful rewrite would do to the head.
psql "$CLEAN_URL" -q -c "drop trigger handoff_receipts_no_update on handoff_receipts" >/dev/null

# Alter the FIRST receipt of that ONE tenant, and alter a member every receipt is required to carry.
#
# Two things went wrong in earlier versions of this probe and both produced a false pass.
# `min(height)` without a tenant matches the first receipt of *every* tenant, because heights are
# per-tenant and all of them start at 1. And `jsonb_set` on `{decision,values,decision}` is a no-op
# when an intermediate member is absent — which it is on a policy receipt whose decision carries no
# values — so the "tampered" copy was byte-identical to the original and of course still verified.
# `decided_at` is required on every receipt by the schema, so setting it always changes the core
# hash, and therefore always changes the digest and every digest after it.
#
# `-q` matters: without it psql prints the `UPDATE 1` command tag to stdout alongside the returned
# id, and the row-count guard below then sees two lines and rejects a perfectly correct update.
ALTERED=$(psql "$CLEAN_URL" -qtA -c \
  "update handoff_receipts
     set body = jsonb_set(body, '{decided_at}', '\"2000-01-01T00:00:00Z\"'::jsonb, true)
   where tenant_ref = '$TENANT'
     and height = (select min(height) from handoff_receipts where tenant_ref = '$TENANT')
   returning id")
if [ "$(printf '%s' "$ALTERED" | grep -c .)" != "1" ]; then
  echo "expected to alter exactly one receipt, altered: [$ALTERED]" >&2
  exit 1
fi

# The alteration has to have actually changed the bytes, or the rest of this proves nothing.
if [ "$(psql "$CLEAN_URL" -qtA -c \
      "select body->>'decided_at' from handoff_receipts where id = '$ALTERED'")" \
     != "2000-01-01T00:00:00Z" ]; then
  echo "the update did not change the stored receipt; the tamper check would prove nothing" >&2
  exit 1
fi

if HANDOFF_DATABASE_URL="$CLEAN_URL" "$HANDOFFD" verify-chain --tenant "$TENANT" >/dev/null 2>&1; then
  echo "the chain still verifies after $ALTERED was rewritten: tamper-evidence in name only" >&2
  exit 1
fi

# The evidence line C-15 matches against. `head_before` is the head this chain had while it was
# intact, and the case requires it to be the head the HTTP surface reported — so the hook has to
# have walked the same history the case just read, not merely exited 0.
echo "altering $ALTERED invalidated the chain head, as §9.4 requires"
echo "tamper_detected altered=$ALTERED head_before=$HEAD_BEFORE head_after=did-not-verify"
exit 0

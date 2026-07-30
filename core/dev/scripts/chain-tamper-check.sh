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

# The verifier must agree with the live store before anything is altered. A copy that was already
# broken would make the rest of this script prove nothing.
if ! HANDOFF_DATABASE_URL="$CLEAN_URL" "$HANDOFFD" verify-chain >/dev/null 2>&1; then
  echo "the untouched copy does not verify; the tamper check would prove nothing" >&2
  exit 1
fi

# Rewrite history. The trigger that makes this impossible in the live store is dropped *here*,
# in the copy, precisely because the point is to see what a successful rewrite would do to the head.
psql "$CLEAN_URL" -q -c "drop trigger handoff_receipts_no_update on handoff_receipts" >/dev/null
ALTERED=$(psql "$CLEAN_URL" -tA -c \
  "update handoff_receipts
     set body = jsonb_set(body, '{decision,values,decision}', '\"reject\"'::jsonb, true)
   where height = (select min(height) from handoff_receipts)
   returning id")
if [ -z "$ALTERED" ]; then
  echo "nothing was altered; there is no history to tamper with yet" >&2
  exit 1
fi

if HANDOFF_DATABASE_URL="$CLEAN_URL" "$HANDOFFD" verify-chain >/dev/null 2>&1; then
  echo "the chain still verifies after $ALTERED was rewritten: tamper-evidence in name only" >&2
  exit 1
fi

echo "altering $ALTERED invalidated the chain head, as §9.4 requires"
exit 0

#!/bin/sh
# C-15. One mutation, aimed where the suite says, as the application's own database role.
#
# This is deliberately ONE command with one statement shape per operation, parameterized by target.
# The earlier version of this hook was two commands that only ever attempted a receipt, which meant
# a deployment could stub both with `false` — non-zero is exactly what a refused mutation looks like
# — and nothing in the suite could tell the difference. C-15 now aims the same command at a row the
# engine PERMITS writing and requires the value to appear over HTTP, so a hook that touches no
# storage fails the case. Keeping the receipt path and the request path in one statement shape is
# what makes that control mean something: there is no branch here that could be honest for one
# target and a lie for the other.
#
# Both receipt operations are refused by the triggers in migration 5, which is the point of the
# case. `psql` reports the engine's own message on stderr and exits non-zero because of
# ON_ERROR_STOP.
set -eu

: "${HANDOFF_DATABASE_URL:?}"
TARGET="${HANDOFF_ARG_TARGET:?}"
OPERATION="${HANDOFF_ARG_OPERATION:?}"
ID="${HANDOFF_ARG_ID:?}"
VALUE="${HANDOFF_ARG_VALUE:-}"

# Where an update writes, per target: a member the object's HTTP representation carries, so that a
# mutation which lands is visible to the suite and one that is refused is visibly absent. Aiming at
# a column nothing reads would make the re-read pass either way.
case "$TARGET" in
  receipt) TABLE=handoff_receipts; SET="body = jsonb_set(body, '{decision,note}', to_jsonb(:'value'::text))";;
  request) TABLE=handoff_requests; SET="prompt = jsonb_set(prompt, '{title}', to_jsonb(:'value'::text))";;
  *) echo "storage_mutate: unknown target '$TARGET'" >&2; exit 2;;
esac

case "$OPERATION" in
  update) STATEMENT="update $TABLE set $SET where id = :'id'";;
  delete) STATEMENT="delete from $TABLE where id = :'id'";;
  *) echo "storage_mutate: unknown operation '$OPERATION'" >&2; exit 2;;
esac

echo "attempted=$OPERATION target=$TARGET table=$TABLE id=$ID"

# Through stdin rather than `-c`: psql performs variable interpolation only on input it reads as a
# script, so `-c "… :'id' …"` reaches the server with the colon intact and fails on syntax — which
# looks exactly like a storage engine refusing the write, and passed the two receipt steps of C-15
# for the wrong reason until the control step caught it.
printf '%s\n' "$STATEMENT" |
  psql "$HANDOFF_DATABASE_URL" -v ON_ERROR_STOP=1 -v id="$ID" -v value="$VALUE" -f -

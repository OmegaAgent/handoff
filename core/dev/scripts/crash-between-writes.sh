#!/bin/sh
# C-23. Kill the Server between the state write and the event write, restart, and assert that
# either both are present or neither is.
#
# §18 calls this the case an implementation is most tempted to skip, because emitting the event just
# after the commit passes every happy-path test and only fails under a crash. So this drives a real
# crash: a second handoffd is started with HANDOFF_CRASH_POINT set, which aborts the process inside
# the open answer transaction after the state row is written and before the event row is. If the two
# writes are one transaction, the rollback takes both.
set -eu

: "${HANDOFF_DATABASE_URL:?}" "${HANDOFFD:?}" "${HANDOFF_BOOTSTRAP:?}"

PORT="${HANDOFF_CRASH_PORT:-8131}"
BASE="http://127.0.0.1:$PORT/v1"
RUN_DIR="${HANDOFF_RUN_DIR:-$(dirname "$0")/../.run}"
LOG="$RUN_DIR/crash-instance.log"
MACHINE="omg_handoff_test_ka_conformance"
HUMAN="hs_editor_one_conformance"
RUN_ID="crash-$$"

mkdir -p "$RUN_DIR"

start_instance() {
  crash_point="$1"
  HANDOFF_BIND="127.0.0.1:$PORT" \
  HANDOFF_CRASH_POINT="$crash_point" \
  "$HANDOFFD" serve >>"$LOG" 2>&1 &
  INSTANCE=$!
  for _ in $(seq 1 60); do
    if curl -sf "$BASE/meta" >/dev/null 2>&1; then return 0; fi
    sleep 0.25
  done
  echo "the crash instance never came up; see $LOG" >&2
  return 1
}

stop_instance() {
  [ -n "${INSTANCE:-}" ] && kill "$INSTANCE" 2>/dev/null || true
  wait "$INSTANCE" 2>/dev/null || true
  INSTANCE=""
}

trap stop_instance EXIT

# ---- 1. A fresh request, raised through the instance that is about to die.
start_instance answer_after_state_write

REQUEST=$(curl -sS -X POST "$BASE/requests" \
  -H "Authorization: Bearer $MACHINE" \
  -H "Content-Type: application/json" \
  -H "Idempotency-Key: $RUN_ID" \
  -d '{"waiter_ref":"run:'"$RUN_ID"'","prompt":{"title":"A request whose answer will be interrupted"},
       "requires":{"v":1,"answer":{"fields":[{"name":"decision","type":"choice","required":true,
       "options":[{"id":"approve","label":"Approve"},{"id":"reject","label":"Reject"}]}]},
       "capabilities":[],"authority":{"min_role":"editor","auth_strength":"session"}}}' \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')

if [ -z "$REQUEST" ]; then
  echo "could not raise the request the crash is supposed to interrupt" >&2
  exit 1
fi

# ---- 2. Answer it. The server aborts mid-transaction; this call therefore never returns a body.
curl -sS -X POST "$BASE/requests/$REQUEST/answer" \
  -H "Authorization: Bearer $HUMAN" \
  -H "Content-Type: application/json" \
  -H "Idempotency-Key: $RUN_ID-answer" \
  -d '{"values":{"decision":"approve"}}' >/dev/null 2>&1 || true

sleep 1
if kill -0 "$INSTANCE" 2>/dev/null; then
  echo "the server did not crash; HANDOFF_CRASH_POINT was not honoured, so nothing was proved" >&2
  exit 1
fi
INSTANCE=""

# ---- 3. Restart, and read what survived.
start_instance ""

STATE=$(psql "$HANDOFF_DATABASE_URL" -tA -c "select state from handoff_requests where id = '$REQUEST'")
EVENTS=$(psql "$HANDOFF_DATABASE_URL" -tA -c "select count(*) from handoff_events where request_id = '$REQUEST' and type = 'request.answered'")
RECEIPTS=$(psql "$HANDOFF_DATABASE_URL" -tA -c "select count(*) from handoff_receipts where request_id = '$REQUEST'")

STATE_PRESENT=0; [ "$STATE" = "answered" ] && STATE_PRESENT=1
EVENT_PRESENT=0; [ "$EVENTS" -gt 0 ] && EVENT_PRESENT=1

if [ "$STATE_PRESENT" != "$EVENT_PRESENT" ]; then
  echo "after the crash the state says '$STATE' and there are $EVENTS request.answered event(s):" >&2
  echo "the state change and its event disagree, which is I12 broken" >&2
  exit 1
fi
if [ "$STATE_PRESENT" = "1" ] && [ "$RECEIPTS" -eq 0 ]; then
  echo "the request is answered with no receipt: the outcome and its record disagree" >&2
  exit 1
fi

# ---- 4. And the request named by the case, which was answered normally, still has both.
NAMED="${HANDOFF_ARG_REQUEST_ID:-}"
if [ -n "$NAMED" ]; then
  NAMED_STATE=$(psql "$HANDOFF_DATABASE_URL" -tA -c "select state from handoff_requests where id = '$NAMED'")
  NAMED_EVENTS=$(psql "$HANDOFF_DATABASE_URL" -tA -c "select count(*) from handoff_events where request_id = '$NAMED' and type = 'request.answered'")
  if [ "$NAMED_STATE" = "answered" ] && [ "$NAMED_EVENTS" -eq 0 ]; then
    echo "$NAMED is answered but has no request.answered event" >&2
    exit 1
  fi
fi

echo "crashed between the two writes; after restart the request is '$STATE' with $EVENTS answered event(s) — they agree"
exit 0

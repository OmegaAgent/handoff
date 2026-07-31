#!/bin/sh
# C-21. Inject a message that arrived on a channel.
#
# It is recorded as a provisional answer and settles nothing. §4.7: a Server MUST NOT derive a
# decision from message content, however authenticated the channel.
#
# Exiting 0 is not the assertion. `true` exits 0 without injecting anything, and a case that only
# checks the status is then asserting that nothing happened after nothing happened. So this prints
# the evidence C-21 matches against:
#
#     channel_message=<row id> request=<request id> channel=<name> request_state=<state>
#
# The id is read back out of storage, so the message row has to exist; the state is read back in
# the same breath, so the case sees both that the injection landed and that it settled nothing.
set -eu

: "${HANDOFF_DATABASE_URL:?}" "${HANDOFFD:?}"
REQUEST="${HANDOFF_ARG_REQUEST_ID:?}"
CHANNEL="${HANDOFF_ARG_CHANNEL:-email}"

"$HANDOFFD" inject-channel-message \
  --request "$REQUEST" \
  --channel "$CHANNEL" \
  --text "${HANDOFF_ARG_TEXT:-}"

MESSAGE=$(psql "$HANDOFF_DATABASE_URL" -qtA -c \
  "select id from handoff_channel_messages
    where request_id = '$REQUEST' and channel = '$CHANNEL' order by id desc limit 1")
if [ -z "$MESSAGE" ]; then
  echo "the injection reported success but recorded no $CHANNEL message for $REQUEST" >&2
  exit 1
fi

STATE=$(psql "$HANDOFF_DATABASE_URL" -qtA -c \
  "select state from handoff_requests where id = '$REQUEST'")

echo "channel_message=$MESSAGE request=$REQUEST channel=$CHANNEL request_state=$STATE"

#!/bin/sh
# C-21. Inject a message that arrived on a channel.
#
# It is recorded as a provisional answer and settles nothing. §4.7: a Server MUST NOT derive a
# decision from message content, however authenticated the channel.
set -eu
exec "${HANDOFFD:?}" inject-channel-message \
  --request "${HANDOFF_ARG_REQUEST_ID:?}" \
  --channel "${HANDOFF_ARG_CHANNEL:-email}" \
  --text "${HANDOFF_ARG_TEXT:-}"

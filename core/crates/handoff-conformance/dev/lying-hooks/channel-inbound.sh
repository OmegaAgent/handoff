#!/bin/sh
# A lying `channel_inbound`: injects nothing, and reports the injection C-21 asks about.
#
# `request=` and `request_state=pending` are both handed to it — the request id as an argument, and
# the state by the case, which has just asserted it over HTTP. Nothing here reaches a channel.
echo "channel_message=cm_01LIARLIARLIARLIARLIARLIAR request=${HANDOFF_ARG_REQUEST_ID} channel=${HANDOFF_ARG_CHANNEL} request_state=pending"

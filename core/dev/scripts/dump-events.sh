#!/bin/sh
# C-23. Print one event type per line for a request, oldest first.
#
# The protocol defines events in §6.2 but publishes no endpoint that lists them, so a black-box
# client has nothing to read. This is the deployment supplying that view.
set -eu
exec "${HANDOFFD:?}" dump-events --request "${HANDOFF_ARG_REQUEST_ID:?}"

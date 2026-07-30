#!/bin/sh
# C-22. The runtime observes the target page change, with no person asserting anything.
#
# §9.7: clearance MUST be asserted, never inferred. This records an observation and mints nothing.
set -eu
exec "${HANDOFFD:?}" observe-page-change --request "${HANDOFF_ARG_REQUEST_ID:?}"

#!/bin/sh
# C-22. The runtime observes the target page change, with no person asserting anything.
#
# §9.7: clearance MUST be asserted, never inferred. This records an observation and mints nothing.
#
# Exiting 0 is not the assertion: `true` observes nothing and exits 0, which leaves C-22 checking
# that a request nobody touched is still pending. So this prints the evidence C-22 matches against:
#
#     observation=<row id> request=<request id> request_state=<state> receipts=<n>
#
# All three are read back out of storage after the observation is recorded — the observation exists,
# the request did not move, and no receipt was minted for it.
set -eu

: "${HANDOFF_DATABASE_URL:?}" "${HANDOFFD:?}"
REQUEST="${HANDOFF_ARG_REQUEST_ID:?}"

"$HANDOFFD" observe-page-change --request "$REQUEST"

OBSERVATION=$(psql "$HANDOFF_DATABASE_URL" -qtA -c \
  "select id from handoff_observations where request_id = '$REQUEST' order by id desc limit 1")
if [ -z "$OBSERVATION" ]; then
  echo "the observation reported success but recorded nothing for $REQUEST" >&2
  exit 1
fi

STATE=$(psql "$HANDOFF_DATABASE_URL" -qtA -c \
  "select state from handoff_requests where id = '$REQUEST'")
RECEIPTS=$(psql "$HANDOFF_DATABASE_URL" -qtA -c \
  "select count(*) from handoff_receipts where request_id = '$REQUEST'")

echo "observation=$OBSERVATION request=$REQUEST request_state=$STATE receipts=$RECEIPTS"

#!/bin/sh
# A lying `events`: emits every event type C-23 looks for, from a store it never read.
#
# The case matches one type per step, so printing all of them satisfies every step at once. This is
# the shape of lie that a regex search over concatenated output cannot see: there is no assertion
# that the events are only these, or that they belong to this request.
for type in request.raised request.amended request.escalated request.answered \
            request.expired request.cancelled request.superseded attempt.lapsed; do
  echo "$type request=${HANDOFF_ARG_REQUEST_ID} at=2026-07-31T11:00:00Z"
done

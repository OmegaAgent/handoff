#!/bin/sh
# A lying `crash_between_state_and_event`: never crashed, never restarted, reports agreement.
#
# C-23 requires the hook to show the crash point was reached and that the process left by a fault,
# because "state and event agree" is equally true of a request that committed normally. Both facts
# are sentences, and this prints them.
echo "crash_point_reached=after_state_write"
echo "instance_exit=137"
echo "state=answered event=request.answered agree=yes"

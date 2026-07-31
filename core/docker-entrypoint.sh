#!/bin/sh
# Materialise the bootstrap credentials without ever putting them in an image layer.
#
# `handoffd` seeds principals from a JSON FILE (HANDOFF_BOOTSTRAP). A container platform hands
# secrets over as environment variables, so something has to bridge the two, and where that file
# lands matters: baked into the image it is published forever and to everyone who pulls it; written
# to the container's writable layer it survives on disk for the life of the machine.
#
# So it goes to /dev/shm, which is tmpfs -- memory only, never written to a disk layer, gone when
# the process is -- with a mode that excludes everyone but this user. If HANDOFF_BOOTSTRAP_JSON is
# not set, nothing is written and handoffd starts with whatever principals its store already holds,
# which is the normal state after the first boot.
set -eu

if [ -n "${HANDOFF_BOOTSTRAP_JSON:-}" ]; then
  BOOTSTRAP_FILE="/dev/shm/handoff-bootstrap.json"
  ( umask 077 && printf '%s' "$HANDOFF_BOOTSTRAP_JSON" > "$BOOTSTRAP_FILE" )
  export HANDOFF_BOOTSTRAP="$BOOTSTRAP_FILE"
  # Unset before exec so the credentials are not sitting in the served process's environment, where
  # anything that dumps `environ` -- a crash handler, a debug endpoint, a future `/debug/env` nobody
  # has written yet -- would carry them out. The file is already open to the process that needs it.
  unset HANDOFF_BOOTSTRAP_JSON
fi

exec /usr/local/bin/handoffd "$@"

#!/bin/sh
# C-24. Canonicalize a document per RFC 8785 and report its bytes and digest.
#
# Two implementations that disagree about number formatting or member ordering compute different
# digests for the same receipt, and nothing errors — the chain simply stops verifying for somebody,
# later. Running this against the published fixtures is how this deployment shows its
# canonicalization agrees with everyone else's.
set -eu
if [ -n "${HANDOFF_ARG_PATH:-}" ]; then
  exec "${HANDOFFD:?}" canonicalize --path "$HANDOFF_ARG_PATH"
fi
exec "${HANDOFFD:?}" canonicalize --json "${HANDOFF_ARG_JSON:?}"

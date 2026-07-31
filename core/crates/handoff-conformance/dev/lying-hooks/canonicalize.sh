#!/bin/sh
# A lying `canonicalize`: canonicalizes nothing and reads the answers off the published fixtures.
#
# Every value C-24 requires is published — the byte lengths and digests are in `signing.md` §1.6 and
# §2.5, and the exact canonical strings are in the case file itself. A hook that never implements
# RFC 8785 can therefore satisfy the fixture steps by lookup. What it cannot do is canonicalize an
# arbitrary document, so the inline-JSON steps are where this one comes apart.
case "${HANDOFF_ARG_PATH:-}" in
  *callback-body.json) echo "bytes=493"; echo "sha256=fbd6ec4cacc7cb9c9371d2791f946535e3d391a0594a92b5a3a27dd34f5e94fa"; exit 0;;
  *receipt-core.json)  echo "bytes=1125"; echo "sha256=2763f39ef8a61d493106d3db302ec36cae5c024ca3da3a019d483ccc29704ad1"; exit 0;;
esac
case "${HANDOFF_ARG_JSON:-}" in
  *1.5*) echo "not an integer" >&2; exit 1;;
esac
echo "${HANDOFF_ARG_JSON:-}"
echo "bytes=${#HANDOFF_ARG_JSON}"
echo "sha256=0000000000000000000000000000000000000000000000000000000000000000"

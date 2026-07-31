#!/bin/sh
# Mint receipts with the reference server, then verify them with the published SDKs.
#
# This is the only check in the repository that spans the server-versus-SDK seam, and that seam is
# where every canonicalization defect this project has shipped actually lived. Both of them —
# receipts a Python holder read as forged because of member ordering, and a receipt neither SDK
# could canonicalize because a float reached a digest-covered column — passed every suite we had.
# They had to: the conformance harness links `handoff-protocol`, so it canonicalizes with the same
# code the server does. Agreement between a producer and a verifier that share an implementation is
# self-consistency, and self-consistency is exactly what a hash chain is not supposed to require.
#
# Conformance case C-26 hands a server-minted receipt to a spec-derived verifier, and for a third
# party that is genuinely independent. It is not independent for us. This script is: the Python and
# TypeScript SDKs share no line of code with `handoffd`, so when all three agree on the bytes, that
# is evidence.
#
#   scripts/verify-minted-receipts.sh          # create a database, mint, verify, tear down
#   KEEP=1 scripts/verify-minted-receipts.sh   # leave the database and server up for inspection
#
# Exits non-zero if any minted receipt fails to verify under either SDK, if the two SDKs disagree
# with each other, or if the server serves a number in a form its own canonicalizer would not emit.
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/.." && pwd)

# One token per run, and every resource this script owns derives from it — database, port, scratch
# directory. `core/dev/run-conformance.sh` learned this the expensive way: fixed ports and a fixed
# scratch directory meant two runs did not fail against each other, they interleaved, and the
# resulting failures read exactly like protocol regressions.
TOKEN="${HANDOFF_RUN_TOKEN:-$(date +%s)_$$}"
RUN_DIR="${HANDOFF_RUN_DIR:-$HERE/.run/verify-minted-$TOKEN}"
mkdir -p "$RUN_DIR"

free_port() {
  python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()'
}

PG_HOST="${PGHOST:-localhost}"
PG_PORT="${PGPORT:-5432}"
PG_USER="${PGUSER:-omega}"
export PGPASSWORD="${PGPASSWORD:-omega}"

DB="${HANDOFF_DB:-handoff_minted_$TOKEN}"
PG_ADMIN_URL="postgres://$PG_USER@$PG_HOST:$PG_PORT/postgres"
export HANDOFF_DATABASE_URL="postgres://$PG_USER@$PG_HOST:$PG_PORT/$DB"
export HANDOFF_BOOTSTRAP="$ROOT/core/dev/bootstrap.json"
export HANDOFF_BIND="${HANDOFF_BIND:-127.0.0.1:$(free_port)}"
export HANDOFF_PUBLIC_BASE="http://$HANDOFF_BIND"
export HANDOFF_LINK_ONLY_PERMITTED=false
export CARGO_INCREMENTAL=0

BASE="http://$HANDOFF_BIND/v1"
MACHINE="omg_handoff_test_ka_conformance"
HUMAN="hs_editor_one_conformance"

# A guard, not a courtesy: this script creates and drops databases.
case "$DB" in
  omega|omega_e2e|postgres|template*) echo "refusing to touch the database named $DB" >&2; exit 1;;
esac

cleanup() {
  if [ -n "${SERVER:-}" ]; then
    kill "$SERVER" 2>/dev/null || true
    wait "$SERVER" 2>/dev/null || true
  fi
  if [ "${KEEP:-0}" != "1" ]; then
    psql "$PG_ADMIN_URL" -q -c "drop database if exists \"$DB\" with (force)" >/dev/null 2>&1 || true
    rm -rf "$RUN_DIR"
  else
    echo "kept: $HANDOFF_DATABASE_URL"
    echo "kept: $RUN_DIR"
  fi
}
trap cleanup EXIT

echo "building handoffd…"
( cd "$ROOT/core" && cargo build --quiet -p handoff-server --bin handoffd )
HANDOFFD="$ROOT/core/target/debug/handoffd"

# Refuse to measure a server this script did not start. An orphan holding the port answers the
# readiness probe happily, and then every number below describes somebody else's build.
if lsof -nP -iTCP:"${HANDOFF_BIND##*:}" -sTCP:LISTEN >/dev/null 2>&1; then
  echo "something is already listening on $HANDOFF_BIND — refusing to run" >&2
  exit 1
fi

psql "$PG_ADMIN_URL" -q -c "drop database if exists \"$DB\" with (force)" >/dev/null 2>&1 || true
psql "$PG_ADMIN_URL" -q -c "create database \"$DB\"" >/dev/null
echo "database: $DB"

: > "$RUN_DIR/handoffd.log"
"$HANDOFFD" serve >>"$RUN_DIR/handoffd.log" 2>&1 &
SERVER=$!

for _ in $(seq 1 80); do
  if ! kill -0 "$SERVER" 2>/dev/null; then
    echo "handoffd exited during startup:" >&2
    tail -n 40 "$RUN_DIR/handoffd.log" >&2
    exit 1
  fi
  if curl -sf "$BASE/meta" >/dev/null 2>&1; then break; fi
  sleep 0.25
done
curl -sf "$BASE/meta" >/dev/null || { echo "handoffd never became ready" >&2; exit 1; }

# ---------------------------------------------------------------------------------------------
# Mint
# ---------------------------------------------------------------------------------------------
#
# The payloads are adversarial on purpose, and each element is here because it is a place two
# canonicalizers can disagree without either one erroring:
#
#   * `！` (U+FF01) and `😀` (U+1F600) as object keys, together. Neither alone proves anything —
#     the orders differ only when a non-BMP name meets a BMP name above U+D7FF, because the
#     surrogate pair begins 0xD83D and therefore sorts below what its code point sorts above.
#   * an empty-string key, which sorts before everything and is easy to mishandle.
#   * numbers at both ends of ±(2^53 − 1), the last values that survive a double intact.
#   * a float that denotes a whole number. §1.4 accepts it by value and requires it to be stored
#     and served in the form the canonicalizer emits, so it must come back as `0`. Before that
#     rule was implemented the server stored the arrival form, and the receipt it minted could not
#     be canonicalized by either SDK.
#   * a string carrying JSON escapes and a raw non-ASCII character, since escaping is the other
#     half of canonical string output.
#
# They go in through `document` and `number` fields on the ordinary answer path, with no privilege
# beyond a normal human answerer — a receipt minted any other way would not prove anything about
# what a real tenant can put in a chain.
#
# The bodies are written by Python rather than assembled as shell strings. That is not
# fastidiousness: the payloads are made of backslashes, quotes, tabs and newlines, and a shell
# quoting slip turns an escape into a real control character, which the server rejects as malformed
# before any of this is measured. Writing them in one place also puts every adversarial element on
# screen next to the reason it is there. Non-ASCII characters are written as `\u` escapes so this
# file stays ASCII and no editor, terminal or CI log viewer can normalize away the very characters
# under test.
python3 - "$RUN_DIR" <<'BODIESEOF'
import json, sys
from pathlib import Path

run = Path(sys.argv[1])

PAYLOADS = {
    # Ordering and escaping. U+FF01 and U+1F600 must appear together: neither alone proves
    # anything, because the two orderings differ only where a non-BMP name meets a BMP name above
    # U+D7FF. The empty key sorts before everything. The nested objects put the same pair below the
    # top level, where a canonicalizer that sorted only the root would still pass.
    "ordering": {
        "payload": {
            "\uff01": 1,
            "\U0001f600": 2,
            "a": 0,
            "": 3,
            "nested": {"\U0001f600": {"\uff01": [1, 2]}, "": {"z": 0, "\uff01": 1}},
        },
        "at_max_safe": 9007199254740991,
        "at_min_safe": -9007199254740991,
        "note": "caf\u00e9 \"quoted\" \\ backslash\nsecond line\ttab",
    },
    # 1.4 rule 3, which is about the form at rest. Every number here is integral in value and so is
    # accepted, and every one must come back written the way the canonicalizer writes it. Before
    # that rule was implemented the server stored the arrival form, and -0.0 produced a receipt
    # neither published SDK could canonicalize at all.
    "normalization": {
        "payload": {
            "amount": -0.0,
            "scaled": 1e2,
            "whole": 2.0,
            "nested": [3.0, {"deep": -0.0}],
        },
        "at_max_safe": 9007199254740991,
        "at_min_safe": 0,
        "note": "a float that denotes a whole number",
    },
}

for name, values in PAYLOADS.items():
    (run / f"answer-{name}.json").write_text(
        json.dumps({"values": values}, ensure_ascii=False), encoding="utf-8"
    )
BODIESEOF

mint() {
  name=$1
  raise=$(curl -sS -X POST "$BASE/requests" \
    -H "Authorization: Bearer $MACHINE" \
    -H "Content-Type: application/json" \
    -H "Idempotency-Key: minted-$name-$TOKEN" \
    -d '{
      "waiter_ref": "run:minted-'"$name"'-'"$TOKEN"'",
      "liveness": "durable",
      "prompt": {"title": "Adversarial canonicalization payload ('"$name"')"},
      "requires": {
        "v": 1,
        "answer": {"fields": [
          {"name": "payload", "label": "Payload", "type": "document", "required": true},
          {"name": "at_max_safe", "label": "Largest exact integer", "type": "number", "required": true},
          {"name": "at_min_safe", "label": "Smallest exact integer", "type": "number", "required": true},
          {"name": "note", "label": "Note", "type": "text", "required": true}
        ]},
        "capabilities": [],
        "authority": {"min_role": "editor", "auth_strength": "session"}
      }
    }')
  id=$(printf '%s' "$raise" | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])' 2>/dev/null) || {
    echo "raise failed for $name: $raise" >&2; exit 1;
  }

  # --data-binary, not -d: -d strips carriage returns and line feeds out of a body read from a
  # file, and one of these payloads deliberately tests how a newline inside a string is escaped.
  # Stripping it would quietly remove the thing being measured.
  answered=$(curl -sS -X POST "$BASE/requests/$id/answer" \
    -H "Authorization: Bearer $HUMAN" \
    -H "Content-Type: application/json" \
    -H "Idempotency-Key: minted-answer-$name-$TOKEN" \
    --data-binary "@$RUN_DIR/answer-$name.json")
  printf '%s' "$answered" | python3 -c '
import json, sys
body = json.load(sys.stdin)
if not body.get("receipt"):
    print("answer did not mint a receipt: " + json.dumps(body)[:400], file=sys.stderr)
    raise SystemExit(1)
' || exit 1
  echo "minted: $name"
}

mint "ordering"
mint "normalization"

# ---------------------------------------------------------------------------------------------
# Export
# ---------------------------------------------------------------------------------------------
curl -sS "$BASE/receipts" -H "Authorization: Bearer $MACHINE" > "$RUN_DIR/receipts.json"
COUNT=$(python3 -c 'import json,sys; print(len(json.load(open(sys.argv[1]))["data"]))' "$RUN_DIR/receipts.json")
if [ "$COUNT" -lt 2 ]; then
  echo "expected at least 2 minted receipts, got $COUNT" >&2
  exit 1
fi
echo "exported $COUNT receipts"

# ---------------------------------------------------------------------------------------------
# Verify — Python SDK
# ---------------------------------------------------------------------------------------------
cat > "$RUN_DIR/verify.py" <<'PYEOF'
import json, sys
from pathlib import Path

root = Path(sys.argv[1])
sys.path.insert(0, str(root / "sdk" / "python"))
from handoff.signing import verify_chain, verify_receipt_chain   # noqa: E402

try:
    from handoff.errors import NonConformingDocument               # noqa: E402
except ImportError:                       # an older SDK signalled this as a bare ValueError
    NonConformingDocument = ValueError

raw = Path(sys.argv[2]).read_text(encoding="utf-8")

# 1.4 rule 3, checked from outside exactly as the specification says it can be: a digest-covered
# number must be served in the form the canonicalizer emits, and the canonicalizer emits integers.
# A float literal anywhere in a served receipt means the bytes at rest are not the bytes an auditor
# would canonicalize, which is the whole defect. It has to be caught at parse time, on the text:
# `-0.0 == 0` is true, so nothing compared after parsing can see it.
floats = []
def note_float(text):
    floats.append(text)
    return float(text)

receipts = json.loads(raw, parse_float=note_float)["data"]

# Three outcomes, kept apart on purpose. `failed` means the digest did not recompute — something
# changed after the receipt was sealed. `non_conforming` means the receipt never had a computable
# digest at all, which is a Server defect and not a tampering finding. Collapsing them is the thing
# the SDKs were just fixed not to do, so this script must not re-collapse them either.
failed, non_conforming = [], []
for receipt in receipts:
    try:
        if not verify_receipt_chain(receipt):
            failed.append(receipt["id"])
    except NonConformingDocument as exc:
        non_conforming.append({"id": receipt["id"], "reason": str(exc)[:160]})

try:
    chain_ok = verify_chain(receipts)
except NonConformingDocument:
    chain_ok = False

# Always written, even on failure: a missing verdict would leave the cross-SDK comparison below
# with nothing to compare, and report that instead of the real finding.
Path(sys.argv[3]).write_text(json.dumps({
    "sdk": "python",
    "count": len(receipts),
    "failed": failed,
    "non_conforming": [item["id"] for item in non_conforming],
    "chain_ok": chain_ok,
}), encoding="utf-8")

if floats:
    print(f"FAIL python: the server served {len(floats)} non-integer number(s) in digest-covered "
          f"content: {floats[:8]}", file=sys.stderr)
    print("       1.4 rule 3 requires the stored and served form to be the form the canonicalizer "
          "emits, so no digest an auditor computes is the digest that was sealed", file=sys.stderr)
if failed:
    print(f"FAIL python: {len(failed)} receipt(s) did not verify: {failed}", file=sys.stderr)
for item in non_conforming:
    print(f"FAIL python: {item['id']} has no canonical form: {item['reason']}", file=sys.stderr)
if not chain_ok:
    print("FAIL python: verify_chain over the tenant returned False", file=sys.stderr)

sys.exit(1 if (floats or failed or non_conforming or not chain_ok) else 0)
PYEOF
# ---------------------------------------------------------------------------------------------
# Verify — TypeScript SDK
# ---------------------------------------------------------------------------------------------
cat > "$RUN_DIR/verify.mjs" <<'TSEOF'
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const [root, receiptsPath, outPath] = process.argv.slice(2);
const sdk = await import(join(root, "sdk", "ts", "src", "index.ts"));
const { verifyChain, verifyReceiptChain, NonConformingDocument } = sdk;

const receipts = JSON.parse(readFileSync(receiptsPath, "utf8")).data;

// The same three outcomes the Python verifier keeps apart, for the same reason.
const failed = [];
const nonConforming = [];
for (const receipt of receipts) {
  try {
    if (!(await verifyReceiptChain(receipt))) failed.push(receipt.id);
  } catch (error) {
    if (NonConformingDocument && error instanceof NonConformingDocument) {
      nonConforming.push({ id: receipt.id, reason: String(error.message).slice(0, 160) });
    } else {
      throw error;
    }
  }
}

let chainOk;
try {
  chainOk = await verifyChain(receipts);
} catch {
  chainOk = false;
}

writeFileSync(outPath, JSON.stringify({
  sdk: "typescript",
  count: receipts.length,
  failed,
  non_conforming: nonConforming.map((item) => item.id),
  chain_ok: chainOk,
}));

if (failed.length) console.error(`FAIL typescript: ${failed.length} receipt(s) did not verify: ${failed}`);
for (const item of nonConforming) {
  console.error(`FAIL typescript: ${item.id} has no canonical form: ${item.reason}`);
}
if (!chainOk) console.error("FAIL typescript: verifyChain over the tenant returned false");

process.exit(failed.length || nonConforming.length || !chainOk ? 1 : 0);
TSEOF

STATUS=0

echo "verifying with the Python SDK…"
python3 "$RUN_DIR/verify.py" "$ROOT" "$RUN_DIR/receipts.json" "$RUN_DIR/python.json" || STATUS=1

echo "verifying with the TypeScript SDK…"
( cd "$ROOT/sdk/ts" && node --experimental-strip-types --no-warnings \
    "$RUN_DIR/verify.mjs" "$ROOT" "$RUN_DIR/receipts.json" "$RUN_DIR/typescript.json" ) || STATUS=1

# ---------------------------------------------------------------------------------------------
# Compare the two verdicts
# ---------------------------------------------------------------------------------------------
#
# Separate from either SDK failing, and worth its own exit path: two published verifiers reaching
# different conclusions about the same bytes is the finding, whichever of them is right. It is the
# shape of both defects this script exists to catch, and it is the one thing a protocol cannot ship
# with.
python3 - "$RUN_DIR" <<'CMPEOF' || STATUS=1
import json, sys
from pathlib import Path

run = Path(sys.argv[1])
verdicts = {}
for name in ("python", "typescript"):
    path = run / f"{name}.json"
    if path.exists():
        verdicts[name] = json.loads(path.read_text())

if len(verdicts) < 2:
    missing = {"python", "typescript"} - set(verdicts)
    print(f"FAIL: {', '.join(sorted(missing))} produced no verdict at all — it crashed rather "
          f"than reaching a conclusion, so the two cannot be compared", file=sys.stderr)
    raise SystemExit(1)

py, ts = verdicts["python"], verdicts["typescript"]
disagreements = []
for field, label in (("failed", "digest did not recompute"),
                     ("non_conforming", "no canonical form")):
    if sorted(py[field]) != sorted(ts[field]):
        disagreements.append(
            f"{label}: python {sorted(py[field]) or 'none'}, typescript {sorted(ts[field]) or 'none'}"
        )
if bool(py["chain_ok"]) != bool(ts["chain_ok"]):
    disagreements.append(f"whole chain: python {py['chain_ok']}, typescript {ts['chain_ok']}")

if disagreements:
    print("FAIL: the two published SDKs disagree about the same server-minted bytes:", file=sys.stderr)
    for line in disagreements:
        print(f"  - {line}", file=sys.stderr)
    print("       Whichever of them is right, a protocol cannot ship with two published verifiers "
          "reaching different conclusions about one receipt.", file=sys.stderr)
    raise SystemExit(1)

print(f"both SDKs agree: {py['count']} receipts verify, chain verifies")
CMPEOF
if [ "$STATUS" -eq 0 ]; then
  echo "OK: $COUNT server-minted receipts verify under the Python and TypeScript SDKs"
else
  echo "server log tail:" >&2
  tail -n 20 "$RUN_DIR/handoffd.log" >&2
fi
exit "$STATUS"

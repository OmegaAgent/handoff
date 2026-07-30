"""Byte-identical round trips against every canonical fixture.

`spec/fixtures/README.md` states the contract: an SDK's serialization is asserted byte-identical
after re-encoding, and the two files under `signing/` are the byte sequences the worked signature
vectors are computed over. These tests assert exactly that, and nothing weaker — comparing parsed
objects instead of bytes would pass while the digests silently disagreed.

Two encodings, and which one applies is a property of the directory, not a guess:

* everything under `fixtures/` and `fixtures/use-cases/` — 2-space indent, member order as
  stored, one trailing newline;
* everything under `fixtures/signing/` — RFC 8785 (JCS), sorted, compact, no trailing newline.
"""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

import pytest

from handoff import Document, canonical_bytes, encode_document
from handoff.models import Receipt, Request, Signal

SPEC = Path(__file__).resolve().parents[3] / "spec"
FIXTURES = SPEC / "fixtures"

DOCUMENT_FIXTURES = sorted(
    [p for p in FIXTURES.glob("*.json")] + [p for p in (FIXTURES / "use-cases").glob("*.json")]
)
SIGNING_FIXTURES = sorted((FIXTURES / "signing").glob("*.json"))


def _name(path: Path) -> str:
    return str(path.relative_to(FIXTURES))


def test_the_fixture_set_is_present():
    assert len(DOCUMENT_FIXTURES) == 27, [_name(p) for p in DOCUMENT_FIXTURES]
    assert len(SIGNING_FIXTURES) == 2, [_name(p) for p in SIGNING_FIXTURES]


@pytest.mark.parametrize("path", DOCUMENT_FIXTURES, ids=_name)
def test_document_fixture_round_trips_byte_identically(path: Path):
    raw = path.read_bytes()
    document = Document.from_json(raw)
    assert document.encode() == raw


@pytest.mark.parametrize("path", SIGNING_FIXTURES, ids=_name)
def test_signing_fixture_round_trips_byte_identically(path: Path):
    """The signing fixtures are stored canonicalized, so parsing and re-canonicalizing must
    reproduce them exactly. An implementation that cannot has a canonicalization bug regardless
    of what its own tests say (fixtures/README.md)."""
    raw = path.read_bytes()
    assert canonical_bytes(json.loads(raw)) == raw


def test_the_round_trip_assertion_is_byte_level_not_parse_level():
    """The guard on the guard.

    A reformatted document parses to an equal object but is a different byte sequence, so a suite
    that compared parsed objects would pass here — and the fixtures would quietly stop being a
    cross-language contract. This proves the distinction is load-bearing rather than assumed.
    """
    raw = (FIXTURES / "05-signal-answered.json").read_bytes()
    parsed = json.loads(raw)
    reformatted = json.dumps(parsed, indent=4, ensure_ascii=False).encode() + b"\n"

    assert json.loads(reformatted) == parsed, "a parse-level comparison cannot tell these two apart"
    assert reformatted != raw, "a byte-level comparison can, and that is what we assert"


def test_signing_fixture_hashes_match_the_worked_vectors():
    """signing.md §1.6 and §2.5 publish these lengths and hashes. They are the check on whether
    this implementation canonicalizes correctly."""
    body = (FIXTURES / "signing" / "callback-body.json").read_bytes()
    core = (FIXTURES / "signing" / "receipt-core.json").read_bytes()
    assert len(body) == 493
    assert hashlib.sha256(body).hexdigest() == "fbd6ec4cacc7cb9c9371d2791f946535e3d391a0594a92b5a3a27dd34f5e94fa"
    assert len(core) == 1125
    assert hashlib.sha256(core).hexdigest() == "2763f39ef8a61d493106d3db302ec36cae5c024ca3da3a019d483ccc29704ad1"


def test_receipt_core_of_the_decision_fixture_is_the_signing_fixture():
    """fixtures/README.md guarantee 1: `08-receipt-decision.json` is exactly
    `signing/receipt-core.json` plus its chain member."""
    receipt = Receipt.from_json((FIXTURES / "08-receipt-decision.json").read_bytes())
    assert canonical_bytes(receipt.core()) == (FIXTURES / "signing" / "receipt-core.json").read_bytes()


def test_two_receipts_carry_the_same_members_in_different_orders():
    """Which is why the SDK preserves wire order instead of imposing a field order.

    `08` is stored in JCS order because it is the signed core plus its chain entry; `09` is in
    declaration order. No fixed field order reproduces both, so member order is data.
    """
    decision = Document.from_json((FIXTURES / "08-receipt-decision.json").read_bytes())
    policy = Document.from_json((FIXTURES / "09-receipt-policy.json").read_bytes())
    assert set(decision) == set(policy)
    assert list(decision) != list(policy)


def test_unknown_members_survive_a_round_trip():
    """§19: new response fields are additive and a client must ignore what it does not know —
    which means carrying it, not dropping it."""
    raw = json.loads((FIXTURES / "05-signal-answered.json").read_bytes())
    raw["x-vendor-annotation"] = {"seen": True}
    signal = Signal(raw)
    assert signal.type == "answered"
    assert json.loads(signal.encode())["x-vendor-annotation"] == {"seen": True}


def test_typed_accessors_read_the_canonical_fixtures():
    request = Request.from_json((FIXTURES / "02-request-created.json").read_bytes())
    assert request.id == "req_01K3M7QW8ZC4YRXB2N6VD9FTHE"
    assert request.is_pending
    assert request.prompt.title == "Refund $2,400 to Acme Corp?"
    assert [f.name for f in request.requires.answer_fields] == ["decision", "note"]

    signal = Signal.from_json((FIXTURES / "05-signal-answered.json").read_bytes())
    assert signal.is_terminal
    assert signal.decision is not None
    assert signal.decision.values["decision"] == "approve"
    assert signal.decision.decided_by_human

    lapsed = Signal.from_json((FIXTURES / "07-signal-attempt-lapsed.json").read_bytes())
    assert not lapsed.is_terminal
    assert lapsed.decision is None

    policy = Receipt.from_json((FIXTURES / "09-receipt-policy.json").read_bytes())
    assert policy.actor_type == "policy"
    assert not policy.decided_by_human


def test_no_fixture_carries_a_request_kind():
    """§5.1 and C-22: the eight interaction patterns are eight declarations, not eight kinds.

    A `kind` member on a receipt is the record's own type (`decision` / `policy` / `correction`)
    and is not an interaction type; it is excluded here by path.
    """
    offenders = []
    for path in DOCUMENT_FIXTURES:
        data = json.loads(path.read_bytes())

        def walk(node, trail):
            if isinstance(node, dict):
                for key, value in node.items():
                    if key == "kind" and not _is_permitted_kind(trail, value):
                        offenders.append(f"{_name(path)}:{'.'.join(trail + [key])}={value}")
                    walk(value, trail + [key])
            elif isinstance(node, list):
                for index, item in enumerate(node):
                    walk(item, trail + [str(index)])

        walk(data, [])
    assert offenders == [], offenders


def _is_permitted_kind(trail: list[str], value) -> bool:
    """A `kind` member is legitimate in four places, none of them a request interaction type:
    the receipt's own record type, an evidence item, a routing target, and a grant session's
    transport."""
    tail = [p for p in trail if not p.isdigit()]
    if value in {"decision", "policy", "correction"}:
        return True
    if tail and tail[-1] in {"evidence", "targets", "to", "delegate_to", "target", "assignees", "identities"}:
        return True
    if tail[-1:] == ["transport"] and value in {"websocket", "webrtc", "http"}:
        return True
    if value in {"link", "table", "text", "image", "principal", "role", "group", "rotation", "anyone"}:
        return True
    return False


def test_use_case_fixtures_cover_all_eight_patterns():
    names = sorted(p.stem for p in (FIXTURES / "use-cases").glob("*.json"))
    assert len(names) == 8, names

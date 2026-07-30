"""Callback and receipt verification against the worked vectors in `spec/signing.md`.

Every constant below is copied from that document. An implementation is expected to reproduce
each one exactly, and all four negative callback vectors must be rejected.
"""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

import pytest

from handoff import verify_callback, verify_chain, verify_receipt_chain
from handoff.errors import CallbackSignatureError
from handoff.signing import callback_canonical_string, chain_digest, receipt_core_hash, sign_callback

SPEC = Path(__file__).resolve().parents[3] / "spec"
FIXTURES = SPEC / "fixtures"

SECRET_A = "whsec_2f8a91c4e7b3d05a6c1e9f47b28d3a05"
SECRET_B = "whsec_9d41c07be5a2f36819b4d0e7c5a81f62"
RETIRED = "whsec_00000000000000000000000000000000"

TIMESTAMP = 1785592064
DELIVERY = "dlv_01K3MB2R6C8ZC4YRXB2N6VD9FT"
OTHER_DELIVERY = "dlv_01K3MB2R6D8ZC4YRXB2N6VD9FT"
SIGNAL = "sig_01K3MB2R4X8ZC4YRXB2N6VD9FT"

SIG_A = "cae13126f8dcd1e918376aa373be2757db7281a3e5aaed2d83d716537e03de80"
SIG_B = "d86b3740bad654e46c1349614523a476be0eb7d6a30a798b2d475374f36c57eb"
SIG_TAMPERED_A = "621af1622c79ccb0d444ae046dae7db4a8e5b96c6ae0d9bd574ff8bc0be26a66"
SIG_OTHER_DELIVERY = "9a674a003d0507ad13369a6bd82713769116a276ec57f26eb2637b2af00f8e68"

BODY = (FIXTURES / "signing" / "callback-body.json").read_bytes()


def headers(signature: str, *, delivery: str = DELIVERY, sequence: str = "1", version: str = "1") -> dict[str, str]:
    return {
        "Handoff-Signature": signature,
        "Handoff-Delivery": delivery,
        "Handoff-Signal": SIGNAL,
        "Handoff-Version": version,
        "Handoff-Sequence": sequence,
        "Handoff-Idempotency-Key": delivery,
        "Content-Type": "application/json",
    }


# -- the construction itself -----------------------------------------------------------------


def test_canonical_string_matches_the_document():
    """signing.md §1.2: exactly three line feeds, no trailing newline."""
    canonical = callback_canonical_string("1", TIMESTAMP, DELIVERY, BODY)
    body_hash = "fbd6ec4cacc7cb9c9371d2791f946535e3d391a0594a92b5a3a27dd34f5e94fa"
    assert canonical == f"1\n{TIMESTAMP}\n{DELIVERY}\n{body_hash}".encode()
    assert canonical.count(b"\n") == 3
    assert not canonical.endswith(b"\n")


def test_signatures_reproduce_both_worked_vectors():
    assert sign_callback(SECRET_A, "1", TIMESTAMP, DELIVERY, BODY) == SIG_A
    assert sign_callback(SECRET_B, "1", TIMESTAMP, DELIVERY, BODY) == SIG_B


# -- positive vectors ------------------------------------------------------------------------


def test_verifies_under_secret_a():
    result = verify_callback(headers(f"t={TIMESTAMP},v1={SIG_A}"), BODY, [SECRET_A], now=TIMESTAMP)
    assert result.delivery_id == DELIVERY
    assert result.signal_id == SIGNAL
    assert result.sequence == 1
    assert result.signal.type == "answered"
    assert result.signal.decision.values["decision"] == "approve"


def test_rotation_overlap_verifies_under_either_secret():
    """signing.md §1.4: while two secrets are active the server signs with both and the receiver
    accepts either, so there is no window in which valid callbacks fail."""
    both = f"t={TIMESTAMP},v1={SIG_A},v1={SIG_B}"
    for active in ([SECRET_A], [SECRET_B], [SECRET_A, SECRET_B], [SECRET_B, SECRET_A]):
        assert verify_callback(headers(both), BODY, active, now=TIMESTAMP).sequence == 1


def test_a_receiver_holding_only_the_new_secret_still_verifies():
    assert verify_callback(headers(f"t={TIMESTAMP},v1={SIG_A},v1={SIG_B}"), BODY, [SECRET_B], now=TIMESTAMP)


# -- the four negative vectors ---------------------------------------------------------------


def test_negative_tampered_body_is_rejected():
    tampered = BODY.replace(b'"approve"', b'"reject"')
    assert hashlib.sha256(tampered).hexdigest() == (
        "8d1b25a370b6de9d1a504ca1acfe97dc7abe10d4c12b0d33dfaf74f5114eb019"
    )
    with pytest.raises(CallbackSignatureError, match="did not match"):
        verify_callback(headers(f"t={TIMESTAMP},v1={SIG_A}"), tampered, [SECRET_A], now=TIMESTAMP)


def test_negative_tampered_body_signature_is_the_documented_one():
    """The vector notes what an attacker *holding the secret* would produce, to make the point
    that one who does not hold it cannot."""
    tampered = BODY.replace(b'"approve"', b'"reject"')
    assert sign_callback(SECRET_A, "1", TIMESTAMP, DELIVERY, tampered) == SIG_TAMPERED_A


def test_negative_replay_onto_another_delivery_is_rejected():
    """The delivery id is inside the signed string, so a valid signature cannot be lifted onto a
    different delivery of the same payload."""
    with pytest.raises(CallbackSignatureError, match="did not match"):
        verify_callback(
            headers(f"t={TIMESTAMP},v1={SIG_A}", delivery=OTHER_DELIVERY), BODY, [SECRET_A], now=TIMESTAMP
        )
    assert sign_callback(SECRET_A, "1", TIMESTAMP, OTHER_DELIVERY, BODY) == SIG_OTHER_DELIVERY


def test_negative_stale_timestamp_is_rejected():
    """301 seconds earlier, signature recomputed and cryptographically valid — and still refused,
    because freshness is receiver-enforced."""
    stale = TIMESTAMP - 301
    valid = sign_callback(SECRET_A, "1", stale, DELIVERY, BODY)
    with pytest.raises(CallbackSignatureError, match="freshness window"):
        verify_callback(headers(f"t={stale},v1={valid}"), BODY, [SECRET_A], now=TIMESTAMP)


def test_negative_retired_secret_is_rejected():
    signed = sign_callback(RETIRED, "1", TIMESTAMP, DELIVERY, BODY)
    with pytest.raises(CallbackSignatureError, match="did not match"):
        verify_callback(headers(f"t={TIMESTAMP},v1={signed}"), BODY, [SECRET_A, SECRET_B], now=TIMESTAMP)


# -- boundary and hygiene --------------------------------------------------------------------


def test_freshness_boundary_is_inclusive_at_300_seconds():
    for offset in (300, -300):
        moment = TIMESTAMP + offset
        signed = sign_callback(SECRET_A, "1", TIMESTAMP, DELIVERY, BODY)
        assert verify_callback(headers(f"t={TIMESTAMP},v1={signed}"), BODY, [SECRET_A], now=moment)
    signed = sign_callback(SECRET_A, "1", TIMESTAMP, DELIVERY, BODY)
    with pytest.raises(CallbackSignatureError):
        verify_callback(headers(f"t={TIMESTAMP},v1={signed}"), BODY, [SECRET_A], now=TIMESTAMP + 301)


def test_sequence_header_disagreeing_with_the_body_is_rejected():
    with pytest.raises(CallbackSignatureError, match="Sequence"):
        verify_callback(
            headers(f"t={TIMESTAMP},v1={SIG_A}", sequence="7"), BODY, [SECRET_A], now=TIMESTAMP
        )


@pytest.mark.parametrize(
    "header",
    ["", "v1=" + SIG_A, f"t={TIMESTAMP}", "t=notanumber,v1=" + SIG_A, f"t={TIMESTAMP},v1=", "garbage"],
)
def test_malformed_signature_headers_are_rejected(header: str):
    with pytest.raises(CallbackSignatureError):
        verify_callback(headers(header), BODY, [SECRET_A], now=TIMESTAMP)


def test_a_string_body_is_refused_rather_than_encoded():
    """Re-encoding a decoded body is the canonicalization trap signing.md §3 names first."""
    with pytest.raises(TypeError, match="bytes"):
        verify_callback(headers(f"t={TIMESTAMP},v1={SIG_A}"), BODY.decode(), [SECRET_A], now=TIMESTAMP)


def test_reserialized_body_does_not_verify():
    """Proves the point above rather than asserting it: json.dumps of the parsed body is a
    different byte sequence and therefore a different hash."""
    reserialized = json.dumps(json.loads(BODY)).encode()
    assert reserialized != BODY
    with pytest.raises(CallbackSignatureError, match="did not match"):
        verify_callback(headers(f"t={TIMESTAMP},v1={SIG_A}"), reserialized, [SECRET_A], now=TIMESTAMP)


def test_headers_are_matched_case_insensitively():
    lowered = {k.lower(): v for k, v in headers(f"t={TIMESTAMP},v1={SIG_A}").items()}
    assert verify_callback(lowered, BODY, [SECRET_A], now=TIMESTAMP).delivery_id == DELIVERY


def test_rejection_messages_never_contain_a_secret():
    with pytest.raises(CallbackSignatureError) as caught:
        verify_callback(headers(f"t={TIMESTAMP},v1={SIG_A}"), BODY, [SECRET_B], now=TIMESTAMP)
    text = str(caught.value)
    assert SECRET_A not in text and SECRET_B not in text
    assert "whsec_" not in text


def test_empty_active_secret_set_is_refused():
    with pytest.raises(CallbackSignatureError):
        verify_callback(headers(f"t={TIMESTAMP},v1={SIG_A}"), BODY, [], now=TIMESTAMP)


# -- receipts ---------------------------------------------------------------------------------


def test_receipt_core_hash_and_chain_digest_match_the_worked_vector():
    receipt = json.loads((FIXTURES / "08-receipt-decision.json").read_bytes())
    assert receipt_core_hash(receipt) == "2763f39ef8a61d493106d3db302ec36cae5c024ca3da3a019d483ccc29704ad1"
    assert chain_digest(
        4211,
        "sha256:" + "0" * 64,
        "2763f39ef8a61d493106d3db302ec36cae5c024ca3da3a019d483ccc29704ad1",
    ) == "sha256:919f8870391849de4e7b1d5b249ccbaaa7d5a7d3f500f5571c5a92dd0c3909db"
    assert verify_receipt_chain(receipt)


def test_a_two_receipt_chain_verifies_end_to_end():
    """`09-receipt-policy.json` presents itself as the next entry after `08` — its `prev_digest`
    is `08`'s digest and its height is one higher — but its stored `chain.digest` does not
    recompute from its own content (see the module note at the bottom of this file). The chain
    mechanism is therefore asserted over a recomputed second entry, so that this test states
    something true about the implementation rather than about a fixture."""
    decision = json.loads((FIXTURES / "08-receipt-decision.json").read_bytes())
    policy = json.loads((FIXTURES / "09-receipt-policy.json").read_bytes())
    assert policy["chain"]["prev_digest"] == decision["chain"]["digest"]
    assert policy["chain"]["height"] == decision["chain"]["height"] + 1

    policy["chain"]["digest"] = chain_digest(
        policy["chain"]["height"], policy["chain"]["prev_digest"], receipt_core_hash(policy)
    )
    assert verify_chain([decision, policy])


@pytest.mark.parametrize(
    "mutate",
    [
        pytest.param(lambda r: r["decision"]["values"].update(decision="reject"), id="core-altered"),
        pytest.param(lambda r: r["chain"].update(height=4210), id="height-changed"),
        pytest.param(lambda r: r["chain"].update(prev_digest="sha256:" + "1" * 64), id="prev-replaced"),
    ],
)
def test_receipt_negative_vectors_are_rejected(mutate):
    """signing.md §2.5. Any of these changes the chain digest, which invalidates the head."""
    receipt = json.loads((FIXTURES / "08-receipt-decision.json").read_bytes())
    mutate(receipt)
    assert not verify_receipt_chain(receipt)


def test_altering_a_historical_receipt_invalidates_the_rest_of_the_chain():
    """§9.4, C-15: the property the chain exists for. Changing an old entry changes its core
    hash, which changes its digest, which breaks the link every later entry depends on."""
    decision = json.loads((FIXTURES / "08-receipt-decision.json").read_bytes())
    policy = json.loads((FIXTURES / "09-receipt-policy.json").read_bytes())
    policy["chain"]["digest"] = chain_digest(
        policy["chain"]["height"], policy["chain"]["prev_digest"], receipt_core_hash(policy)
    )
    assert verify_chain([decision, policy])

    decision["decision"]["values"]["note"] = "tampered"
    assert not verify_chain([decision, policy])


def test_a_chain_with_a_relinked_gap_is_rejected():
    """Height is inside the chain input, so an entry cannot be excised and the rest re-linked."""
    decision = json.loads((FIXTURES / "08-receipt-decision.json").read_bytes())
    excised = dict(decision)
    excised["chain"] = dict(decision["chain"], height=decision["chain"]["height"] - 1)
    assert not verify_receipt_chain(excised)


# NOTE — fixture defect, reported upstream, not worked around here:
# `spec/fixtures/09-receipt-policy.json` carries
#   chain.digest = sha256:c1a4f0bb7d2e6935481acdf20e7b3c56d9084e1fa27bc3d5608e94af1236b7d0
# but recomputing it from that receipt's own core (a4070dc2…) at height 4212 with its stated
# prev_digest yields
#   sha256:1c4738c06a55a1ecc2217b55ac20fa6ba65319e81fc3b7ac49a726536afeb669
# `08-receipt-decision.json` recomputes exactly, matching signing.md §2.5, so the canonicalization
# is right and the fixture's digest is a placeholder.

"""Callback and receipt verification against the worked vectors in `spec/signing.md`.

Every constant below is copied from that document. An implementation is expected to reproduce
each one exactly, and all four negative callback vectors must be rejected.
"""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

import pytest

from handoff import canonical_bytes, verify_callback, verify_chain, verify_receipt_chain
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


def test_every_published_receipt_fixture_verifies_as_published():
    """Verbatim, with nothing recomputed first.

    This is the assertion the suite was missing, and its absence is how `09-receipt-policy.json`
    shipped with a `chain.digest` that did not recompute from its own content. Every other receipt
    test here rewrote the digest before checking it, which measures the implementation against
    itself and says nothing about the bytes an independent implementer will actually download. A
    published fixture that fails the project's own verifier is the first thing they will find.
    """
    published = sorted(FIXTURES.glob("*receipt*.json"))
    assert [p.name for p in published] == [
        "08-receipt-decision.json",
        "09-receipt-policy.json",
    ], "a new receipt fixture must be added to this assertion, not silently skipped"

    for path in published:
        receipt = json.loads(path.read_bytes())
        assert verify_receipt_chain(receipt), f"{path.name} does not verify as published"


def test_a_two_receipt_chain_verifies_end_to_end():
    """`09-receipt-policy.json` is the next entry after `08` — its `prev_digest` is `08`'s digest
    and its height is one higher — so the two verify as a chain exactly as published."""
    decision = json.loads((FIXTURES / "08-receipt-decision.json").read_bytes())
    policy = json.loads((FIXTURES / "09-receipt-policy.json").read_bytes())
    assert policy["chain"]["prev_digest"] == decision["chain"]["digest"]
    assert policy["chain"]["height"] == decision["chain"]["height"] + 1

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
    assert verify_chain([decision, policy])

    decision["decision"]["values"]["note"] = "tampered"
    assert not verify_chain([decision, policy])


def test_a_chain_with_a_relinked_gap_is_rejected():
    """Height is inside the chain input, so an entry cannot be excised and the rest re-linked."""
    decision = json.loads((FIXTURES / "08-receipt-decision.json").read_bytes())
    excised = dict(decision)
    excised["chain"] = dict(decision["chain"], height=decision["chain"]["height"] - 1)
    assert not verify_receipt_chain(excised)


# -- the reference verifier the specification publishes ----------------------------------------


def _reference_verifier_source() -> str:
    """The Python snippet from signing.md §2.5, extracted from the document itself.

    Extracted rather than copied, because a copy is a second implementation that drifts. The
    marker and the fence are located structurally, and a failure to find either is an assertion
    failure rather than a skip — if the document is restructured, this test says so instead of
    quietly measuring nothing.
    """
    lines = (SPEC / "signing.md").read_text(encoding="utf-8").splitlines()
    marker = next(
        (i for i, line in enumerate(lines) if line.startswith("**Reference verifier**") and "cryptography" in line),
        None,
    )
    assert marker is not None, "signing.md no longer publishes a receipt reference verifier"

    opened = next((i for i in range(marker, len(lines)) if lines[i].strip() == "```python"), None)
    assert opened is not None, "the reference verifier is no longer a ```python fence"
    closed = next((i for i in range(opened + 1, len(lines)) if lines[i].strip() == "```"), None)
    assert closed is not None, "the reference verifier fence is unterminated"

    source = "\n".join(lines[opened + 1 : closed])
    assert "def verify_receipt" in source and "def canonical_json" in source, source[:200]
    return source


def _reference_verifier_namespace() -> dict:
    namespace: dict = {}
    exec(compile(_reference_verifier_source(), "signing.md#reference-verifier", "exec"), namespace)
    return namespace


def test_the_published_reference_verifier_executes_and_agrees_with_this_sdk():
    """The specification's own verifier, actually run.

    It had never been executed by anything in this repository, which is how it came to ship
    ordering that contradicted the RFC it named in the same sentence. An implementer's first
    move is to copy this snippet, so it is the artifact most likely to be trusted and was the
    one least likely to be checked.

    Executing it is also the only cross-implementation check available without a server: the
    snippet shares no code with this SDK, so agreement between them is evidence rather than
    self-consistency.
    """
    reference = _reference_verifier_namespace()

    # 1. It reproduces the published canonical bytes, which is signing.md §3's own criterion.
    core = json.loads((FIXTURES / "signing" / "receipt-core.json").read_bytes())
    published = (FIXTURES / "signing" / "receipt-core.json").read_bytes()
    assert reference["canonical_json"](core) == published
    assert (
        hashlib.sha256(reference["canonical_json"](core)).hexdigest()
        == "2763f39ef8a61d493106d3db302ec36cae5c024ca3da3a019d483ccc29704ad1"
    )

    # 2. It agrees with this SDK on the document that broke them apart. A non-BMP key sorts
    #    below a BMP key above U+D7FF by code unit and above it by code point, so a verifier
    #    built from the old wording disagrees here and nowhere else.
    adversarial = {"！": 1, "\U0001f600": 2, "a": 0, "": 3, "zé": "café\n\"q\""}
    assert reference["canonical_json"](adversarial) == canonical_bytes(adversarial)
    assert (
        reference["canonical_json"](adversarial)
        == '{"":3,"a":0,"zé":"café\\n\\"q\\"","\U0001f600":2,"！":1}'.encode()
    ), "empty key first, then 'a' (U+0061), 'z…' (U+007A), the surrogate pair (0xD83D), U+FF01"

    # 3. It verifies the published receipt, chain and Ed25519 signature together, as published.
    keys = {"rk_01K3MB2R4Y8ZC4YRXB2N6VD9FT": bytes.fromhex(
        "fb83e7234defb5402d3123ce1753df2e30313285cf194f4b7651bf5530646f98"
    )}
    signature = {
        "alg": "Ed25519",
        "kid": "rk_01K3MB2R4Y8ZC4YRXB2N6VD9FT",
        "sig": "av8Iq2KkysJR6J3na_k6GHTS26ajN3CNsT4iOyHcJUy9mTxvF1hD0moPcg4kFGkklv1u2cGiijm76V2icmwZCw",
    }
    decision = json.loads((FIXTURES / "08-receipt-decision.json").read_bytes())
    assert reference["verify_receipt"](decision, signature, keys) is True

    # And it rejects what it must: an altered core, and a kid it was not given.
    tampered = json.loads((FIXTURES / "08-receipt-decision.json").read_bytes())
    tampered["decision"]["values"]["decision"] = "reject"
    assert reference["verify_receipt"](tampered, signature, keys) is False
    assert reference["verify_receipt"](decision, dict(signature, kid="rk_unknown"), keys) is False


def test_the_published_reference_verifier_refuses_a_float_like_both_sdks():
    """§1.4: a float in a digest-covered position means the bytes at rest are not the bytes the
    canonicalizer emits, so no digest an auditor computes is the one that was sealed. The
    snippet an implementer copies has to refuse it too, or it will verify receipts that this
    SDK and the TypeScript SDK both reject."""
    reference = _reference_verifier_namespace()
    for bad in [{"amount": 2400.5}, {"amount": -0.0}, {"nested": [1.0]}]:
        with pytest.raises(ValueError):
            reference["canonical_json"](bad)

"""The public surface: declarations, the error taxonomy, and what must never be printed."""

from __future__ import annotations

import json
import warnings

import pytest

from fake_server import FakeServer

import handoff
from handoff import Client, fields, requires
from handoff.errors import (
    AlreadyAnswered,
    AuthorizationSpent,
    HandoffProtocolError,
    RequesterMayNotAnswer,
    UnsupportedRequiresVersion,
    from_error_body,
)


# -- declarations ----------------------------------------------------------------------------


def test_a_raised_body_carries_no_kind_anywhere():
    """I14 and C-22: the shorthands are constructors, not kinds. The server cannot tell which
    one the caller used, because nothing on the wire says."""
    with FakeServer() as server:
        client = Client(server.base_url, "test-key")
        client.raise_request(
            waiter_ref="run:decl",
            prompt=handoff.prompt("Refund $2,400?", "Double charged."),
            requires=requires(
                [fields.choice("decision", "Decision", ["approve", "reject"])],
                authority=handoff.authority("editor", "session"),
            ),
        )
        raised = next(iter(server.state.requests.values()))
        wire = json.dumps({"prompt": raised["prompt"], "requires": raised["requires"]})
        assert '"kind"' not in wire


def test_the_eight_patterns_are_eight_declarations_over_one_shape():
    """§5.6. Each of these differs only in what it declares — field types, capabilities,
    authority, expiry policy. None of them adds a branch."""
    approve = requires([fields.choice("decision", "Decision", ["approve", "reject"])])
    question = requires([fields.text("answer", "Answer")])
    login = requires(
        [fields.text("email", "Email"), fields.secret("password", "Password", sink_ref="snk_x")],
        capabilities=[handoff.capability("interactive_surface", scope="drive", optional=True)],
        authority=handoff.authority("admin", "session"),
    )
    takeover = requires(
        [], capabilities=[handoff.capability("interactive_surface", scope="drive")],
        authority=handoff.authority("admin", "session"),
    )
    review = requires([fields.document("doc", "Review", schema_ref="s", initial={"a": 1})])
    confirm = requires(
        [fields.choice("decision", "Decision", ["confirm", "cancel"])],
        authority=handoff.authority("editor", "reauth"),
    )

    for declaration in (approve, question, login, takeover, review, confirm):
        assert declaration["v"] == 1
        assert "kind" not in json.dumps(declaration)

    assert takeover["answer"]["fields"] == [], "an empty field list is a legitimate attestation (§5.3)"
    assert login["answer"]["fields"][1]["type"] == "secret"


def test_authority_always_forbids_the_requester():
    """§4.3: the member exists for clarity and a server rejects any other value. It is not a
    setting the caller can relax."""
    assert handoff.authority()["forbid_requester"] is True
    assert handoff.authority("admin", "mfa")["forbid_requester"] is True


def test_a_declared_default_needs_a_default_answer():
    """§6.4: `default` is the only policy that produces an outcome without a person, so the
    answer must have been declared before anyone knew they would go quiet."""
    with pytest.raises(ValueError, match="declared at raise time"):
        handoff.ttl_policy("default")
    policy = handoff.ttl_policy("default", default_answer={"answer": "no"})
    assert policy["default_answer"] == {"answer": "no"}


def test_ask_declares_its_default_at_raise_time_rather_than_guessing_later():
    """The ergonomic the prior art got half-right. Returning a client-side default on timeout
    produces the same value with no record; declaring it produces a policy receipt."""
    with FakeServer() as server:
        client = Client(server.base_url, "test-key")
        try:
            client.ask("Which address?", default="the billing address", timeout=1)
        except Exception:
            pass
        raised = next(iter(server.state.requests.values()))
        assert raised["requires"]["answer"]["fields"][0]["type"] == "text"


def test_ask_returns_the_default_instead_of_raising_on_a_local_deadline():
    with FakeServer() as server:
        client = Client(server.base_url, "test-key")
        assert client.ask("Which address?", default="the billing one", timeout=1) == "the billing one"


def test_ask_raises_when_no_default_was_declared():
    with FakeServer() as server:
        client = Client(server.base_url, "test-key")
        with pytest.raises(handoff.HandoffTimeout):
            client.ask("Which address?", timeout=1)


def test_approve_is_truthy_only_when_a_person_approved():
    import threading
    import time

    with FakeServer() as server:
        client = Client(server.base_url, "test-key")

        def answer_with(value: str):
            time.sleep(0.3)
            request_id = next(iter(server.state.requests))
            Client(server.base_url, "human").answer(request_id, {"decision": value})

        threading.Thread(target=lambda: answer_with("approve"), daemon=True).start()
        outcome = client.approve("Refund $2,400?", timeout=10)
        assert bool(outcome) is True
        assert outcome.decided_by_human
        assert outcome.value("decision") == "approve"

    with FakeServer() as server:
        client = Client(server.base_url, "test-key")

        def reject():
            time.sleep(0.3)
            request_id = next(iter(server.state.requests))
            Client(server.base_url, "human").answer(request_id, {"decision": "reject"})

        threading.Thread(target=reject, daemon=True).start()
        outcome = client.approve("Refund $2,400?", timeout=10)
        assert bool(outcome) is False


# -- errors ----------------------------------------------------------------------------------


def test_error_envelope_maps_to_the_taxonomy():
    from pathlib import Path

    fixtures = Path(__file__).resolve().parents[3] / "spec" / "fixtures"

    answered = from_error_body(json.loads((fixtures / "14-error-already-answered.json").read_bytes()), status=409)
    assert isinstance(answered, AlreadyAnswered)
    assert answered.receipt_id == "rcpt_01K3MB2R4Y8ZC4YRXB2N6VD9FT"
    assert answered.request_id == "req_01K3M7QW8ZC4YRXB2N6VD9FTHE"

    machine = from_error_body(
        json.loads((fixtures / "15-error-requester-may-not-answer.json").read_bytes()), status=403
    )
    assert isinstance(machine, RequesterMayNotAnswer)

    version = from_error_body(
        json.loads((fixtures / "16-error-unsupported-requires-version.json").read_bytes()), status=400
    )
    assert isinstance(version, UnsupportedRequiresVersion)


def test_an_unknown_code_fails_closed_with_the_code_intact():
    """I21. Mapping an unrecognized code onto a familiar one would handle a state the client does
    not understand as one it does."""
    error = from_error_body({"error": {"code": "some_future_code", "message": "…"}}, status=409)
    assert type(error) is HandoffProtocolError
    assert error.code == "some_future_code"


def test_validation_failures_carry_per_field_detail():
    error = from_error_body(
        {
            "error": {
                "code": "answer_validation_failed",
                "message": "…",
                "fields": [{"name": "password", "code": "secret_value_not_permitted"}],
            }
        },
        status=422,
    )
    assert [f.name for f in error.fields] == ["password"]
    assert error.fields[0].code == "secret_value_not_permitted"


def test_a_conflict_from_the_server_raises_the_right_class():
    with FakeServer() as server:
        client = Client(server.base_url, "test-key")
        pending = client.raise_request(
            waiter_ref="run:conflict",
            prompt={"title": "t"},
            requires={"v": 1, "answer": {"fields": []}, "capabilities": []},
        )
        human = Client(server.base_url, "human")
        human.answer(pending.id, {"decision": "approve"})
        with pytest.raises(AlreadyAnswered):
            human.answer(pending.id, {"decision": "reject"})


# -- nothing secret is ever printed ----------------------------------------------------------


def test_the_client_never_reprs_its_api_key():
    client = Client("https://handoff.example.com", "sk_live_do_not_print_me")
    assert "sk_live" not in repr(client)
    assert "<redacted>" in repr(client)


def test_a_signal_repr_redacts_its_resume_token():
    """The resume token authorizes the ack. It is not an identifier and must not be logged."""
    from pathlib import Path

    from handoff.models import Signal

    fixtures = Path(__file__).resolve().parents[3] / "spec" / "fixtures"
    signal = Signal.from_json((fixtures / "05-signal-answered.json").read_bytes())
    assert "rt_01K3MB2R55" not in repr(signal)
    assert "<redacted>" in repr(signal)
    assert signal.resume_token == "rt_01K3MB2R558ZC4YRXB2N6VD9FT", "redaction is display-only"
    assert json.loads(signal.encode())["resume_token"] == "rt_01K3MB2R558ZC4YRXB2N6VD9FT"


def test_a_grant_session_repr_redacts_its_transport_url():
    """§11.2: the resolved transport URL is the only resolvable address in a conforming system,
    and it must not be persisted, logged, or echoed."""
    from pathlib import Path

    from handoff import Document

    fixtures = Path(__file__).resolve().parents[3] / "spec" / "fixtures"
    session = Document.from_json((fixtures / "13-grant-session.json").read_bytes())
    assert "wss://relay.example.com" not in repr(session)


def test_ack_cannot_be_called_with_a_bare_signal_id():
    """The resume token travels on the signal, so there is no ergonomic path that acks something
    the caller never actually received."""
    with FakeServer() as server:
        waiter = Client(server.base_url, "k").waiter("run:x")
        with pytest.raises(TypeError, match="resume_token"):
            waiter.ack("sig_01K3MB2R4X8ZC4YRXB2N6VD9FT")


def test_canonicalization_refuses_non_integer_numbers():
    """signing.md §3 trap 2: a naive float format produces a digest that is stable in one
    implementation and wrong across two."""
    from handoff import canonical_bytes

    assert canonical_bytes({"n": 4211}) == b'{"n":4211}'
    with pytest.raises(handoff.NonConformingDocument, match="digest-covered content carries"):
        canonical_bytes({"amount": 2400.5})


def test_a_receipt_with_no_canonical_form_is_not_reported_as_a_failed_verification():
    """A verifier that cannot tell "this Server is broken" from "someone tampered with this" is
    not much of a verifier, and the difference is the difference between a bug report and an
    incident. `False` is reserved for the digest failing to recompute; a receipt that has no
    canonical form at all raises instead."""
    from pathlib import Path

    from handoff.signing import verify_receipt_chain

    fixtures = Path(__file__).resolve().parents[3] / "spec" / "fixtures"
    sealed = json.loads((fixtures / "08-receipt-decision.json").read_bytes())
    assert verify_receipt_chain(sealed) is True

    # Tampered: the digest no longer recomputes. That is what False means.
    tampered = json.loads((fixtures / "08-receipt-decision.json").read_bytes())
    tampered["decision"]["values"]["decision"] = "reject"
    assert verify_receipt_chain(tampered) is False

    # Non-conforming: a float in a digest-covered position, which §1.4 says a conforming Server
    # never serves. Nobody tampered with it — and the SDK must not imply that anyone did.
    non_conforming = json.loads((fixtures / "08-receipt-decision.json").read_bytes())
    non_conforming["decision"]["values"]["amount"] = -0.0
    with pytest.raises(handoff.NonConformingDocument) as raised:
        verify_receipt_chain(non_conforming)
    assert "not evidence that anyone tampered" in str(raised.value)


def test_members_are_ordered_by_utf16_code_units_not_code_points():
    """RFC 8785 §3.2.3 orders members by the UTF-16 code units of their names.

    A ``document`` field carries any JSON value, so an answer puts caller-chosen object keys into
    ``decision.values`` and from there into the receipt core. U+1F600 is non-BMP: it encodes as
    the surrogate pair 0xD83D 0xDE00, so it sorts *below* U+FF01 by code unit and *above* it by
    code point. Ordering by code point made this SDK compute a different receipt core hash than
    the reference server and the TypeScript SDK for the same receipt — one answer would then have
    read as forged to every holder of this SDK, and to no one else.
    """
    from handoff import canonical_bytes

    document = {"！": 1, "\U0001f600": 2, "a": 0}
    expected = '{"a":0,"\U0001f600":2,"！":1}'.encode("utf-8")

    assert canonical_bytes(document) == expected
    assert (
        json.dumps(document, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
        != expected
    ), "code-point order must be a different byte sequence, or this test is not measuring anything"


# -- the deprecated module -------------------------------------------------------------------


def test_the_human_module_still_imports_and_warns():
    import importlib
    import sys

    sys.modules.pop("human", None)
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        module = importlib.import_module("human")
    assert any(issubclass(w.category, DeprecationWarning) for w in caught)
    assert module.ask is handoff.ask
    assert module.__version__ == handoff.__version__


def test_clear_wall_explains_why_it_cannot_be_honoured():
    """It took a resolvable live-view URL, and the protocol carries capabilities as opaque
    handles (§11.1, I8). Failing with the replacement in the message beats a silent removal."""
    with pytest.raises(NotImplementedError, match="opaque handles"):
        handoff.clear_wall(reason="a wall")

"""The resumable client: the wait survives the process that started it.

The prior art's fatal flaw was that the agent process owned the wait loop, so if the agent died
the wait died with it. These tests kill the waiting process — a real `SIGKILL` to a real
subprocess in the middle of a real long poll — and then prove that a later process reattaches by
`waiter_ref` and finds the answer still there, still unacked.

What is asserted here is the protocol's actual guarantee (§1.3): the answer reaches the runtime
as typed data, delivery is at-least-once with an idempotent ack, and one answer authorizes
exactly one effect. Not that execution resumed — nothing here claims that, because nothing can.
"""

from __future__ import annotations

import subprocess
import sys
import textwrap
import threading
import time
from pathlib import Path

import pytest

from fake_server import FakeServer

from handoff import Client, SignalNotApplied
from handoff.errors import HandoffTimeout

SDK_ROOT = Path(__file__).resolve().parents[1]


def _raise_one(client: Client, waiter_ref: str = "run:0198f2a1"):
    return client.raise_request(
        waiter_ref=waiter_ref,
        prompt={"title": "Refund $2,400 to Acme Corp?"},
        requires={
            "v": 1,
            "answer": {"fields": [{"name": "decision", "label": "Decision", "type": "choice"}]},
            "capabilities": [],
            "authority": {"min_role": "editor", "auth_strength": "session"},
        },
    )


@pytest.fixture()
def server():
    with FakeServer() as running:
        yield running


def test_client_death_mid_wait_loses_nothing(server):
    """Kill -9 a process that is blocked in a long poll, then reattach from a fresh one.

    The subprocess is a genuine separate interpreter, so nothing in the parent's memory can be
    what makes this work. The only thing that crosses the boundary is the `waiter_ref` string.
    """
    client = Client(server.base_url, "test-key")
    pending = _raise_one(client)

    waiting = subprocess.Popen(
        [
            sys.executable,
            "-c",
            textwrap.dedent(
                f"""
                import sys
                sys.path.insert(0, {str(SDK_ROOT)!r})
                from handoff import Client
                client = Client({server.base_url!r}, "test-key")
                print("polling", flush=True)
                client.waiter({pending.waiter_ref!r}).next(timeout=60)
                print("SHOULD NOT REACH", flush=True)
                """
            ),
        ],
        stdout=subprocess.PIPE,
        text=True,
    )
    assert waiting.stdout is not None
    assert waiting.stdout.readline().strip() == "polling"
    time.sleep(0.3)

    waiting.kill()
    waiting.wait(timeout=10)
    assert waiting.returncode != 0

    # The person answers while nobody at all is listening.
    human = Client(server.base_url, "human-session")
    human.answer(pending.id, {"decision": "approve", "note": "Confirmed with Acme on the phone."})

    # A later process, with nothing but the waiter_ref.
    resumed = Client(server.base_url, "test-key").resume(pending.waiter_ref)
    signal = resumed.next(timeout=5)

    assert signal.type == "answered"
    assert signal.acked_at is None, "reading a signal must not consume it (§8.3)"
    assert signal.decision is not None
    assert signal.decision.values["decision"] == "approve"
    assert signal.decision.decided_by_human
    assert server.state.reattach_calls == [pending.waiter_ref]


def test_answer_that_lands_before_anyone_reattaches_is_still_waiting(server):
    """The wait was never in a client process, so there is no window in which the answer needs a
    listener to survive."""
    client = Client(server.base_url, "test-key")
    pending = _raise_one(client, "run:no-listener")
    Client(server.base_url, "human-session").answer(pending.id, {"decision": "approve"})

    result = Client(server.base_url, "test-key").reattach(pending.waiter_ref)
    assert result.state == "signalled"
    assert [s.type for s in result.signals] == ["answered"]
    assert result.signals[0].acked_at is None


def test_double_ack_is_idempotent(server):
    """C-12: both calls return 200, `first_ack` is true then false, redelivery stops once."""
    client = Client(server.base_url, "test-key")
    pending = _raise_one(client, "run:double-ack")
    Client(server.base_url, "human-session").answer(pending.id, {"decision": "approve"})

    waiter = client.waiter(pending.waiter_ref)
    signal = waiter.next(timeout=5)

    first = waiter.ack(signal)
    second = waiter.ack(signal)
    assert first.first_ack is True
    assert second.first_ack is False
    assert first.acked_at == second.acked_at

    assert waiter.signals() == [], "an acked signal is consumed and stops being returned"


def test_receive_acks_only_after_the_block_completes(server):
    """The ordering that turns at-least-once delivery into effectively-once application."""
    client = Client(server.base_url, "test-key")
    pending = _raise_one(client, "run:receive-ok")
    Client(server.base_url, "human-session").answer(pending.id, {"decision": "approve"})

    waiter = client.waiter(pending.waiter_ref)
    applied: list[str] = []
    with waiter.receive(timeout=5) as received:
        assert server.state.ack_calls == [], "must not ack before the caller has applied anything"
        applied.append(received.values["decision"])

    assert applied == ["approve"]
    assert len(server.state.ack_calls) == 1
    assert server.state.ack_calls[0][1] is True


def test_an_exception_in_the_block_leaves_the_signal_unacked(server):
    """If applying the outcome failed, the outcome was not received. The signal stays queued and
    the next process to reattach still finds it."""
    client = Client(server.base_url, "test-key")
    pending = _raise_one(client, "run:receive-boom")
    Client(server.base_url, "human-session").answer(pending.id, {"decision": "approve"})

    waiter = client.waiter(pending.waiter_ref)
    with pytest.raises(ZeroDivisionError):
        with waiter.receive(timeout=5):
            1 / 0

    assert server.state.ack_calls == []
    survivor = client.resume(pending.waiter_ref).next(timeout=5)
    assert survivor.type == "answered"
    assert survivor.acked_at is None


def test_unable_records_non_application_without_being_an_error(server):
    """§8.3: `applied: false` with a reason is a fact the record should hold, not an error to
    swallow. Redelivery stops and the reason is kept."""
    client = Client(server.base_url, "test-key")
    pending = _raise_one(client, "run:unable")
    Client(server.base_url, "human-session").answer(pending.id, {"decision": "approve"})

    waiter = client.waiter(pending.waiter_ref)
    with waiter.receive(timeout=5) as received:
        received.unable("the refund API was down")

    assert server.state.ack_calls == [(server.state.ack_calls[0][0], False, "the refund API was down")]
    assert waiter.signals() == []


def test_signal_not_applied_raised_deep_in_a_call_stack_is_recorded(server):
    client = Client(server.base_url, "test-key")
    pending = _raise_one(client, "run:not-applied")
    Client(server.base_url, "human-session").answer(pending.id, {"decision": "approve"})

    waiter = client.waiter(pending.waiter_ref)

    def apply_deeply():
        raise SignalNotApplied("downstream ledger rejected the entry")

    with waiter.receive(timeout=5):
        apply_deeply()

    assert server.state.ack_calls[0][1] is False
    assert server.state.ack_calls[0][2] == "downstream ledger rejected the entry"


def test_a_long_poll_returns_as_soon_as_the_answer_lands(server):
    """The blocking form: one call, held open server-side, satisfied the moment a person acts."""
    client = Client(server.base_url, "test-key")
    pending = _raise_one(client, "run:long-poll")

    def answer_shortly():
        time.sleep(0.4)
        Client(server.base_url, "human-session").answer(pending.id, {"decision": "approve"})

    threading.Thread(target=answer_shortly, daemon=True).start()
    started = time.monotonic()
    signal = pending.wait(timeout=20)
    elapsed = time.monotonic() - started

    assert signal.type == "answered"
    assert elapsed < 10, f"the poll should be satisfied by the answer, not by the window ({elapsed:.1f}s)"


def test_an_unmatched_signal_is_left_in_the_queue(server):
    """A waiter may hold signals for several requests. Retiring one the caller did not ask about
    would lose it, so a filtered-out signal is never acked and never dropped."""
    client = Client(server.base_url, "test-key")
    first = _raise_one(client, "run:two-requests")
    second = _raise_one(client, "run:two-requests")
    human = Client(server.base_url, "human-session")
    human.answer(first.id, {"decision": "approve"})
    human.answer(second.id, {"decision": "reject"})

    waiter = client.waiter("run:two-requests")
    signal = waiter.next(timeout=5, accept=lambda s: s.request_id == second.id)
    assert signal.request_id == second.id
    assert server.state.ack_calls == []

    still_there = [s.request_id for s in client.waiter("run:two-requests").signals()]
    assert first.id in still_there


def test_attempt_lapsed_is_a_nudge_and_the_terminal_signal_still_arrives(server):
    """W2: a non-terminal nudge must never overwrite or mask a later terminal signal, because
    signals are a queue and not a mutable field."""
    client = Client(server.base_url, "test-key")
    pending = _raise_one(client, "run:nudge")
    server.state.enqueue(pending.id, "attempt_lapsed", None)

    nudges = []
    Client(server.base_url, "human-session").answer(pending.id, {"decision": "approve"})

    signal = pending.wait(timeout=5, on_attempt_lapsed=nudges.append)
    assert [n.type for n in nudges] == ["attempt_lapsed"]
    assert signal.type == "answered"
    assert signal.sequence > nudges[0].sequence
    assert pending.waiter.highest_sequence == signal.sequence


def test_redemption_is_idempotent_per_effect_key(server):
    """C-13: one answer, one authorization, one effect. A retried turn must not refund twice."""
    client = Client(server.base_url, "test-key")
    pending = _raise_one(client, "run:redeem")
    Client(server.base_url, "human-session").answer(pending.id, {"decision": "approve"})

    with pending.receive(timeout=5) as received:
        first = received.outcome.redeem("stripe:refund:ch_1B")
        second = received.outcome.redeem("stripe:refund:ch_1B")

    assert first.first_redemption is True
    assert second.first_redemption is False

    authorization_id = received.outcome.authorization_id
    assert authorization_id is not None
    from handoff.errors import AuthorizationSpent

    with pytest.raises(AuthorizationSpent):
        client.redeem(authorization_id, "stripe:refund:ch_DIFFERENT")


def test_local_timeout_is_not_a_protocol_outcome(server):
    """A caller's deadline passing says nothing about the request, which is still pending and
    still answerable. The exception says so and reattaching proves it."""
    client = Client(server.base_url, "test-key")
    pending = _raise_one(client, "run:timeout")

    with pytest.raises(HandoffTimeout) as caught:
        pending.wait(timeout=1)
    assert "durable wait is unaffected" in str(caught.value)

    Client(server.base_url, "human-session").answer(pending.id, {"decision": "approve"})
    assert client.resume(pending.waiter_ref).next(timeout=5).type == "answered"

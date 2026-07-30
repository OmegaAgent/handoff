"""The resumable client: receive, apply, ack.

The failure this exists to prevent is the obvious one. If the wait lives inside the agent
process, the wait dies with the process, and a person's answer lands somewhere nobody is
listening. In this protocol the wait is a durable server-side row (§8) and the client is a
disposable reader of it.

So the SDK offers two shapes over the same durable wait:

* a **blocking** call that long-polls in server-capped windows until a deadline, and
* a **resumable** one where the process may die at any point and a later process reattaches by
  ``waiter_ref``, receives the still-unacked signal, and acks it idempotently.

The ack is the hinge. Delivery is at-least-once and the ack is idempotent, and those two
together give effectively-once *application* — but only if the ack happens after the outcome has
actually been applied. :meth:`Waiter.receive` is built so that the ordering is not something the
caller has to remember: the ack is sent when the block completes, and an exception leaves the
signal unacked and redeliverable.

What this does not do, and no implementation of this protocol can, is resume your execution.
The defensible claim is narrower and worth more: one human answer, delivered to your agent
exactly once, as typed data, authorizing exactly one effect.
"""

from __future__ import annotations

import time
from contextlib import contextmanager
from typing import Any, Callable, Iterator, Mapping, Optional, Sequence

from .errors import HandoffTimeout, SignalNotApplied, TransportError
from .models import AckResult, Decision, ReattachResult, Request, Signal

__all__ = ["Waiter", "Received", "Outcome", "PendingRequest"]


class Outcome:
    """The typed result of one intervention.

    Data the runtime reads, never an instruction it must obey. ``values`` never carries a secret:
    a ``secret`` field arrives as ``{"provided": true}`` and the value itself went to the sink
    the runtime owns (§12, I7).
    """

    __slots__ = ("signal", "_client", "_truthy")

    def __init__(self, signal: Signal, client: Any = None, truthy: Optional[bool] = None):
        self.signal = signal
        self._client = client
        self._truthy = truthy

    @property
    def decision(self) -> Optional[Decision]:
        return self.signal.decision

    @property
    def outcome(self) -> str:
        """``answered``, ``expired``, ``cancelled``, ``superseded`` — or ``attempt_lapsed`` for
        the non-terminal nudge, which decides nothing."""
        decision = self.signal.decision
        return decision.outcome if decision is not None else self.signal.type

    @property
    def values(self) -> dict[str, Any]:
        decision = self.signal.decision
        return decision.values if decision is not None else {}

    @property
    def source(self) -> Optional[str]:
        decision = self.signal.decision
        return decision.source if decision is not None else None

    @property
    def decided_by_human(self) -> bool:
        """True only where a person decided. A policy expiry and a runtime inference are both
        legitimate outcomes and neither is a person (§9.6, I16)."""
        return self.source == "human"

    @property
    def receipt_id(self) -> Optional[str]:
        decision = self.signal.decision
        return decision.receipt_id if decision is not None else None

    @property
    def authorization_id(self) -> Optional[str]:
        decision = self.signal.decision
        return decision.authorization_id if decision is not None else None

    def value(self, name: str, default: Any = None) -> Any:
        return self.values.get(name, default)

    def redeem(self, effect_key: str, *, effect_digest: Optional[str] = None):
        """Spend this decision on exactly one effect, idempotently per ``effect_key`` (§10).

        Call it immediately before performing the effect and act only when
        ``first_redemption`` is true. That is what stops a replayed turn from refunding twice.
        """
        if self._client is None:
            raise RuntimeError("this Outcome was not produced by a Client and cannot redeem")
        authorization_id = self.authorization_id
        if not authorization_id:
            raise RuntimeError(
                f"outcome {self.outcome!r} minted no authorization; there is nothing to spend"
            )
        return self._client.redeem(authorization_id, effect_key, effect_digest=effect_digest)

    def __bool__(self) -> bool:
        if self._truthy is not None:
            return self._truthy
        return self.outcome == "answered"

    def __repr__(self) -> str:
        return f"Outcome(outcome={self.outcome!r}, source={self.source!r}, values={self.values!r})"


class Received:
    """One signal handed to the caller, not yet acked.

    Let the block finish normally and the signal is acked as applied. Call :meth:`unable` (or
    raise :class:`~handoff.errors.SignalNotApplied`) and it is acked as *not* applied, with the
    reason recorded. Let an exception escape and it is **not acked at all**, so the server keeps
    it and the next process to reattach still finds it.
    """

    __slots__ = ("signal", "outcome", "_applied", "_reason")

    def __init__(self, signal: Signal, client: Any = None):
        self.signal = signal
        self.outcome = Outcome(signal, client)
        self._applied = True
        self._reason: Optional[str] = None

    def unable(self, reason: str) -> None:
        """Record that the decision arrived and could not be acted on.

        Not an error. The server accepts it, stops redelivery, and keeps the fact (§8.3).
        """
        self._applied = False
        self._reason = reason

    @property
    def decision(self) -> Optional[Decision]:
        return self.signal.decision

    @property
    def values(self) -> dict[str, Any]:
        return self.outcome.values

    def __repr__(self) -> str:
        return f"Received(type={self.signal.type!r}, sequence={self.signal.sequence!r})"


class Waiter:
    """A handle to one durable server-side wait.

    Constructing this object registers nothing and costs nothing. The waiter itself was created
    server-side by the raise (§8.2 W1) and outlives every process that reads it.
    """

    def __init__(self, client: Any, waiter_ref: str, *, buffered: Sequence[Signal] = ()):
        self.client = client
        self.waiter_ref = waiter_ref
        self._buffer: list[Signal] = list(buffered)
        self._highest_sequence: int = 0

    def __repr__(self) -> str:
        return f"Waiter(waiter_ref={self.waiter_ref!r})"

    # -- reads (never consume) --------------------------------------------------------------

    def signals(self, *, wait: int = 0) -> list[Signal]:
        """Poll for unacked signals. Reading does not consume them (§8.3)."""
        return self.client.poll_signals(self.waiter_ref, wait=wait)

    def reattach(self) -> ReattachResult:
        """Re-arm the lease and collect every unacked signal (§8.5, W7).

        Signals returned here are buffered locally, so the next :meth:`receive` hands them over
        without another round trip.
        """
        result = self.client.reattach(self.waiter_ref)
        known = {s.id for s in self._buffer}
        self._buffer.extend(s for s in result.signals if s.id not in known)
        return result

    def ack(self, signal: Signal | str, *, applied: bool = True, reason: Optional[str] = None) -> AckResult:
        """Consume a signal. Safe to call twice: the second returns ``first_ack: false``."""
        if isinstance(signal, str):
            raise TypeError(
                "ack() needs the Signal itself: the resume_token that authorizes the ack travels "
                "on the signal and is deliberately not something you can pass by id"
            )
        return self.client.ack(signal.id, signal.resume_token, applied=applied, reason=reason)

    # -- blocking ---------------------------------------------------------------------------

    def next(
        self,
        *,
        timeout: Optional[float] = None,
        accept: Optional[Callable[[Signal], bool]] = None,
        poll_wait: Optional[int] = None,
    ) -> Signal:
        """Block until a signal is available and return it **unacked**.

        Long-polls in windows the server caps (``meta.max_wait_seconds``), looping until
        ``timeout``. Hanging up between windows does not affect the durable wait, so a dropped
        connection costs one window and nothing else.

        Signals that ``accept`` rejects are left in the queue untouched — never acked, never
        dropped — because a waiter may be holding signals for several requests and retiring one
        the caller did not ask about would lose it.

        Raises :class:`~handoff.errors.HandoffTimeout` when the caller's own deadline passes.
        That is a local deadline, not a protocol outcome: the request may still be answered, and
        reattaching later will still find the signal.
        """
        deadline = None if timeout is None else time.monotonic() + timeout
        window = poll_wait or self.client.max_wait_seconds()
        failures = 0

        while True:
            for index, signal in enumerate(self._buffer):
                if accept is None or accept(signal):
                    self._buffer.pop(index)
                    self._observe(signal)
                    return signal

            remaining = None if deadline is None else deadline - time.monotonic()
            if remaining is not None and remaining <= 0:
                raise HandoffTimeout(
                    f"no matching signal for {self.waiter_ref!r} within the local deadline; the "
                    "durable wait is unaffected and reattaching later will still find it",
                    waiter_ref=self.waiter_ref,
                )
            this_window = window if remaining is None else max(1, min(window, int(remaining)))

            try:
                found = self.signals(wait=this_window)
                failures = 0
            except TransportError:
                # The wait is on the server. Losing the connection to it is a retry, not a loss.
                failures += 1
                if failures >= 5:
                    raise
                time.sleep(min(2**failures, 10))
                continue

            known = {s.id for s in self._buffer}
            self._buffer.extend(s for s in found if s.id not in known)
            if not found and remaining is None and this_window <= 0:  # pragma: no cover
                time.sleep(1)

    def _observe(self, signal: Signal) -> None:
        sequence = signal.get("sequence")
        if isinstance(sequence, int):
            self._highest_sequence = max(self._highest_sequence, sequence)

    @property
    def highest_sequence(self) -> int:
        """The highest sequence this handle has handed out.

        Sequence is monotonic per ``waiter_ref``, so a gap means a signal is in flight or was
        reordered. A gap is not by itself an error — delivery is at-least-once and retries
        reorder — but one that never closes is worth raising operationally (signing.md §1.3).
        """
        return self._highest_sequence

    @contextmanager
    def receive(
        self,
        *,
        timeout: Optional[float] = None,
        accept: Optional[Callable[[Signal], bool]] = None,
        poll_wait: Optional[int] = None,
    ) -> Iterator[Received]:
        """Receive one signal, apply it in the block, and ack on the way out.

            with waiter.receive(timeout=3600) as received:
                apply(received.values)          # your work

        The ack is sent only after the block completes. If the block raises, nothing is acked and
        the signal stays queued for the next process to reattach and find. That ordering is the
        point: acking first and applying second would turn at-least-once delivery into
        at-most-once application, which is the bug this protocol exists to make impossible.

        Raising :class:`~handoff.errors.SignalNotApplied` inside the block acks with
        ``applied: false`` and the reason, and does not propagate — it is a way to record
        non-application from deep in a call stack, and the recording is the handling.
        """
        signal = self.next(timeout=timeout, accept=accept, poll_wait=poll_wait)
        received = Received(signal, self.client)
        try:
            yield received
        except SignalNotApplied as exc:
            self.ack(signal, applied=False, reason=exc.reason)
        except BaseException:
            raise  # deliberately no ack: unapplied means undelivered
        else:
            self.ack(signal, applied=received._applied, reason=received._reason)

    def wait_for_outcome(
        self,
        *,
        request_id: Optional[str] = None,
        timeout: Optional[float] = None,
        on_attempt_lapsed: Optional[Callable[[Signal], None]] = None,
        ack_nudges: bool = True,
    ) -> Signal:
        """Block until a **terminal** signal arrives and return it unacked.

        ``attempt_lapsed`` is a nudge, not an outcome: the request is still pending and still
        answerable, and the person has simply gone quiet for a while (§6.3). It is reported to
        ``on_attempt_lapsed`` and then acked, because there is no outcome to apply and leaving it
        unacked only makes the server redeliver a notification you have already seen. Pass
        ``ack_nudges=False`` to leave it queued.

        The returned signal is unacked on purpose. Ack it once you have applied the outcome —
        or use :meth:`receive`, which sequences that for you.
        """
        deadline = None if timeout is None else time.monotonic() + timeout
        while True:
            remaining = None if deadline is None else max(0.0, deadline - time.monotonic())
            signal = self.next(
                timeout=remaining,
                accept=lambda s: request_id is None or s.request_id == request_id,
            )
            if signal.is_terminal:
                return signal
            if on_attempt_lapsed is not None:
                on_attempt_lapsed(signal)
            if ack_nudges:
                self.ack(signal, applied=True)


class PendingRequest:
    """A raised request and the durable wait registered with it.

    Hold on to :attr:`waiter_ref`, not to this object. It is the ``waiter_ref`` that a later
    process needs, and it is the only thing that has to survive.
    """

    __slots__ = ("request", "waiter", "was_created", "_client")

    def __init__(self, client: Any, request: Request, was_created: bool):
        self._client = client
        self.request = request
        self.was_created = was_created
        self.waiter = Waiter(client, request.waiter_ref)

    @property
    def id(self) -> str:
        return self.request.id

    @property
    def waiter_ref(self) -> str:
        return self.request.waiter_ref

    @property
    def surface_url(self) -> Optional[str]:
        """Where a person goes to answer. A locator, not a capability: opening it prompts for
        authentication and holding it authorizes nothing (§4.6)."""
        return self.request.surface_url

    def wait(
        self,
        *,
        timeout: Optional[float] = None,
        on_attempt_lapsed: Optional[Callable[[Signal], None]] = None,
    ) -> Signal:
        """Block for this request's terminal signal. Returns it **unacked**."""
        return self.waiter.wait_for_outcome(
            request_id=self.id, timeout=timeout, on_attempt_lapsed=on_attempt_lapsed
        )

    @contextmanager
    def receive(
        self, *, timeout: Optional[float] = None, on_attempt_lapsed: Optional[Callable[[Signal], None]] = None
    ) -> Iterator[Received]:
        """Wait for this request's outcome, apply it in the block, ack on the way out."""
        signal = self.wait(timeout=timeout, on_attempt_lapsed=on_attempt_lapsed)
        received = Received(signal, self._client)
        try:
            yield received
        except SignalNotApplied as exc:
            self.waiter.ack(signal, applied=False, reason=exc.reason)
        except BaseException:
            raise
        else:
            self.waiter.ack(signal, applied=received._applied, reason=received._reason)

    def cancel(self, reason: str) -> Request:
        return self._client.cancel(self.id, reason)

    def __repr__(self) -> str:
        return f"PendingRequest(id={self.id!r}, waiter_ref={self.waiter_ref!r}, was_created={self.was_created!r})"

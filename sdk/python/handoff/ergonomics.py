"""Two shorthands for the two asks people reach for first.

Both are ordinary declarations. ``ask`` declares one ``text`` field, ``approve`` declares one
``choice`` field, and the server cannot tell which function you called — there is no kind on the
wire and no branch behind it (I14). Anything they do not cover is
:meth:`handoff.Client.raise_request` with a declaration you build yourself, which is the same
code path with more of it spelled out.
"""

from __future__ import annotations

from typing import Any, Mapping, Optional, Sequence

from .client import new_idempotency_key
from .errors import HandoffTimeout
from .models import authority, fields, prompt, requires, ttl_policy
from .waiter import Outcome

__all__ = ["ask", "approve", "iso_duration"]


def iso_duration(seconds: Optional[float]) -> Optional[str]:
    """Seconds to an ISO 8601 duration (§1.4)."""
    if seconds is None:
        return None
    return f"PT{int(seconds)}S"


def _waiter_ref(explicit: Optional[str]) -> str:
    return explicit or f"run:{new_idempotency_key()}"


def ask(
    client: Any,
    question: str,
    *,
    default: Optional[str] = None,
    body: Optional[str] = None,
    evidence: Sequence[Mapping[str, Any]] = (),
    timeout: Optional[float] = 600,
    ttl: Optional[str] = None,
    waiter_ref: Optional[str] = None,
    min_role: str = "viewer",
    auth_strength: str = "session",
    label: str = "Answer",
    metadata: Optional[Mapping[str, Any]] = None,
    ack: bool = True,
    **raise_kwargs: Any,
) -> Optional[str]:
    """Ask a person a question and block until they answer. Returns their text.

    ``default`` is declared **at raise time**, not applied locally afterwards. It becomes
    ``ttl_policy: {on_expiry: "default", default_answer: …}``, so when nobody answers the server
    mints a policy receipt with ``actor.type = "policy"`` and the record cannot be mistaken for
    consent (§6.4). Guessing a default in the client after the fact would produce the same value
    with no record at all — the same behaviour and a worse audit trail.

    A ``default`` also needs a ``ttl``: without one the request never expires, so the policy
    never fires. If you give a ``timeout`` and no ``ttl``, the timeout is used as the ttl so that
    the local deadline and the declared one agree.

    This is the convenient blocking form and it acks on receipt, which means an answer is
    consumed the moment it reaches this process. Where losing it to a crash between here and
    your side effect would matter, use :meth:`handoff.Waiter.receive` instead: it acks after
    your block has applied the outcome, and leaves the signal queued if it raises.
    """
    if default is not None and ttl is None:
        ttl = iso_duration(timeout)
    policy = (
        ttl_policy("default", default_answer={"answer": default})
        if default is not None and ttl is not None
        else None
    )
    pending = client.raise_request(
        waiter_ref=_waiter_ref(waiter_ref),
        prompt=prompt(question, body, evidence),
        requires=requires(
            [fields.text("answer", label, required=True)],
            authority=authority(min_role, auth_strength),
        ),
        ttl=ttl,
        ttl_policy=policy,
        metadata=dict(metadata) if metadata else None,
        **raise_kwargs,
    )
    try:
        signal = pending.wait(timeout=timeout)
    except HandoffTimeout:
        if default is not None:
            return default
        raise
    if ack:
        pending.waiter.ack(signal, applied=True)
    outcome = Outcome(signal, client)
    if outcome.outcome != "answered" and "answer" not in outcome.values:
        if default is not None:
            return default
        raise HandoffTimeout(
            f"request {pending.id} ended as {outcome.outcome!r} with no answer",
            waiter_ref=pending.waiter_ref,
            request_id=pending.id,
        )
    return outcome.value("answer")


def approve(
    client: Any,
    title: str,
    *,
    body: Optional[str] = None,
    evidence: Sequence[Mapping[str, Any]] = (),
    approve_label: str = "Approve",
    reject_label: str = "Reject",
    with_note: bool = True,
    timeout: Optional[float] = 600,
    ttl: Optional[str] = None,
    waiter_ref: Optional[str] = None,
    min_role: str = "editor",
    auth_strength: str = "session",
    reason: Optional[str] = None,
    mode: Optional[str] = None,
    metadata: Optional[Mapping[str, Any]] = None,
    ack: bool = True,
    **raise_kwargs: Any,
) -> Outcome:
    """Ask a person to approve or reject, and block. Returns an :class:`~handoff.Outcome`.

    The outcome is truthy only when a person chose to approve. An expiry, a cancellation, and a
    supersession are all falsey and all carry their own typed outcome — the request never goes
    quiet, and "nobody answered" is never silently the same as "approved" (§6.4, I11).

    Pass ``mode="gated"`` when the effect must not happen without a redemption, then call
    ``outcome.redeem(effect_key)`` immediately before performing it and act only when
    ``first_redemption`` is true (§10).
    """
    declared = [
        fields.choice(
            "decision",
            "Decision",
            [("approve", approve_label), ("reject", reject_label)],
            required=True,
        )
    ]
    if with_note:
        declared.append(fields.text("note", "Add a note", required=False, max_len=500))

    pending = client.raise_request(
        waiter_ref=_waiter_ref(waiter_ref),
        prompt=prompt(title, body, evidence),
        requires=requires(declared, authority=authority(min_role, auth_strength, reason=reason)),
        ttl=ttl,
        mode=mode,
        metadata=dict(metadata) if metadata else None,
        **raise_kwargs,
    )
    signal = pending.wait(timeout=timeout)
    if ack:
        pending.waiter.ack(signal, applied=True)
    outcome = Outcome(signal, client)
    return Outcome(signal, client, truthy=outcome.value("decision") == "approve")

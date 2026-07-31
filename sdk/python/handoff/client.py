"""The HTTP client. Standard library only.

What this client does *not* own is the wait. The wait is a durable row on the server, keyed by
``waiter_ref`` (§8). This object is a handle to it and is cheap to throw away: a later process
that knows the ``waiter_ref`` reattaches and finds the same unacked signals. That is the whole
difference between a protocol and a loop.
"""

from __future__ import annotations

import json
import os
import random
import time
import urllib.error
import urllib.parse
import urllib.request
from typing import Any, Mapping, Optional

from ._document import ordered
from .errors import HandoffError, TransportError, from_error_body
from .models import (
    AckResult,
    AnswerResult,
    Authorization,
    Meta,
    ReattachResult,
    RedeemResult,
    Request,
    Signal,
)
from .waiter import PendingRequest, Waiter

__all__ = ["Client", "new_idempotency_key"]

DEFAULT_BASE_URL = "https://handoff.omegas.dev"
_CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"


def new_idempotency_key() -> str:
    """A fresh time-sortable key.

    Reused verbatim across this client's own transport retries, so a retry that reaches the
    server after a response was lost returns the stored representation instead of raising a
    second ask (§3.3, §3.5).
    """
    value = (int(time.time() * 1000) << 80) | random.getrandbits(80)
    out = []
    for _ in range(26):
        out.append(_CROCKFORD[value & 0x1F])
        value >>= 5
    return "".join(reversed(out))


class _Secret:
    """Holds a credential so that a stray repr, log line, or traceback cannot print it (I18)."""

    __slots__ = ("_value",)

    def __init__(self, value: Optional[str]):
        self._value = value

    def reveal(self) -> Optional[str]:
        return self._value

    def __bool__(self) -> bool:
        return bool(self._value)

    def __repr__(self) -> str:
        return "<redacted>" if self._value else "None"

    __str__ = __repr__


class Client:
    """A Handoff API client.

    ``api_key`` authenticates an org-scoped service account. It can raise, read, poll, ack, and
    redeem — and it can never answer, by principal type and under no configuration (§4.2, I15).
    """

    def __init__(
        self,
        base_url: Optional[str] = None,
        api_key: Optional[str] = None,
        *,
        timeout: float = 30.0,
        user_agent: str = "handoff-python/0.2.0",
        max_transport_retries: int = 3,
    ):
        self.base_url = (base_url or os.environ.get("HANDOFF_URL") or DEFAULT_BASE_URL).rstrip("/")
        self._api_key = _Secret(api_key or os.environ.get("HANDOFF_API_KEY"))
        self.timeout = timeout
        self.user_agent = user_agent
        self.max_transport_retries = max_transport_retries
        self._max_wait: Optional[int] = None

    def __repr__(self) -> str:
        return f"Client(base_url={self.base_url!r}, api_key=<redacted>)"

    # -- transport -------------------------------------------------------------------------

    def call(
        self,
        method: str,
        path: str,
        body: Optional[Mapping[str, Any]] = None,
        *,
        idempotency_key: Optional[str] = None,
        timeout: Optional[float] = None,
        query: Optional[Mapping[str, Any]] = None,
        retries: Optional[int] = None,
    ) -> tuple[int, Any]:
        """One request. Returns ``(status, parsed_body)``; ``204`` parses as ``None``.

        Secrets never reach the URL: query values are for wait windows and cursors only (I18).
        """
        url = self.base_url + path
        if query:
            url += "?" + urllib.parse.urlencode({k: v for k, v in query.items() if v is not None})
        payload = json.dumps(body).encode("utf-8") if body is not None else None
        attempts = self.max_transport_retries if retries is None else retries

        last: Optional[Exception] = None
        for attempt in range(max(1, attempts)):
            request = urllib.request.Request(url, data=payload, method=method)
            request.add_header("Accept", "application/json")
            request.add_header("User-Agent", self.user_agent)
            if payload is not None:
                request.add_header("Content-Type", "application/json")
            if self._api_key:
                request.add_header("Authorization", f"Bearer {self._api_key.reveal()}")
            if idempotency_key:
                request.add_header("Idempotency-Key", idempotency_key)
            try:
                with urllib.request.urlopen(request, timeout=timeout or self.timeout) as response:
                    raw = response.read()
                    if response.status == 204 or not raw:
                        return response.status, None
                    return response.status, json.loads(raw)
            except urllib.error.HTTPError as exc:
                raise self._error_from(exc) from None
            except (urllib.error.URLError, TimeoutError, OSError) as exc:
                # A dropped long poll is expected, not exceptional: the durable wait is
                # unaffected and re-polling picks up exactly where this left off.
                last = exc
                if attempt + 1 < max(1, attempts):
                    time.sleep(min(2**attempt, 8) * (0.5 + random.random() / 2))
                    continue
                raise TransportError(f"{method} {path} could not be completed: {exc}") from None
        raise TransportError(f"{method} {path} failed: {last}")  # pragma: no cover

    def _error_from(self, exc: "urllib.error.HTTPError") -> HandoffError:
        try:
            body = json.loads(exc.read() or b"{}")
        except (ValueError, OSError):
            body = {}
        retry_after: Optional[float] = None
        raw_retry = exc.headers.get("Retry-After") if exc.headers else None
        if raw_retry:
            try:
                retry_after = float(raw_retry)
            except ValueError:
                retry_after = None
        if not isinstance(body, Mapping) or "error" not in body:
            return TransportError(
                f"HTTP {exc.code} without the protocol error envelope", status=exc.code, retry_after=retry_after
            )
        return from_error_body(body, status=exc.code, retry_after=retry_after)

    # -- discovery -------------------------------------------------------------------------

    def meta(self) -> Meta:
        """What this deployment supports (§19). Unauthenticated."""
        _, body = self.call("GET", "/v1/meta")
        result = Meta(body)
        self._max_wait = result.max_wait_seconds
        return result

    def max_wait_seconds(self) -> int:
        """The server's long-poll cap, discovered once and cached. Falls back to the protocol's
        documented 30s if discovery is unavailable — a larger value is clamped, not rejected."""
        if self._max_wait is None:
            try:
                self.meta()
            except (HandoffError, OSError):
                self._max_wait = 30
        return self._max_wait or 30

    # -- requests --------------------------------------------------------------------------

    def raise_request(
        self,
        waiter_ref: str,
        prompt: Mapping[str, Any],
        requires: Mapping[str, Any],
        *,
        liveness: Optional[str] = None,
        urgency: Optional[str] = None,
        ttl: Optional[str] = None,
        ttl_policy: Optional[Mapping[str, Any]] = None,
        attempt_ttl: Optional[str] = None,
        on_waiter_terminal: Optional[str] = None,
        routing: Optional[Mapping[str, Any]] = None,
        mode: Optional[str] = None,
        presentation_binding: Optional[str] = None,
        dedupe_key: Optional[str] = None,
        resume_ref: Optional[str] = None,
        resume_payload: Optional[str] = None,
        callback: Optional[Mapping[str, Any]] = None,
        metadata: Optional[Mapping[str, Any]] = None,
        test_mode: Optional[bool] = None,
        idempotency_key: Optional[str] = None,
    ) -> "PendingRequest":
        """Raise a request and return a handle to it and its durable wait.

        ``PendingRequest.was_created`` is the ``201``/``200`` distinction and it is contract: it
        is how a caller tells "I asked" from "I already asked" (§3.3). A replay after a person
        has answered returns the answered request with its receipt and pages nobody.

        The raise does not block on delivery. Deliveries come back ``queued``, and a channel
        outage never takes the caller's agent down with it (§7.3).
        """
        body = ordered(
            ("waiter_ref", waiter_ref),
            ("liveness", liveness),
            ("urgency", urgency),
            ("prompt", dict(prompt)),
            ("requires", dict(requires)),
            ("ttl", ttl),
            ("ttl_policy", dict(ttl_policy) if ttl_policy else None),
            ("attempt_ttl", attempt_ttl),
            ("on_waiter_terminal", on_waiter_terminal),
            ("routing", dict(routing) if routing else None),
            ("mode", mode),
            ("presentation_binding", presentation_binding),
            ("dedupe_key", dedupe_key),
            ("resume_ref", resume_ref),
            ("resume_payload", resume_payload),
            ("callback", dict(callback) if callback else None),
            ("metadata", dict(metadata) if metadata else None),
            ("test_mode", test_mode),
        )
        status, response = self.call(
            "POST", "/v1/requests", body, idempotency_key=idempotency_key or new_idempotency_key()
        )
        return PendingRequest(self, Request(response), status == 201)

    def get_request(self, request_id: str) -> Request:
        _, body = self.call("GET", f"/v1/requests/{urllib.parse.quote(request_id, safe='')}")
        return Request(body)

    def cancel(self, request_id: str, reason: str, *, idempotency_key: Optional[str] = None) -> Request:
        """Withdraw the ask. A landed answer wins the race and this returns ``409
        already_answered`` — a machine changing its mind must not discard a person's work (R11)."""
        _, body = self.call(
            "POST",
            f"/v1/requests/{urllib.parse.quote(request_id, safe='')}/cancel",
            {"reason": reason},
            idempotency_key=idempotency_key or new_idempotency_key(),
        )
        return Request(body)

    def answer(
        self,
        request_id: str,
        values: Mapping[str, Any],
        *,
        via_delivery_id: Optional[str] = None,
        partial: Optional[bool] = None,
        note: Optional[str] = None,
        disposition: Optional[str] = None,
        rendered_digest: Optional[str] = None,
        idempotency_key: Optional[str] = None,
    ) -> AnswerResult:
        """Settle a request. **Human principals only.**

        A machine key gets ``403 requester_may_not_answer``, by principal type rather than by
        role or setting (§4.2). This method exists for clients holding a person's own session —
        a surface, or a test harness standing in for one — and calling it with a service-account
        key is expected to fail.
        """
        body = ordered(
            ("values", dict(values)),
            ("via_delivery_id", via_delivery_id),
            ("partial", partial),
            ("note", note),
            ("disposition", disposition),
            ("rendered_digest", rendered_digest),
        )
        _, response = self.call(
            "POST",
            f"/v1/requests/{urllib.parse.quote(request_id, safe='')}/answer",
            body,
            idempotency_key=idempotency_key or new_idempotency_key(),
        )
        return AnswerResult(response)

    # -- waiters ---------------------------------------------------------------------------

    def poll_signals(self, waiter_ref: str, *, wait: int = 0) -> list[Signal]:
        """Every unacked signal for this waiter, oldest first. **Reading does not consume.**

        Consumption is the ack. A client that reads a signal and then dies has not received it,
        and the server keeps it until an explicit ack arrives (§8.3).
        """
        status, body = self.call(
            "GET",
            f"/v1/waiters/{urllib.parse.quote(waiter_ref, safe='')}/signals",
            query={"wait": wait} if wait else None,
            timeout=self.timeout + wait,
        )
        if status == 204 or not body:
            return []
        return [Signal(s) for s in body.get("data", [])]

    def waiter(self, waiter_ref: str) -> Waiter:
        """A handle to an existing durable wait. Registers nothing; costs nothing."""
        return Waiter(self, waiter_ref)

    def resume(self, waiter_ref: str) -> Waiter:
        """Reattach to a wait a previous process was holding, and return a handle carrying
        everything it was still owed (§8.5).

        This is the whole restart recipe. The only thing that had to survive the crash is the
        ``waiter_ref`` string.
        """
        waiter = Waiter(self, waiter_ref)
        waiter.reattach()
        return waiter

    def reattach(self, waiter_ref: str, *, idempotency_key: Optional[str] = None) -> ReattachResult:
        """Re-arm this waiter and collect everything it is still holding (§8.5, W7).

        This is the operation that makes a client's own process death survivable. The wait was
        never in that process, so nothing was lost while it was gone.
        """
        _, body = self.call(
            "POST",
            f"/v1/waiters/{urllib.parse.quote(waiter_ref, safe='')}/reattach",
            {},
            idempotency_key=idempotency_key or new_idempotency_key(),
        )
        return ReattachResult(body)

    def ack(
        self, signal_id: str, resume_token: str, *, applied: bool = True, reason: Optional[str] = None
    ) -> AckResult:
        """Consume a signal. Idempotent: ``first_ack`` is true then false, and redelivery stops
        once (§3.5, C-12).

        ``applied=False`` with a reason is not an error. It records that the decision arrived and
        could not be acted on, which is a fact worth holding rather than swallowing (§8.3).
        """
        body = ordered(("resume_token", resume_token), ("applied", applied), ("reason", reason))
        _, response = self.call(
            "POST", f"/v1/signals/{urllib.parse.quote(signal_id, safe='')}/ack", body
        )
        return AckResult(response)

    # -- shorthands ------------------------------------------------------------------------

    def ask(self, question: str, **kwargs: Any) -> Optional[str]:
        """Ask a person a question and block until they answer. See :func:`handoff.ask`."""
        from .ergonomics import ask as _ask

        return _ask(self, question, **kwargs)

    def approve(self, title: str, **kwargs: Any) -> Any:
        """Ask a person to approve or reject, and block. See :func:`handoff.approve`."""
        from .ergonomics import approve as _approve

        return _approve(self, title, **kwargs)

    # -- authorizations --------------------------------------------------------------------

    def get_authorization(self, authorization_id: str) -> Authorization:
        _, body = self.call("GET", f"/v1/authorizations/{urllib.parse.quote(authorization_id, safe='')}")
        return Authorization(body)

    def redeem(
        self, authorization_id: str, effect_key: str, *, effect_digest: Optional[str] = None
    ) -> RedeemResult:
        """Spend one decision on exactly one effect.

        Idempotent per ``effect_key``: a replay returns ``first_redemption: false``, so a retried
        agent turn cannot send the customer a second refund. The key must be **stable per
        effect** — one that varies per attempt defeats the entire mechanism (§10, C-13).
        """
        body = ordered(("effect_key", effect_key), ("effect_digest", effect_digest))
        _, response = self.call(
            "POST",
            f"/v1/authorizations/{urllib.parse.quote(authorization_id, safe='')}/redeem",
            body,
        )
        return RedeemResult(response)

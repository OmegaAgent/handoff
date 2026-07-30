"""The exception taxonomy, mirroring protocol §13.

Every error the server returns uses one envelope and carries a stable machine-readable
``code``. Client code branches on the code — or on the exception class, which is the same
thing with better ergonomics — and never on ``message``, which is written for people and may
change at any time.

An unrecognized code raises :class:`HandoffProtocolError` with the code intact rather than
being coerced into the nearest familiar class. Guessing at an error you do not understand is
how a client silently mishandles a state the server is telling it about.
"""

from __future__ import annotations

from typing import Any, Mapping, Optional, Sequence

__all__ = [
    "HandoffError",
    "HandoffProtocolError",
    "TransportError",
    "InvalidRequest",
    "UnsupportedFieldType",
    "UnsupportedCapabilityType",
    "UnsupportedRequiresVersion",
    "AuthenticationError",
    "InsufficientScope",
    "NotEntitled",
    "InsufficientAuthority",
    "RequesterMayNotAnswer",
    "TenantMismatch",
    "AuthStrengthNotPermitted",
    "NotFound",
    "RequestNotFound",
    "CapabilityNotFound",
    "SignalNotFound",
    "AuthorizationNotFound",
    "AlreadyAnswered",
    "RequestExpired",
    "RequestCancelled",
    "RequestSuperseded",
    "RequestInProgress",
    "IdempotencyKeyReused",
    "AuthorizationSpent",
    "EffectDigestMismatch",
    "BlastRadiusMismatch",
    "GrantAlreadyHeld",
    "PresentationStale",
    "CapabilityExpired",
    "AnswerValidationFailed",
    "RateLimited",
    "DeliveryUnavailable",
    "FieldError",
    "HandoffTimeout",
    "SignalNotApplied",
    "CallbackSignatureError",
    "from_error_body",
]


class FieldError:
    """One field-level validation failure, so a surface can mark the offending input."""

    __slots__ = ("name", "code", "message")

    def __init__(self, name: str, code: str, message: Optional[str] = None):
        self.name = name
        self.code = code
        self.message = message

    @classmethod
    def from_json(cls, data: Mapping[str, Any]) -> "FieldError":
        return cls(name=data.get("name", ""), code=data.get("code", ""), message=data.get("message"))

    def __repr__(self) -> str:
        return f"FieldError(name={self.name!r}, code={self.code!r})"


class HandoffError(Exception):
    """Base class for everything this SDK raises against a Handoff server.

    Carries the protocol's error envelope verbatim. Never carries a credential: the message
    is the server's own text plus identifiers, all of which are ordinary data (§1.4).
    """

    code: str = ""

    def __init__(
        self,
        message: str,
        *,
        code: str = "",
        status: Optional[int] = None,
        request_id: Optional[str] = None,
        receipt_id: Optional[str] = None,
        superseded_by: Optional[str] = None,
        fields: Sequence[FieldError] = (),
        docs: Optional[str] = None,
        retry_after: Optional[float] = None,
    ):
        super().__init__(message)
        self.message = message
        self.code = code or type(self).code
        self.status = status
        self.request_id = request_id
        self.receipt_id = receipt_id
        self.superseded_by = superseded_by
        self.fields = tuple(fields)
        self.docs = docs
        self.retry_after = retry_after

    def __repr__(self) -> str:
        return f"{type(self).__name__}(code={self.code!r}, status={self.status!r}, message={self.message!r})"


class HandoffProtocolError(HandoffError):
    """The server returned an error code this SDK version does not recognize.

    Fail closed and keep the code (§19, I21): a client that maps an unknown code onto a
    familiar one handles a state it does not understand as one it does.
    """


class TransportError(HandoffError):
    """The server could not be reached, or answered with something that was not the envelope."""

    code = "transport_error"


class HandoffTimeout(TimeoutError):
    """The caller's own deadline passed with no terminal signal.

    This is a local deadline, not a protocol outcome. The durable wait is still on the server
    and the request may still be answered; reattach to pick it up. Where "nobody answered"
    must be an outcome, declare it at raise time with ``ttl_policy`` (§6.4) so that the record
    says a policy decided.
    """

    def __init__(self, message: str, *, waiter_ref: Optional[str] = None, request_id: Optional[str] = None):
        super().__init__(message)
        self.waiter_ref = waiter_ref
        self.request_id = request_id


class SignalNotApplied(Exception):
    """Raised inside a ``receive()`` block to record that the decision could not be applied.

    The signal is acked with ``applied: false`` and the reason, which stops redelivery and
    records the fact (§8.3). It is not swallowed and it is not an error the server rejects.
    """

    def __init__(self, reason: str):
        super().__init__(reason)
        self.reason = reason


class CallbackSignatureError(Exception):
    """An inbound callback failed verification (§15, signing.md §1.3).

    The message names which check failed and never includes a secret, a computed signature, or
    any value derived from a secret — an attacker who can read rejection detail should learn
    nothing they could not compute themselves.
    """


# -- concrete codes ------------------------------------------------------------------------


class InvalidRequest(HandoffError):
    code = "invalid_request"


class UnsupportedFieldType(HandoffError):
    """A declared answer field type this server will not accept. Nothing was created (§5.3, I21)."""

    code = "unsupported_field_type"


class UnsupportedCapabilityType(HandoffError):
    code = "unsupported_capability_type"


class UnsupportedRequiresVersion(HandoffError):
    """The ``requires.v`` envelope version is not implemented here. No request exists (§5.2, C-16)."""

    code = "unsupported_requires_version"


class AuthenticationError(HandoffError):
    """Absent, malformed, revoked, or expired credentials — deliberately one code (§13)."""

    code = "invalid_api_key"


class AuthenticationRequired(AuthenticationError):
    code = "authentication_required"


class InsufficientScope(HandoffError):
    code = "insufficient_scope"


class NotEntitled(HandoffError):
    code = "product_not_entitled"


class InsufficientAuthority(HandoffError):
    """The answerer did not meet the authority the request declared (§4.3, §4.4)."""

    code = "insufficient_authority"


class RequesterMayNotAnswer(HandoffError):
    """A machine principal tried to answer. Enforced by principal type and by nothing else (§4.2)."""

    code = "requester_may_not_answer"


class TenantMismatch(HandoffError):
    code = "tenant_mismatch"


class AuthStrengthNotPermitted(HandoffError):
    code = "auth_strength_not_permitted"


class NotFound(HandoffError):
    """Returned instead of 403 wherever existence is itself sensitive (§3.2)."""

    code = "request_not_found"


class RequestNotFound(NotFound):
    code = "request_not_found"


class CapabilityNotFound(NotFound):
    code = "capability_not_found"


class SignalNotFound(NotFound):
    code = "signal_not_found"


class AuthorizationNotFound(NotFound):
    code = "authorization_not_found"


class AlreadyAnswered(HandoffError):
    """A person already settled this. ``receipt_id`` names the decision that exists (§6.7, I5)."""

    code = "already_answered"


class RequestExpired(HandoffError):
    code = "request_expired"


class RequestCancelled(HandoffError):
    code = "request_cancelled"


class RequestSuperseded(HandoffError):
    """``superseded_by`` names where to send the person instead (§6.5)."""

    code = "request_superseded"


class RequestInProgress(HandoffError):
    """Somebody has begun answering; amend is refused and the caller must supersede (§6.2 R2)."""

    code = "request_in_progress"


class IdempotencyKeyReused(HandoffError):
    """Same key, different body. The stored request was not modified (§3.3)."""

    code = "idempotency_key_reused"


class AuthorizationSpent(HandoffError):
    """A single-use authorization was redeemed with a different ``effect_key`` (§10, I10)."""

    code = "authorization_spent"


class EffectDigestMismatch(HandoffError):
    """The effect's shape disagrees with what was authorized (§10). Approval of one amount
    cannot be spent on another."""

    code = "effect_digest_mismatch"


class BlastRadiusMismatch(HandoffError):
    code = "blast_radius_mismatch"


class GrantAlreadyHeld(HandoffError):
    code = "grant_already_held"


class PresentationStale(HandoffError):
    """Under ``presentation_binding: strict``, the answer was against wording the person is no
    longer being shown (§9.3)."""

    code = "presentation_stale"


class CapabilityExpired(HandoffError):
    code = "capability_expired"


class AnswerValidationFailed(HandoffError):
    """Carries per-field detail in ``.fields`` (§5.3, §13)."""

    code = "answer_validation_failed"


class RateLimited(HandoffError):
    code = "rate_limited"


class DeliveryUnavailable(HandoffError):
    """The request exists and the ladder will retry. A channel outage never loses the ask (§7.3)."""

    code = "delivery_unavailable"


_BY_CODE: dict[str, type[HandoffError]] = {
    cls.code: cls
    for cls in (
        InvalidRequest,
        UnsupportedFieldType,
        UnsupportedCapabilityType,
        UnsupportedRequiresVersion,
        AuthenticationError,
        AuthenticationRequired,
        InsufficientScope,
        NotEntitled,
        InsufficientAuthority,
        RequesterMayNotAnswer,
        TenantMismatch,
        AuthStrengthNotPermitted,
        RequestNotFound,
        CapabilityNotFound,
        SignalNotFound,
        AuthorizationNotFound,
        AlreadyAnswered,
        RequestExpired,
        RequestCancelled,
        RequestSuperseded,
        RequestInProgress,
        IdempotencyKeyReused,
        AuthorizationSpent,
        EffectDigestMismatch,
        BlastRadiusMismatch,
        GrantAlreadyHeld,
        PresentationStale,
        CapabilityExpired,
        AnswerValidationFailed,
        RateLimited,
        DeliveryUnavailable,
    )
}


def from_error_body(
    body: Mapping[str, Any], *, status: Optional[int] = None, retry_after: Optional[float] = None
) -> HandoffError:
    """Build the right exception from the protocol's single error envelope (§13)."""
    error = body.get("error") if isinstance(body.get("error"), Mapping) else {}
    code = str(error.get("code") or "")
    message = str(error.get("message") or f"HTTP {status}")
    cls = _BY_CODE.get(code, HandoffProtocolError)
    return cls(
        message,
        code=code,
        status=status,
        request_id=error.get("request_id"),
        receipt_id=error.get("receipt_id"),
        superseded_by=error.get("superseded_by"),
        fields=[FieldError.from_json(f) for f in error.get("fields") or () if isinstance(f, Mapping)],
        docs=error.get("docs"),
        retry_after=retry_after,
    )

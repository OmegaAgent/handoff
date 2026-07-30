"""The Python client for the Handoff protocol — human intervention in automated work.

A program that cannot proceed asks a person, a person answers, and the answer comes back as
typed data with a durable record of who decided what, on what basis, and through which channel.

    import handoff

    handoff.configure(base_url="https://handoff.example.com", api_key=...)

    # Ask a person. Blocks until they answer.
    address = handoff.ask("Which shipping address should I use?", default="the billing address")

    # Ask for a decision, then spend it on exactly one effect.
    outcome = handoff.approve("Refund $2,400 to Acme Corp?", mode="gated")
    if outcome and outcome.redeem("stripe:refund:ch_1B").first_redemption:
        stripe.refund("ch_1B")

The wait is not in this process. It is a durable row on the server keyed by ``waiter_ref``
(protocol §8), so a client may die at any point and a later one reattaches and finds the answer
still waiting, unacked:

    waiter = handoff.resume("run:0198f2a1")
    with waiter.receive() as received:
        apply(received.values)      # the ack is sent when this block completes

What that buys is precise, and it is worth stating precisely: **one human answer, delivered to
your agent exactly once, as typed data, authorizing exactly one effect.** It is not execution
resumption. Whether your program can pick up where it stopped is a property of your program;
this protocol makes sure the answer is there when it does (§1.3).

Standard library only, deliberately. The one exception is documented where it lives:
:func:`handoff.signing.verify_receipt_signature` needs ``cryptography`` for Ed25519, and the
hash chain that the protocol actually requires is stdlib and always available.
"""

from __future__ import annotations

import warnings
from typing import Any, Mapping, Optional, Sequence

from ._document import Document, canonical_bytes, digest, encode_document
from .client import Client, DEFAULT_BASE_URL, new_idempotency_key
from .errors import (
    AlreadyAnswered,
    AnswerValidationFailed,
    AuthStrengthNotPermitted,
    AuthenticationError,
    AuthorizationNotFound,
    AuthorizationSpent,
    BlastRadiusMismatch,
    CallbackSignatureError,
    CapabilityExpired,
    CapabilityNotFound,
    DeliveryUnavailable,
    EffectDigestMismatch,
    FieldError,
    GrantAlreadyHeld,
    HandoffError,
    HandoffProtocolError,
    HandoffTimeout,
    IdempotencyKeyReused,
    InsufficientAuthority,
    InsufficientScope,
    InvalidRequest,
    NotEntitled,
    NotFound,
    PresentationStale,
    RateLimited,
    RequestCancelled,
    RequestExpired,
    RequestInProgress,
    RequestNotFound,
    RequestSuperseded,
    RequesterMayNotAnswer,
    SignalNotApplied,
    SignalNotFound,
    TenantMismatch,
    TransportError,
    UnsupportedCapabilityType,
    UnsupportedFieldType,
    UnsupportedRequiresVersion,
)
from .models import (
    AckResult,
    AnswerResult,
    Authorization,
    Decision,
    Meta,
    ReattachResult,
    Receipt,
    RedeemResult,
    Request,
    Signal,
    TERMINAL_SIGNAL_TYPES,
    authority,
    capability,
    evidence,
    fields,
    prompt,
    requires,
    ttl_policy,
)
from .signing import (
    VerifiedCallback,
    chain_digest,
    receipt_core_hash,
    verify_callback,
    verify_chain,
    verify_receipt_chain,
)
from .waiter import Outcome, PendingRequest, Received, Waiter

__all__ = [
    "__version__",
    # configuration
    "configure",
    "client",
    "Client",
    "DEFAULT_BASE_URL",
    "new_idempotency_key",
    # raising and asking
    "raise_request",
    "ask",
    "approve",
    "meta",
    # waiting
    "waiter",
    "resume",
    "Waiter",
    "PendingRequest",
    "Received",
    "Outcome",
    # spending
    "redeem",
    # declarations
    "prompt",
    "requires",
    "authority",
    "capability",
    "fields",
    "evidence",
    "ttl_policy",
    # typed objects
    "Document",
    "Request",
    "Signal",
    "Decision",
    "Receipt",
    "Authorization",
    "AnswerResult",
    "AckResult",
    "RedeemResult",
    "ReattachResult",
    "Meta",
    "TERMINAL_SIGNAL_TYPES",
    # callbacks and receipts
    "verify_callback",
    "VerifiedCallback",
    "verify_receipt_chain",
    "verify_chain",
    "receipt_core_hash",
    "chain_digest",
    "canonical_bytes",
    "encode_document",
    "digest",
    # errors
    "HandoffError",
    "HandoffProtocolError",
    "HandoffTimeout",
    "SignalNotApplied",
    "CallbackSignatureError",
    "TransportError",
    "FieldError",
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
    # deprecated
    "clear_wall",
]

__version__ = "0.2.0"
PROTOCOL_VERSION = "0.1"

_default: Optional[Client] = None


def configure(base_url: Optional[str] = None, api_key: Optional[str] = None, **kwargs: Any) -> Client:
    """Point the module-level helpers at a server. Falls back to ``$HANDOFF_URL`` and
    ``$HANDOFF_API_KEY``."""
    global _default
    _default = Client(base_url, api_key, **kwargs)
    return _default


def client() -> Client:
    """The module-level client, created from the environment on first use."""
    global _default
    if _default is None:
        _default = Client()
    return _default


def raise_request(waiter_ref: str, prompt: Mapping[str, Any], requires: Mapping[str, Any], **kwargs: Any) -> PendingRequest:
    """Raise a request on the module-level client. See :meth:`Client.raise_request`."""
    return client().raise_request(waiter_ref, prompt, requires, **kwargs)


def ask(question: str, **kwargs: Any) -> Optional[str]:
    """Ask a person a question and block until they answer. See :func:`handoff.ergonomics.ask`."""
    from .ergonomics import ask as _ask

    return _ask(client(), question, **kwargs)


def approve(title: str, **kwargs: Any) -> Outcome:
    """Ask a person to approve or reject, and block. See :func:`handoff.ergonomics.approve`."""
    from .ergonomics import approve as _approve

    return _approve(client(), title, **kwargs)


def waiter(waiter_ref: str) -> Waiter:
    """A handle to an existing durable wait."""
    return client().waiter(waiter_ref)


def resume(waiter_ref: str) -> Waiter:
    """Reattach to a wait a previous process was holding (§8.5).

    The only thing that had to survive is the ``waiter_ref``.
    """
    return client().resume(waiter_ref)


def meta() -> Meta:
    """What the configured deployment supports (§19)."""
    return client().meta()


def redeem(authorization_id: str, effect_key: str, *, effect_digest: Optional[str] = None) -> RedeemResult:
    """Spend one decision on exactly one effect, idempotently per ``effect_key`` (§10)."""
    return client().redeem(authorization_id, effect_key, effect_digest=effect_digest)


def clear_wall(*args: Any, **kwargs: Any) -> Any:
    """Deprecated. Removed in 0.3.0.

    In 0.1.x this took a ``live_view_url`` and blocked until somebody said the wall was cleared.
    The protocol does not carry a resolvable address by value anywhere — not in a request, a
    receipt, a signal, or a delivery (§11.1, I8) — so the old signature cannot be honoured. A
    live surface is now declared as an opaque capability the person's own client resolves:

        handoff.raise_request(
            waiter_ref=...,
            prompt=handoff.prompt("Finish the sign-in the agent cannot pass"),
            requires=handoff.requires(
                [handoff.fields.attestation("cleared", "I cleared it")],
                capabilities=[handoff.capability("interactive_surface", scope="drive",
                                                 provider=..., resource_ref=...)],
                authority=handoff.authority("admin", "session"),
            ),
        )
    """
    raise NotImplementedError(
        "clear_wall() was removed when the SDK moved to the Handoff protocol: it took a "
        "resolvable live-view URL, and the protocol carries capabilities as opaque handles the "
        "answerer's own client resolves. Declare an 'interactive_surface' capability instead — "
        "see the docstring for the equivalent raise."
    )

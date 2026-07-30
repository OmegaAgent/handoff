"""Callback verification and receipt chain verification (protocol §15, signing.md).

Two independent schemes, answering different questions:

* **Callback signatures** — HMAC-SHA-256 over a canonical string, verified by the receiver with
  a shared secret. Implemented here in full, in the standard library.
* **Receipt integrity** — a per-tenant hash chain (the MUST) with an optional detached Ed25519
  signature (a MAY). The chain is implemented here in the standard library. The Ed25519 layer
  needs a curve implementation Python does not ship, so :func:`verify_receipt_signature`
  requires ``cryptography`` and says so plainly rather than degrading to "chain looked fine".

The claim a signature makes, and the one it does not
----------------------------------------------------
A valid signature proves the **sender**. It never proves the **tenant**. Resolve tenancy from
your own stored state — keyed on the endpoint that received the callback, or on the secret that
verified it — and never from a field in the body. Trusting the body would let anyone holding one
valid key target an arbitrary tenant (§15, I13). This module gives you no way to read tenancy
from a callback body, on purpose.
"""

from __future__ import annotations

import hashlib
import hmac
import time
from typing import Any, Iterable, Mapping, Optional, Sequence

from ._document import canonical_bytes
from .errors import CallbackSignatureError
from .models import Signal

__all__ = [
    "VerifiedCallback",
    "verify_callback",
    "callback_canonical_string",
    "sign_callback",
    "receipt_core_hash",
    "chain_digest",
    "verify_receipt_chain",
    "verify_chain",
    "verify_receipt_signature",
    "FRESHNESS_WINDOW_SECONDS",
    "SIGNATURE_VERSION",
]

FRESHNESS_WINDOW_SECONDS = 300
"""signing.md §1.3 step 2. Receiver-enforced, and not negotiable downward by the sender."""

SIGNATURE_VERSION = "1"

_ZERO_DIGEST = "sha256:" + "0" * 64
"""The predecessor of the first receipt in a tenant's chain (signing.md §2.2)."""


class VerifiedCallback:
    """A callback that passed every check in signing.md §1.3.

    ``delivery_id`` is the deduplication key: the same signal may legitimately arrive more than
    once, and applying it twice is the caller's bug, not the sender's. Returning ``2xx`` does
    **not** consume the signal — consumption is ``POST /v1/signals/{id}/ack`` (§8.3), which is
    what :meth:`handoff.Waiter.ack` does.
    """

    __slots__ = ("signal", "delivery_id", "signal_id", "sequence", "timestamp")

    def __init__(self, signal: Signal, delivery_id: str, signal_id: str, sequence: int, timestamp: int):
        self.signal = signal
        self.delivery_id = delivery_id
        self.signal_id = signal_id
        self.sequence = sequence
        self.timestamp = timestamp

    def __repr__(self) -> str:
        return (
            f"VerifiedCallback(delivery_id={self.delivery_id!r}, signal_id={self.signal_id!r}, "
            f"sequence={self.sequence!r}, type={self.signal.get('type')!r})"
        )


def callback_canonical_string(version: str, timestamp: int | str, delivery_id: str, raw_body: bytes) -> bytes:
    """``version LF timestamp LF delivery_id LF sha256_hex(body)`` — exactly three line feeds,
    no trailing newline (signing.md §1.2).

    The body's **hash** is signed rather than the body, so a receiver can verify before
    buffering, and so a body that begins with a digit cannot be confused with the timestamp
    that precedes it. ``delivery_id`` is inside the signed string so a valid signature cannot be
    lifted onto a different delivery of the same payload.
    """
    body_hash = hashlib.sha256(raw_body).hexdigest()
    return f"{version}\n{timestamp}\n{delivery_id}\n{body_hash}".encode("utf-8")


def sign_callback(secret: str, version: str, timestamp: int, delivery_id: str, raw_body: bytes) -> str:
    """Produce the lowercase-hex ``v1`` value for one secret. Present so tests can build valid
    and deliberately invalid vectors; a client verifies, it does not sign."""
    canonical = callback_canonical_string(version, timestamp, delivery_id, raw_body)
    return hmac.new(secret.encode("utf-8"), canonical, hashlib.sha256).hexdigest()


def _header(headers: Mapping[str, Any], name: str) -> Optional[str]:
    """Case-insensitive lookup. HTTP header names are case-insensitive and frameworks disagree
    about which case they hand you."""
    lowered = name.lower()
    for key, value in headers.items():
        if isinstance(key, str) and key.lower() == lowered:
            return value if isinstance(value, str) else str(value)
    return None


def _parse_signature_header(raw: str) -> tuple[int, list[str]]:
    timestamp: Optional[int] = None
    signatures: list[str] = []
    for part in raw.split(","):
        part = part.strip()
        if "=" not in part:
            raise CallbackSignatureError("malformed Handoff-Signature: element without '='")
        key, _, value = part.partition("=")
        if key == "t":
            if timestamp is not None:
                raise CallbackSignatureError("malformed Handoff-Signature: more than one 't' element")
            try:
                timestamp = int(value)
            except ValueError:
                raise CallbackSignatureError("malformed Handoff-Signature: 't' is not an integer") from None
        elif key == "v1":
            if not value:
                raise CallbackSignatureError("malformed Handoff-Signature: empty 'v1' element")
            signatures.append(value)
    if timestamp is None:
        raise CallbackSignatureError("malformed Handoff-Signature: no 't' element")
    if not signatures:
        raise CallbackSignatureError("malformed Handoff-Signature: no 'v1' element")
    return timestamp, signatures


def verify_callback(
    headers: Mapping[str, Any],
    body: bytes,
    secrets: str | Sequence[str],
    *,
    window: int = FRESHNESS_WINDOW_SECONDS,
    now: Optional[float] = None,
) -> VerifiedCallback:
    """Verify an inbound callback and return its typed signal, or raise.

    ``body`` MUST be the raw bytes as received, before any parsing or re-serialization: the
    signature covers the bytes on the wire, and re-encoding a parsed object produces a
    different hash for the same document (signing.md §3). Passing ``str`` is refused rather
    than silently encoded, because the encoding you would get is not necessarily the one that
    arrived.

    ``secrets`` is the **active** set. A secret that has been retired must not be in it — a
    receiver that keeps retired secrets active has made rotation meaningless. During a rotation
    overlap both secrets are active and either one verifying is a pass (signing.md §1.4).

    Every check of signing.md §1.3 runs in order and the first failure rejects. Rejection
    messages name the check and never include a secret or any value derived from one.
    """
    if isinstance(body, str):
        raise TypeError(
            "verify_callback() needs the raw request body as bytes: the signature covers the "
            "bytes as transmitted, and re-encoding a decoded body changes the hash"
        )
    if isinstance(secrets, str):
        secrets = [secrets]
    active = [s for s in secrets if s]
    if not active:
        raise CallbackSignatureError("no active callback secrets configured")

    raw_signature = _header(headers, "Handoff-Signature")
    if not raw_signature:
        raise CallbackSignatureError("missing Handoff-Signature header")
    timestamp, supplied = _parse_signature_header(raw_signature)

    # 2. Freshness, before any cryptography: a replayed callback is cheap to reject.
    current = time.time() if now is None else now
    if abs(current - timestamp) > window:
        raise CallbackSignatureError(
            f"timestamp outside the {window}s freshness window"
        )

    version = _header(headers, "Handoff-Version")
    if not version:
        raise CallbackSignatureError("missing Handoff-Version header")
    delivery_id = _header(headers, "Handoff-Delivery")
    if not delivery_id:
        raise CallbackSignatureError("missing Handoff-Delivery header")

    # 3 & 4. Hash the received bytes; rebuild the canonical string from headers only. Reading
    # any of these from the body would let the body attest to its own authenticity.
    canonical = callback_canonical_string(version, timestamp, delivery_id, body)

    matched = False
    for secret in active:
        expected = hmac.new(secret.encode("utf-8"), canonical, hashlib.sha256).hexdigest()
        for candidate in supplied:
            if hmac.compare_digest(expected, candidate):
                matched = True
                break
        if matched:
            break
    if not matched:
        raise CallbackSignatureError("signature did not match any active secret")

    # 6. The header is a convenience mirror; the body field is authoritative because the body
    # hash covers it. A disagreement means one of the two was tampered with.
    try:
        parsed = Signal.from_json(body)
    except ValueError as exc:
        raise CallbackSignatureError(f"body is not a JSON object: {exc}") from None
    sequence = parsed.get("sequence")
    header_sequence = _header(headers, "Handoff-Sequence")
    if header_sequence is not None:
        try:
            if int(header_sequence) != sequence:
                raise CallbackSignatureError("Handoff-Sequence disagrees with the body's sequence")
        except ValueError:
            raise CallbackSignatureError("malformed Handoff-Sequence header") from None

    return VerifiedCallback(
        signal=parsed,  # type: ignore[arg-type]
        delivery_id=delivery_id,
        signal_id=_header(headers, "Handoff-Signal") or parsed.get("id", ""),
        sequence=sequence,
        timestamp=timestamp,
    )


# -- receipts ---------------------------------------------------------------------------------


def receipt_core_hash(receipt: Mapping[str, Any]) -> str:
    """``sha256`` of the receipt with its ``chain`` member removed, canonicalized (signing.md §2.2)."""
    core = {k: v for k, v in receipt.items() if k != "chain"}
    return hashlib.sha256(canonical_bytes(core)).hexdigest()


def chain_digest(height: int, prev_digest: str, core_hash: str) -> str:
    """``height LF prev_digest LF core_hash``, hashed and prefixed.

    ``height`` is inside the input so an entry cannot be excised and the rest re-linked without
    detection.
    """
    chain_input = f"{height}\n{prev_digest}\n{core_hash}".encode("utf-8")
    return "sha256:" + hashlib.sha256(chain_input).hexdigest()


def verify_receipt_chain(receipt: Mapping[str, Any]) -> bool:
    """Recompute one receipt's ``chain.digest`` from its own content and position.

    This is the base tamper-evidence mechanism and it needs no key management at all, which is
    why the protocol makes it a MUST and detached signatures only a MAY (§9.4).
    """
    chain = receipt.get("chain")
    if not isinstance(chain, Mapping):
        return False
    try:
        expected = chain_digest(chain["height"], chain["prev_digest"], receipt_core_hash(receipt))
    except (KeyError, TypeError, ValueError):
        return False
    return hmac.compare_digest(expected, str(chain.get("digest", "")))


def verify_chain(receipts: Iterable[Mapping[str, Any]], *, genesis: str = _ZERO_DIGEST) -> bool:
    """Verify a whole tenant chain in order: each digest recomputes, and each links to the last.

    Altering any historical receipt changes its core hash, which changes its digest, which
    invalidates every digest after it and therefore the exported head (§9.4, C-15).
    """
    previous = genesis
    for receipt in receipts:
        chain = receipt.get("chain")
        if not isinstance(chain, Mapping):
            return False
        if str(chain.get("prev_digest")) != previous:
            return False
        if not verify_receipt_chain(receipt):
            return False
        previous = str(chain.get("digest"))
    return True


def verify_receipt_signature(receipt: Mapping[str, Any], signature: Mapping[str, Any], keys: Mapping[str, bytes]) -> bool:
    """Verify the OPTIONAL detached Ed25519 signature over a receipt's chain digest (§2.3).

    The chain is checked first: a broken chain is a rejection and the signature is not even
    consulted. An unknown ``kid`` is a rejection and no other published key is tried — falling
    back to "any key we have" would make key retirement meaningless (§2.4).

    Requires ``cryptography``. Everything else in this SDK is standard library only; Ed25519 is
    the one thing Python does not ship, and reporting that honestly beats pretending a receipt
    was verified when only its chain was.
    """
    try:
        from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey
    except ImportError:  # pragma: no cover - depends on the host environment
        raise RuntimeError(
            "verify_receipt_signature() needs the 'cryptography' package for Ed25519. The hash "
            "chain (verify_receipt_chain) is the protocol's required mechanism and is stdlib-only."
        ) from None

    import base64

    if not verify_receipt_chain(receipt):
        return False
    kid = signature.get("kid")
    if kid not in keys:
        return False
    chain = receipt["chain"]
    sig_input = f"handoff-receipt-v1\n{kid}\n{chain['digest']}".encode("utf-8")
    raw = str(signature.get("sig", ""))
    try:
        decoded = base64.urlsafe_b64decode(raw + "=" * (-len(raw) % 4))
        Ed25519PublicKey.from_public_bytes(keys[kid]).verify(decoded, sig_input)
        return True
    except Exception:
        return False

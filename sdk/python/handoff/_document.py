"""Ordered JSON documents and the two serializations the protocol defines.

The protocol uses two encodings and they are not interchangeable:

``encode_document``
    Two-space indent, member order preserved as received, one trailing newline. This is how
    the canonical fixtures in ``spec/fixtures`` are stored, and the form this SDK is asserted
    byte-identical against.

``canonical_bytes``
    RFC 8785 (JCS): members sorted by code point, no insignificant whitespace, no trailing
    newline. Every digest in the protocol (§1.4) is taken over this.

Member order is preserved rather than normalized for two reasons. Protocol §19 makes new
response fields additive, so a client that drops members it does not recognize corrupts
anything it forwards or re-hashes. And the canonical receipt fixtures carry the same members
in two different orders — ``08-receipt-decision.json`` in JCS order because it is the signed
core plus its chain entry, ``09-receipt-policy.json`` in declaration order — so no fixed field
order can reproduce both.

A ``Document`` is therefore a typed view over the parsed mapping, never a copy of it.
"""

from __future__ import annotations

import json
from typing import Any, Iterator, Mapping

__all__ = [
    "Document",
    "encode_document",
    "decode_document",
    "canonical_bytes",
    "digest",
    "ordered",
]

# Member names whose values are capability-adjacent and MUST NOT be rendered into a repr,
# a log line, or an exception message (§11.1, §12, I8, I18). Redaction applies to display
# only; `to_json()` always returns the real document.
_REDACTED_MEMBERS = frozenset(
    {
        "resume_token",
        "resume_payload",
        "url",
        "transport",
        "secret",
        "secrets",
        "password",
        "api_key",
        "authorization",
    }
)


def ordered(*pairs: tuple[str, Any], **rest: Any) -> dict[str, Any]:
    """Build a mapping in declaration order, dropping members whose value is ``None``.

    Omitting a member is not the same as sending ``null``: several protocol members are
    "nullable and required" while others are simply optional, and sending an explicit null
    for the second kind asserts something the caller did not mean.
    """
    out: dict[str, Any] = {}
    for name, value in pairs:
        if value is not None:
            out[name] = value
    for name, value in rest.items():
        if value is not None:
            out[name] = value
    return out


def encode_document(obj: Any) -> bytes:
    """Encode in the canonical fixture form: 2-space indent, source order, trailing newline."""
    return json.dumps(_unwrap(obj), indent=2, ensure_ascii=False).encode("utf-8") + b"\n"


def decode_document(raw: bytes | str) -> Any:
    """Parse JSON, preserving member order (Python mappings are insertion-ordered)."""
    return json.loads(raw)


def canonical_bytes(obj: Any) -> bytes:
    """Serialize to RFC 8785 (JCS) — the input to every digest in the protocol.

    Non-integer numbers are rejected rather than serialized. JCS specifies a number format
    that ``repr(float)`` does not reproduce, so a float here yields a digest that is stable
    in this implementation and wrong across two. The protocol's canonicalized objects carry
    no non-integer numbers; failing loudly keeps it that way.
    """
    value = _unwrap(obj)
    _reject_floats(value, "")
    # sort_keys sorts recursively by code point, which matches JCS for the ASCII member
    # names this protocol uses. signing.md's own reference verifier canonicalizes the same way.
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")


def digest(obj: Any) -> str:
    """The ``sha256:<hex>`` digest of an object's canonical form (§1.4)."""
    import hashlib

    return "sha256:" + hashlib.sha256(canonical_bytes(obj)).hexdigest()


def _reject_floats(value: Any, path: str) -> None:
    if isinstance(value, float):
        raise ValueError(
            f"cannot canonicalize a non-integer number at {path or '<root>'}: RFC 8785 "
            "specifies a number serialization that this would not reproduce"
        )
    if isinstance(value, dict):
        for key, item in value.items():
            _reject_floats(item, f"{path}.{key}" if path else key)
    elif isinstance(value, (list, tuple)):
        for index, item in enumerate(value):
            _reject_floats(item, f"{path}[{index}]")


def _unwrap(obj: Any) -> Any:
    if isinstance(obj, Document):
        return obj.to_json()
    if isinstance(obj, dict):
        return {key: _unwrap(value) for key, value in obj.items()}
    if isinstance(obj, (list, tuple)):
        return [_unwrap(item) for item in obj]
    return obj


def _redact(value: Any, member: str = "") -> Any:
    if member in _REDACTED_MEMBERS and value is not None:
        return "<redacted>"
    if isinstance(value, dict):
        return {key: _redact(item, key) for key, item in value.items()}
    if isinstance(value, list):
        return [_redact(item, member) for item in value]
    return value


class Document(Mapping[str, Any]):
    """A typed view over one protocol object, preserving its wire member order.

    Subclasses add named accessors. Members the subclass does not name are still readable
    by key and are re-serialized untouched, which is what §19's additive-compatibility rule
    requires of a client.
    """

    __slots__ = ("_d",)

    def __init__(self, data: Mapping[str, Any] | None = None, **members: Any):
        object.__setattr__(self, "_d", dict(data) if data is not None else dict(members))

    @classmethod
    def from_json(cls, raw: bytes | str | Mapping[str, Any]) -> "Document":
        data = raw if isinstance(raw, Mapping) else decode_document(raw)
        if not isinstance(data, Mapping):
            raise ValueError(f"expected a JSON object, got {type(data).__name__}")
        return cls(data)

    def to_json(self) -> dict[str, Any]:
        """The underlying mapping, in wire order. Mutating it mutates the document."""
        return self._d

    def encode(self) -> bytes:
        """Fixture form: 2-space indent, source order, trailing newline."""
        return encode_document(self._d)

    def canonical(self) -> bytes:
        """RFC 8785 canonical form — what digests are taken over."""
        return canonical_bytes(self._d)

    def digest(self) -> str:
        return digest(self._d)

    # -- Mapping ---------------------------------------------------------------
    def __getitem__(self, key: str) -> Any:
        return self._d[key]

    def __iter__(self) -> Iterator[str]:
        return iter(self._d)

    def __len__(self) -> int:
        return len(self._d)

    def get(self, key: str, default: Any = None) -> Any:  # type: ignore[override]
        return self._d.get(key, default)

    def __eq__(self, other: object) -> bool:
        if isinstance(other, Document):
            return self._d == other._d
        if isinstance(other, Mapping):
            return self._d == dict(other)
        return NotImplemented

    def __hash__(self) -> int:  # documents are mutable views; hashing them is a bug
        raise TypeError(f"{type(self).__name__} is not hashable")

    def __repr__(self) -> str:
        body = json.dumps(_redact(self._d), separators=(",", ":"), ensure_ascii=False)
        if len(body) > 400:
            body = body[:397] + "..."
        return f"{type(self).__name__}({body})"

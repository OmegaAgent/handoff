"""Ordered JSON documents and the two serializations the protocol defines.

The protocol uses two encodings and they are not interchangeable:

``encode_document``
    Two-space indent, member order preserved as received, one trailing newline. This is how
    the canonical fixtures in ``spec/fixtures`` are stored, and the form this SDK is asserted
    byte-identical against.

``canonical_bytes``
    RFC 8785 (JCS): members sorted by the UTF-16 code units of their names, no insignificant
    whitespace, no trailing newline. Every digest in the protocol (§1.4) is taken over this.

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

from .errors import NonConformingDocument

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

    Floats are rejected rather than serialized. JCS specifies a number format that
    ``repr(float)`` does not reproduce, so a float here yields a digest that is stable in this
    implementation and wrong across two. §1.4 requires every digest-covered number to be
    written as an integer literal, and in Python that rule needs no separate check: ``json``
    parses ``2`` to an ``int`` and ``2.0`` to a ``float``, so the type still carries the
    distinction the text made. Refusing the float is refusing the literal.
    """
    return json.dumps(
        _canonicalize(_unwrap(obj), ""), separators=(",", ":"), ensure_ascii=False
    ).encode("utf-8")


def digest(obj: Any) -> str:
    """The ``sha256:<hex>`` digest of an object's canonical form (§1.4)."""
    import hashlib

    return "sha256:" + hashlib.sha256(canonical_bytes(obj)).hexdigest()


def _utf16_units(name: str) -> bytes:
    """A member name as its UTF-16 code units, big-endian, for ordering only.

    Comparing these bytes is comparing the code-unit sequence: every unit is two bytes and the
    high byte leads, so a byte-lexicographic comparison is a unit-lexicographic one.
    """
    return name.encode("utf-16-be")


def _canonicalize(value: Any, path: str) -> Any:
    """Reject what JCS cannot carry, and impose JCS member order.

    Member order is **UTF-16 code units** (RFC 8785 §3.2.3, "Sorting of Object Properties"), not
    code points. The two agree for every name below U+D800 and diverge above it, because a
    non-BMP character encodes as a surrogate pair starting at 0xD800 and therefore sorts *below*
    every BMP character above U+D7FF while its code point sorts above them.

    This used to be ``json.dumps(sort_keys=True)``, which orders by code point — and that was
    not a lapse. It is what the published specification said to do: ``signing.md`` named RFC 8785
    and then, in the same sentence, required members "sorted by code point", and shipped a
    reference verifier doing exactly that. This SDK implemented the document faithfully and the
    document was wrong about the standard it named, so the RFC is cited here rather than the
    spec. Anyone who copies that reference verifier reproduces the old behaviour, which is why
    the specification is being corrected alongside this.

    It matters because a ``document`` field accepts any JSON value (§5.3), so caller-chosen
    object keys reach ``decision.values`` and from there the receipt core. One such key put this
    SDK's canonicalization at odds with the reference server's and the TypeScript SDK's over the
    same receipt, which is the one disagreement a chain anybody can verify cannot survive.
    """
    if isinstance(value, float):
        raise NonConformingDocument(
            f"{path or '<root>'} carries the float {value!r}, and digest-covered content carries "
            "integers only (§1.4). This document has no canonical form and therefore no digest, "
            "so it cannot have been produced by a conforming Server — §1.4 requires every "
            "digest-covered number to be stored and served in the form the canonicalizer emits. "
            "That is a defect in whatever minted this, and it is not evidence that anyone "
            "tampered with it."
        )
    if isinstance(value, dict):
        return {
            key: _canonicalize(value[key], f"{path}.{key}" if path else key)
            for key in sorted(value, key=_utf16_units)
        }
    if isinstance(value, (list, tuple)):
        return [_canonicalize(item, f"{path}[{index}]") for index, item in enumerate(value)]
    return value


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

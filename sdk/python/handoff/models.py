"""Typed views over the protocol's objects, and the builders that declare a request.

Nothing here switches on an interaction type. A request declares *what it needs* — the shape
of the answer, the capabilities the person must be handed, the authority required to give it —
and every one of the eight interaction patterns in §5.6 is a different declaration rather than
a different code path (I14). The builders below construct declarations; they are constructors,
not kinds, and the server never sees which one you called.
"""

from __future__ import annotations

from typing import Any, Mapping, Optional, Sequence

from ._document import Document, ordered

__all__ = [
    "Prompt",
    "Evidence",
    "Field",
    "Authority",
    "Requires",
    "Capability",
    "TtlPolicy",
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
    "fields",
    "evidence",
    "prompt",
    "requires",
    "authority",
    "capability",
]

TERMINAL_SIGNAL_TYPES = frozenset({"answered", "expired", "cancelled", "superseded"})
"""§8.3. ``attempt_lapsed`` is deliberately absent: it is a nudge, and the request stays pending."""


# -- declaration builders --------------------------------------------------------------------


class _Fields:
    """Constructors for the declared answer fields of §5.3.

    The field list is metadata: it declares the shape of an answer and never carries one.
    """

    @staticmethod
    def choice(
        name: str,
        label: str,
        options: Sequence[Mapping[str, str] | tuple[str, str] | str],
        *,
        required: bool = True,
        multi: Optional[bool] = None,
    ) -> dict[str, Any]:
        normalized: list[dict[str, str]] = []
        for option in options:
            if isinstance(option, Mapping):
                normalized.append({"id": option["id"], "label": option.get("label", option["id"])})
            elif isinstance(option, tuple):
                normalized.append({"id": option[0], "label": option[1]})
            else:
                normalized.append({"id": option, "label": option})
        return ordered(
            ("name", name),
            ("label", label),
            ("type", "choice"),
            ("required", required),
            ("options", normalized),
            ("multi", multi),
        )

    @staticmethod
    def text(
        name: str, label: str, *, required: bool = True, max_len: Optional[int] = None
    ) -> dict[str, Any]:
        return ordered(
            ("name", name), ("label", label), ("type", "text"), ("required", required), ("max_len", max_len)
        )

    @staticmethod
    def number(name: str, label: str, *, required: bool = True) -> dict[str, Any]:
        return ordered(("name", name), ("label", label), ("type", "number"), ("required", required))

    @staticmethod
    def boolean(name: str, label: str, *, required: bool = True) -> dict[str, Any]:
        return ordered(("name", name), ("label", label), ("type", "boolean"), ("required", required))

    @staticmethod
    def secret(name: str, label: str, *, required: bool = True, sink_ref: Optional[str] = None) -> dict[str, Any]:
        """A value the person types that the agent must never see (§12).

        Declaring one raises the effective authority floor server-side to the administrative
        role (§4.3), and the answer carries ``{"provided": true}`` and nothing else. The value
        itself travels to the sink, which the runtime owns; this protocol carries the
        declaration, never the credential.
        """
        return ordered(
            ("name", name),
            ("label", label),
            ("type", "secret"),
            ("required", required),
            ("sink_ref", sink_ref),
        )

    @staticmethod
    def attestation(name: str, label: str, *, required: bool = True) -> dict[str, Any]:
        return ordered(("name", name), ("label", label), ("type", "attestation"), ("required", required))

    @staticmethod
    def document(
        name: str,
        label: str,
        *,
        required: bool = True,
        schema_ref: Optional[str] = None,
        initial: Any = None,
    ) -> dict[str, Any]:
        return ordered(
            ("name", name),
            ("label", label),
            ("type", "document"),
            ("required", required),
            ("schema_ref", schema_ref),
            ("initial", initial),
        )

    @staticmethod
    def file_ref(name: str, label: str, *, required: bool = True) -> dict[str, Any]:
        return ordered(("name", name), ("label", label), ("type", "file_ref"), ("required", required))


fields = _Fields()


class _Evidence:
    """What the person is shown alongside the ask, so the receipt can bind to it."""

    @staticmethod
    def link(label: str, url: str) -> dict[str, Any]:
        return ordered(("kind", "link"), ("label", label), ("url", url))

    @staticmethod
    def table(label: str, columns: Sequence[str], rows: Sequence[Sequence[Any]]) -> dict[str, Any]:
        return ordered(
            ("kind", "table"),
            ("label", label),
            ("value", {"columns": list(columns), "rows": [list(r) for r in rows]}),
        )

    @staticmethod
    def text(label: str, value: str) -> dict[str, Any]:
        return ordered(("kind", "text"), ("label", label), ("value", value))


evidence = _Evidence()


def prompt(title: str, body: Optional[str] = None, evidence: Sequence[Mapping[str, Any]] = ()) -> dict[str, Any]:
    """What the person reads (§5.2)."""
    return ordered(("title", title), ("body", body), ("evidence", list(evidence) or None))


def authority(
    min_role: str = "editor",
    auth_strength: str = "session",
    *,
    assignees: Optional[Sequence[Mapping[str, str]]] = None,
    quorum: Optional[int] = None,
    reason: Optional[str] = None,
) -> dict[str, Any]:
    """Who is entitled to answer, evaluated at answer time against the answerer (§4.3).

    ``forbid_requester`` is always true and is emitted for clarity; a server rejects any other
    value. A machine cannot answer its own request under any configuration (§4.2, I15).
    """
    return ordered(
        ("min_role", min_role),
        ("auth_strength", auth_strength),
        ("assignees", list(assignees) if assignees else None),
        ("quorum", quorum),
        ("forbid_requester", True),
        ("reason", reason),
    )


def capability(
    type: str,
    *,
    scope: str = "view",
    handle: Optional[str] = None,
    provider: Optional[str] = None,
    resource_ref: Optional[str] = None,
    optional: Optional[bool] = None,
    ttl: Optional[str] = None,
    label: Optional[str] = None,
    purpose: Optional[str] = None,
    constraints: Optional[Mapping[str, Any]] = None,
) -> dict[str, Any]:
    """Declare something the person must be handed in order to be able to answer (§5.4, §11).

    A capability is carried as an opaque handle. Nothing resolvable — no URL, no token, no
    credential — travels in a declaration, a receipt, a signal, or a delivery (§11.1, I8). The
    person's own client exchanges the handle for a session; the agent runtime cannot.
    """
    return ordered(
        ("handle", handle),
        ("type", type),
        ("scope", scope),
        ("provider", provider),
        ("resource_ref", resource_ref),
        ("optional", optional),
        ("ttl", ttl),
        ("label", label),
        ("purpose", purpose),
        ("constraints", dict(constraints) if constraints else None),
    )


def requires(
    answer_fields: Sequence[Mapping[str, Any]] = (),
    *,
    capabilities: Sequence[Mapping[str, Any]] = (),
    authority: Optional[Mapping[str, Any]] = None,
    value_sink: Optional[Mapping[str, Any]] = None,
    v: int = 1,
) -> dict[str, Any]:
    """The versioned declaration envelope (§5.2).

    An empty field list is legitimate and means the whole request is an attestation: there is
    nothing to type and the person acts out of band.
    """
    answer = ordered(("fields", list(answer_fields)), ("value_sink", dict(value_sink) if value_sink else None))
    if "fields" not in answer:
        answer["fields"] = []
    return ordered(
        ("v", v),
        ("answer", answer),
        ("capabilities", list(capabilities)),
        ("authority", dict(authority) if authority is not None else None),
    )


def ttl_policy(
    on_expiry: str = "expire_and_deny",
    *,
    default_answer: Optional[Mapping[str, Any]] = None,
    reminder_every: Optional[str] = None,
) -> dict[str, Any]:
    """What happens when nobody answers (§6.4).

    ``default`` is the only policy that produces an outcome without a person, so the default
    answer must be declared here, at raise time, before anyone knew the person would go quiet.
    The resulting receipt records ``actor.type = "policy"`` and no audit can mistake it for
    consent.
    """
    if on_expiry == "default" and default_answer is None:
        raise ValueError(
            'ttl_policy(on_expiry="default") requires default_answer: the pre-agreed answer must '
            "be declared at raise time (§6.4)"
        )
    return ordered(
        ("on_expiry", on_expiry),
        ("default_answer", dict(default_answer) if default_answer else None),
        ("reminder_every", reminder_every),
    )


# -- typed views -----------------------------------------------------------------------------


class Prompt(Document):
    @property
    def title(self) -> str:
        return self["title"]

    @property
    def body(self) -> Optional[str]:
        return self.get("body")


class Evidence(Document):
    pass


class Field(Document):
    @property
    def name(self) -> str:
        return self["name"]

    @property
    def type(self) -> str:
        return self["type"]


class Authority(Document):
    pass


class Requires(Document):
    @property
    def v(self) -> int:
        return self["v"]

    @property
    def answer_fields(self) -> list[Field]:
        return [Field(f) for f in (self.get("answer") or {}).get("fields", [])]


class Capability(Document):
    @property
    def handle(self) -> Optional[str]:
        return self.get("handle")


class TtlPolicy(Document):
    pass


class Decision(Document):
    """The typed outcome a runtime consumes.

    It is data the runtime reads, never an instruction it must obey. ``values`` never carries a
    secret: a ``secret`` field is reduced to ``{"provided": true}`` before anything leaves the
    sink (§12, I7).
    """

    @property
    def outcome(self) -> str:
        return self["outcome"]

    @property
    def values(self) -> dict[str, Any]:
        return self.get("values") or {}

    @property
    def source(self) -> str:
        """``human``, ``policy``, or ``runtime_inference``. The protocol never fabricates a person."""
        return self["source"]

    @property
    def decided_by_human(self) -> bool:
        return self.get("source") == "human"

    @property
    def effective(self) -> Optional[str]:
        return self.get("effective")

    @property
    def receipt_id(self) -> Optional[str]:
        return self.get("receipt_id")

    @property
    def authorization_id(self) -> Optional[str]:
        return self.get("authorization_id")

    @property
    def superseded_by(self) -> Optional[str]:
        return self.get("superseded_by")


class Signal(Document):
    """One queued notification to a waiter.

    Signals are a queue, not a flag, so an ``attempt_lapsed`` nudge can never overwrite a
    subsequent terminal signal (§8.2 W2). Reading a signal does not consume it — consumption is
    the ack, and that two-step is what turns at-least-once delivery into effectively-once
    application (§8.3).
    """

    @property
    def id(self) -> str:
        return self["id"]

    @property
    def request_id(self) -> str:
        return self["request_id"]

    @property
    def waiter_ref(self) -> str:
        return self["waiter_ref"]

    @property
    def type(self) -> str:
        return self["type"]

    @property
    def sequence(self) -> int:
        return self["sequence"]

    @property
    def resume_token(self) -> str:
        """Required to ack this signal. Never logged, never rendered into a repr."""
        return self["resume_token"]

    @property
    def decision(self) -> Optional[Decision]:
        raw = self.get("decision")
        return Decision(raw) if isinstance(raw, Mapping) else None

    @property
    def is_terminal(self) -> bool:
        return self["type"] in TERMINAL_SIGNAL_TYPES

    @property
    def acked_at(self) -> Optional[str]:
        return self.get("acked_at")

    @property
    def resume_ref(self) -> Optional[str]:
        """Level 2. Whatever the runtime stored at raise time, returned byte-identical (§14)."""
        return self.get("resume_ref")

    @property
    def resume_payload(self) -> Optional[str]:
        """Level 2. Opaque bytes the runtime owns; the server stores them and never reads them."""
        return self.get("resume_payload")


class Request(Document):
    @property
    def id(self) -> str:
        return self["id"]

    @property
    def state(self) -> str:
        return self["state"]

    @property
    def waiter_ref(self) -> str:
        return self["waiter_ref"]

    @property
    def version(self) -> int:
        return self.get("version", 1)

    @property
    def is_pending(self) -> bool:
        return self.get("state") == "pending"

    @property
    def surface_url(self) -> Optional[str]:
        """Where a person goes to answer. A locator, not a capability: opening it prompts for
        authentication, and possessing it authorizes nothing (§4.6)."""
        return self.get("surface_url")

    @property
    def prompt(self) -> Prompt:
        return Prompt(self.get("prompt") or {})

    @property
    def requires(self) -> Requires:
        return Requires(self.get("requires") or {})

    @property
    def receipt(self) -> Optional["Receipt"]:
        raw = self.get("receipt")
        return Receipt(raw) if isinstance(raw, Mapping) else None


class Receipt(Document):
    """The immutable record of an outcome, minted in the same transaction as the state change."""

    @property
    def id(self) -> str:
        return self["id"]

    @property
    def kind(self) -> str:
        return self["kind"]

    @property
    def decided_at(self) -> str:
        return self["decided_at"]

    @property
    def actor_type(self) -> str:
        """``user``, ``policy``, ``runtime``, or ``anonymous_link``. A receipt that cannot say
        who decided is not a receipt (§4.4, §9.2)."""
        return (self.get("actor") or {}).get("type", "")

    @property
    def decided_by_human(self) -> bool:
        return self.actor_type == "user"

    @property
    def chain_digest(self) -> Optional[str]:
        return (self.get("chain") or {}).get("digest")

    def core(self) -> dict[str, Any]:
        """The receipt without its ``chain`` member — the byte sequence digests are taken over
        (signing.md §2.2)."""
        return {k: v for k, v in self.to_json().items() if k != "chain"}


class Authorization(Document):
    """What the runtime spends. One answer mints exactly one (§10, I10)."""

    @property
    def id(self) -> str:
        return self["id"]

    @property
    def single_use(self) -> bool:
        return self.get("single_use", True)

    @property
    def expires_at(self) -> Optional[str]:
        return self.get("expires_at")


class AnswerResult(Document):
    @property
    def receipt_id(self) -> str:
        return (self.get("receipt") or {})["id"]

    @property
    def authorization_id(self) -> Optional[str]:
        auth = self.get("authorization")
        return auth.get("id") if isinstance(auth, Mapping) else None


class AckResult(Document):
    @property
    def first_ack(self) -> bool:
        """False on a replay. Both calls return 200; redelivery stops once (§3.5, C-12)."""
        return self["first_ack"]

    @property
    def acked_at(self) -> str:
        return self["acked_at"]


class RedeemResult(Document):
    @property
    def first_redemption(self) -> bool:
        """The whole answer a caller needs: true means act, false means this effect already
        happened and must not happen again (§10, C-13)."""
        return self["first_redemption"]

    @property
    def redeemed_at(self) -> str:
        return self["redeemed_at"]


class ReattachResult(Document):
    """What a restarted process gets back: the waiter's state, its open requests, and every
    signal that is still unacked. Nothing was lost while the client was gone (§8.5)."""

    @property
    def waiter_ref(self) -> str:
        return self["waiter_ref"]

    @property
    def state(self) -> str:
        return self["state"]

    @property
    def open_requests(self) -> list[str]:
        return list(self.get("open_requests") or [])

    @property
    def signals(self) -> list[Signal]:
        return [Signal(s) for s in self.get("signals") or []]


class Meta(Document):
    """What a deployment supports. Read it to learn that a declaration will fail closed before
    making it, rather than after (§19)."""

    @property
    def protocol_version(self) -> str:
        return self["protocol_version"]

    @property
    def conformance_level(self) -> int:
        return self["conformance_level"]

    @property
    def max_wait_seconds(self) -> int:
        return self["max_wait_seconds"]

    @property
    def field_types(self) -> list[str]:
        return list(self.get("field_types") or [])

    @property
    def capability_types(self) -> list[str]:
        return list(self.get("capability_types") or [])

    @property
    def extensions(self) -> list[str]:
        return list(self.get("extensions") or [])

    def supports(self, extension: str) -> bool:
        return extension in self.extensions

//! Pure protocol types, the three Handoff state machines, and the records they mint.
//!
//! This crate is the executable half of [`spec/`]. It holds request identity, the `requires`
//! envelope, the REQUEST / DELIVERY / WAITER machines, the receipt with its canonical form and hash
//! chain, and the single-use authorization. It performs **no I/O**, spawns no tasks, and depends on
//! no async runtime, so an implementer can reason about protocol correctness without standing up a
//! database or a delivery channel.
//!
//! Anything that talks to the outside world belongs in `handoff-core` behind a port, not here.
//!
//! # No I/O, by construction
//!
//! There is no filesystem access, no network, and no clock read. The last one is enforced by the
//! build rather than by review: `chrono` is compiled without its `clock` feature, so `Utc::now()`
//! does not exist inside this crate. Time arrives through [`clock::Clock`]; entropy for identifiers
//! arrives as a parameter to [`id::Id::from_parts`]. Both are the caller's to supply.
//!
//! # Shape of the crate
//!
//! | Module | What it owns |
//! |---|---|
//! | [`id`] | `<prefix>_<26-char Crockford base32 ULID>`, typed per kind |
//! | [`clock`] | the injected `Clock` port, RFC 3339 timestamps, ISO 8601 durations |
//! | [`requires`] | the versioned `requires` envelope, field types, authority (§5) |
//! | [`request`] | the REQUEST machine, request identity, what a raise declares (§3, §6) |
//! | [`delivery`] | the DELIVERY machine and the four evidence grades (§7) |
//! | [`waiter`] | the WAITER machine, signals, acks, reattachment (§8) |
//! | [`receipt`] | the receipt, canonical JSON, digests, the per-tenant hash chain (§9) |
//! | [`authorization`] | single-use spend, `effect_key` idempotency, `effect_digest` binding (§10) |
//! | [`error`] | the error taxonomy of §13 |
//! | [`invariants`] | the numbered registry I1..I21 and its conformance mapping (§17, §18) |
//!
//! # Two rules that shaped every module
//!
//! **Every state machine is a total function.** `transition(state, event)` either returns a
//! transition or a typed error; nothing panics, no illegal move is silently accepted, and each
//! machine has an exhaustive test over its whole `(state, event)` domain. The domains are finite,
//! so the tests enumerate them rather than sampling.
//!
//! **Everything unknown fails closed** (I21). An unrecognized `requires.v`, field type, or
//! capability type is an error and nothing is created. A parse that cannot be fully interpreted
//! must never degrade, because a request the Server only partly understands is a request the person
//! will be shown incompletely — and the receipt would then record consent to something nobody saw.
//!
//! # What this crate deliberately does not do
//!
//! It does not implement execution resumption. The Level 2 `continuation` fields (§14) are carried
//! in [`request::Continuation`] and **never interpreted**: a `resume_ref` is never dereferenced and
//! a `resume_payload` is never parsed. Continuation belongs to the runtime (§1.3).
//!
//! # Ambiguities resolved
//!
//! Where the specification left a choice open, this crate made one and says so here.
//!
//! * **A-1 — identifier spelling.** Crockford base32 is case-insensitive and folds `I`/`L` to `1`
//!   and `O` to `0`. [`id::Id::parse`] accepts **canonical uppercase only**. An identifier with two
//!   spellings has two digests, and identifiers go inside receipts that get hashed.
//! * **A-2 — grade skipping.** §7.1's diagram draws an edge between `delivered` and `seen` that is
//!   already on the main path, leaving it unclear whether a delivery may skip a grade. This crate
//!   treats the four grades as an ordered ladder a delivery advances along, possibly skipping a
//!   rung it never got evidence for: a person opening an emailed link is `seen` whether or not the
//!   channel ever reported `delivered`. What is enforced is monotonicity plus the channel's
//!   declared `max_grade`, which is what §7.2 actually requires.
//! * **A-3 — requiredness on a partial answer.** §5.5 says a partial answer validates "the
//!   submitted values" against the current field set. [`requires::AnswerMode`] therefore enforces
//!   `required` only on a settling answer: a step that has not been asked for the later fields
//!   cannot be failed for omitting them, and the normative fixture
//!   `use-cases/07-reassign-escalate.json` delegates with `"values": {}`.
//! * **A-4 — chain height.** `ChainLink.height` is one-based here, so that `ChainHead.height` —
//!   documented as "the number of receipts in the chain" — equals the height of the last link.
//!   `openapi.yaml` gives `ChainLink.height` a minimum of 0, which reads as zero-based; the two
//!   cannot both hold, and agreeing with the exportable head is the more useful reading.
//! * **A-5 — cancelled and superseded waiters.** §8.2 lists R7 and R8 against both W2
//!   (→ `signalled`) and W8 (→ `released`). This crate takes W8, because W8 names those two
//!   transitions specifically and says no ack is required.
//! * **A-6 — the `armed ⇄ signalled` edge.** §8.1 draws the edge but §8.2 has no row for it. Here,
//!   acking the last queued signal returns the waiter to `armed` when the request is still
//!   `pending` — which is exactly the `attempt_lapsed` case — and to `acked` when it has settled.
//!
//! # Spec defects found
//!
//! Reported, not fixed: this crate does not own `spec/`.
//!
//! * **D-1 — withdrawn, no longer true.** Raised against an earlier draft of §18 in which I12 and
//!   I19 had no conformance case. The current §18 covers both — I12 → C-23, I19 → C-8 — and every
//!   other invariant, and all 24 case files exist. The entry is kept numbered but empty because
//!   identifiers are stable here for the same reason §18 keeps a withdrawn case's id: renumbering
//!   D-2..D-5 would silently invalidate every reference already made to them.
//! * **D-2 — `value_sink.ref` contradicts its own fixture.** `openapi.yaml` types it as `SinkRef`
//!   (`^snk_[0-9A-HJKMNP-TV-Z]{26}$`), while spec §5.6.1 and the normative fixture
//!   `use-cases/03-login-assistance.json` both carry `"ref": "opaque:bs_4KpQ"`, which that pattern
//!   rejects. §12 rule 4 says the sink is runtime-owned, so this crate takes the opaque reading and
//!   keeps the fixtures parseable.
//! * **D-3 — §6.2's table has no row for two pending → pending moves.** A progressive-disclosure
//!   step (§5.5) and a non-deciding disposition (§6.6) both leave the request `pending` and both
//!   have prose but no number. They appear here as [`request::TransitionRule::ProgressiveStep`] and
//!   [`request::TransitionRule::NonDecidingDisposition`] so nothing is silently unnumbered.
//! * **D-5 — the `Duration` pattern admits years and months.** `openapi.yaml` permits `P1Y` and
//!   `P1M`, whose length depends on when you start. [`clock::IsoDuration`] rejects both; a deadline
//!   that means something different in February is not a deadline.
//!
//! [`spec/`]: https://github.com/OmegaAgent/handoff/tree/main/spec

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

pub mod authorization;
pub mod clock;
pub mod delivery;
pub mod error;
pub mod id;
pub mod invariants;
pub mod receipt;
pub mod request;
pub mod requires;
pub mod waiter;

pub use authorization::{Authorization, AuthorizationBinding, RedeemRequest, RedeemResult};
pub use clock::{Clock, IsoDuration, Timestamp};
pub use delivery::{ChannelCapabilities, DeliveryGrade, DeliveryState};
pub use error::{ErrorCode, ProtocolError, Result};
pub use invariants::{Invariant, InvariantId, INVARIANTS};
pub use receipt::{canonical_json, digest_of, Digest, Receipt, ReceiptKind};
pub use request::{RaiseRequest, RequestEvent, RequestState, RequestTransition};
pub use requires::{DeploymentProfile, FieldType, Requires};
pub use waiter::{Decision, Signal, SignalType, WaiterState};

/// The protocol version this crate implements, as it appears at `GET /v1/meta`.
pub const PROTOCOL_VERSION: &str = "0.1";

/// The HTTP path prefix for the major version this crate implements.
pub const PATH_PREFIX: &str = "/v1";

/// Version of this crate, as published to crates.io.
///
/// Reported by the reference server's version endpoint so a deployment's running core version is
/// observable from outside. See `GOVERNANCE.md`, "What 'released' means".
pub const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_version_is_populated() {
        assert!(!CRATE_VERSION.is_empty());
    }

    #[test]
    fn the_crate_reports_the_protocol_version_it_implements() {
        assert_eq!(PROTOCOL_VERSION, "0.1");
        assert_eq!(PATH_PREFIX, "/v1");
        assert_eq!(requires::REQUIRES_VERSION, 1);
    }
}

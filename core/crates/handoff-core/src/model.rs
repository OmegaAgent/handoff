//! The domain values that cross the store port.
//!
//! Commands go in, views come out, and every one of them carries `tenant_ref` explicitly. That is
//! deliberate: I17 requires every lookup and every uniqueness constraint to be tenant-scoped, and
//! the cheapest way to make that hard to forget is to give the store no way to name an object
//! without also naming its tenant.

use handoff_protocol::authorization::{Authorization, Redemption};
use handoff_protocol::clock::{IsoDuration, Timestamp};
use handoff_protocol::delivery::{DeliveryGrade, DeliveryState};
use handoff_protocol::id::{
    AuthorizationId, DeliveryId, GrantHandle, GrantSessionRef, ReceiptId, RequestId, SignalId,
};
use handoff_protocol::receipt::PresentationBinding;
use handoff_protocol::receipt::{ChainHead, Digest, Receipt};
use handoff_protocol::request::{
    Callback, Continuation, Disposition, Liveness, Mode, OnWaiterTerminal, Prompt, RequestState,
    Routing, TtlPolicy, Urgency, UrgencyState,
};
use handoff_protocol::requires::{CapabilityScope, Requires, Target};
use handoff_protocol::waiter::{Signal, WaiterState};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::auth::Principal;
use crate::capability::BlastRadius;

/// The tenant an object belongs to.
///
/// An **opaque string the core never parses**. The engine has no concept of an organization, a
/// workspace, or a space; it compares this value and nothing else, which is what lets a
/// self-hosted deployment define tenancy however it already does.
pub type TenantRef = String;

/// A request as the engine and the API see it.
#[derive(Debug, Clone, PartialEq)]
pub struct RequestView {
    /// Server-minted identity (§3.1). Never accepted from a client.
    pub id: RequestId,
    /// The tenant. Every lookup that produced this view was scoped to it.
    pub tenant_ref: TenantRef,
    /// The caller's opaque grouping key. Stored and matched byte-for-byte, never parsed (§3.4).
    pub waiter_ref: String,
    /// Where the request is in §6.1.
    pub state: RequestState,
    /// Bumped by every amendment; a receipt records the version the person actually saw.
    pub version: u64,
    /// As declared at raise time.
    pub urgency: Urgency,
    /// A label and a sort key, never a filter (§6.3, I4).
    pub urgency_state: UrgencyState,
    /// What the person reads.
    pub prompt: Prompt,
    /// The three declarations (§5.2).
    pub requires: Requires,
    /// Advisory or gated (§10.1).
    pub mode: Mode,
    /// How strictly the answer must match what was shown (§9.3).
    pub presentation_binding: PresentationBinding,
    /// Where the wait lives.
    pub liveness: Liveness,
    /// What happens to this request when its waiter goes terminal.
    pub on_waiter_terminal: OnWaiterTerminal,
    /// What happens at `expires_at`.
    pub ttl_policy: Option<TtlPolicy>,
    /// The ladder as snapshotted at raise time, so a policy edit mid-flight cannot retroactively
    /// change what happened (§7.4).
    pub routing: Routing,
    /// The attempt window this request re-arms with.
    pub attempt_ttl: IsoDuration,
    /// When it was raised.
    pub created_at: Timestamp,
    /// `None` when no TTL was declared: the ask waits indefinitely (§6.3).
    pub expires_at: Option<Timestamp>,
    /// `None` until an attempt is armed.
    pub attempt_expires_at: Option<Timestamp>,
    /// Server clock at the moment the settling write committed.
    pub answered_at: Option<Timestamp>,
    /// The successor, when this request was superseded.
    pub superseded_by: Option<RequestId>,
    /// Shown to any person who was mid-answer when the request was withdrawn.
    pub cancel_reason: Option<String>,
    /// Every delivery minted so far, across every rung.
    pub deliveries: Vec<DeliveryView>,
    /// Present once the request settled.
    pub receipt: Option<Receipt>,
    /// Present once an answer minted one.
    pub authorization: Option<Authorization>,
    /// The waiting side, as seen from the request.
    pub waiter: WaiterView,
    /// Caller-owned annotations, returned verbatim and never interpreted (§19).
    pub metadata: Map<String, Value>,
    /// The highest ladder rung fired so far.
    pub rung: u32,
    /// Digest of the request as it stands, recorded on the receipt (§9.2).
    pub request_digest: Digest,
    /// Digest of what a person is shown at this version.
    ///
    /// Held per step rather than re-derived, because §9.2 forbids computing it later from the
    /// request's *current* content: a request may be amended in place, and losing the prior
    /// renderings is what must not happen.
    pub rendered_digest: Digest,
    /// Opaque pointer to the retained rendering. Never a public URL.
    pub rendered_ref: String,
    /// Where the callback for this request's signals goes, if one was registered (§15).
    pub callback: Option<Callback>,
    /// The Level 2 continuation fields, carried verbatim and interpreted by nothing (§14).
    pub continuation: Continuation,
    /// Whether the attempt lapse has already been stamped. It fires **once, ever** (§6.2 R3).
    pub attempt_lapse_notified: bool,
}

/// The waiting side, as seen from the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaiterView {
    /// Where the waiter is in §8.1. `orphaned` is visible on purpose (§8.4).
    pub state: WaiterState,
    /// How its death is detected.
    pub liveness: Liveness,
}

/// One tracked attempt to reach one target on one channel (§7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryView {
    /// This delivery.
    pub id: DeliveryId,
    /// The one request it belongs to. A delivery pointing at a second request id is escalation
    /// implemented as re-raising, which I3 forbids.
    pub request_id: RequestId,
    /// Open vocabulary. The core carries the name and looks up an adapter; it never switches on it.
    pub channel: String,
    /// Who it was addressed to.
    pub target: Target,
    /// Which ladder rung minted it. Rung 0 fires at raise time.
    pub rung: u32,
    /// Where it is in §7.1.
    pub state: DeliveryState,
    /// The strongest evidence this delivery achieved (§7.2).
    pub grade_reached: Option<DeliveryGrade>,
    /// The best grade this channel can ever prove, declared by its adapter.
    pub max_grade: DeliveryGrade,
    /// Whether this channel can establish *who* received it (§4.7, §7.2).
    pub can_authenticate_person: bool,
    /// Every transport-level send.
    pub attempts: Vec<DeliveryAttemptView>,
    /// When it was minted.
    pub created_at: Timestamp,
    /// When it last changed.
    pub updated_at: Timestamp,
}

/// One transport-level send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryAttemptView {
    /// Attempt number, from 1.
    pub n: u32,
    /// When it started.
    pub started_at: Timestamp,
    /// When it finished.
    pub ended_at: Option<Timestamp>,
    /// What happened at the transport level.
    pub outcome: String,
    /// The channel's own status string, carried verbatim for debugging.
    pub transport_status: Option<String>,
    /// Failure detail. **Never contains message content.**
    pub error: Option<String>,
}

/// Everything a raise carries into the store, after parsing and validation.
#[derive(Debug, Clone)]
pub struct RaiseCommand {
    /// Who is raising it.
    pub principal: Principal,
    /// The caller's `Idempotency-Key`, if any.
    pub idempotency_key: Option<String>,
    /// Digest of the raise body, so a reused key with a different body is a conflict (§3.3 rule 2).
    pub body_digest: Digest,
    /// The parsed raise.
    pub raise: handoff_protocol::request::RaiseRequest,
    /// The `dedupe_key` this raise collapses on, supplied or derived (§3.3).
    pub dedupe_key: String,
    /// The ladder resolved server-side and snapshotted onto the request (§7.4).
    pub routing: Routing,
    /// The grants to mint, one per declared capability, inside the raise transaction (§11.4).
    pub grants: Vec<GrantToMint>,
    /// The deliveries rung 0 mints.
    pub deliveries: Vec<DeliveryToMint>,
    /// When the ask stops being worth answering.
    pub expires_at: Option<Timestamp>,
    /// Server clock.
    pub now: Timestamp,
}

/// A capability grant, minted server-side inside the raise transaction (§11.4).
#[derive(Debug, Clone, PartialEq)]
pub struct GrantToMint {
    /// The opaque handle, from a CSPRNG and never derived from anything recomputable (§11.1).
    pub handle: GrantHandle,
    /// The capability kind.
    pub capability_type: String,
    /// The maximum scope this grant can produce.
    pub scope: CapabilityScope,
    /// Opaque provider name.
    pub provider: Option<String>,
    /// Opaque provider resource id.
    pub resource_ref: Option<String>,
    /// What this is, in the person's words.
    pub label: Option<String>,
    /// Why the person is being handed it.
    pub purpose: Option<String>,
    /// Whether the request is answerable without resolving it.
    pub optional: bool,
    /// The scope of consequence the person is accepting (§11.5).
    pub blast_radius: BlastRadius,
    /// Digest of that blast radius, which binds the resolve.
    pub blast_radius_digest: Digest,
    /// When the grant stops being resolvable.
    pub expires_at: Timestamp,
    /// How many distinct people may hold it at once.
    pub max_holders: i32,
}

/// A delivery a rung mints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryToMint {
    /// This delivery.
    pub id: DeliveryId,
    /// The channel name, carried verbatim.
    pub channel: String,
    /// Who it is addressed to.
    pub target: Target,
    /// Which rung minted it.
    pub rung: u32,
    /// The best grade this channel can prove.
    pub max_grade: DeliveryGrade,
    /// Whether it can establish who received it.
    pub can_authenticate_person: bool,
}

/// What a raise resolved to.
#[derive(Debug, Clone)]
pub struct RaiseResult {
    /// The request, in whatever state it is now.
    pub request: RequestView,
    /// `201` for a new request, `200` for a replay or a dedupe collapse (§3.3).
    pub status: u16,
}

/// Which requests a listing wants. Tenancy is not here: it is a separate argument the store cannot
/// omit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestFilter {
    /// Byte-for-byte match on the caller's grouping key.
    pub waiter_ref: Option<String>,
    /// Lifecycle states to include. Empty means every state.
    pub states: Vec<RequestState>,
    /// Page size.
    pub limit: i64,
}

/// A person's settling write.
#[derive(Debug, Clone)]
pub struct AnswerCommand {
    /// The request being answered.
    pub request_id: RequestId,
    /// Who is answering.
    pub principal: Principal,
    /// The caller's `Idempotency-Key`, if any. A retried click is not a conflict (§6.7 rule 3).
    pub idempotency_key: Option<String>,
    /// Digest of the answer body.
    pub body_digest: Digest,
    /// The submitted values, keyed by declared field name.
    pub values: Map<String, Value>,
    /// The person's own words, recorded verbatim in the receipt.
    pub note: Option<String>,
    /// Decide, delegate, or report being unable (§6.6).
    pub disposition: Disposition,
    /// Where a delegation sends it.
    pub delegate_to: Option<Target>,
    /// Which delivery the person answered through, which is what grades it to `acted`.
    pub via_delivery_id: Option<DeliveryId>,
    /// An intermediate step of a progressive-disclosure ladder (§5.5).
    pub partial: bool,
    /// Which capability sessions the person held while answering.
    pub capability_uses: Vec<CapabilityUse>,
    /// Digest of exactly what this person was shown (§9.3).
    pub rendered_digest: Option<Digest>,
    /// Server clock.
    pub now: Timestamp,
}

/// One capability session the answerer held.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityUse {
    /// The grant handle.
    pub handle: GrantHandle,
    /// The session produced by resolving it.
    pub session_ref: GrantSessionRef,
}

/// What a settling write produced.
#[derive(Debug, Clone)]
pub struct AnswerResult {
    /// The settled request.
    pub request: RequestView,
    /// The receipt, minted in the same transaction as the state change (§9.1).
    ///
    /// `None` when nothing was decided. A `partial` step and a `delegate` or `unable` disposition
    /// leave the request `pending`, and §2 is explicit that nothing but an **outcome** mints a
    /// receipt — the delegation is recorded on the eventual one.
    pub receipt: Option<Receipt>,
    /// `None` for a partial answer and for any disposition other than `decide`.
    pub authorization: Option<Authorization>,
}

/// A simple mutation that needs no body beyond a reason.
#[derive(Debug, Clone)]
pub struct RequestCommand {
    /// The request.
    pub request_id: RequestId,
    /// Who is calling.
    pub principal: Principal,
    /// The caller's `Idempotency-Key`, if any.
    pub idempotency_key: Option<String>,
    /// Digest of the body.
    pub body_digest: Digest,
    /// Server clock.
    pub now: Timestamp,
}

/// What an amendment merges forward.
#[derive(Debug, Clone)]
pub struct AmendPatch {
    /// The new prompt, when one was supplied.
    pub prompt: Option<Prompt>,
    /// The new declarations, when supplied.
    pub requires: Option<Requires>,
}

/// One entry in the callback attempt log (§15.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallbackAttemptView {
    /// Attempt number, from 1.
    pub n: u32,
    /// When it started.
    pub started_at: Timestamp,
    /// When it finished.
    pub ended_at: Option<Timestamp>,
    /// The status the receiver returned. A `2xx` marks it dispatched, never consumed (§15.4).
    pub status_code: Option<i32>,
    /// How long it took.
    pub duration_ms: Option<i64>,
    /// What happened.
    pub outcome: String,
    /// Failure detail.
    pub error: Option<String>,
}

/// What a reattach returns (§8.5).
#[derive(Debug, Clone)]
pub struct ReattachView {
    /// The caller's grouping key, echoed.
    pub waiter_ref: String,
    /// Where the waiter is now.
    pub state: WaiterState,
    /// Requests under this waiter that are still `pending`.
    pub open_requests: Vec<RequestId>,
    /// Every unacked signal. Nothing was lost while the client was gone.
    pub signals: Vec<Signal>,
}

/// The result of acking a signal (§3.5, §8.2 W4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AckResult {
    /// When it was consumed.
    pub acked_at: Timestamp,
    /// `true` on the write, `false` on the replay. A second ack is a retry, not a second
    /// application.
    pub first_ack: bool,
}

/// A capability grant as a surface reads it before the person accepts (§11.5).
#[derive(Debug, Clone, PartialEq)]
pub struct GrantView {
    /// The opaque handle.
    pub handle: GrantHandle,
    /// The request that declared it.
    pub request_id: RequestId,
    /// The capability kind.
    pub capability_type: String,
    /// The maximum scope this grant can produce.
    pub scope: CapabilityScope,
    /// Opaque provider name.
    pub provider: Option<String>,
    /// Opaque provider resource id.
    pub resource_ref: Option<String>,
    /// What this is, in the person's words.
    pub label: Option<String>,
    /// Why the person is being offered it.
    pub purpose: Option<String>,
    /// Whether the request is answerable without resolving it.
    pub optional: bool,
    /// The full scope of consequence, rendered before the accept control (§11.5 rule 1).
    pub blast_radius: BlastRadius,
    /// The digest the resolve call must echo back (§11.5 rule 2).
    pub blast_radius_digest: Digest,
    /// After this instant the grant resolves nothing.
    pub expires_at: Timestamp,
    /// Non-null means no further sessions, ever.
    pub revoked_at: Option<Timestamp>,
    /// How many distinct people may hold it at once.
    pub max_holders: i32,
    /// The principal this grant pinned to on first successful resolve.
    pub bound_principal: Option<String>,
}

/// A live, leased capability session (§11.2).
///
/// The `transport.url` this produces is minted at resolve time, bound to this one session, and
/// **never persisted** — it exists in the response body and nowhere else in the system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantSessionView {
    /// This session.
    pub session_ref: GrantSessionRef,
    /// What it may actually do.
    pub scopes: Vec<CapabilityScope>,
    /// The session closes at this instant unless renewed.
    pub lease_until: Timestamp,
    /// Renew after this many milliseconds, not at the last moment.
    pub renew_after_ms: i64,
}

/// What a sink accepted. **Names only** — a response that echoed a value would defeat the point
/// (§12 rule 6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SinkAcceptance {
    /// Which declared field names the sink took.
    pub accepted: Vec<String>,
    /// The sink's own opaque progress label.
    pub state: Option<String>,
}

/// What one sweep pass did, for the log and for tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Attempt clocks that lapsed, each stamped once, ever (§6.2 R3).
    pub attempts_lapsed: u64,
    /// Requests that reached a terminal state under their TTL policy (§6.2 R6).
    pub requests_expired: u64,
    /// Ladder rungs that fired on time.
    pub rungs_fired: u64,
}

/// The chain head plus the receipts beneath it, for the open verifier.
#[derive(Debug, Clone, PartialEq)]
pub struct ChainExport {
    /// The head as of now.
    pub head: Option<ChainHead>,
    /// Every receipt in the tenant's chain, oldest first.
    pub receipts: Vec<Receipt>,
}

/// A redemption outcome (§10.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedeemOutcome {
    /// When it was spent.
    pub redeemed_at: Timestamp,
    /// `true` means act; `false` means this effect already happened and must not happen again.
    pub first_redemption: bool,
}

/// A signal identified for an ack.
#[derive(Debug, Clone)]
pub struct AckCommand {
    /// The signal.
    pub signal_id: SignalId,
    /// Proof that the acking client is the waiter this was enqueued for.
    pub resume_token: String,
    /// Whether the runtime actually applied the decision. `false` is not an error (§8.3).
    pub applied: bool,
    /// What stopped the runtime from applying it.
    pub reason: Option<String>,
    /// Server clock.
    pub now: Timestamp,
}

/// Everything the store needs to record a redemption.
#[derive(Debug, Clone)]
pub struct RedeemCommand {
    /// The authorization to spend.
    pub authorization_id: AuthorizationId,
    /// The caller's own stable identifier for the effect.
    pub effect_key: String,
    /// Digest of the effect's parameters, compared against the binding when one is set.
    pub effect_digest: Option<Digest>,
    /// Server clock.
    pub now: Timestamp,
}

/// A redemption row, as stored.
pub type StoredRedemption = Redemption;

/// A receipt paired with the id of the request it settles, for listing.
#[derive(Debug, Clone, PartialEq)]
pub struct ReceiptRow {
    /// The receipt.
    pub receipt: Receipt,
    /// Which request it settles.
    pub request_id: RequestId,
    /// Which receipt it is.
    pub id: ReceiptId,
}

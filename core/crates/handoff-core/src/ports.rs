//! The ports a deployment implements.
//!
//! Everything external sits behind one of these. The store is the largest, and its shape is a
//! deliberate consequence of I12: **every state transition emits its event in the same transaction
//! as the state change.** A fine-grained store — `update_state`, then `insert_event` — would put
//! the two writes in the caller's hands and make the invariant a convention. So every method here
//! that changes state is *one transaction*, named for the transition it performs, and there is no
//! method that writes a state without its event.
//!
//! The methods are synchronous in their signatures and `async` through [`BoxFuture`] rather than a
//! macro, so this crate needs no async runtime and no procedural-macro dependency to state what a
//! store must do.

use handoff_protocol::error::Result;
use handoff_protocol::id::{
    AuthorizationId, DeliveryId, GrantHandle, GrantSessionRef, ReceiptId, RequestId, SignalId,
};
use handoff_protocol::receipt::Receipt;
use handoff_protocol::request::Prompt;
use handoff_protocol::requires::{CapabilityScope, Requires, Target};
use handoff_protocol::waiter::Signal;
use serde_json::{Map, Value};
use std::future::Future;
use std::pin::Pin;

use crate::model::*;

/// A boxed future, so the trait is object-safe without an async-trait macro.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Where time comes from.
///
/// Injected rather than read, because a Server MUST use its own clock for every recorded time and
/// MUST NOT accept a client-supplied one (§1.4) — and because a TTL sweep is untestable against a
/// wall clock nobody controls.
pub trait Clock: Send + Sync {
    /// The current instant.
    fn now(&self) -> handoff_protocol::clock::Timestamp;
}

/// The durable store.
///
/// Every method is one transaction. Every method that reads takes the tenant explicitly, because
/// I17 requires lookups to be tenant-scoped and a signature that lets you forget is a signature
/// that eventually gets forgotten.
pub trait Store: Send + Sync {
    /// R1. Insert the request, register its waiter, mint declared grants, enqueue rung-0
    /// deliveries, and emit `request.raised` — or return the existing request when a key or a
    /// `dedupe_key` collapses it (§3.3).
    fn raise(&self, command: RaiseCommand) -> BoxFuture<'_, Result<RaiseResult>>;

    /// Read one request, within the caller's tenant and nowhere else.
    fn get_request(
        &self,
        tenant: String,
        id: RequestId,
    ) -> BoxFuture<'_, Result<Option<RequestView>>>;

    /// List requests, scoped to the caller's tenant.
    fn list_requests(
        &self,
        tenant: String,
        filter: RequestFilter,
    ) -> BoxFuture<'_, Result<Vec<RequestView>>>;

    /// R2. Merge `prompt` and `requires` forward, increment `version`, emit `request.amended`.
    fn amend(
        &self,
        command: RequestCommand,
        patch: AmendPatch,
    ) -> BoxFuture<'_, Result<RequestView>>;

    /// R7. Withdraw the ask, signal the waiter, emit `request.cancelled`.
    fn cancel(&self, command: RequestCommand, reason: String)
        -> BoxFuture<'_, Result<RequestView>>;

    /// R8. Link the successor both ways, signal the waiter, emit `request.superseded`.
    fn supersede(
        &self,
        command: RequestCommand,
        by: RequestId,
    ) -> BoxFuture<'_, Result<RequestView>>;

    /// R4. Fire a ladder rung, minting **deliveries and never a request** (I3).
    fn escalate(
        &self,
        command: RequestCommand,
        rung: Option<u32>,
    ) -> BoxFuture<'_, Result<RequestView>>;

    /// Retarget the request. An operation, not a state change and not a receipt (§6.6).
    fn reassign(
        &self,
        command: RequestCommand,
        to: Target,
        reason: Option<String>,
    ) -> BoxFuture<'_, Result<RequestView>>;

    /// Arm or re-arm the attempt clock **fresh**, never inheriting a near-expired countdown
    /// (§6.3).
    fn arm_attempt(
        &self,
        command: RequestCommand,
        ttl: Option<handoff_protocol::clock::IsoDuration>,
    ) -> BoxFuture<'_, Result<RequestView>>;

    /// R5. The conditional write on `state = 'pending'`, the receipt, the authorization, the
    /// signal, and the event — one transaction, or none of it (§6.2, §9.1, I12).
    fn answer(&self, command: AnswerCommand) -> BoxFuture<'_, Result<AnswerResult>>;

    /// The receipt for a settled request. `404` while it is still `pending` (§9).
    fn request_receipt(
        &self,
        tenant: String,
        id: RequestId,
    ) -> BoxFuture<'_, Result<Option<Receipt>>>;

    /// One receipt by id.
    fn receipt(&self, tenant: String, id: ReceiptId) -> BoxFuture<'_, Result<Option<Receipt>>>;

    /// The tenant's chain, head first, for the open verifier (§9.4).
    fn chain(&self, tenant: String) -> BoxFuture<'_, Result<ChainExport>>;

    /// Every delivery for one request, in creation order.
    fn deliveries(&self, tenant: String, id: RequestId)
        -> BoxFuture<'_, Result<Vec<DeliveryView>>>;

    /// Unacked signals for one waiter. **Reading does not consume** (§8.3).
    fn signals(&self, tenant: String, waiter_ref: String) -> BoxFuture<'_, Result<Vec<Signal>>>;

    /// W7. Return every unacked signal and re-arm the lease (§8.5).
    fn reattach(&self, tenant: String, waiter_ref: String) -> BoxFuture<'_, Result<ReattachView>>;

    /// W4. Consume a signal, idempotently (§3.5).
    fn ack(&self, tenant: String, command: AckCommand) -> BoxFuture<'_, Result<Option<AckResult>>>;

    /// The callback attempt log for one signal (§15.5).
    fn signal_attempts(
        &self,
        tenant: String,
        id: SignalId,
    ) -> BoxFuture<'_, Result<Option<Vec<CallbackAttemptView>>>>;

    /// One authorization.
    fn authorization(
        &self,
        tenant: String,
        id: AuthorizationId,
    ) -> BoxFuture<'_, Result<Option<handoff_protocol::authorization::Authorization>>>;

    /// Spend an authorization against one effect, idempotently per `effect_key` (§10.2).
    fn redeem(
        &self,
        tenant: String,
        command: RedeemCommand,
    ) -> BoxFuture<'_, Result<Option<RedeemOutcome>>>;

    /// Read a grant declaration, blast radius included, before the person accepts (§11.5).
    fn grant(
        &self,
        tenant: String,
        handle: GrantHandle,
    ) -> BoxFuture<'_, Result<Option<GrantView>>>;

    /// Every grant on a request.
    fn grants_for_request(
        &self,
        tenant: String,
        id: RequestId,
    ) -> BoxFuture<'_, Result<Vec<GrantView>>>;

    /// Revoke a grant. A single write on a single grant, affecting no other (§11.4).
    fn revoke_grant(
        &self,
        tenant: String,
        handle: GrantHandle,
        reason: Option<String>,
        now: handoff_protocol::clock::Timestamp,
    ) -> BoxFuture<'_, Result<bool>>;

    /// Open a session on a grant, having checked §11.2's six conditions in order.
    ///
    /// The transport address is **not** an argument and **not** stored: the caller mints it after
    /// this returns, and it lives only in the response body (§11.2).
    fn open_grant_session(
        &self,
        tenant: String,
        resolve: ResolveGrant,
    ) -> BoxFuture<'_, Result<GrantSessionView>>;

    /// Accept `secret` values on their way to a runtime-owned sink.
    ///
    /// Keys are checked against the declared field names of the request that owns the sink, so a
    /// compromised surface cannot smuggle arbitrary keys through (§12 rule 1). The values are not
    /// returned, not logged, and not stored anywhere the protocol can read them.
    fn submit_sink_values(
        &self,
        tenant: String,
        sink_ref: String,
        values: Map<String, Value>,
    ) -> BoxFuture<'_, Result<SinkAcceptance>>;

    /// Run one sweep: attempt lapses (R3), ladder rungs (R4), and TTL expiries (R6).
    ///
    /// A sweep is a transition like any other, so each one commits its state and its event
    /// together. "Update the row, then publish" is exactly the shape I12 forbids, and a background
    /// job is where it is most tempting.
    fn sweep(&self, now: handoff_protocol::clock::Timestamp) -> BoxFuture<'_, Result<SweepReport>>;

    /// Record that a message arrived on a channel.
    ///
    /// It is stored as a **provisional** answer and it settles nothing: a Server MUST NOT derive a
    /// decision from message content, however authenticated the channel (§4.7, C-21).
    fn record_channel_message(
        &self,
        tenant: String,
        id: RequestId,
        channel: String,
        text: String,
        now: handoff_protocol::clock::Timestamp,
    ) -> BoxFuture<'_, Result<bool>>;

    /// Record that a runtime observed the target change state.
    ///
    /// An observation, never a person. Clearance MUST be asserted, never inferred (§9.7, C-22).
    fn record_observation(
        &self,
        tenant: String,
        id: RequestId,
        note: String,
        now: handoff_protocol::clock::Timestamp,
    ) -> BoxFuture<'_, Result<bool>>;

    /// Claim one signal that is due for a callback push, leasing it so two workers cannot both
    /// send it.
    fn claim_callback(
        &self,
        now: handoff_protocol::clock::Timestamp,
    ) -> BoxFuture<'_, Result<Option<CallbackJob>>>;

    /// Record the outcome of one callback attempt.
    ///
    /// A `2xx` marks it dispatched and **does not** consume the signal: consumption is the ack
    /// (§15.4).
    fn record_callback_attempt(
        &self,
        job: CallbackJob,
        attempt: CallbackAttemptView,
        next_attempt_at: Option<handoff_protocol::clock::Timestamp>,
    ) -> BoxFuture<'_, Result<()>>;

    /// Resolve a credential to the principal it authenticates.
    ///
    /// Tenancy comes from here — stored state bound to the credential — and never from a request
    /// body (§4.1, I13).
    fn authenticate(
        &self,
        presented_secret: String,
    ) -> BoxFuture<'_, Result<Option<crate::auth::Principal>>>;

    /// Replay a stored idempotent response, if this key and body were seen before (§3.5).
    fn idempotent_replay(
        &self,
        slot: IdempotencySlot,
    ) -> BoxFuture<'_, Result<Option<StoredResponse>>>;

    /// Store a response against an idempotency key.
    fn remember_idempotent(
        &self,
        slot: IdempotencySlot,
        response: StoredResponse,
        now: handoff_protocol::clock::Timestamp,
    ) -> BoxFuture<'_, Result<()>>;
}

/// Which idempotency slot a call occupies.
///
/// The scope is `(tenant, principal, operation, key)` and every one of those is required, because
/// §3.2 rule 2 makes the scoping a correctness property rather than a detail: an unscoped
/// uniqueness constraint does not merely risk a collision, it lets one tenant's key silently absorb
/// another tenant's write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdempotencySlot {
    /// The tenant, resolved from the credential.
    pub tenant: String,
    /// The principal, because §3.1 scopes the key to `(org_id, principal_id)`.
    pub principal: String,
    /// Which operation the key was presented against.
    pub operation: String,
    /// The caller's key.
    pub key: String,
    /// Digest of the body, so the same key with a different body is a conflict (§3.3 rule 2).
    pub body_digest: handoff_protocol::receipt::Digest,
}

/// Everything §11.2's six checks need in order to open a session on a grant.
#[derive(Debug, Clone)]
pub struct ResolveGrant {
    /// The grant being resolved.
    pub handle: GrantHandle,
    /// The person's own authenticated principal. Never the handle itself (§11.2).
    pub principal: crate::auth::Principal,
    /// A subset of the grant's scope. Asking for more is refused.
    pub scopes: Vec<CapabilityScope>,
    /// The digest of the blast radius this person was actually shown (§11.5 rule 2).
    pub accepted_blast_radius_digest: handoff_protocol::receipt::Digest,
    /// The session identity to mint.
    pub session_ref: GrantSessionRef,
    /// Server clock.
    pub now: handoff_protocol::clock::Timestamp,
}

/// A response stored against an idempotency key, so a retry returns exactly what the first call
/// did (§3.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredResponse {
    /// The status the first call returned.
    pub status: u16,
    /// The body the first call returned.
    pub body: String,
}

/// One outbound callback push, claimed under a lease.
#[derive(Debug, Clone, PartialEq)]
pub struct CallbackJob {
    /// The signal being pushed.
    pub signal_id: SignalId,
    /// The tenant it belongs to, so the worker never has to read it from a body.
    pub tenant_ref: String,
    /// Where to POST.
    pub url: String,
    /// The delivery identity for this attempt. New per attempt, so a signature cannot be lifted
    /// from one delivery onto another (`signing.md` §1.2).
    pub delivery_id: DeliveryId,
    /// The signal, serialized exactly as it will be sent.
    pub body: Value,
    /// The sequence, mirrored into a header.
    pub sequence: u64,
    /// Which attempt this is, from 1.
    pub attempt: u32,
}

/// Everything an amendment may change.
#[derive(Debug, Clone, PartialEq)]
pub struct AmendInput {
    /// The new prompt.
    pub prompt: Option<Prompt>,
    /// The new declarations.
    pub requires: Option<Requires>,
}

//! The deployment seam: the ports a deployment supplies **around** the store.
//!
//! [`ports`](crate::ports) holds the two things every deployment must have — a clock and a store.
//! This module holds the ones a deployment may or may not have, because they describe systems that
//! exist outside it: an identity provider, a directory of people, a meter, an audit log, a delivery
//! fleet, an attestation key, a takeover surface.
//!
//! They live in their own module rather than beside [`Store`](crate::ports::Store) for a reason
//! that is about editing rather than about design: the store port is one large trait that changes
//! whenever a transition changes, and the seam is a set of small traits that change whenever the
//! outside world does. Keeping them apart means a change to one does not land in the other's file.
//!
//! # The rule these traits exist to enforce
//!
//! Every port here has a self-hosted answer that is **correct**, and a hosted answer that is
//! **better operated**. Never one that is more correct. If a hosted deployment needs behaviour the
//! open core cannot express, the fix is a port here — visible, documented, implementable by anyone
//! — and never a private variant of the server.
//!
//! # Absent is a real answer
//!
//! Every one of these is optional, and a deployment that supplies nothing is not a degraded
//! deployment. A single operator has no external meter, no audit mirror, and no takeover surface,
//! and the honest representation of that is `None` rather than a sink that accepts writes and
//! discards them. There is deliberately no `DiscardingMeter` in this module: a port that silently
//! drops what it is handed is worse than an absent one, because it looks like it is working.
//!
//! What must **never** appear here is a port whose default is to refuse. That would be a dormant
//! gate, and `GOVERNANCE.md` treats a gate in this crate as a boundary violation. A hosted adapter
//! may fail closed when its own dependency is missing — that is the adapter's honesty about its own
//! deployment — but the trait must not require it.

use handoff_protocol::clock::{IsoDuration, Timestamp};
use handoff_protocol::delivery::{ChannelCapabilities, DeliveryGrade, DeliveryState};
use handoff_protocol::error::{ErrorCode, ProtocolError, Result};
use handoff_protocol::id::{DeliveryId, GrantHandle, PrincipalId, ReceiptId, RequestId};
use handoff_protocol::receipt::Digest;
use handoff_protocol::request::Prompt;
use handoff_protocol::requires::Target;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::auth::Principal;
use crate::ports::BoxFuture;

// ---------------------------------------------------------------------------------------------
// Authentication and tenancy
// ---------------------------------------------------------------------------------------------

/// Resolve an inbound credential to the principal it authenticates.
///
/// This is the same contract as [`Store::authenticate`](crate::ports::Store::authenticate), stated
/// as its own trait so that a deployment can answer it from somewhere other than its own database.
/// A self-hosted operator keeps credentials in the store and needs nothing here. A hosted
/// deployment verifies a short-lived token offline against a JWKS, so that the credential-issuing
/// service is not on the hot path of a product whose whole promise is a durable wait.
///
/// One verification port, two issuers. That is the property that stops the hosted tier having an
/// authentication mechanism the open core does not have — which is precisely the shape that makes
/// an open core rot.
///
/// **Tenancy comes from the credential and never from a request body** (§4.1, I13). An
/// implementation that reads a tenant out of the payload it was handed has broken the isolation
/// rule no matter what else it does correctly.
pub trait CallerAuthenticator: Send + Sync {
    /// Resolve a presented secret, or `Ok(None)` if it authenticates nobody.
    ///
    /// `Ok(None)` and an error are different answers: `None` means the credential is not valid,
    /// and an error means we could not find out. A deployment that cannot reach its identity
    /// provider MUST return an error rather than `None`, because reporting "invalid credential"
    /// for an outage tells the caller to go and rotate a key that was never the problem.
    fn authenticate(&self, presented_secret: String) -> BoxFuture<'_, Result<Option<Principal>>>;
}

/// Map an authenticated principal to the tenant whose data it may touch.
///
/// The engine never learns what a tenant *is*. It carries an opaque reference, and every store
/// lookup is scoped by it. That is what lets a hosted deployment map to an organization without
/// the open core carrying a vestigial org table that self-hosters ignore and that we would then
/// have to keep migrating forever.
pub trait TenantResolver: Send + Sync {
    /// The tenant this principal acts within.
    fn tenant_of(&self, principal: &Principal) -> Result<String>;
}

/// The resolver every deployment starts with: the tenant the credential itself carries.
///
/// Correct for a single operator, and correct for a hosted deployment too — the difference is what
/// put the value on the credential, which is not this trait's business.
#[derive(Debug, Clone, Copy, Default)]
pub struct CredentialTenant;

impl TenantResolver for CredentialTenant {
    fn tenant_of(&self, principal: &Principal) -> Result<String> {
        Ok(principal.tenant_ref.clone())
    }
}

// ---------------------------------------------------------------------------------------------
// The directory of people
// ---------------------------------------------------------------------------------------------

/// One way to reach one person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactPoint {
    /// The channel name this address belongs to, matching a registered channel.
    pub channel: String,
    /// The address itself. Opaque to the engine; meaningful to the channel adapter.
    pub address: String,
    /// Whether the person has confirmed this address is theirs.
    ///
    /// An unverified address may still be delivered to, but a delivery through one can never be
    /// evidence about *who* received it, so §4.7 forbids it carrying an answer.
    pub verified: bool,
}

/// When a person has said they do not want to be interrupted.
///
/// Minutes from local midnight, so a window that crosses midnight is `start > end` rather than two
/// records. Quiet hours suppress a delivery; suppression is a recorded outcome and never a silent
/// drop ([`DeliveryState::Suppressed`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuietHours {
    /// Minutes after local midnight when quiet hours begin.
    pub from_minute: u16,
    /// Minutes after local midnight when they end.
    pub to_minute: u16,
}

/// A person the directory knows how to reach.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recipient {
    /// Their principal identity, when they have one in this deployment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal_id: Option<PrincipalId>,
    /// What to call them on a surface.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
    /// IANA zone, for quiet hours and for saying "3pm" in a way that means something.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// Every way to reach them.
    #[serde(default)]
    pub contacts: Vec<ContactPoint>,
    /// When not to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quiet_hours: Option<QuietHours>,
}

/// Turn a [`Target`] into the people it names.
///
/// §7.5 requires every target kind to resolve through **one** port returning a set of principals,
/// with no branch per kind anywhere else in the core. A rotation resolves at rung-fire time rather
/// than at raise time, which is why this is a call and not a snapshot taken when the request was
/// raised.
pub trait RecipientDirectory: Send + Sync {
    /// Everyone this target names, within this tenant and nowhere else.
    ///
    /// An empty vector is a legitimate answer — a rotation with nobody on call, a role with no
    /// members — and it is not an error. The engine records that no delivery could be attempted;
    /// §7.3 requires the request to survive that, because failing the raise would put a directory
    /// outage inside the caller's agent.
    fn resolve(&self, tenant: String, target: Target) -> BoxFuture<'_, Result<Vec<Recipient>>>;
}

// ---------------------------------------------------------------------------------------------
// Delivery
// ---------------------------------------------------------------------------------------------

/// Everything one delivery attempt needs, and nothing it does not.
///
/// There is no answer payload here and no capability secret. A channel adapter puts an ask in front
/// of a person and hands them a locator; it never carries the thing the ask is protecting.
#[derive(Debug, Clone, PartialEq)]
pub struct Envelope {
    /// The owning tenant.
    pub tenant: String,
    /// The request being delivered.
    pub request_id: RequestId,
    /// The delivery being attempted.
    ///
    /// **Stable across this delivery's attempts**, not new per attempt. §7.3 gives one delivery an
    /// ordered list of attempts, and the attempt number is what distinguishes them. A per-attempt
    /// identifier would also defeat the receiver's dedupe: `Handoff-Idempotency-Key` carries this
    /// value, so a retry that changed it would look like a new delivery to every receiver
    /// (`signing.md` §1.3 rule 7).
    pub delivery_id: DeliveryId,
    /// Which channel is being asked to carry it.
    pub channel: String,
    /// Who it is for.
    pub recipient: Recipient,
    /// What to show them.
    pub prompt: Prompt,
    /// Where they go to answer. A locator, never an authorization (§4.6).
    pub surface_url: String,
    /// Which ladder rung minted this attempt.
    pub rung: u32,
}

/// What one delivery attempt achieved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryReport {
    /// The strongest thing this attempt proved, or `None` when it proved nothing.
    ///
    /// `None` is not a degenerate case, it is the common one: a suppressed delivery was never sent
    /// and a failed one did not arrive, so neither has evidence to offer. The weakest *grade*,
    /// `dispatched`, already means "our transport accepted it" — a real claim — so spending it on
    /// an attempt that never reached a transport would record evidence that does not exist. There
    /// is no grade for "nothing happened", and there should not be one; absence is the honest
    /// representation.
    ///
    /// An adapter MUST NOT report a grade above what its [`ChannelCapabilities::max_grade`]
    /// declares; the engine clamps it regardless, because a grade is evidence and evidence that
    /// grades itself is not evidence.
    pub grade: Option<DeliveryGrade>,
    /// The state the delivery now occupies.
    pub state: DeliveryState,
    /// Operator-facing detail. Never shown to a responder and never part of a receipt.
    pub detail: Option<String>,
    /// Whether trying again could plausibly succeed. A bounced address is not retryable; a 503 is.
    pub retryable: bool,
}

impl DeliveryReport {
    /// The transport accepted it, and that is all we know.
    pub fn dispatched() -> Self {
        Self {
            grade: Some(DeliveryGrade::Dispatched),
            state: DeliveryState::Dispatched,
            detail: None,
            retryable: false,
        }
    }

    /// It could not be sent, and whether another attempt is worth making.
    pub fn failed(detail: impl Into<String>, retryable: bool) -> Self {
        Self {
            // Nothing was proved. A failed attempt that recorded `dispatched` would claim a
            // transport accepted something it never received.
            grade: None,
            state: if retryable {
                DeliveryState::Retrying
            } else {
                DeliveryState::Failed
            },
            detail: Some(detail.into()),
            retryable,
        }
    }

    /// Policy withheld it. A real outcome and a visible one (§7.1).
    pub fn suppressed(detail: impl Into<String>) -> Self {
        Self {
            // Withheld before any transport saw it, so there is nothing to grade.
            grade: None,
            state: DeliveryState::Suppressed,
            detail: Some(detail.into()),
            retryable: false,
        }
    }
}

/// One channel that can put a request in front of a person.
///
/// The engine looks an adapter up by name and reads its declaration. It never switches on the name,
/// and adding a provider must not add a branch outside the adapter itself. An adapter that requires
/// one is not finished.
///
/// **No adapter may carry a single global destination.** In the prior art a global recipient meant
/// every tenant's alert paged one person; that shape is a defect, not a configuration.
pub trait DeliveryChannel: Send + Sync {
    /// The name requests and ladders refer to this channel by.
    fn name(&self) -> &str;

    /// What this channel can prove (§7.2). Both fields are required of every adapter.
    fn capabilities(&self) -> ChannelCapabilities;

    /// Make one attempt.
    ///
    /// An `Err` means the attempt could not be made at all. A [`DeliveryReport`] with a failed
    /// state means it was made and did not work — a distinction that matters, because only the
    /// second is worth recording against the channel's reputation.
    fn deliver(&self, envelope: Envelope) -> BoxFuture<'_, Result<DeliveryReport>>;
}

// ---------------------------------------------------------------------------------------------
// Metering and the audit mirror
// ---------------------------------------------------------------------------------------------

/// One thing that happened, and is worth counting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeterReading {
    /// The tenant it happened in.
    pub tenant: String,
    /// What was consumed. One value for this product, so a meter reader never has to know the
    /// product's internal vocabulary.
    pub resource: String,
    /// Which kind of consumption.
    pub kind: String,
    /// How much.
    pub quantity: i64,
    /// Of what — `count`, normally.
    pub unit: String,
    /// The dedupe key.
    ///
    /// **It MUST be tenant-scoped.** §3.2 makes this a correctness property rather than a detail:
    /// a globally unique meter key does not merely risk a collision, it lets one tenant's usage
    /// silently absorb another's, and it produces no error anywhere.
    pub idempotency_key: String,
    /// Server clock when it happened.
    pub occurred_at: Timestamp,
}

impl MeterReading {
    /// Check the one property that cannot be recovered from later.
    ///
    /// A meter key that does not contain its tenant is a cross-tenant write waiting to happen, and
    /// the failure is silent, so it has to be caught at construction rather than at read time.
    pub fn validate(&self) -> Result<()> {
        if self.idempotency_key.is_empty() {
            return Err(ProtocolError::new(
                ErrorCode::InvalidRequest,
                "a meter reading needs an idempotency key",
            ));
        }
        if !self.idempotency_key.contains(&self.tenant) {
            return Err(ProtocolError::new(
                ErrorCode::InvalidRequest,
                "a meter idempotency key must be scoped by its tenant, or one tenant's usage \
                 silently absorbs another's",
            ));
        }
        Ok(())
    }
}

/// Where usage goes, when it goes anywhere outside this deployment.
///
/// The engine's own event log already records what happened. This port exists for the separate
/// question of what a *billing* system is told, which is a different system with different
/// durability requirements and a different owner.
pub trait MeterSink: Send + Sync {
    /// Record a batch. At-least-once: the sink is expected to dedupe on
    /// [`MeterReading::idempotency_key`].
    fn record(&self, readings: Vec<MeterReading>) -> BoxFuture<'_, Result<()>>;
}

/// A summary of something that happened, for an audit log that is not ours.
///
/// **The payload is a summary and never the content.** A mirror carries the receipt id, the digest,
/// the actor and the outcome; it does not carry the answer and it does not carry the prompt. The
/// detail stays in the store that wrote it in the outcome transaction, because no transaction spans
/// two services.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditEvent {
    /// The tenant it belongs to.
    pub tenant: String,
    /// A versioned `domain.name.vN` type.
    pub event_type: String,
    /// Server clock at the original transition, not at mirror time. A derived record may be
    /// delayed; it must not misreport when the thing happened.
    pub occurred_at: Timestamp,
    /// What the event is about — a request id, a receipt id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// The summary itself.
    pub payload: Value,
}

/// Where a derived audit record goes.
///
/// This is a **mirror**, never the record itself. The store already wrote the authoritative event
/// in the same transaction as the state change (I12); this port hands a summary to a wider index so
/// that "what happened in this organization" has one place to be asked. If this sink is absent, the
/// authoritative record is unaffected — which is the test of whether it is really a mirror.
///
/// Derived records may be delayed. They must not be silently dropped, which is why a deployment
/// wiring this port is expected to feed it from a durable queue rather than from the write path.
pub trait EventSink: Send + Sync {
    /// Append a batch. At-least-once; the sink is expected to be idempotent on
    /// `(tenant, event_type, subject, occurred_at)`.
    fn append(&self, events: Vec<AuditEvent>) -> BoxFuture<'_, Result<()>>;
}

// ---------------------------------------------------------------------------------------------
// Attestation
// ---------------------------------------------------------------------------------------------

/// A signature over a receipt digest, by a party the receipt names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attestation {
    /// Which key signed, so a verifier can find the public half.
    pub key_id: String,
    /// The algorithm, named rather than assumed.
    pub algorithm: String,
    /// The signature, base64url without padding.
    pub signature: String,
    /// Who is making the claim. A verifier needs to know whose word this is.
    pub issuer: String,
}

/// Sign a receipt digest.
///
/// A self-hosted receipt is signed with a key the operator controls, which is adequate for internal
/// control and worthless as evidence *against* the operator — a party can always produce any
/// receipt it wishes about itself. Only a party that is not the operator can attest. That is the
/// definition of a third party, not a feature we withheld.
///
/// The verifier is deliberately **not** a port. `handoff-protocol` verifies any receipt as a pure
/// function, and it must verify self-signed and attested receipts alike. A receipt only its issuer
/// can check is a vendor claim rather than evidence, and making the verifier pluggable would be the
/// first step to exactly that.
pub trait ReceiptSigner: Send + Sync {
    /// Attest to one receipt digest.
    fn attest(
        &self,
        tenant: String,
        receipt_id: ReceiptId,
        digest: Digest,
    ) -> BoxFuture<'_, Result<Attestation>>;
}

// ---------------------------------------------------------------------------------------------
// Takeover
// ---------------------------------------------------------------------------------------------

/// A handed-over view of something a person has to act on directly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TakeoverGrant {
    /// The opaque handle. Not a URL, and not a credential.
    pub handle: GrantHandle,
    /// The one resolvable address, minted per session and never stored (§11.2).
    pub viewer_url: String,
    /// When it stops working, whatever else happens.
    pub expires_at: Timestamp,
}

/// Mint, and revoke, a live view a person can act through.
///
/// Most deployments have no such surface, and `Ok(None)` is the right answer for them rather than
/// an error — a runtime that exposes no public ingress is not misconfigured. This follows the
/// existing precedent that a browser session's live URL is `None` for providers that cannot offer
/// one.
///
/// A grant minted here MUST be per-session, short-lived, and revocable. A broadcast URL that works
/// for anyone who learns it is not a grant.
pub trait TakeoverBroker: Send + Sync {
    /// Mint a grant, or `None` where this deployment has no takeover surface.
    fn mint(
        &self,
        tenant: String,
        session_ref: String,
        ttl: IsoDuration,
    ) -> BoxFuture<'_, Result<Option<TakeoverGrant>>>;

    /// Revoke one. A single write on a single grant, affecting no other (§11.4).
    fn revoke(&self, tenant: String, handle: GrantHandle) -> BoxFuture<'_, Result<bool>>;
}

/// The broker for a deployment with nothing to hand over.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoTakeover;

impl TakeoverBroker for NoTakeover {
    fn mint(
        &self,
        _tenant: String,
        _session_ref: String,
        _ttl: IsoDuration,
    ) -> BoxFuture<'_, Result<Option<TakeoverGrant>>> {
        Box::pin(async { Ok(None) })
    }

    fn revoke(&self, _tenant: String, _handle: GrantHandle) -> BoxFuture<'_, Result<bool>> {
        Box::pin(async { Ok(false) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use handoff_protocol::requires::{AuthStrength, Role};
    use std::future::Future;

    fn principal(tenant: &str) -> Principal {
        Principal {
            id: None,
            kind: crate::auth::PrincipalKind::Human,
            tenant_ref: tenant.to_string(),
            role: Role::Viewer,
            auth_strength: AuthStrength::Session,
            display: None,
            scopes: vec!["*".into()],
        }
    }

    #[test]
    fn the_default_resolver_takes_the_tenant_from_the_credential() {
        let resolved = CredentialTenant
            .tenant_of(&principal("org_01K3M7QW8ZC4YRXB2N6VD9FTHE"))
            .unwrap();
        assert_eq!(resolved, "org_01K3M7QW8ZC4YRXB2N6VD9FTHE");
    }

    #[test]
    fn a_meter_key_that_does_not_name_its_tenant_is_refused() {
        // The failure this catches is silent in every system that does not catch it: two tenants
        // pick the same key, and one of them simply loses a row.
        let mut reading = MeterReading {
            tenant: "org_a".into(),
            resource: "handoff".into(),
            kind: "intervention".into(),
            quantity: 1,
            unit: "count".into(),
            idempotency_key: "req_01K3M7QW8ZC4YRXB2N6VD9FTHE:intervention".into(),
            occurred_at: Timestamp::from_millis(0).unwrap(),
        };
        assert!(reading.validate().is_err());

        reading.idempotency_key =
            "handoff:org_a:req_01K3M7QW8ZC4YRXB2N6VD9FTHE:intervention".into();
        assert!(reading.validate().is_ok());
    }

    #[test]
    fn a_meter_reading_without_a_key_is_refused() {
        let reading = MeterReading {
            tenant: "org_a".into(),
            resource: "handoff".into(),
            kind: "request".into(),
            quantity: 1,
            unit: "count".into(),
            idempotency_key: String::new(),
            occurred_at: Timestamp::from_millis(0).unwrap(),
        };
        assert!(reading.validate().is_err());
    }

    /// Drive one future to completion without an async runtime.
    ///
    /// This crate deliberately has no runtime dependency — the whole point of expressing the ports
    /// through [`BoxFuture`] rather than a macro — and a test is not a good enough reason to
    /// acquire one. The defaults here never actually yield, so a no-op waker is sufficient.
    fn block_on<T>(future: impl Future<Output = T>) -> T {
        use std::sync::Arc;
        use std::task::{Context, Poll, Wake, Waker};

        struct Noop;
        impl Wake for Noop {
            fn wake(self: Arc<Self>) {}
        }

        let mut future = Box::pin(future);
        let waker = Waker::from(Arc::new(Noop));
        let mut context = Context::from_waker(&waker);
        loop {
            if let Poll::Ready(value) = future.as_mut().poll(&mut context) {
                return value;
            }
        }
    }

    #[test]
    fn a_deployment_with_no_takeover_surface_says_so_rather_than_failing() {
        let minted = block_on(NoTakeover.mint(
            "org_a".into(),
            "hs_01K3M7QW8ZC4YRXB2N6VD9FTHE".into(),
            IsoDuration::from_mins(5),
        ))
        .unwrap();
        assert!(minted.is_none());
        assert!(!block_on(NoTakeover.revoke(
            "org_a".into(),
            GrantHandle::parse("hg_01K3M7QW8ZC4YRXB2N6VD9FTHE").unwrap()
        ))
        .unwrap());
    }

    #[test]
    fn an_attempt_that_proved_nothing_carries_no_grade() {
        // The defect this guards against is a quiet one: `dispatched` is the weakest grade, so
        // using it as a filler for "nothing happened" reads as harmless — but `dispatched` means
        // "our transport accepted it", which a suppressed or failed attempt never established.
        // The delivery would then carry evidence of a send that did not occur.
        assert_eq!(DeliveryReport::failed("no such mailbox", false).grade, None);
        assert_eq!(DeliveryReport::failed("connection reset", true).grade, None);
        assert_eq!(DeliveryReport::suppressed("quiet hours").grade, None);
        assert_eq!(
            DeliveryReport::dispatched().grade,
            Some(DeliveryGrade::Dispatched)
        );
    }

    #[test]
    fn a_failed_report_records_whether_another_attempt_is_worth_making() {
        assert_eq!(
            DeliveryReport::failed("connection reset", true).state,
            DeliveryState::Retrying
        );
        assert_eq!(
            DeliveryReport::failed("no such mailbox", false).state,
            DeliveryState::Failed
        );
        // Suppression is an outcome, not a failure: an invisible suppression is indistinguishable
        // from a bug.
        assert_eq!(
            DeliveryReport::suppressed("quiet hours").state,
            DeliveryState::Suppressed
        );
    }
}

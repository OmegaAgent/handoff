//! `EventSink` — and the second of the two contradictions the plan leaves open.
//!
//! # The contradiction
//!
//! PLAN:870-871 instructs: *"Extend the existing log; **do not build a second audit table.**"* Once
//! Handoff is a separate service with its own database, that cannot hold literally. Its requests,
//! delivery attempts, and receipts have nowhere else to live — and the open core certainly cannot
//! write into an Ωmegas table, because a self-hosted deployment has no such table and never will.
//! The instruction and the out-of-repo decision (PLAN:1210) are simply not both satisfiable as
//! written.
//!
//! # The reconciliation, argued rather than assumed
//!
//! What the instruction is *for* survives; what does not survive is the claim that one physical
//! table holds every row. ADR-0011's intent is **one place to ask who did what in an organization**,
//! and that intent is met by a derived index rather than by a single table:
//!
//! - **Handoff's own store is the system of record** for handoff detail — the request, every
//!   delivery attempt, what the person was shown, the receipt and its signature. It has to be: the
//!   receipt is written in the same transaction as the outcome, and **no transaction spans two
//!   services**.
//! - **The Ωmegas `events` log stays the authoritative org-level index.** On each terminal receipt
//!   this adapter appends one summary event carrying the tenant, the request id, the outcome, the
//!   responder, the timestamps, and the receipt digest — enough to answer "what happened in this
//!   org" and to link out for the detail.
//! - The mirror is **derived, and emitted from a durable outbox, retried until acked**
//!   ([`crate::outbox`]). Derived records may be delayed. They must never be silently dropped, which
//!   is the difference between a read model and a data loss.
//!
//! # Two rules enforced here, not documented and hoped for
//!
//! **The namespace is closed.** Only `handoff.*` types are emitted, so a compromised Handoff
//! credential cannot forge an `org.*` administrative event into the audit log that governs the
//! organization itself. This is checked on the way out, in [`OmegasEvents::append`], because the
//! check that matters is the one on our side of the boundary.
//!
//! **A summary is not the content.** The payload carries identifiers, an outcome, and a digest. It
//! **never** carries the answer and it **never** carries the prompt. A mirror that replicated the
//! content would put the thing a person typed into a second system with a different retention
//! policy, a different access model, and a different blast radius — and would do it by accident.
//!
//! # Two prerequisites this exposes
//!
//! Both are on Handoff's critical path rather than on someone else's backlog: the control plane's
//! `events` table is **not** append-only at the database and carries two live
//! `UPDATE events SET instance_id` statements that must go first; and the API drops `payload`
//! entirely from its event DTO, which is exactly the field a receipt summary lives in.

use handoff_core::ports::BoxFuture;
use handoff_core::seam::{AuditEvent, EventSink};
use handoff_protocol::error::{ErrorCode, ProtocolError, Result};
use handoff_protocol::receipt::Receipt;
use std::sync::Arc;

use crate::control_plane::{ControlPlane, Request};
use crate::dependency::MissingDependency;

/// The only namespace this service may write into the shared log.
pub const NAMESPACE: &str = "handoff.";

/// The one event type the mirror emits today.
pub const RECEIPT_RECORDED: &str = "handoff.receipt.recorded.v1";

/// Field names that must never appear in a mirrored payload.
///
/// Checked by name rather than trusted to the builder below, because the builder is one careless
/// edit away from including a whole receipt and the check is what would catch it.
const FORBIDDEN: &[&str] = &["answer", "prompt", "values", "secret", "rendered", "steps"];

/// Build the summary for one receipt.
///
/// Everything here is either an identifier, a timestamp, or a digest. If a future field cannot be
/// described that way, it does not belong in a mirror.
pub fn summarize(receipt: &Receipt) -> Result<AuditEvent> {
    let digest = receipt
        .chain
        .as_ref()
        .map(|link| link.digest.as_str().to_string());

    let payload = serde_json::json!({
        "receipt_id": receipt.id.to_string(),
        "request_id": receipt.request_id.to_string(),
        "kind": receipt.kind,
        // The disposition, not the answer: whether the person decided, delegated, or reported being
        // unable is an org-level fact. What they decided is not.
        "disposition": receipt.decision.disposition,
        "actor_type": receipt.actor.actor_type,
        "actor_id": receipt.actor.principal_id.as_ref().map(ToString::to_string),
        "satisfied_strength": receipt.authority.satisfied,
        "decided_at": receipt.decided_at.to_millis(),
        "request_version": receipt.request_version,
        "request_digest": receipt.request_digest.as_str(),
        "receipt_digest": digest,
        "chain_height": receipt.chain.as_ref().map(|link| link.height),
    });

    let event = AuditEvent {
        tenant: receipt.org_id.to_string(),
        event_type: RECEIPT_RECORDED.to_string(),
        occurred_at: receipt.decided_at,
        subject: Some(receipt.request_id.to_string()),
        payload,
    };
    check(&event)?;
    Ok(event)
}

/// The two rules, applied to one event.
fn check(event: &AuditEvent) -> Result<()> {
    if !event.event_type.starts_with(NAMESPACE) {
        return Err(ProtocolError::new(
            ErrorCode::InsufficientScope,
            format!(
                "this service may only write `{NAMESPACE}*` events; `{}` is outside its namespace",
                event.event_type
            ),
        ));
    }
    if let Some(object) = event.payload.as_object() {
        if let Some(leaked) = FORBIDDEN.iter().find(|f| object.contains_key(**f)) {
            return Err(ProtocolError::new(
                ErrorCode::InvalidRequest,
                format!(
                    "an audit mirror carries a summary, never content: `{leaked}` must not leave \
                     the Handoff store"
                ),
            ));
        }
    }
    Ok(())
}

/// The control-plane audit mirror.
pub struct OmegasEvents {
    control: Arc<ControlPlane>,
}

impl OmegasEvents {
    /// Build one.
    pub fn new(control: Arc<ControlPlane>) -> Self {
        Self { control }
    }
}

impl EventSink for OmegasEvents {
    fn append(&self, events: Vec<AuditEvent>) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            if events.is_empty() {
                return Ok(());
            }
            let tenant = events[0].tenant.clone();
            if events.iter().any(|e| e.tenant != tenant) {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidRequest,
                    "an audit batch must belong to exactly one organization",
                ));
            }
            for event in &events {
                check(event)?;
            }

            let body = serde_json::json!({
                "events": events.iter().map(|e| serde_json::json!({
                    "type": e.event_type,
                    "subject": e.subject,
                    "occurred_at": e.occurred_at.to_millis(),
                    "payload": e.payload,
                })).collect::<Vec<_>>()
            });

            self.control
                .call(
                    Request::post("/api/events/ingest", tenant, body),
                    MissingDependency::EVENT_INGEST,
                )
                .await
                .map(|_| ())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_plane::{FakeControlPlane, Response};
    use crate::fixtures;

    const ORG: &str = "org_01K3M7QW8ZC4YRXB2N6VD9FTHE";

    fn events(fake: FakeControlPlane) -> OmegasEvents {
        OmegasEvents::new(Arc::new(ControlPlane::new(Box::new(fake))))
    }

    #[test]
    fn a_summary_carries_identifiers_and_digests_and_nothing_a_person_typed() {
        let summary = summarize(&fixtures::sealed_receipt()).expect("summarize");
        let payload = summary.payload.as_object().expect("object");
        assert_eq!(summary.event_type, RECEIPT_RECORDED);
        assert_eq!(summary.tenant, ORG);
        assert!(payload.contains_key("receipt_id"));
        assert!(payload.contains_key("request_digest"));
        assert!(payload.contains_key("receipt_digest"));
        for forbidden in FORBIDDEN {
            assert!(
                !payload.contains_key(*forbidden),
                "`{forbidden}` must not leave the Handoff store"
            );
        }
    }

    #[test]
    fn the_occurred_at_is_the_decision_time_and_not_the_mirror_time() {
        // A derived record may be delayed. It must not misreport when the thing happened.
        let receipt = fixtures::sealed_receipt();
        assert_eq!(
            summarize(&receipt).expect("summarize").occurred_at,
            receipt.decided_at
        );
    }

    #[tokio::test]
    async fn an_event_outside_the_handoff_namespace_is_refused_on_the_way_out() {
        // A compromised Handoff credential must not be able to forge an administrative event into
        // the log that governs the organization.
        let mut forged = summarize(&fixtures::sealed_receipt()).expect("summarize");
        forged.event_type = "org.member.removed.v1".into();
        let error = events(FakeControlPlane::new())
            .append(vec![forged])
            .await
            .expect_err("outside the namespace");
        assert_eq!(error.code, ErrorCode::InsufficientScope);
        assert!(error.message.contains("outside its namespace"));
    }

    #[tokio::test]
    async fn a_payload_carrying_content_is_refused_even_if_the_type_is_ours() {
        let mut leaky = summarize(&fixtures::sealed_receipt()).expect("summarize");
        leaky.payload["answer"] = serde_json::json!({"approve": true});
        let error = events(FakeControlPlane::new())
            .append(vec![leaky])
            .await
            .expect_err("content must not be mirrored");
        assert!(error.message.contains("a summary, never content"));
    }

    #[tokio::test]
    async fn a_batch_spanning_two_organizations_is_refused() {
        let mut other = summarize(&fixtures::sealed_receipt()).expect("summarize");
        other.tenant = "org_01K3M7QW8ZC4YRXB2N6VD9FTHF".into();
        let batch = vec![
            summarize(&fixtures::sealed_receipt()).expect("summarize"),
            other,
        ];
        assert!(events(FakeControlPlane::new()).append(batch).await.is_err());
    }

    #[tokio::test]
    async fn a_well_formed_batch_reaches_the_ingestion_endpoint() {
        let fake =
            Arc::new(FakeControlPlane::new().reply("/api/events/ingest", Response::new(200, "{}")));
        let sink = OmegasEvents::new(Arc::new(ControlPlane::new(Box::new(Arc::clone(&fake)))));
        sink.append(vec![
            summarize(&fixtures::sealed_receipt()).expect("summarize")
        ])
        .await
        .expect("scripted 200");
        let sent = fake.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].org.as_deref(), Some(ORG));
        assert_eq!(
            sent[0].body.as_ref().expect("body")["events"][0]["type"],
            RECEIPT_RECORDED
        );
    }

    #[tokio::test]
    async fn an_absent_ingestion_endpoint_names_the_dependency_and_its_blocker() {
        let error = events(FakeControlPlane::new())
            .append(vec![
                summarize(&fixtures::sealed_receipt()).expect("summarize")
            ])
            .await
            .expect_err("the endpoint does not exist yet");
        assert!(error.message.contains("POST /api/events/ingest"));
        assert!(error.message.contains("append-only"));
    }
}

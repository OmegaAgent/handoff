//! `MeterSink` — usage reaches the one wallet, over HTTP.
//!
//! `usage_events` in the control plane is already a generic append-only stream of
//! `(org_id, space_id, resource, kind, quantity, unit, meta, idempotency_key, occurred_at)`, so
//! Handoff meters itself with **no schema change** on the control-plane side. What is net-new is the
//! ingestion endpoint, because an out-of-repo service cannot call `UsageLedgerRepo::record_usage`
//! in process.
//!
//! | `resource` | `kind` | `unit` |
//! |---|---|---|
//! | `handoff` | `request` | `count` |
//! | `handoff` | `delivery` | `count` |
//! | `handoff` | `intervention` | `count` |
//! | `handoff` | `callback` | `count` |
//!
//! # Two known defects this adapter is built around
//!
//! **B-10: `usage_events.idempotency_key` is globally unique, not per-org.** Two tenants that
//! generate the same key do not collide loudly — one of them silently loses a row, and no error
//! appears anywhere. Every key minted here is therefore `handoff:{org_id}:{request_id}:{kind}`, and
//! [`handoff_core::seam::MeterReading::validate`] refuses any reading whose key does not contain its
//! own tenant. That turns a convention into a check that runs before the batch leaves this process.
//!
//! **B-15: control-plane usage writes are best-effort and failures are swallowed.** Metering that
//! merely reports volume can tolerate that. Metering that *bills* cannot. This adapter does not
//! bill — per-intervention charging is deliberately not at v1 — but it also does not rely on the
//! remote side's durability: readings are queued in [`crate::outbox`] and retried until acked, so a
//! swallowed write on their side is still a pending row on ours.

use handoff_core::ports::BoxFuture;
use handoff_core::seam::{MeterReading, MeterSink};
use handoff_protocol::clock::Timestamp;
use handoff_protocol::error::Result;
use handoff_protocol::id::RequestId;
use std::sync::Arc;

use crate::control_plane::{ControlPlane, Request};
use crate::dependency::MissingDependency;

/// The one resource name Handoff meters under.
pub const RESOURCE: &str = "handoff";

/// The counted unit. One name, because a meter with two units is a meter nobody can sum.
pub const UNIT: &str = "count";

/// What Handoff counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Metered {
    /// A request was raised.
    Request,
    /// One delivery attempt was made.
    Delivery,
    /// A person actually intervened — the dimension that would be priced, if anything were.
    Intervention,
    /// A callback was pushed to a waiting runtime.
    Callback,
}

impl Metered {
    /// The `kind` written to the ledger.
    pub const fn kind(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Delivery => "delivery",
            Self::Intervention => "intervention",
            Self::Callback => "callback",
        }
    }
}

/// Build one reading with an org-scoped dedupe key.
///
/// This is the only way a reading should be constructed in this crate. A hand-built key is how B-10
/// bites.
pub fn reading(
    tenant: &str,
    request: &RequestId,
    metered: Metered,
    occurred_at: Timestamp,
) -> Result<MeterReading> {
    let reading = MeterReading {
        tenant: tenant.to_string(),
        resource: RESOURCE.into(),
        kind: metered.kind().into(),
        quantity: 1,
        unit: UNIT.into(),
        idempotency_key: format!("{RESOURCE}:{tenant}:{request}:{}", metered.kind()),
        occurred_at,
    };
    reading.validate()?;
    Ok(reading)
}

/// The control-plane meter.
pub struct OmegasMeter {
    control: Arc<ControlPlane>,
}

impl OmegasMeter {
    /// Build one.
    pub fn new(control: Arc<ControlPlane>) -> Self {
        Self { control }
    }
}

impl MeterSink for OmegasMeter {
    fn record(&self, readings: Vec<MeterReading>) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            if readings.is_empty() {
                return Ok(());
            }
            // One tenant per call. The tenant travels as a header and the endpoint reads it from
            // the credential's binding, so a batch spanning two orgs has no honest representation —
            // and the failure mode of pretending otherwise is one org's usage billed to another.
            let tenant = readings[0].tenant.clone();
            if readings.iter().any(|r| r.tenant != tenant) {
                return Err(handoff_protocol::error::ProtocolError::new(
                    handoff_protocol::error::ErrorCode::InvalidRequest,
                    "a usage batch must belong to exactly one organization",
                ));
            }
            for entry in &readings {
                entry.validate()?;
            }

            let body = serde_json::json!({
                "readings": readings.iter().map(|r| serde_json::json!({
                    "resource": r.resource,
                    "kind": r.kind,
                    "quantity": r.quantity,
                    "unit": r.unit,
                    "idempotency_key": r.idempotency_key,
                    "occurred_at": r.occurred_at.to_millis(),
                })).collect::<Vec<_>>()
            });

            self.control
                .call(
                    Request::post("/api/usage/ingest", tenant, body),
                    MissingDependency::USAGE_INGEST,
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

    const ORG: &str = "org_01K3M7QW8ZC4YRXB2N6VD9FTHE";
    const OTHER: &str = "org_01K3M7QW8ZC4YRXB2N6VD9FTHF";

    fn request_id() -> RequestId {
        RequestId::parse("req_01K3M7QW8ZC4YRXB2N6VD9FTHE").expect("parse")
    }

    fn now() -> Timestamp {
        Timestamp::from_millis(1_700_000_000_000).expect("timestamp")
    }

    #[test]
    fn every_key_carries_its_own_organization() {
        // B-10: the column is globally unique, so a key without a tenant is one tenant's usage
        // silently absorbing another's — with no error anywhere.
        let mine = reading(ORG, &request_id(), Metered::Intervention, now()).expect("reading");
        let theirs = reading(OTHER, &request_id(), Metered::Intervention, now()).expect("reading");
        assert!(mine.idempotency_key.contains(ORG));
        assert_ne!(mine.idempotency_key, theirs.idempotency_key);
    }

    #[test]
    fn the_four_metered_dimensions_have_distinct_keys_for_one_request() {
        let keys: Vec<String> = [
            Metered::Request,
            Metered::Delivery,
            Metered::Intervention,
            Metered::Callback,
        ]
        .iter()
        .map(|m| {
            reading(ORG, &request_id(), *m, now())
                .expect("reading")
                .idempotency_key
        })
        .collect();
        let mut unique = keys.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), 4);
        assert_eq!(keys.len(), 4);
    }

    #[tokio::test]
    async fn a_batch_spanning_two_organizations_is_refused() {
        let meter = OmegasMeter::new(Arc::new(ControlPlane::new(Box::new(
            FakeControlPlane::new().reply("/api/usage/ingest", Response::new(200, "{}")),
        ))));
        let mixed = vec![
            reading(ORG, &request_id(), Metered::Request, now()).expect("reading"),
            reading(OTHER, &request_id(), Metered::Request, now()).expect("reading"),
        ];
        assert!(meter.record(mixed).await.is_err());
    }

    #[tokio::test]
    async fn the_tenant_travels_in_the_header_and_never_in_the_body() {
        // A verified credential proves who is calling, never which tenant the payload is about. A
        // body that carries an org invites a server that trusts it.
        let fake =
            Arc::new(FakeControlPlane::new().reply("/api/usage/ingest", Response::new(200, "{}")));
        let meter = OmegasMeter::new(Arc::new(ControlPlane::new(Box::new(Arc::clone(&fake)))));
        meter
            .record(vec![
                reading(ORG, &request_id(), Metered::Request, now()).expect("reading")
            ])
            .await
            .expect("scripted 200");

        let sent = fake.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].org.as_deref(), Some(ORG));
        let body = sent[0].body.as_ref().expect("a body");
        assert!(body.get("org_id").is_none());
        assert!(body.get("org").is_none());
        // And the reading itself carries no tenant field either — only the key, which contains it
        // for dedupe rather than for routing.
        let first = &body["readings"][0];
        assert!(first.get("tenant").is_none());
        assert_eq!(first["kind"], "request");
    }

    #[tokio::test]
    async fn an_empty_batch_makes_no_call_at_all() {
        // An unbuilt endpoint 404s, so a meter that called on an empty batch would fail every sweep
        // in which nothing happened.
        let meter = OmegasMeter::new(Arc::new(ControlPlane::new(Box::new(
            FakeControlPlane::new(),
        ))));
        assert!(meter.record(Vec::new()).await.is_ok());
    }

    #[tokio::test]
    async fn an_absent_ingestion_endpoint_names_the_dependency() {
        let meter = OmegasMeter::new(Arc::new(ControlPlane::new(Box::new(
            FakeControlPlane::new(),
        ))));
        let error = meter
            .record(vec![
                reading(ORG, &request_id(), Metered::Request, now()).expect("reading")
            ])
            .await
            .expect_err("the endpoint does not exist yet");
        assert!(error.message.contains("POST /api/usage/ingest"));
    }
}

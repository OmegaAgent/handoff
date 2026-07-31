//! `TakeoverBroker` — Operator live-view grants, blocked on a token that does not exist.
//!
//! Browser takeover is the highest-bandwidth channel there is, and the only one where a person can
//! *act* rather than reply. It is also the one with the sharpest failure mode, because handing
//! someone a live view of a running session is handing them everything that session can reach.
//!
//! What exists today in the control plane is a **broadcast live-view URL**: not per-session, not
//! short-lived, not revocable, and valid for anyone who learns it. That is defect B-2, and it is the
//! shape this adapter must not be built on. The replacement — a per-session, short-TTL, revocable
//! viewer token — is post-M0 work that has not landed.
//!
//! So [`OperatorTakeover`] refuses. Note carefully what it does **not** do: it does not fall back to
//! the broadcast URL. A fallback would be the single most damaging line in this crate, because it
//! would work, demo well, and quietly hand out a permanent capability. The refusal is a feature.
//!
//! # `None` and an error are different answers
//!
//! The port's own default returns `Ok(None)`, meaning "this deployment has no takeover surface" —
//! correct and unremarkable for a runtime with no public ingress. This adapter returns an **error**
//! instead, because Ωmegas *does* have a takeover surface and cannot currently offer it safely.
//! Reporting that as "no surface here" would hide a blocked capability behind a normal-looking
//! absence.

use handoff_core::ports::BoxFuture;
use handoff_core::seam::{TakeoverBroker, TakeoverGrant};
use handoff_protocol::clock::IsoDuration;
use handoff_protocol::error::Result;
use handoff_protocol::id::GrantHandle;

use crate::dependency::MissingDependency;

/// Operator's live view, as far as it can be offered.
#[derive(Debug, Clone, Copy, Default)]
pub struct OperatorTakeover;

impl TakeoverBroker for OperatorTakeover {
    fn mint(
        &self,
        _tenant: String,
        _session_ref: String,
        _ttl: IsoDuration,
    ) -> BoxFuture<'_, Result<Option<TakeoverGrant>>> {
        Box::pin(async { Err(MissingDependency::VIEWER_TOKEN.into_error()) })
    }

    fn revoke(&self, _tenant: String, _handle: GrantHandle) -> BoxFuture<'_, Result<bool>> {
        // Revocation refuses too, and it must. Reporting "revoked" for a grant this adapter never
        // minted would tell an operator a live view had been closed when nothing was closed.
        Box::pin(async { Err(MissingDependency::VIEWER_TOKEN.into_error()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle() -> GrantHandle {
        GrantHandle::parse("hg_01K3M7QW8ZC4YRXB2N6VD9FTHE").expect("parse")
    }

    #[tokio::test]
    async fn minting_refuses_and_names_the_defect_it_will_not_fall_back_to() {
        let error = OperatorTakeover
            .mint(
                "org_01K3M7QW8ZC4YRXB2N6VD9FTHE".into(),
                "hs_01K3M7QW8ZC4YRXB2N6VD9FTHE".into(),
                IsoDuration::from_mins(5),
            )
            .await
            .expect_err("there is no revocable viewer token");
        assert!(error.message.contains("revocable viewer token"));
        assert!(error.message.contains("B-2"));
    }

    #[tokio::test]
    async fn refusing_is_not_reported_as_having_no_takeover_surface() {
        // `Ok(None)` would be a lie of a particular kind: it says "nothing to hand over here",
        // when the truth is "there is something and we cannot hand it over safely".
        let minted = OperatorTakeover
            .mint(
                "org_01K3M7QW8ZC4YRXB2N6VD9FTHE".into(),
                "hs_01K3M7QW8ZC4YRXB2N6VD9FTHE".into(),
                IsoDuration::from_mins(5),
            )
            .await;
        assert!(minted.is_err(), "must not be Ok(None)");
    }

    #[tokio::test]
    async fn revoking_refuses_rather_than_reporting_a_close_that_did_not_happen() {
        assert!(OperatorTakeover
            .revoke("org_01K3M7QW8ZC4YRXB2N6VD9FTHE".into(), handle())
            .await
            .is_err());
    }
}

//! What this adapter needs from the control plane, and what it does when it is not there.
//!
//! Most of the Ωmegas surface this adapter consumes **does not exist yet**. Machine auth is
//! control-plane M5, entitlements are M4, and neither has landed; per-person contact records, an
//! attestation key, and a revocable viewer token have no owner at all. An adapter written against
//! absent dependencies has exactly two honest options, and stubbing them out is not one of them:
//! refuse, and say precisely what is missing.
//!
//! So every adapter here that depends on something unbuilt fails closed through
//! [`MissingDependency`], which names the surface, the milestone that would deliver it, and the
//! document that decided it. The failure is loud on purpose: a hosted service that silently
//! degrades to "no metering" or "no attestation" is a hosted service selling something it is not
//! doing.
//!
//! **The refusals here are not a dormant gate.** A gate withholds behaviour the code could perform;
//! these refuse behaviour that has no implementation anywhere, in either tier. The open core
//! contains none of them, which is the test.

use handoff_protocol::error::{ErrorCode, ProtocolError};
use std::fmt;

/// A control-plane capability this adapter needs and cannot get.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingDependency {
    /// What this adapter was trying to do.
    pub capability: &'static str,
    /// The surface that would provide it — an endpoint, a table, a key.
    pub surface: &'static str,
    /// Which milestone owns it, or `None` where nothing does yet.
    pub milestone: Option<&'static str>,
    /// Where the decision or the gap is recorded.
    pub source: &'static str,
}

impl MissingDependency {
    /// `POST /api/token` — the client-credentials exchange, without which nothing authenticates.
    pub const TOKEN_EXCHANGE: Self = Self {
        capability: "authenticate a caller",
        surface: "POST /api/token (omg_ key -> short-lived ES256 JWT) and the JWKS to verify it",
        milestone: Some("M5 (machine auth)"),
        source: "13-open-closed-boundary §3.1.1, §4.1; PLAN:1212",
    };

    /// `POST /api/usage/ingest`.
    pub const USAGE_INGEST: Self = Self {
        capability: "record usage against the one wallet",
        surface: "POST /api/usage/ingest",
        milestone: Some("M4 (entitlements) threads product_code; the endpoint itself is net-new"),
        source: "13-open-closed-boundary §4.1, §6.2",
    };

    /// `POST /api/events/ingest`.
    pub const EVENT_INGEST: Self = Self {
        capability: "mirror a receipt summary into the org-level audit index",
        surface: "POST /api/events/ingest, restricted to the handoff.* event namespace",
        milestone: Some("net-new; blocked on `events` becoming append-only at the database"),
        source: "13-open-closed-boundary §3.1.2, §4.1",
    };

    /// `GET /api/orgs/{id}/members`.
    pub const ORG_MEMBERS: Self = Self {
        capability: "resolve a target to the people it names",
        surface: "GET /api/orgs/{id}/members",
        milestone: Some("planned as an org surface"),
        source: "07-shared-control-plane §3.2 (07:239)",
    };

    /// Per-person contact records, which no table holds.
    pub const CONTACT_POINTS: Self = Self {
        capability: "reach a person on any channel other than the in-app surface",
        surface: "a per-person contact record (phone, handle, verified address)",
        milestone: None,
        source: "07-shared-control-plane §5 (07:319) — no such record exists in any migration",
    };

    /// Entitlements.
    pub const ENTITLEMENTS: Self = Self {
        capability: "check whether this org may use Handoff",
        surface: "is_entitled(org, 'handoff', now), or the same as a JWT claim",
        milestone: Some("M4 (entitlements)"),
        source: "13-open-closed-boundary §6.1 — no entitlement table exists in 74 migrations",
    };

    /// The attestation key.
    pub const ATTESTATION_KEY: Self = Self {
        capability: "attest a receipt as a party other than the operator",
        surface: "an Ωmegas-held signing key and a public verification endpoint",
        milestone: None,
        source: "13-open-closed-boundary §5.1 — the attestation service does not exist yet either",
    };

    /// The revocable viewer token.
    pub const VIEWER_TOKEN: Self = Self {
        capability: "hand a person a live view they can act through",
        surface: "a per-session, short-TTL, revocable viewer token",
        milestone: Some("post-M0"),
        source: "07-shared-control-plane (07:97-99); today's broadcast URL is defect B-2",
    };

    /// Turn this into the error a caller sees.
    ///
    /// [`ErrorCode::ProductNotEntitled`] is deliberately **not** used for an absent dependency: it
    /// means "this tenant may not", and telling an entitled tenant they are not entitled because we
    /// have not built something would be a lie in the one field a support engineer reads first.
    pub fn into_error(self) -> ProtocolError {
        ProtocolError::new(ErrorCode::DeliveryUnavailable, self.to_string())
    }
}

impl fmt::Display for MissingDependency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the managed adapter cannot {}: it needs {}, which does not exist yet",
            self.capability, self.surface
        )?;
        match self.milestone {
            Some(milestone) => write!(f, " ({milestone})")?,
            None => write!(f, " (no milestone owns it)")?,
        }
        write!(f, ". See {}.", self.source)
    }
}

impl From<MissingDependency> for ProtocolError {
    fn from(missing: MissingDependency) -> Self {
        missing.into_error()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_names_the_surface_the_milestone_and_the_source() {
        // A refusal that does not say what would fix it produces a support ticket rather than a
        // change, which is the whole reason this type exists instead of a bare string.
        let message = MissingDependency::TOKEN_EXCHANGE.to_string();
        assert!(message.contains("POST /api/token"));
        assert!(message.contains("M5"));
        assert!(message.contains("§3.1.1"));
    }

    #[test]
    fn a_dependency_nobody_owns_says_so_rather_than_inventing_a_milestone() {
        let message = MissingDependency::ATTESTATION_KEY.to_string();
        assert!(message.contains("no milestone owns it"));
    }

    #[test]
    fn an_absent_dependency_is_never_reported_as_an_entitlement_problem() {
        // Reporting our own gap as the tenant's entitlement problem sends the wrong person to fix
        // the wrong thing.
        for missing in [
            MissingDependency::TOKEN_EXCHANGE,
            MissingDependency::ATTESTATION_KEY,
            MissingDependency::VIEWER_TOKEN,
            MissingDependency::CONTACT_POINTS,
        ] {
            assert_ne!(missing.into_error().code, ErrorCode::ProductNotEntitled);
        }
    }
}

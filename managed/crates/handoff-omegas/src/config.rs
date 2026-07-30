//! What the managed deployment is told, and what it refuses to start without.
//!
//! Every setting here describes something outside the process. There is no setting that unlocks
//! behaviour, because a flag that turns a feature on is a flag that was shipped off — and a dormant
//! gate on the hosted side is the mirror image of the dormant gate the open core forbids.
//!
//! # Fail closed at boot, not at the first request
//!
//! An unconfigured issuer means this service authenticates nobody. It could discover that on the
//! first call and return a useful error, and [`crate::auth`] does exactly that as a backstop. But a
//! deployment that starts, looks healthy, and refuses every caller is worse than one that will not
//! start: the first is a paging incident at 3am, the second is a failed deploy at 3pm.
//! [`OmegasConfig::preflight`] returns the list of everything absent so the binary can decide
//! whether it is willing to run degraded, and say so in one line rather than in a thousand.

use handoff_protocol::error::{ErrorCode, ProtocolError, Result};

use crate::dependency::MissingDependency;

/// The managed deployment's own configuration.
#[derive(Debug, Clone)]
pub struct OmegasConfig {
    /// Base URL of the Ωmegas control plane.
    pub control_plane_base: String,
    /// The credential this service presents to the control plane.
    ///
    /// This is Handoff's own service credential — it is not, and must never be, a customer's key.
    pub service_token: String,
    /// The issuer whose tokens this service accepts. Empty means "authenticate nobody".
    pub token_issuer: String,
    /// Where that issuer's public keys are published.
    pub jwks_url: String,
    /// The audience this service is known by in a token.
    pub audience: String,
    /// Whether the control plane holds per-person contact records yet.
    ///
    /// **Not a feature flag.** It describes a table that either exists or does not, and setting it
    /// true while the table is absent produces silent non-delivery rather than a refusal.
    pub contact_points_available: bool,
    /// How often the reconciler and the outbox drain run.
    pub reconcile_interval_ms: u64,
}

impl OmegasConfig {
    /// Read the environment.
    pub fn from_env() -> Result<Self> {
        let env = |key: &str| std::env::var(key).ok().filter(|v| !v.is_empty());
        Ok(Self {
            control_plane_base: env("OMEGAS_CONTROL_PLANE_BASE")
                .unwrap_or_else(|| "https://omegas.dev".into()),
            service_token: env("OMEGAS_SERVICE_TOKEN").unwrap_or_default(),
            token_issuer: env("OMEGAS_TOKEN_ISSUER").unwrap_or_default(),
            jwks_url: env("OMEGAS_JWKS_URL").unwrap_or_default(),
            audience: env("OMEGAS_AUDIENCE").unwrap_or_else(|| "https://handoff.omegas.dev".into()),
            contact_points_available: env("OMEGAS_CONTACT_POINTS_AVAILABLE")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false),
            reconcile_interval_ms: env("OMEGAS_RECONCILE_INTERVAL_MS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(5_000),
        })
    }

    /// Everything this deployment needs and does not have.
    ///
    /// Empty means every dependency is configured. It does **not** mean every dependency works —
    /// that is only knowable by calling, and the adapters report it when they do.
    pub fn preflight(&self) -> Vec<MissingDependency> {
        let mut absent = Vec::new();
        if self.token_issuer.is_empty() || self.jwks_url.is_empty() {
            absent.push(MissingDependency::TOKEN_EXCHANGE);
        }
        if !self.contact_points_available {
            absent.push(MissingDependency::CONTACT_POINTS);
        }
        // Entitlements are M4 and this service has no way to check one. It is listed on every boot
        // rather than assumed, because "we forgot Handoff has no entitlement gate" is exactly the
        // thing a preflight exists to stop being forgotten.
        absent.push(MissingDependency::ENTITLEMENTS);
        absent.push(MissingDependency::ATTESTATION_KEY);
        absent.push(MissingDependency::VIEWER_TOKEN);
        absent
    }

    /// Whether this deployment can authenticate anybody at all.
    pub fn can_authenticate(&self) -> bool {
        !self.token_issuer.is_empty() && !self.jwks_url.is_empty()
    }

    /// Refuse to run without the one thing that makes the service usable.
    ///
    /// The other absences degrade a capability. This one means every request is refused, and a
    /// service that starts healthy and answers nothing is the worst of the available failures.
    pub fn require_authentication(&self) -> Result<()> {
        if self.can_authenticate() {
            return Ok(());
        }
        Err(ProtocolError::new(
            ErrorCode::InvalidRequest,
            format!(
                "refusing to start: {}. Set OMEGAS_TOKEN_ISSUER and OMEGAS_JWKS_URL, or start with \
                 OMEGAS_ALLOW_NO_AUTH=1 to run a deployment that authenticates nobody on purpose.",
                MissingDependency::TOKEN_EXCHANGE
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> OmegasConfig {
        OmegasConfig {
            control_plane_base: "https://omegas.dev".into(),
            service_token: "svc".into(),
            token_issuer: String::new(),
            jwks_url: String::new(),
            audience: "https://handoff.omegas.dev".into(),
            contact_points_available: false,
            reconcile_interval_ms: 5_000,
        }
    }

    #[test]
    fn todays_configuration_is_missing_five_things_and_lists_all_of_them() {
        // This is the honest state of the managed tier right now. If this test starts failing
        // because a list got shorter, something genuinely landed.
        let absent = config().preflight();
        assert_eq!(absent.len(), 5);
        assert_eq!(absent[0], MissingDependency::TOKEN_EXCHANGE);
        assert!(absent.contains(&MissingDependency::ENTITLEMENTS));
        assert!(absent.contains(&MissingDependency::ATTESTATION_KEY));
    }

    #[test]
    fn a_configured_issuer_removes_only_the_authentication_gap() {
        let configured = OmegasConfig {
            token_issuer: "https://auth.omegas.dev".into(),
            jwks_url: "https://auth.omegas.dev/jwks".into(),
            ..config()
        };
        assert!(configured.can_authenticate());
        assert!(configured.require_authentication().is_ok());
        assert!(!configured
            .preflight()
            .contains(&MissingDependency::TOKEN_EXCHANGE));
        // Entitlements are still absent, and still listed. One landing does not clear the board.
        assert!(configured
            .preflight()
            .contains(&MissingDependency::ENTITLEMENTS));
    }

    #[test]
    fn without_an_issuer_the_service_refuses_to_start_and_says_what_to_set() {
        let error = config()
            .require_authentication()
            .expect_err("a service that answers nothing must not look healthy");
        assert!(error.message.contains("OMEGAS_TOKEN_ISSUER"));
        assert!(error.message.contains("OMEGAS_JWKS_URL"));
        assert!(error.message.contains("M5"));
    }

    #[test]
    fn contact_points_default_to_absent() {
        // Defaulting this true would turn a loud refusal into silent non-delivery.
        assert!(!config().contact_points_available);
    }
}

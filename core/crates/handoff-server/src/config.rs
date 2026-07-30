//! Configuration, and the bootstrap that seeds credentials.
//!
//! Everything a deployment must decide is read from the environment, and every default is one a
//! single operator can run on their own machine. Nothing here reaches for a hosted service, and
//! there is no setting that unlocks behaviour — a feature flag waiting to be switched on would be a
//! boundary violation under `GOVERNANCE.md`.

use handoff_core::auth::AuthPolicy;
use handoff_core::capability::{CapabilityRegistry, EphemeralProvider};
use handoff_core::channel::{starter_ladder, ChannelRegistry};
use handoff_protocol::error::{ErrorCode, ProtocolError, Result};
use handoff_protocol::requires::DeploymentProfile;
use serde::Deserialize;

/// What this deployment was told.
#[derive(Debug, Clone)]
pub struct Config {
    /// Where the store lives.
    pub database_url: String,
    /// Where to listen.
    pub bind: String,
    /// Base for `surface_url`. A locator, not a capability (§4.6).
    pub public_base: String,
    /// Whether `link_only` may settle a request here (§4.4).
    pub link_only_permitted: bool,
    /// Active callback signing secrets, newest first.
    ///
    /// §1.4 of `signing.md` requires at least two to be simultaneously active, because rotation is
    /// an **overlap, not a cutover**: while both are active every callback is signed with both and
    /// carries both as separate `v1=` elements, so there is no window in which valid callbacks fail.
    pub callback_secrets: Vec<String>,
    /// Scheme and authority for the one resolvable address in the system (§11.2).
    pub capability_transport_base: String,
    /// A file of principals to seed at startup.
    pub bootstrap_file: Option<String>,
    /// How often the sweep runs.
    pub sweep_interval_ms: u64,
    /// Size of the store's connection pool.
    ///
    /// A serving process wants a real pool; a one-shot subcommand wants one or two connections and
    /// no more. Postgres budgets connections globally, so a CLI tool that reserves a serving pool
    /// is a CLI tool that takes the database down when a few of them run at once.
    pub max_connections: u32,
}

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

impl Config {
    /// Read the environment.
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            database_url: env("HANDOFF_DATABASE_URL").ok_or_else(|| {
                ProtocolError::new(
                    ErrorCode::InvalidRequest,
                    "HANDOFF_DATABASE_URL is required: handoffd owns its own database and will \
                     create its own tables there",
                )
            })?,
            bind: env("HANDOFF_BIND").unwrap_or_else(|| "127.0.0.1:8080".into()),
            public_base: env("HANDOFF_PUBLIC_BASE")
                .unwrap_or_else(|| "http://127.0.0.1:8080".into()),
            link_only_permitted: env("HANDOFF_LINK_ONLY_PERMITTED")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false),
            callback_secrets: env("HANDOFF_CALLBACK_SECRETS")
                .map(|v| {
                    v.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            capability_transport_base: env("HANDOFF_CAPABILITY_TRANSPORT_BASE")
                .unwrap_or_else(|| "wss://127.0.0.1:8443".into()),
            bootstrap_file: env("HANDOFF_BOOTSTRAP"),
            sweep_interval_ms: env("HANDOFF_SWEEP_INTERVAL_MS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(500),
            max_connections: env("HANDOFF_MAX_CONNECTIONS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(16),
        })
    }

    /// What this deployment will accept in a declaration.
    ///
    /// `allow_link_only` is `true` here and the answer-time policy is separate, because C-6b
    /// requires a deployment that forbids the grade to still **accept the raise** that declares it
    /// and refuse the answer. Rejecting the raise would mean the person is never asked at all.
    pub fn deployment_profile(&self) -> DeploymentProfile {
        DeploymentProfile {
            allow_link_only: true,
            ..DeploymentProfile::default()
        }
        .with_capability_types(["interactive_surface", "document", "remote_desktop"])
    }

    /// The deployment's view of `link_only`, applied at answer time (§4.4).
    pub fn auth_policy(&self) -> AuthPolicy {
        AuthPolicy {
            link_only_permitted: self.link_only_permitted,
        }
    }

    /// Channels and the default ladder.
    ///
    /// The descriptors come from the adapters this build compiled in, rather than from a list kept
    /// alongside them: what a ladder may name and what the process can actually do is then one
    /// fact, and a channel cannot declare a grade its adapter never implements.
    pub fn channels(&self) -> ChannelRegistry {
        ChannelRegistry::new(
            crate::delivery::Adapters::new(crate::delivery::shipped_adapters()).descriptors(),
            starter_ladder(),
        )
    }

    /// Capability providers.
    pub fn capabilities(&self) -> CapabilityRegistry {
        CapabilityRegistry::new(Box::new(EphemeralProvider {
            transport_base: self.capability_transport_base.clone(),
        }))
    }
}

/// One credential to seed.
///
/// The token is present here and nowhere else: [`seed`] stores its SHA-256 and forgets it, because
/// §4.1 requires that secrets are not stored recoverably.
#[derive(Debug, Clone, Deserialize)]
pub struct BootstrapPrincipal {
    /// The principal identity: `usr_…` for a person, `sa_…` for a machine. Absent for a link.
    #[serde(default)]
    pub id: Option<String>,
    /// The tenant this credential resolves to. Opaque to the engine (§4.1, I13).
    pub tenant_ref: String,
    /// `machine`, `human`, or `anonymous_link`.
    pub kind: String,
    /// The bearer token this credential presents.
    pub token: String,
    /// The role held.
    #[serde(default = "default_role")]
    pub role: String,
    /// The grade this credential authenticates at.
    #[serde(default = "default_strength")]
    pub auth_strength: String,
    /// Display name, frozen onto a receipt at decision time.
    #[serde(default)]
    pub display: Option<String>,
    /// Scopes. `*` means every scope.
    #[serde(default = "default_scopes")]
    pub scopes: Vec<String>,
}

fn default_role() -> String {
    "viewer".into()
}
fn default_strength() -> String {
    "session".into()
}
fn default_scopes() -> Vec<String> {
    vec!["*".into()]
}

/// A bootstrap file.
#[derive(Debug, Clone, Deserialize)]
pub struct Bootstrap {
    /// The credentials to seed.
    pub principals: Vec<BootstrapPrincipal>,
}

/// Seed credentials from a file, storing only their digests.
pub async fn seed(pool: &sqlx::PgPool, path: &str) -> Result<usize> {
    use sha2::{Digest, Sha256};

    let text = std::fs::read_to_string(path).map_err(|e| {
        ProtocolError::new(
            ErrorCode::InvalidRequest,
            format!("cannot read the bootstrap file {path}: {e}"),
        )
    })?;
    let bootstrap: Bootstrap = serde_json::from_str(&text)
        .map_err(|e| ProtocolError::new(ErrorCode::InvalidRequest, format!("{path}: {e}")))?;

    let mut seeded = 0;
    for principal in &bootstrap.principals {
        // A tenant reference has to be spellable as the `org_id` a receipt records, and finding
        // that out at bootstrap is better than finding it out when the first person answers.
        handoff_core::plan::tenant_as_org(&principal.tenant_ref)?;

        let id = principal
            .id
            .clone()
            .unwrap_or_else(|| format!("{}::link", principal.tenant_ref));
        let hash = format!("{:x}", Sha256::digest(principal.token.as_bytes()));
        sqlx::query(
            "insert into handoff_principals \
             (id, tenant_ref, kind, secret_sha256, role, auth_strength, display, scopes) \
             values ($1,$2,$3,$4,$5,$6,$7,$8) \
             on conflict (secret_sha256) do update set \
               tenant_ref = excluded.tenant_ref, kind = excluded.kind, role = excluded.role, \
               auth_strength = excluded.auth_strength, display = excluded.display, \
               scopes = excluded.scopes",
        )
        .bind(&id)
        .bind(&principal.tenant_ref)
        .bind(&principal.kind)
        .bind(&hash)
        .bind(&principal.role)
        .bind(&principal.auth_strength)
        .bind(principal.display.as_deref())
        .bind(&principal.scopes)
        .execute(pool)
        .await
        .map_err(|e| {
            ProtocolError::new(
                ErrorCode::InvalidRequest,
                format!("cannot seed principal {id}: {e}"),
            )
        })?;
        seeded += 1;
    }
    Ok(seeded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bootstrap_file_parses_with_sensible_defaults() {
        let bootstrap: Bootstrap = serde_json::from_str(
            r#"{"principals":[{"tenant_ref":"org_01K3M7QW8ZC4YRXB2N6VD9FTHE",
                               "kind":"machine","token":"omg_x","id":"sa_01K3M7QW8ZC4YRXB2N6VD9FTHE"}]}"#,
        )
        .unwrap();
        let principal = &bootstrap.principals[0];
        assert_eq!(principal.role, "viewer");
        assert_eq!(principal.auth_strength, "session");
        assert_eq!(principal.scopes, vec!["*".to_string()]);
    }
}

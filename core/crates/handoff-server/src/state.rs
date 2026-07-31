//! What every handler shares.

use handoff_core::capability::CapabilityRegistry;
use handoff_core::channel::ChannelRegistry;
use handoff_core::seam::CallerAuthenticator;
use handoff_protocol::clock::Timestamp;
use handoff_protocol::requires::DeploymentProfile;
use handoff_store_postgres::PgStore;
use std::sync::Arc;

use crate::config::Config;

/// The seam ports this deployment supplies, if any.
///
/// Every field is `Option`, and `None` means "this deployment has no such external system" rather
/// than "this deployment is broken". A single operator running `handoffd` on their own machine
/// legitimately supplies none of them, which is why [`Default`] is the complete, correct
/// configuration and not a placeholder.
///
/// A deployment that needs different behaviour fills a field here. It does **not** fork this crate:
/// a `[patch]` entry or a path dependency pointing at a modified copy is the moment a fork begins,
/// and `CONTRIBUTING.md` treats it as one.
#[derive(Default, Clone)]
pub struct Deployment {
    /// Where credentials are verified, when they are not verified against the store.
    ///
    /// This is the one seam port the reference server reads today, because it is the one that
    /// cannot be answered any other way: a deployment whose credentials are minted elsewhere and
    /// verified offline has no row in `handoff_principals` to look up. The rest of
    /// [`handoff_core::seam`] is driven by the deployment's own binary rather than from inside a
    /// request, so it needs no field here.
    pub authenticator: Option<Arc<dyn CallerAuthenticator>>,
}

/// The running server.
pub struct AppState {
    /// The store. Every write it performs is one transaction.
    pub store: Arc<PgStore>,
    /// What this deployment was told.
    pub config: Config,
    /// What it will accept in a declaration.
    pub profile: DeploymentProfile,
    /// Channels and the default ladder.
    pub channels: ChannelRegistry,
    /// Capability providers.
    pub capabilities: CapabilityRegistry,
    /// The seam ports this deployment supplies. Empty by default.
    pub deployment: Deployment,
}

impl AppState {
    /// The server's own clock.
    ///
    /// §1.4: a Server MUST use its own clock for all recorded times and MUST NOT accept a
    /// client-supplied `decided_at`. There is deliberately no path by which a request body can
    /// influence this value.
    pub fn now(&self) -> Timestamp {
        Timestamp::from_millis(chrono::Utc::now().timestamp_millis())
            .unwrap_or_else(|| Timestamp::from_millis(0).expect("epoch is representable"))
    }
}

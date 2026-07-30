//! What every handler shares.

use handoff_core::capability::CapabilityRegistry;
use handoff_core::channel::ChannelRegistry;
use handoff_protocol::clock::Timestamp;
use handoff_protocol::requires::DeploymentProfile;
use handoff_store_postgres::PgStore;
use std::sync::Arc;

use crate::config::Config;

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

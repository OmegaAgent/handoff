//! The Handoff reference server, as a library.
//!
//! The `handoffd` binary is a thin wrapper over this crate that wires the default ports. A
//! deployment that needs different ports — its own authentication, its own delivery fleet, its own
//! receipt signer — depends on this crate, supplies its own implementations, and starts the same
//! server. That is the only supported way to build on the reference server: replacing a port, never
//! forking the crate.
//!
//! Consumers pin this crate **by published version or tag**. A `[patch]` entry or a path dependency
//! pointing at a modified copy is the moment a fork begins, and `CONTRIBUTING.md` treats it as one.
//!
//! # What is where
//!
//! - [`routes`] is the `/v1` surface, and it is thin: anything that decides lives in
//!   `handoff-core`, anything that writes lives in the store inside a transaction.
//! - [`wire`] builds every response body by hand, so that `null` and absent stay different answers
//!   to different questions.
//! - [`workers`] runs the sweep and the callback pusher, and [`delivery`] runs the delivery
//!   worker. None of them decides anything; all of them call store methods that commit a state and
//!   its event together (I12).
//! - [`cli`] holds the maintenance subcommands, including the open chain verifier.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

pub mod cli;
pub mod config;
pub mod delivery;
pub mod http;
/// Error reporting, behind a feature that is off by default so the open build cannot phone home.
#[cfg(feature = "sentry")]
pub mod observability;
pub mod routes;
pub mod state;
pub mod wire;
pub mod workers;

use handoff_protocol::error::Result;
use std::sync::Arc;

/// Version of this crate, as published to crates.io.
pub const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// What a running server reports about itself.
///
/// A deployment's core version being observable from outside is what makes it checkable that a
/// hosted service is running the open core rather than a private variant. See `GOVERNANCE.md`,
/// "What 'released' means".
pub fn version_line() -> String {
    format!(
        "handoffd {CRATE_VERSION} (core {}, protocol {})",
        handoff_core::CRATE_VERSION,
        handoff_protocol::PROTOCOL_VERSION
    )
}

/// Build the store and the shared state from configuration, with no seam ports supplied.
pub async fn build(config: config::Config) -> Result<Arc<state::AppState>> {
    build_with(config, state::Deployment::default()).await
}

/// Build the same server with a deployment's own seam ports.
///
/// This is the supported way to build a hosted deployment on top of the reference server: depend on
/// this crate at a published version, supply implementations of the ports you need, and start the
/// same binary. Anything that cannot be expressed this way belongs upstream as a new port, never in
/// a private copy of this crate.
pub async fn build_with(
    config: config::Config,
    deployment: state::Deployment,
) -> Result<Arc<state::AppState>> {
    let store = handoff_store_postgres::PgStore::connect_with(
        &config.database_url,
        config.max_connections,
        config.deployment_profile(),
        config.auth_policy(),
        config.channels(),
        config.capabilities(),
    )
    .await?;

    if let Some(path) = &config.bootstrap_file {
        let seeded = config::seed(store.pool(), path).await?;
        tracing::info!(seeded, path, "credentials seeded");
    }

    Ok(Arc::new(state::AppState {
        profile: config.deployment_profile(),
        channels: config.channels(),
        capabilities: config.capabilities(),
        store: Arc::new(store),
        config,
        deployment,
    }))
}

/// Serve until the process is asked to stop.
pub async fn serve(state: Arc<state::AppState>) -> Result<()> {
    let bind = state.config.bind.clone();
    tokio::spawn(workers::sweep_loop(Arc::clone(&state)));
    tokio::spawn(workers::callback_loop(Arc::clone(&state)));
    tokio::spawn(delivery::delivery_loop(
        Arc::clone(&state),
        delivery::Adapters::new(delivery::shipped_adapters()),
    ));

    let app = routes::router(Arc::clone(&state));
    let listener = tokio::net::TcpListener::bind(&bind).await.map_err(|e| {
        handoff_protocol::error::ProtocolError::new(
            handoff_protocol::error::ErrorCode::InvalidRequest,
            format!("cannot bind {bind}: {e}"),
        )
    })?;
    tracing::info!(bind = %bind, "{}", version_line());
    axum::serve(listener, app).await.map_err(|e| {
        handoff_protocol::error::ProtocolError::new(
            handoff_protocol::error::ErrorCode::InvalidRequest,
            format!("the server stopped: {e}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_line_names_every_layer() {
        let line = version_line();
        assert!(line.contains(CRATE_VERSION));
        assert!(line.contains(handoff_core::CRATE_VERSION));
        assert!(line.contains(handoff_protocol::PROTOCOL_VERSION));
    }
}

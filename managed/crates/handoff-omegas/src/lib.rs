//! The Ωmegas adapter — the entire closed surface of the managed Handoff service.
//!
//! **CLOSED SOURCE.** Nothing in this crate may be relicensed, published, or moved into `core/`.
//!
//! # What this is
//!
//! The managed service is the **open** `handoff-server`, plus this crate. There is no private
//! variant of the protocol, no managed-only state machine, and no second implementation of anything
//! the spec describes. Every type this crate exports is an implementation of a port declared in
//! [`handoff_core::seam`], and [`main`](../handoff_omegas_server/index.html) wires them into the
//! same server a self-hoster runs.
//!
//! That is the whole design, and the reason for it is the failure mode it avoids: the company
//! builds against the closed thing, the open thing becomes a stale marketing artifact, and the
//! community notices within about two release cycles. Four mechanisms guard against it, and the
//! out-of-repo decision upgrades three of them from discipline to structure:
//!
//! 1. **Version-pinned dependency, no path dep, no fork, no `[patch]`.** In `handoff-cloud` the core
//!    is pinned by *tag*. A `[patch.crates-io]` entry for a `handoff-*` crate is the moment the fork
//!    begins — [`no_patch_section_exists`](crate::tests) makes that a build failure rather than a
//!    rule someone has to remember.
//! 2. **Managed adds no protocol behaviour.** Every item here implements an open port or surrounds
//!    one. A managed feature needing a new state, a new terminal, or a new wire field goes in
//!    `spec/` first. With the closed surface this small, a new behaviour has nowhere to hide.
//! 3. **The open conformance suite gates the managed deploy.** `handoff-conformance` takes a base
//!    URL, so it runs against `handoffd` in the open repo's CI *and* against this service in
//!    staging. This is the one mechanism the architecture does not enforce for us.
//! 4. **Dogfood over the public surface.** Operator calls Handoff through the public API using the
//!    open SDK — now forced rather than chosen, since Operator lives in a repo this service does
//!    not.
//!
//! # The two contradictions
//!
//! Both are consequences of an out-of-repo decision meeting designs written for an in-repo product.
//! Neither is papered over; each is handled in the module that has to live with it.
//!
//! - **Machine auth as specified cannot serve an out-of-repo Handoff.** See [`auth`], which
//!   implements the recommended client-credentials exchange and documents the open decision.
//! - **"Do not build a second audit table" cannot hold literally.** See [`events`] for the argument
//!   and [`reconciler`] plus [`outbox`] for the shape: Handoff's store is the system of record, and
//!   the org-level log gets a derived summary from a durable outbox, retried until acked.
//!
//! # What is real and what is fake-backed
//!
//! Most of the control plane this consumes **does not exist yet**. Machine auth is M5, entitlements
//! are M4, and per-person contact records, an attestation key, and a revocable viewer token have no
//! owner at all. Nothing here simulates them. Every adapter with an absent dependency fails closed
//! through [`dependency::MissingDependency`], which names the surface and the milestone; every
//! adapter is tested against [`control_plane::FakeControlPlane`], which behaves the way the contract
//! says the real one will.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

pub mod auth;
pub mod config;
pub mod control_plane;
pub mod delivery;
pub mod dependency;
pub mod directory;
pub mod events;
pub mod meter;
pub mod outbox;
pub mod reconciler;
pub mod signer;
pub mod takeover;
pub mod tenancy;

#[cfg(test)]
mod fixtures;
#[cfg(test)]
mod integration;

use std::sync::Arc;

use handoff_protocol::error::Result;

/// Version of this adapter.
pub const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// What this deployment reports about itself.
///
/// The open core's version being observable from outside is what makes it checkable that the hosted
/// service is running the open core rather than a private variant. If it drifts more than one minor
/// release behind the latest published tag, the strategy is failing — visibly, to everyone,
/// including us.
pub fn version_line() -> String {
    format!(
        "handoff-omegas {CRATE_VERSION} ({})",
        handoff_server::version_line()
    )
}

/// Everything the managed deployment builds, in one place.
pub struct Managed {
    /// The control-plane client every adapter shares.
    pub control: Arc<control_plane::ControlPlane>,
    /// The audit mirror.
    pub events: Arc<events::OmegasEvents>,
    /// The usage meter.
    pub meter: Arc<meter::OmegasMeter>,
    /// The org directory.
    pub directory: Arc<directory::OmegasDirectory>,
    /// The durable queue behind the two sinks.
    pub outbox: Arc<outbox::Outbox>,
}

impl Managed {
    /// Assemble the adapters against a control-plane transport and the managed database.
    pub async fn assemble(
        settings: &config::OmegasConfig,
        transport: Box<dyn control_plane::Transport>,
        pool: sqlx::PgPool,
    ) -> Result<Self> {
        let control = Arc::new(control_plane::ControlPlane::new(transport));
        Ok(Self {
            events: Arc::new(events::OmegasEvents::new(Arc::clone(&control))),
            meter: Arc::new(meter::OmegasMeter::new(Arc::clone(&control))),
            directory: Arc::new(directory::OmegasDirectory::new(
                Arc::clone(&control),
                settings.contact_points_available,
            )),
            outbox: Arc::new(outbox::Outbox::open(pool).await?),
            control,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The anti-rot rule, as a test rather than as a convention.
    ///
    /// A `[patch]` entry redirecting a `handoff-*` crate is how a fork starts: the managed build
    /// silently compiles against a modified core while every manifest still claims the published
    /// one. Grepping for it is cheap and the failure is unambiguous.
    #[test]
    fn no_patch_section_exists() {
        let manifest = include_str!("../../../Cargo.toml");
        for line in manifest.lines().map(str::trim) {
            assert!(
                !line.starts_with("[patch"),
                "a [patch] section redirects a dependency without changing what the manifest \
                 claims. If the managed service needs a change to the core, it lands upstream and \
                 this workspace bumps its pin."
            );
        }
    }

    /// The boundary, as a test.
    ///
    /// Closed-by-default with per-crate opt-in is the boundary made mechanical: opening something
    /// has to be a visible diff that overrides both of these.
    #[test]
    fn this_workspace_is_closed_and_unpublishable() {
        let manifest = include_str!("../../../Cargo.toml");
        assert!(manifest.contains(r#"license = "UNLICENSED""#));
        assert!(manifest.contains("publish = false"));
    }

    #[test]
    fn the_version_line_names_the_open_core_it_is_running() {
        let line = version_line();
        assert!(line.contains(CRATE_VERSION));
        assert!(line.contains(handoff_core::CRATE_VERSION));
        assert!(line.contains(handoff_protocol::PROTOCOL_VERSION));
    }
}

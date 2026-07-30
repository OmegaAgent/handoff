//! The Handoff engine and the port traits a deployment implements.
//!
//! [`handoff-protocol`] decides what is correct; this crate makes it happen against the outside
//! world. Everything external sits behind a port trait — the store, the clock, delivery channels,
//! the recipient directory, the receipt signer and verifier, caller authentication, tenancy,
//! metering, callback dispatch. Each port has a default that is *correct* on one operator's own
//! machine; a hosted deployment supplies an implementation that is *better operated*, never one
//! that is more correct.
//!
//! Two rules constrain what may be added here, both from `GOVERNANCE.md`:
//!
//! - The receipt **verifier is always open and always a pure function**. A receipt only its issuer
//!   can verify is a vendor claim rather than evidence.
//! - This crate contains **no dormant gate**: no disabled feature flag, no entitlement check, no
//!   paywall waiting to be switched on. A gate here is a boundary violation.
//!
//! The engine knows nothing about organizations. Tenancy is an opaque reference that defaults to a
//! single tenant, so a self-hosted deployment carries no vestigial multi-tenant machinery.
//!
//! # How the pieces fit
//!
//! - [`auth`] holds the requester ≠ decider rule, enforced by principal **type** (§4.2, I15).
//! - [`plan`] holds the decisions as **pure functions**: what a receipt says, whether an answer
//!   settles anything, what an expiry mints. They take a snapshot and return rows, so the rules
//!   that matter most are decidable in a unit test rather than only against a database.
//! - [`ports`] holds the store, and its shape is a consequence of I12: every method that changes
//!   state is one transaction named for the transition it performs, and there is no method that
//!   writes a state without its event.
//! - [`channel`] and [`capability`] are registries, never matches. Adding a channel or a capability
//!   provider is an entry in a map and zero branches anywhere else (§7.4, §11.1).
//!
//! [`handoff-protocol`]: https://docs.rs/handoff-protocol

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

pub use handoff_protocol as protocol;

pub mod auth;
pub mod capability;
pub mod channel;
pub mod events;
pub mod ids;
pub mod model;
pub mod plan;
pub mod ports;

/// Version of this crate, as published to crates.io.
pub const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_version_is_populated() {
        assert!(!CRATE_VERSION.is_empty());
    }
}

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
//! # Status
//!
//! Skeleton only. The ports and the engine land in milestone H2, driven by the conformance suite
//! written in H1.
//!
//! [`handoff-protocol`]: https://docs.rs/handoff-protocol

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

pub use handoff_protocol as protocol;

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

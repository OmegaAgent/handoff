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
//! # Status
//!
//! Skeleton only. Routes and wiring land in milestone H2, when the conformance suite goes green.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

/// Version of this crate, as published to crates.io.
pub const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// What a running server reports about itself.
///
/// A deployment's core version being observable from outside is what makes it checkable that a
/// hosted service is running the open core rather than a private variant. See `GOVERNANCE.md`,
/// "What 'released' means".
pub fn version_line() -> String {
    format!(
        "handoffd {CRATE_VERSION} (core {})",
        handoff_core::CRATE_VERSION
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_line_names_both_versions() {
        let line = version_line();
        assert!(line.contains(CRATE_VERSION));
        assert!(line.contains(handoff_core::CRATE_VERSION));
    }
}

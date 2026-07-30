//! Pure protocol types, the handoff state machine, and policy evaluation.
//!
//! This crate is the executable half of [`spec/`]. It holds the request and receipt types, the
//! state machine that moves a request from raised to receipted, and the deterministic evaluation
//! of an escalation policy against a clock. It performs **no I/O**, spawns no tasks, and depends
//! on no async runtime, so an implementer can reason about protocol correctness without standing
//! up a database or a delivery channel.
//!
//! Anything that talks to the outside world belongs in [`handoff-core`] behind a port, not here.
//!
//! # Status
//!
//! Skeleton only. The types and the state machine land in milestone H1 alongside the conformance
//! suite, so that every behaviour in this crate arrives with a case that pins it. See
//! `CONTRIBUTING.md` for why a behaviour change without a conformance case is not merged.
//!
//! [`spec/`]: https://github.com/OmegaAgent/handoff/tree/main/spec
//! [`handoff-core`]: https://docs.rs/handoff-core

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

/// Version of this crate, as published to crates.io.
///
/// Reported by the reference server's version endpoint so a deployment's running core version is
/// observable from outside. See `GOVERNANCE.md`, "What 'released' means".
pub const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_version_is_populated() {
        assert!(!CRATE_VERSION.is_empty());
    }
}

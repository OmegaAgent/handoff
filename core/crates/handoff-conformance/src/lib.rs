//! The Handoff conformance suite.
//!
//! Takes a base URL and runs the declarative cases in `conformance/cases/` against whatever is
//! listening there. It does not care whether that is the reference server, someone else's
//! implementation, or a hosted service — which is the whole point, and the reason this crate is the
//! project's governance instrument rather than a test helper.
//!
//! Three things follow from that, all of them policy rather than implementation detail:
//!
//! - **"Handoff-compatible" means a published passing run** for a stated version. Not a badge, not
//!   a claim. See `TRADEMARKS.md`.
//! - **A behaviour change in the core without a case here is not merged.** This is what stops the
//!   suite lagging the implementation, which is the normal way a conformance suite dies.
//! - **The suite gates deploys, including ours.** A hosted service that cannot pass the open suite
//!   turns the build red. That converts "we did not fork the core" from an intention into a check
//!   anyone can rerun.
//!
//! # Status
//!
//! Skeleton only. The cases land in milestone H1, where they are all expected to fail against a
//! stub, and go green in H2. A suite that passes before the server exists would be measuring
//! nothing.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

/// Version of this crate, as published to crates.io.
pub const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Number of conformance cases currently implemented.
///
/// Zero until H1. The CI job that runs the suite fails while this is zero, so a green pipeline
/// never implies conformance that has not been measured.
pub const CASE_COUNT: usize = 0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_version_is_populated() {
        assert!(!CRATE_VERSION.is_empty());
    }
}

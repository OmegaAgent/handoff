//! Delivery channel adapters: how a request actually reaches a person.
//!
//! One crate, feature-gated per channel, so a deployment compiles in only what it will use. An
//! operator supplies their own credentials for whichever channels they enable.
//!
//! **A channel declares capabilities; the engine routes on what a request requires.** Can this
//! channel carry a rich action, capture free text, interrupt someone, survive being ignored? The
//! engine reads the answers and decides. There is no match over channel names anywhere in the
//! core, and adding a provider must never add a branch outside this crate — an adapter that
//! requires one is not finished.
//!
//! What is *not* here, and cannot be: sender reputation, a warmed IP, a marketplace-reviewed app,
//! a phone number with carrier history. Those are operational assets rather than code, and a fresh
//! deployment starts at zero on all of them. The adapter code being open does not make
//! deliverability open, and this crate should not imply otherwise.
//!
//! # Status
//!
//! Skeleton only. Channels land in milestone H4. No channel may hardcode a single global
//! destination: in the prior art a global recipient meant every tenant's alert paged one person,
//! and the conformance suite treats that shape as a defect.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

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

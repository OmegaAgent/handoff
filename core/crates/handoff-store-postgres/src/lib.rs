//! Reference Postgres implementation of the Handoff store, with its own migration set.
//!
//! Holds requests, delivery attempts, durable waits, and receipts. It is the reference
//! implementation of the store port, not the only permitted one — the conformance suite decides
//! whether an alternative store is correct.
//!
//! Two structural commitments:
//!
//! - **Own tables, own migrations, no foreign key into anyone else's schema.** A deployment that
//!   also runs other software joins to it by an opaque tenant key and nothing else. This is what
//!   lets Handoff's schema evolve without wedging an unrelated service's boot, and it is what makes
//!   self-hosting possible at all.
//! - **A durable wait survives process death.** "The wait outlives a `kill -9`" is decidable with
//!   one Postgres and a terminal, and the conformance suite decides it that way.
//!
//! # Layout
//!
//! [`migrations`] holds the nine migrations, embedded and applied at startup. [`store`] holds the
//! transactions. There is no query builder and no ORM: every statement is written out, because the
//! two properties this store exists to guarantee — the tenant predicate and the state-conditional
//! write — are properties of the SQL, and hiding the SQL hides them.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

pub mod delivery;
pub mod migrations;
pub mod store;

pub use store::PgStore;

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

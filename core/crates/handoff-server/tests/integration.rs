//! The integration tests, in one binary.
//!
//! They share a harness that starts a real `handoffd` against a real, disposable Postgres. One
//! binary rather than several so that every helper the harness exposes is exercised by something —
//! a shared test module split across binaries reports most of itself as dead code in each one, and
//! silencing that would hide the case where a helper really has stopped being used.

#[path = "suite/harness/mod.rs"]
mod harness;

#[path = "suite/callbacks.rs"]
mod callbacks;

#[path = "suite/deliveries.rs"]
mod deliveries;

#[path = "suite/durability.rs"]
mod durability;

#[path = "suite/isolation.rs"]
mod isolation;

#[path = "suite/transitions.rs"]
mod transitions;

#[path = "suite/chain.rs"]
mod chain;

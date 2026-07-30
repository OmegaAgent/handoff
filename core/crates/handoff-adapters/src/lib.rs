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
//! # What each adapter actually does
//!
//! Stated plainly, because a channel that looks wired and is not is worse than an absent one:
//!
//! | Adapter | Feature | Status |
//! |---|---|---|
//! | `inapp` | always on | **Real.** Needs no external system: dispatching is making the request listable at its own URL, which this deployment already does |
//! | `webhook` | `webhook` | **Real.** Signs and POSTs to the address the directory holds for that person |
//! | `memory` | `memory` | **Real, and sends nothing.** An in-memory recorder for tests |
//! | `email` | `email` | **Scaffold.** Declares its grades, refuses every send with `channel_not_configured`, and transmits nothing |
//! | `slack` | `slack` | **Scaffold.** As above |
//! | `voice` | `voice` | **Scaffold.** As above |
//!
//! A scaffold suppresses rather than fails, because §7.1 admits suppression as a real, visible
//! outcome while a failure invites a retry that could never succeed.
//!
//! # No global destination
//!
//! No adapter here holds an address. Every send is handed a
//! [`Recipient`](handoff_core::seam::Recipient) resolved from the deployment's directory, and there
//! is no constructor argument, environment variable, or default anywhere in this crate that could
//! name a destination. In the prior art a single global recipient meant every tenant's alert
//! paged one person, and the shape has no legitimate use — so it is absent structurally rather
//! than forbidden by convention.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

use handoff_core::seam::DeliveryChannel;
use std::sync::Arc;

#[cfg(feature = "email")]
pub mod email;
pub mod inapp;
#[cfg(feature = "memory")]
pub mod memory;
#[cfg(feature = "slack")]
pub mod slack;
#[cfg(feature = "voice")]
pub mod voice;
#[cfg(feature = "webhook")]
pub mod webhook;

#[cfg(any(feature = "email", feature = "slack", feature = "voice"))]
mod scaffold;

#[cfg(test)]
mod fixtures;

/// Version of this crate, as published to crates.io.
pub const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Every adapter this build compiled in, except those needing a runtime the caller owns.
///
/// The list is assembled from the enabled features, so what a deployment can route to and what it
/// linked are the same fact. A channel named in a ladder with no adapter behind it is not an error
/// at startup: the delivery is minted and then **suppressed with `channel_unknown`**, which is
/// visible in the delivery log where an operator can act on it.
///
/// The `webhook` adapter is not here because it needs an HTTP client the deployment configures,
/// and `memory` is not because a fake channel that appeared by default could silently swallow real
/// deliveries. Both are added explicitly by whoever wants them.
pub fn default_adapters() -> Vec<Arc<dyn DeliveryChannel>> {
    compiled_channels(Vec::new())
}

/// Add whatever the enabled features compiled in.
///
/// Split out so the feature gates read as one list. Taking and returning the vector rather than
/// building one here keeps the gates the only thing in the function, which is what makes adding a
/// channel a one-line edit in a place a reader can find.
fn compiled_channels(mut adapters: Vec<Arc<dyn DeliveryChannel>>) -> Vec<Arc<dyn DeliveryChannel>> {
    // Not feature-gated, and deliberately. The in-app surface is this deployment's own API rather
    // than a provider it might not have, so a build with no channels at all is a build that cannot
    // reach anyone — which is never what somebody meant to ask for.
    adapters.push(Arc::new(inapp::InApp::new()));
    #[cfg(feature = "email")]
    adapters.push(Arc::new(email::Email::new()));
    #[cfg(feature = "slack")]
    adapters.push(Arc::new(slack::Slack::new()));
    #[cfg(feature = "voice")]
    adapters.push(Arc::new(voice::Voice::new()));
    adapters
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_version_is_populated() {
        assert!(!CRATE_VERSION.is_empty());
    }

    #[test]
    fn every_compiled_adapter_declares_a_distinct_name() {
        let mut names: Vec<String> = default_adapters()
            .iter()
            .map(|adapter| adapter.name().to_string())
            .collect();
        let before = names.len();
        names.sort();
        names.dedup();
        assert_eq!(before, names.len(), "two adapters claimed the same channel");
    }

    #[test]
    fn no_adapter_in_the_default_set_can_hold_a_destination() {
        // The structural check behind the paragraph above: every adapter here is constructible
        // with no arguments, so there is nowhere for a global recipient to be configured. If this
        // stops compiling because an adapter grew a constructor parameter, that parameter is the
        // thing to question.
        for adapter in default_adapters() {
            assert!(!adapter.name().is_empty());
        }
    }

    #[test]
    fn the_fake_never_appears_on_its_own() {
        // A build that compiled in `memory` still must not route to it by accident: a channel that
        // records and discards looks identical to one that works, right up until nobody is paged.
        assert!(
            !default_adapters()
                .iter()
                .any(|adapter| adapter.name() == "memory"),
            "the in-memory recorder must be registered deliberately or not at all"
        );
    }
}

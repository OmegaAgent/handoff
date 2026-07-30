//! Voice. **Scaffold: declares its grades, transmits nothing.**
//!
//! The one channel where getting this wrong wakes somebody up. It is deliberately the strictest
//! declaration in the crate: a phone call reaches an endpoint, and a person answering a phone is
//! not an authenticated principal. §7.2 therefore caps it at `delivered`, and §4.7 means whatever
//! is said on the call is at most a **provisional** answer that settles nothing.
//!
//! There is no number in this file, and nowhere to put one. A predecessor system carried a single
//! deployment-wide destination, so every tenant's page-out rang the platform owner's phone. That is
//! not a configuration mistake to warn about; it is a shape to make unrepresentable, and the
//! address for a voice delivery can only ever arrive on the resolved
//! [`Recipient`](handoff_core::seam::Recipient)'s own contact points.

use handoff_core::ports::BoxFuture;
use handoff_core::seam::{DeliveryChannel, DeliveryReport, Envelope};
use handoff_protocol::delivery::ChannelCapabilities;
use handoff_protocol::error::Result;

use crate::scaffold::{Scaffold, CARRIES_A_NOTICE};

/// The voice channel.
#[derive(Debug, Clone)]
pub struct Voice(Scaffold);

impl Voice {
    /// The channel name this adapter registers under.
    pub const NAME: &'static str = "voice";

    /// Construct it.
    pub fn new() -> Self {
        Self(Scaffold::new(
            Self::NAME,
            CARRIES_A_NOTICE,
            "a telephony credential, and a per-person number held in the directory rather than in \
             this deployment's configuration",
        ))
    }
}

impl Default for Voice {
    fn default() -> Self {
        Self::new()
    }
}

impl DeliveryChannel for Voice {
    fn name(&self) -> &str {
        self.0.name()
    }

    fn capabilities(&self) -> ChannelCapabilities {
        self.0.capabilities()
    }

    fn deliver(&self, envelope: Envelope) -> BoxFuture<'_, Result<DeliveryReport>> {
        self.0.deliver(envelope)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use handoff_protocol::delivery::DeliveryGrade;

    #[test]
    fn a_phone_call_is_never_consent() {
        let capabilities = Voice::new().capabilities();
        assert_eq!(
            capabilities.max_grade,
            DeliveryGrade::Delivered,
            "§7.2 — a Server MUST NOT record a grade above a channel's declared max_grade"
        );
        assert!(!capabilities.can_authenticate_person);
    }

    #[tokio::test]
    async fn the_reason_points_an_operator_at_the_directory_not_at_a_config_file() {
        // The regression guard for the global-number defect: whatever else changes here, the
        // remedy this adapter names must stay "per-person, in the directory".
        let envelope = fixtures::envelope(Voice::NAME, fixtures::recipient(Voice::NAME, None));
        let report = Voice::new().deliver(envelope).await.expect("it answers");
        let detail = report.detail.expect("a suppression says why");
        assert!(detail.contains("directory"), "{detail}");
        assert!(!detail.contains("configuration file"));
    }
}

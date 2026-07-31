//! Slack. **Scaffold: declares its grades, transmits nothing.**
//!
//! What is missing is not the API call. It is a reviewed marketplace app, an installed workspace,
//! and the per-workspace token that comes from that install. A chat message also cannot say *who*
//! read it, which is why this channel tops out at `delivered` however it is implemented, and why a
//! ladder rung that ends in a room rather than a person is a broadcast (§7.5).

use handoff_core::ports::BoxFuture;
use handoff_core::seam::{DeliveryChannel, DeliveryReport, Envelope};
use handoff_protocol::delivery::ChannelCapabilities;
use handoff_protocol::error::Result;

use crate::scaffold::{Scaffold, CARRIES_A_NOTICE};

/// The Slack channel.
#[derive(Debug, Clone)]
pub struct Slack(Scaffold);

impl Slack {
    /// The channel name this adapter registers under.
    pub const NAME: &'static str = "slack";

    /// Construct it.
    pub fn new() -> Self {
        Self(Scaffold::new(
            Self::NAME,
            CARRIES_A_NOTICE,
            "an installed workspace app and its per-workspace bot token",
        ))
    }
}

impl Default for Slack {
    fn default() -> Self {
        Self::new()
    }
}

impl DeliveryChannel for Slack {
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

    #[test]
    fn a_chat_message_cannot_say_who_read_it() {
        assert!(!Slack::new().capabilities().can_authenticate_person);
    }
}

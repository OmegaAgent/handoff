//! Email. **Scaffold: declares its grades, transmits nothing.**
//!
//! Email is where the gap between open code and working delivery is widest. The protocol part is
//! small; the part that decides whether a message arrives is a warmed sending domain, SPF, DKIM,
//! DMARC alignment, and a reputation nobody can hand you. Shipping a wired adapter here would
//! misrepresent what a fresh deployment can do on its first day.

use handoff_core::ports::BoxFuture;
use handoff_core::seam::{DeliveryChannel, DeliveryReport, Envelope};
use handoff_protocol::delivery::ChannelCapabilities;
use handoff_protocol::error::Result;

use crate::scaffold::{Scaffold, CARRIES_A_NOTICE};

/// The email channel.
#[derive(Debug, Clone)]
pub struct Email(Scaffold);

impl Email {
    /// The channel name this adapter registers under.
    pub const NAME: &'static str = "email";

    /// Construct it.
    pub fn new() -> Self {
        Self(Scaffold::new(
            Self::NAME,
            CARRIES_A_NOTICE,
            "a sending credential and a domain with SPF, DKIM, and DMARC aligned",
        ))
    }
}

impl Default for Email {
    fn default() -> Self {
        Self::new()
    }
}

impl DeliveryChannel for Email {
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
    use handoff_protocol::delivery::DeliveryGrade;

    #[test]
    fn email_declares_delivered_at_most_and_cannot_identify_a_person() {
        let capabilities = Email::new().capabilities();
        assert_eq!(capabilities.max_grade, DeliveryGrade::Delivered);
        assert!(
            !capabilities.can_authenticate_person,
            "a reply to an email is not an authenticated decision (§4.7)"
        );
    }
}

//! The shape a channel has before its credentials exist.
//!
//! Three channels in this crate — email, chat, voice — declare what they could prove and cannot
//! yet send, because none of them works without an operational asset this repository cannot ship:
//! a warmed sending domain, a reviewed marketplace app, a phone number with carrier history.
//!
//! One mechanism rather than three copies. A scaffold hand-written per channel would be the same
//! per-item duplication the crate documentation warns about, and it would let the three drift into
//! claiming different things about the same absence.
//!
//! It **suppresses** rather than fails. §7.1 makes suppression a real, visible outcome; a transport
//! failure invites a retry that could never succeed and burns the delivery's attempt budget
//! pretending. The delivery ends in `suppressed` with a reason naming what an operator would have
//! to supply, and no grade is ever recorded — a scaffold that returned `dispatched` would put
//! evidence in a receipt for a message that does not exist.

use handoff_core::outbound::Suppression;
use handoff_core::ports::BoxFuture;
use handoff_core::seam::{DeliveryChannel, DeliveryReport, Envelope};
use handoff_protocol::delivery::{ChannelCapabilities, DeliveryGrade};
use handoff_protocol::error::Result;

/// A channel that declares its grades and transmits nothing.
#[derive(Debug, Clone)]
pub(crate) struct Scaffold {
    name: &'static str,
    capabilities: ChannelCapabilities,
    needs: &'static str,
}

impl Scaffold {
    pub(crate) const fn new(
        name: &'static str,
        capabilities: ChannelCapabilities,
        needs: &'static str,
    ) -> Self {
        Self {
            name,
            capabilities,
            needs,
        }
    }

    /// Why this channel is withholding everything.
    pub(crate) fn why(&self) -> Suppression {
        Suppression::NotConfigured {
            needs: self.needs.to_string(),
        }
    }
}

impl DeliveryChannel for Scaffold {
    fn name(&self) -> &str {
        self.name
    }

    fn capabilities(&self) -> ChannelCapabilities {
        self.capabilities
    }

    fn deliver(&self, _envelope: Envelope) -> BoxFuture<'_, Result<DeliveryReport>> {
        Box::pin(async move { Ok(self.why().report()) })
    }
}

/// Every scaffolded channel tops out at `delivered` and cannot say who received it.
///
/// §7.2 and §4.7: a channel that cannot authenticate a person may deliver and may collect an
/// intent, but produces a **provisional** answer only and MUST NOT settle a request. Declaring this
/// at the ceiling rather than at the moment of use is what stops a phone call being treated as
/// consent later.
pub(crate) const CARRIES_A_NOTICE: ChannelCapabilities = ChannelCapabilities {
    max_grade: DeliveryGrade::Delivered,
    can_authenticate_person: false,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use handoff_core::outbound::Suppression;
    use handoff_protocol::delivery::DeliveryState;

    #[tokio::test]
    async fn a_scaffold_suppresses_and_says_what_is_missing() {
        let scaffold = Scaffold::new("email", CARRIES_A_NOTICE, "a verified sending domain");
        let envelope = fixtures::envelope("email", fixtures::recipient("email", Some("d@e.test")));

        let report = scaffold
            .deliver(envelope)
            .await
            .expect("a scaffold answers");
        assert_eq!(report.state, DeliveryState::Suppressed);
        let detail = report.detail.expect("a suppression says why");
        assert_eq!(Suppression::code_of(&detail), "channel_not_configured");
        assert!(
            detail.contains("a verified sending domain"),
            "the reason must name what an operator has to supply"
        );
        assert!(!report.retryable, "no retry can conjure a credential");
    }

    #[test]
    fn a_scaffold_never_claims_it_can_authenticate_a_person() {
        const { assert!(!CARRIES_A_NOTICE.can_authenticate_person) };
        assert_eq!(CARRIES_A_NOTICE.max_grade, DeliveryGrade::Delivered);
    }
}

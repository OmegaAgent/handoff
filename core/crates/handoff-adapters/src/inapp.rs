//! The in-app surface: the one channel that needs no external system.
//!
//! Dispatching here means the request is listable and answerable at its canonical URL, which I4
//! already requires of every `pending` request. So this adapter transmits nothing and still makes
//! a truthful `dispatched` claim: our transport — the deployment's own API — accepted it.
//!
//! It is also the only channel in this crate that can reach `acted`, because it is the only one
//! where the person authenticates on the surface they answer through (§7.2, §4.7). Every other
//! channel carries a notice and a locator; the decision happens here.

use handoff_core::ports::BoxFuture;
use handoff_core::seam::{DeliveryChannel, DeliveryReport, Envelope};
use handoff_protocol::delivery::ChannelCapabilities;
use handoff_protocol::error::Result;

/// The in-app request surface.
#[derive(Debug, Clone, Copy, Default)]
pub struct InApp;

impl InApp {
    /// The channel name this adapter registers under.
    pub const NAME: &'static str = "inapp";

    /// Construct it. There is nothing to configure, and nothing to point somewhere.
    pub fn new() -> Self {
        Self
    }
}

impl DeliveryChannel for InApp {
    fn name(&self) -> &str {
        Self::NAME
    }

    fn capabilities(&self) -> ChannelCapabilities {
        // The person opens the surface and authenticates there, so this channel can prove every
        // grade — and is the only one here that may carry an answer.
        ChannelCapabilities::IN_APP
    }

    fn deliver(&self, envelope: Envelope) -> BoxFuture<'_, Result<DeliveryReport>> {
        Box::pin(async move {
            // `dispatched`, and no further. The request being listable is our transport accepting
            // it, not the person having seen anything. `seen` and `acted` are recorded later, when
            // the person actually opens the surface and answers — observed rather than assumed
            // (§7.2). Recording `delivered` here would be inventing evidence out of an API call.
            Ok(DeliveryReport {
                detail: Some(format!("listed at {}", envelope.surface_url)),
                ..DeliveryReport::dispatched()
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use handoff_protocol::delivery::{DeliveryGrade, DeliveryState};

    #[test]
    fn it_is_the_only_channel_here_that_can_carry_an_answer() {
        let capabilities = InApp::new().capabilities();
        assert_eq!(capabilities.max_grade, DeliveryGrade::Acted);
        assert!(capabilities.can_authenticate_person);
    }

    #[tokio::test]
    async fn dispatching_claims_only_that_the_request_is_listed() {
        // No address is ever needed: the surface belongs to this deployment, which is exactly why
        // this channel works on an operator's laptop on its first day.
        let envelope = fixtures::envelope(InApp::NAME, fixtures::recipient(InApp::NAME, None));
        let report = InApp::new().deliver(envelope).await.expect("it dispatches");

        assert_eq!(report.state, DeliveryState::Dispatched);
        assert_eq!(report.grade, Some(DeliveryGrade::Dispatched));
        assert!(report
            .detail
            .expect("it says where")
            .contains("https://example.test/requests/"));
    }
}

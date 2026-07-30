//! An in-memory channel: real code, no transport.
//!
//! Every delivery is recorded and nothing leaves the process, which is what a test needs in order
//! to assert on what the engine *tried to send* rather than on what a network did. Outcomes can be
//! scripted, so a test can drive the retry, bounce, and suppression edges of §7.1 without a flaky
//! dependency.
//!
//! It is not registered by default. A fake channel that appeared automatically could swallow a real
//! deployment's deliveries while every dashboard showed `dispatched`.

use handoff_core::ports::BoxFuture;
use handoff_core::seam::{DeliveryChannel, DeliveryReport, Envelope};
use handoff_protocol::delivery::{ChannelCapabilities, DeliveryGrade};
use handoff_protocol::error::Result;
use std::collections::VecDeque;
use std::sync::Mutex;

/// A channel that records instead of transmitting.
#[derive(Debug)]
pub struct Memory {
    name: String,
    capabilities: ChannelCapabilities,
    sent: Mutex<Vec<Envelope>>,
    scripted: Mutex<VecDeque<DeliveryReport>>,
}

impl Memory {
    /// A channel that accepts everything, under a name of your choosing.
    ///
    /// Defaults to the capabilities of a notice-carrying channel — `delivered` at most, and unable
    /// to say who received it — because that is the honest default for anything that is not the
    /// in-app surface, and a test needing more should have to say so.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            capabilities: ChannelCapabilities {
                max_grade: DeliveryGrade::Delivered,
                can_authenticate_person: false,
            },
            sent: Mutex::new(Vec::new()),
            scripted: Mutex::new(VecDeque::new()),
        }
    }

    /// Declare different capabilities, to exercise the ceiling rules of §7.2.
    pub fn with_capabilities(mut self, capabilities: ChannelCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// Queue the reports the next deliveries will produce, in order. Once the script runs out,
    /// every further delivery is `dispatched`.
    pub fn script(&self, reports: impl IntoIterator<Item = DeliveryReport>) {
        let mut scripted = self.scripted.lock().expect("the script mutex");
        scripted.clear();
        scripted.extend(reports);
    }

    /// Everything this channel was asked to send, oldest first.
    pub fn sent(&self) -> Vec<Envelope> {
        self.sent.lock().expect("the log mutex").clone()
    }

    /// How many deliveries it has seen.
    pub fn count(&self) -> usize {
        self.sent.lock().expect("the log mutex").len()
    }
}

impl DeliveryChannel for Memory {
    fn name(&self) -> &str {
        &self.name
    }

    fn capabilities(&self) -> ChannelCapabilities {
        self.capabilities
    }

    fn deliver(&self, envelope: Envelope) -> BoxFuture<'_, Result<DeliveryReport>> {
        Box::pin(async move {
            self.sent.lock().expect("the log mutex").push(envelope);
            Ok(self
                .scripted
                .lock()
                .expect("the script mutex")
                .pop_front()
                .unwrap_or_else(DeliveryReport::dispatched))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use handoff_core::outbound::Suppression;
    use handoff_protocol::delivery::DeliveryState;

    #[tokio::test]
    async fn it_records_what_it_was_asked_to_send() {
        let channel = Memory::new("memory");
        let report = channel
            .deliver(fixtures::envelope(
                "memory",
                fixtures::recipient("memory", Some("d@e.test")),
            ))
            .await
            .expect("it answers");

        assert_eq!(report.grade, Some(DeliveryGrade::Dispatched));
        let sent = channel.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].prompt.title, "Approve the release?");
        assert_eq!(sent[0].rung, 0);
    }

    #[tokio::test]
    async fn a_script_drives_the_failure_edges_and_then_runs_out() {
        let channel = Memory::new("memory");
        channel.script([
            DeliveryReport::failed("503 from the provider", true),
            Suppression::QuietHours.report(),
        ]);
        let envelope =
            || fixtures::envelope("memory", fixtures::recipient("memory", Some("d@e.test")));

        let retrying = channel.deliver(envelope()).await.expect("it answers");
        assert_eq!(retrying.state, DeliveryState::Retrying);
        assert!(retrying.retryable);

        let suppressed = channel.deliver(envelope()).await.expect("it answers");
        assert_eq!(suppressed.state, DeliveryState::Suppressed);

        let accepted = channel.deliver(envelope()).await.expect("it answers");
        assert_eq!(
            accepted.state,
            DeliveryState::Dispatched,
            "past the end of the script it dispatches"
        );
        assert_eq!(channel.count(), 3);
    }

    #[test]
    fn its_default_ceiling_is_the_honest_one_for_a_channel_that_is_not_the_surface() {
        let capabilities = Memory::new("memory").capabilities();
        assert_eq!(capabilities.max_grade, DeliveryGrade::Delivered);
        assert!(!capabilities.can_authenticate_person);
    }
}

//! `DeliveryChannel` — the managed fleet, and the one channel it refuses to ship.
//!
//! Deliverability is the clearest thing the hosted tier actually sells, and it is not code. It is a
//! Slack app that has passed marketplace review, an SES identity out of the sandbox with DKIM and
//! DMARC aligned on a domain with history, phone numbers with carrier relationships, and warmed IPs.
//! A fresh self-hosted deployment starts at zero on every one of them. **The adapter code being open
//! does not make deliverability open**, which is why these adapters are thin and the assets behind
//! them are the product.
//!
//! # Voice is refused, and this is the point
//!
//! The in-repo pager uses **one global destination number**, so every tenant's page rings the
//! platform owner's phone. The exec plan calls that a functional blocker and bans shipping
//! per-tenant paging on it. That ban is implemented here rather than remembered:
//! [`ManagedChannel::new`] refuses to construct any channel whose transport declares a global
//! destination, and there is no flag that overrides it.
//!
//! This is not the same kind of refusal as the rest of this crate. The others say "the dependency
//! does not exist yet". This one says "the dependency exists and using it would page the wrong
//! person" — a defect we would be shipping, not a gap we are waiting on.
//!
//! # What a credential-less adapter does
//!
//! Nothing is faked. A transport with no credential reports the delivery **suppressed** with the
//! stable code [`handoff_core::outbound::Suppression::NoAdapter`]'s sibling reasoning: the attempt
//! is recorded, visible, and countable, and no request is failed. §7.3 is explicit that a channel
//! outage must not end up inside the caller's agent.

use handoff_core::ports::BoxFuture;
use handoff_core::seam::{DeliveryChannel, DeliveryReport, Envelope};
use handoff_protocol::delivery::{ChannelCapabilities, DeliveryGrade};
use handoff_protocol::error::{ErrorCode, ProtocolError, Result};

/// How a channel's messages actually leave the building.
pub trait ChannelTransport: Send + Sync {
    /// Whether this transport addresses each recipient individually.
    ///
    /// `false` means it has one destination for everybody, which is the shape that pages the
    /// platform owner for every tenant. [`ManagedChannel::new`] refuses it.
    fn addresses_each_recipient(&self) -> bool;

    /// Whether the credentials and the operational assets are actually in place.
    fn credentialed(&self) -> bool;

    /// Send one message.
    fn send(&self, envelope: &Envelope) -> BoxFuture<'_, Result<DeliveryReport>>;
}

/// One managed channel.
///
/// `Debug` names the channel and its declaration and deliberately says nothing about the transport,
/// which is where the credentials are.
pub struct ManagedChannel {
    name: String,
    capabilities: ChannelCapabilities,
    transport: Box<dyn ChannelTransport>,
}

impl ManagedChannel {
    /// Build a channel, refusing the two shapes that must never reach production.
    pub fn new(
        name: impl Into<String>,
        capabilities: ChannelCapabilities,
        transport: Box<dyn ChannelTransport>,
    ) -> Result<Self> {
        let name = name.into();
        if !transport.addresses_each_recipient() {
            return Err(ProtocolError::new(
                ErrorCode::InvalidRequest,
                format!(
                    "the `{name}` transport has a single global destination, so every tenant's \
                     page would reach the same person. Per-tenant paging must not ship on a global \
                     destination; give the transport a per-recipient address first."
                ),
            ));
        }
        // A channel that cannot establish who received it may not claim a grade above `delivered`
        // (§7.2), and a grade above that is what turns a phone call into consent. Checking at
        // construction means no adapter can quietly declare otherwise.
        if !capabilities.can_authenticate_person
            && capabilities.max_grade > DeliveryGrade::Delivered
        {
            return Err(ProtocolError::new(
                ErrorCode::InvalidRequest,
                format!(
                    "the `{name}` channel cannot authenticate a person, so it must not declare a \
                     grade above `delivered`"
                ),
            ));
        }
        Ok(Self {
            name,
            capabilities,
            transport,
        })
    }
}

impl std::fmt::Debug for ManagedChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ManagedChannel")
            .field("name", &self.name)
            .field("capabilities", &self.capabilities)
            .finish_non_exhaustive()
    }
}

impl DeliveryChannel for ManagedChannel {
    fn name(&self) -> &str {
        &self.name
    }

    fn capabilities(&self) -> ChannelCapabilities {
        self.capabilities
    }

    fn deliver(&self, envelope: Envelope) -> BoxFuture<'_, Result<DeliveryReport>> {
        Box::pin(async move {
            if !self.transport.credentialed() {
                // Recorded, visible, countable — and the request survives.
                return Ok(DeliveryReport::suppressed(format!(
                    "no_credential: the managed {} channel has no credential in this deployment",
                    self.name
                )));
            }
            let report = self.transport.send(&envelope).await?;
            // Clamp rather than trust. A grade is evidence, and evidence that grades itself is not
            // evidence.
            Ok(DeliveryReport {
                grade: report.grade.min(self.capabilities.max_grade),
                ..report
            })
        })
    }
}

/// The channels the managed fleet is meant to operate, with what each can honestly prove.
///
/// This is a declaration, not an implementation: none of the transports exist in this tree, and
/// none of the operational assets behind them can be checked in. It is here so that the fleet is
/// enumerable and so that the voice entry's absence is visible rather than implied.
pub fn fleet_capabilities() -> Vec<(&'static str, ChannelCapabilities)> {
    vec![
        (
            // The person opens the request surface and authenticates there, which is the only way
            // a channel can carry an answer at all (§4.7).
            "inapp",
            ChannelCapabilities::IN_APP,
        ),
        (
            "email",
            ChannelCapabilities {
                max_grade: DeliveryGrade::Delivered,
                can_authenticate_person: false,
            },
        ),
        (
            "chat",
            ChannelCapabilities {
                max_grade: DeliveryGrade::Delivered,
                can_authenticate_person: false,
            },
        ),
        (
            "push",
            ChannelCapabilities {
                max_grade: DeliveryGrade::Delivered,
                can_authenticate_person: false,
            },
        ),
        // `voice` is deliberately absent. See the module documentation: the only transport we have
        // pages one global number, and shipping per-tenant paging on it is banned.
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;

    struct Transport {
        per_recipient: bool,
        credentialed: bool,
        claims: DeliveryGrade,
    }

    impl ChannelTransport for Transport {
        fn addresses_each_recipient(&self) -> bool {
            self.per_recipient
        }
        fn credentialed(&self) -> bool {
            self.credentialed
        }
        fn send(&self, _envelope: &Envelope) -> BoxFuture<'_, Result<DeliveryReport>> {
            let grade = self.claims;
            Box::pin(async move {
                Ok(DeliveryReport {
                    grade,
                    ..DeliveryReport::dispatched()
                })
            })
        }
    }

    fn transport(per_recipient: bool, credentialed: bool, claims: DeliveryGrade) -> Box<Transport> {
        Box::new(Transport {
            per_recipient,
            credentialed,
            claims,
        })
    }

    fn delivered_only() -> ChannelCapabilities {
        ChannelCapabilities {
            max_grade: DeliveryGrade::Delivered,
            can_authenticate_person: false,
        }
    }

    #[test]
    fn a_transport_with_one_global_destination_cannot_be_built() {
        // The prior art's actual behaviour: every tenant's wall paged the platform owner's phone.
        let error = ManagedChannel::new(
            "voice",
            delivered_only(),
            transport(false, true, DeliveryGrade::Delivered),
        )
        .expect_err("per-tenant paging must not ship on a global destination");
        assert!(error.message.contains("single global destination"));
        assert!(error.message.contains("per-recipient address"));
    }

    #[test]
    fn the_managed_fleet_does_not_list_voice() {
        let names: Vec<&str> = fleet_capabilities().iter().map(|(n, _)| *n).collect();
        assert_eq!(names, vec!["inapp", "email", "chat", "push"]);
    }

    #[test]
    fn a_channel_that_cannot_identify_a_person_may_not_declare_that_it_can_prove_they_acted() {
        let error = ManagedChannel::new(
            "voice-ish",
            ChannelCapabilities {
                max_grade: DeliveryGrade::Acted,
                can_authenticate_person: false,
            },
            transport(true, true, DeliveryGrade::Acted),
        )
        .expect_err("a phone call is not consent");
        assert!(error
            .message
            .contains("must not declare a grade above `delivered`"));
    }

    #[tokio::test]
    async fn an_adapter_that_over_reports_its_grade_is_clamped_rather_than_believed() {
        let channel = ManagedChannel::new(
            "email",
            delivered_only(),
            transport(true, true, DeliveryGrade::Acted),
        )
        .expect("build");
        let report = channel
            .deliver(fixtures::envelope("email"))
            .await
            .expect("deliver");
        assert_eq!(report.grade, DeliveryGrade::Delivered);
    }

    #[tokio::test]
    async fn a_channel_with_no_credential_suppresses_visibly_and_does_not_fail_the_request() {
        // §7.3: a channel outage must not end up inside the caller's agent.
        let channel = ManagedChannel::new(
            "email",
            delivered_only(),
            transport(true, false, DeliveryGrade::Delivered),
        )
        .expect("build");
        let report = channel
            .deliver(fixtures::envelope("email"))
            .await
            .expect("no error");
        assert_eq!(
            report.state,
            handoff_protocol::delivery::DeliveryState::Suppressed
        );
        assert!(report.detail.expect("detail").contains("no_credential"));
    }
}

//! An outbound webhook: **real**, and the one adapter here that transmits.
//!
//! It POSTs a signed notice to the address the directory holds for that person, using the same
//! HMAC-SHA-256 scheme `signing.md` §1 specifies for callbacks. One scheme rather than two: a
//! receiver that already verifies Handoff callbacks verifies these with the same nine lines, and
//! there is no second canonical string to get subtly wrong.
//!
//! What travels is a title, a locator, and identifiers. §15.6 and I8/I18 forbid a delivery from
//! carrying a capability handle's resolved address, a bearer URL, or a `secret` value, and there is
//! no field in the body below that could hold one. Nothing in the notice is a credential, so a
//! receiver holding it still cannot answer the request: the person authenticates at the surface.
//!
//! It has no address of its own. The endpoint comes from the recipient's own contact points on
//! every send, so a deployment-wide destination cannot be configured here even by mistake.

use handoff_core::outbound::Suppression;
use handoff_core::ports::BoxFuture;
use handoff_core::seam::{DeliveryChannel, DeliveryReport, Envelope};
use handoff_core::signing;
use handoff_protocol::delivery::{ChannelCapabilities, DeliveryGrade};
use handoff_protocol::error::Result;

/// A signed HTTP POST, per person.
#[derive(Debug, Clone)]
pub struct Webhook {
    name: String,
    client: reqwest::Client,
    secrets: Vec<String>,
}

impl Webhook {
    /// The channel name this adapter registers under by default.
    pub const NAME: &'static str = "webhook";

    /// Construct it with the signing secrets this deployment issued.
    ///
    /// Two may be active at once, and while they are, every request carries both as separate `v1=`
    /// elements — rotation is an overlap, not a cutover (`signing.md` §1.4).
    pub fn new(client: reqwest::Client, secrets: Vec<String>) -> Self {
        Self {
            name: Self::NAME.to_string(),
            client,
            secrets,
        }
    }

    /// Register under a different channel name, for a deployment whose ladder calls it something
    /// else. The name is data; nothing in the core switches on it.
    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// The endpoint for this person on this channel, if the directory holds one.
    fn address(&self, envelope: &Envelope) -> Option<String> {
        envelope
            .recipient
            .contacts
            .iter()
            .find(|contact| contact.channel == self.name && !contact.address.is_empty())
            .map(|contact| contact.address.clone())
    }
}

impl DeliveryChannel for Webhook {
    fn name(&self) -> &str {
        &self.name
    }

    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            // A 2xx from an endpoint is that endpoint accepting bytes. It is not a person, and
            // §7.2 is explicit that `dispatched` is not evidence anybody received anything.
            max_grade: DeliveryGrade::Dispatched,
            can_authenticate_person: false,
        }
    }

    fn deliver(&self, envelope: Envelope) -> BoxFuture<'_, Result<DeliveryReport>> {
        Box::pin(async move {
            // Both checks withhold rather than fail: neither a missing endpoint nor a missing
            // secret is something a retry could fix, and recording either as a transport failure
            // would spend the delivery's attempt budget pretending otherwise (§7.1).
            let Some(address) = self.address(&envelope) else {
                return Ok(Suppression::NoAddress.report());
            };
            if self.secrets.is_empty() {
                return Ok(Suppression::NotConfigured {
                    needs: "at least one signing secret; an unsigned webhook is one a receiver \
                            cannot tell from an attacker's"
                        .into(),
                }
                .report());
            }

            // Identifiers and typed values only.
            let body = serde_json::json!({
                "type": "handoff.delivery",
                "delivery_id": envelope.delivery_id.to_string(),
                "request_id": envelope.request_id.to_string(),
                "channel": envelope.channel,
                "rung": envelope.rung,
                "title": envelope.prompt.title,
                "surface_url": envelope.surface_url,
            })
            .to_string();

            // Signed over the exact bytes about to be transmitted, never over a re-serialization.
            // The timestamp is this sender's own clock; the freshness window of `signing.md` §1.3
            // is enforced by the receiver against theirs.
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|since| since.as_secs() as i64)
                .unwrap_or_default();
            let delivery_id = envelope.delivery_id.to_string();
            let signature = signing::sign(&self.secrets, timestamp, &delivery_id, body.as_bytes());

            let response = self
                .client
                .post(&address)
                .header("Content-Type", "application/json")
                .header("Handoff-Signature", signature)
                .header("Handoff-Delivery", &delivery_id)
                .header("Handoff-Version", signing::SIGNATURE_VERSION)
                // The delivery identifier, stable across this delivery's attempts, so a receiver
                // can dedupe a retry without parsing the body (`signing.md` §1.3 rule 7).
                .header("Handoff-Idempotency-Key", &delivery_id)
                .body(body)
                .send()
                .await;

            Ok(match response {
                Ok(response) => {
                    let status = response.status();
                    if status.is_success() {
                        DeliveryReport {
                            detail: Some(format!("the endpoint returned {status}")),
                            ..DeliveryReport::dispatched()
                        }
                    } else if status.is_server_error() || status.as_u16() == 429 {
                        DeliveryReport::failed(format!("the endpoint returned {status}"), true)
                    } else {
                        DeliveryReport::failed(format!("the endpoint refused it: {status}"), false)
                    }
                }
                Err(error) if error.is_timeout() => {
                    DeliveryReport::failed("the endpoint did not respond in time", true)
                }
                Err(error) if error.is_connect() => {
                    DeliveryReport::failed(format!("cannot reach the endpoint: {error}"), true)
                }
                Err(error) => {
                    DeliveryReport::failed(format!("the request could not be made: {error}"), false)
                }
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use handoff_protocol::delivery::DeliveryState;

    fn webhook() -> Webhook {
        Webhook::new(
            reqwest::Client::new(),
            vec!["whsec_2f8a91c4e7b3d05a6c1e9f47b28d3a05".into()],
        )
    }

    #[tokio::test]
    async fn a_person_with_no_endpoint_is_suppressed_rather_than_attempted() {
        let envelope = fixtures::envelope(Webhook::NAME, fixtures::recipient(Webhook::NAME, None));
        let report = webhook().deliver(envelope).await.expect("it answers");

        assert_eq!(report.state, DeliveryState::Suppressed);
        assert_eq!(
            Suppression::code_of(&report.detail.expect("it says why")),
            "no_address"
        );
    }

    #[tokio::test]
    async fn an_unsigned_deployment_withholds_rather_than_sending_in_the_clear() {
        let unsigned = Webhook::new(reqwest::Client::new(), Vec::new());
        let envelope = fixtures::envelope(
            Webhook::NAME,
            fixtures::recipient(Webhook::NAME, Some("https://receiver.example.test/hook")),
        );
        let report = unsigned.deliver(envelope).await.expect("it answers");

        assert_eq!(report.state, DeliveryState::Suppressed);
        assert_eq!(
            Suppression::code_of(&report.detail.expect("it says why")),
            "channel_not_configured"
        );
    }

    #[tokio::test]
    async fn an_address_on_another_channel_is_not_this_channel_s_address() {
        // The directory holds one contact point per channel. Reaching for somebody's email address
        // because the webhook one is missing is how a notice ends up somewhere nobody chose.
        let envelope = fixtures::envelope(
            Webhook::NAME,
            fixtures::recipient("email", Some("dana@example.test")),
        );
        let report = webhook().deliver(envelope).await.expect("it answers");
        assert_eq!(report.state, DeliveryState::Suppressed);
    }

    #[test]
    fn a_2xx_is_dispatched_and_never_more_than_dispatched() {
        // The ceiling is the point: this channel cannot report that a person received anything,
        // so its declaration must stop the engine recording that they did (§7.2).
        let capabilities = webhook().capabilities();
        assert_eq!(capabilities.max_grade, DeliveryGrade::Dispatched);
        assert!(!capabilities.can_authenticate_person);
    }

    #[test]
    fn the_channel_name_is_data_not_a_branch() {
        assert_eq!(webhook().named("pager").name(), "pager");
    }
}

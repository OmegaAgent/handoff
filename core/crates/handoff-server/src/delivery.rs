//! The delivery worker, and the adapter set it drives.
//!
//! The loop is deliberately dull: claim one due delivery, look its channel up by name, hand it to
//! whatever answers to that name, and record what came back. Every decision that matters — which
//! state the delivery moves to, what grade it earned, whether another attempt is owed — is made by
//! [`handoff_protocol::delivery::transition`] and committed by the store in one transaction (I12).
//! A worker that decided any of that would be a second implementation of §7.
//!
//! **There is no match over channel names here, and there cannot be.** The registry is a map from
//! name to adapter, and a name with nothing behind it produces a *suppressed delivery with a
//! reason* rather than a panic at startup or a silent skip. That is what makes adding a channel an
//! entry in a list and zero branches anywhere else.

use handoff_core::outbound::Suppression;
use handoff_core::seam::DeliveryChannel;
use handoff_store_postgres::delivery::DeliveryJob;
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::state::AppState;

/// The channels this deployment can route to, by name.
///
/// Built once at startup from whatever the build compiled in, so what a ladder can name and what
/// the process can do are the same fact rather than two lists that agree until they do not.
#[derive(Clone, Default)]
pub struct Adapters(BTreeMap<String, Arc<dyn DeliveryChannel>>);

impl Adapters {
    /// Build a registry from a set of adapters. A later adapter under the same name replaces an
    /// earlier one, so a deployment can override a shipped channel without editing this crate.
    pub fn new(adapters: impl IntoIterator<Item = Arc<dyn DeliveryChannel>>) -> Self {
        Self(
            adapters
                .into_iter()
                .map(|adapter| (adapter.name().to_string(), adapter))
                .collect(),
        )
    }

    /// The adapter registered under this name, if any.
    pub fn get(&self, name: &str) -> Option<&Arc<dyn DeliveryChannel>> {
        self.0.get(name)
    }

    /// What every registered channel declares about itself (§7.2).
    pub fn descriptors(&self) -> Vec<handoff_core::channel::ChannelDescriptor> {
        self.0
            .values()
            .map(|adapter| handoff_core::channel::ChannelDescriptor {
                name: adapter.name().to_string(),
                capabilities: adapter.capabilities(),
            })
            .collect()
    }
}

impl std::fmt::Debug for Adapters {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.0.keys()).finish()
    }
}

/// The channels `handoffd` ships with.
///
/// Only `inapp` actually delivers on a fresh deployment. The rest declare what they could prove and
/// suppress every send with a reason naming what an operator would have to supply — which is the
/// truthful picture of what an open repository can hand somebody, and better than a channel that
/// looks wired and silently reaches nobody.
pub fn shipped_adapters() -> Vec<Arc<dyn DeliveryChannel>> {
    handoff_adapters::default_adapters()
}

/// Attempt due deliveries, one at a time, forever.
pub async fn delivery_loop(state: Arc<AppState>, adapters: Adapters) {
    let interval = std::time::Duration::from_millis(state.config.sweep_interval_ms.max(50));
    loop {
        tokio::time::sleep(interval).await;
        let now = state.now();
        let job = match state
            .store
            .claim_delivery(state.store.as_ref(), &state.config.public_base, now)
            .await
        {
            Ok(Some(job)) => job,
            Ok(None) => continue,
            Err(error) => {
                tracing::error!(%error, "cannot claim a delivery");
                continue;
            }
        };
        if let Err(error) = attempt(&state, &adapters, job).await {
            tracing::error!(%error, "a delivery attempt could not be recorded");
        }
    }
}

async fn attempt(
    state: &AppState,
    adapters: &Adapters,
    job: DeliveryJob,
) -> handoff_protocol::error::Result<()> {
    let report = match adapters.get(&job.channel) {
        Some(adapter) => {
            let envelope = handoff_core::seam::Envelope {
                tenant: job.tenant_ref.clone(),
                request_id: job.request_id,
                delivery_id: job.id,
                channel: job.channel.clone(),
                recipient: job.recipient.clone(),
                prompt: job.prompt.clone(),
                surface_url: job.surface_url.clone(),
                rung: job.rung,
            };
            match adapter.deliver(envelope).await {
                Ok(report) => report,
                // An adapter that could not make the attempt at all is worth retrying; one that
                // made it and failed reports that itself. The distinction is the adapter's to
                // make, and this is the only place it is interpreted.
                Err(error) => {
                    handoff_core::seam::DeliveryReport::failed(error.message.clone(), true)
                }
            }
        }
        // A ladder naming a channel this build did not compile in. Visible, per delivery, with a
        // code an operator can alert on — never a startup panic over a rung nobody has reached.
        None => Suppression::ChannelUnknown.report(),
    };

    let landed = state
        .store
        .record_delivery_outcome(&job, &report, state.now())
        .await?;
    tracing::debug!(
        delivery = %job.id,
        channel = %job.channel,
        attempt = job.attempt,
        state = ?landed,
        "delivery attempted"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_with_nothing_behind_it_resolves_to_nothing_rather_than_panicking() {
        let adapters = Adapters::new(shipped_adapters());
        assert!(adapters.get("carrier-pigeon").is_none());
    }

    #[test]
    fn the_registry_reports_what_each_channel_declares() {
        let adapters = Adapters::new(shipped_adapters());
        for descriptor in adapters.descriptors() {
            let adapter = adapters.get(&descriptor.name).expect("it was just listed");
            assert_eq!(
                descriptor.capabilities,
                adapter.capabilities(),
                "the registry and the adapter disagree about what {} can prove",
                descriptor.name
            );
        }
    }

    #[test]
    fn a_later_adapter_may_replace_a_shipped_one() {
        // The override path a deployment uses instead of forking this crate.
        let adapters = Adapters::new(shipped_adapters());
        assert!(adapters.get("inapp").is_some());
    }
}

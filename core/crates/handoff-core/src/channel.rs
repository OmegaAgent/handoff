//! The channel registry, and the routing that reads it.
//!
//! §7.4 states the division: **the Client declares urgency; the Server decides the channel.** So a
//! channel name is an open vocabulary the core carries and looks up — never a value it switches on.
//! Two facts come back from the lookup, and §7.2 makes both mandatory:
//!
//! - `max_grade` — the strongest evidence this channel can ever produce. A Server MUST NOT record a
//!   grade above it, which is what stops a phone call being treated as consent.
//! - `can_authenticate_person` — whether the channel can establish *who* received it. A channel
//!   that cannot, cannot carry an answer (§4.7).

use handoff_protocol::clock::IsoDuration;
use handoff_protocol::delivery::{ChannelCapabilities, DeliveryGrade};
use handoff_protocol::request::{Routing, RoutingRung};
use handoff_protocol::requires::{Target, TargetKind};
use std::collections::BTreeMap;

/// What one channel adapter declares about itself (§7.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelDescriptor {
    /// The name requests and ladders refer to it by.
    pub name: String,
    /// What it can prove.
    pub capabilities: ChannelCapabilities,
}

/// Channels by name, plus the deployment's default ladder.
///
/// The ladder lives here rather than in a request because §7.4 makes it deployment policy:
/// overriding it per request requires a separate scope from raising one, since a key that can ask a
/// question and a key that can page the on-call at 3 a.m. are different blast radiuses.
#[derive(Debug, Clone)]
pub struct ChannelRegistry {
    channels: BTreeMap<String, ChannelCapabilities>,
    default_ladder: Routing,
    fallback: ChannelCapabilities,
}

impl ChannelRegistry {
    /// Build a registry from descriptors and a default ladder.
    pub fn new(descriptors: Vec<ChannelDescriptor>, default_ladder: Routing) -> Self {
        Self {
            channels: descriptors
                .into_iter()
                .map(|d| (d.name, d.capabilities))
                .collect(),
            default_ladder,
            // An unregistered channel is assumed to prove the least, never the most. Guessing
            // upward would let an unknown adapter mint evidence it cannot support.
            fallback: ChannelCapabilities {
                max_grade: DeliveryGrade::Dispatched,
                can_authenticate_person: false,
            },
        }
    }

    /// What a channel can prove. An unregistered name gets the conservative floor.
    pub fn capabilities(&self, channel: &str) -> ChannelCapabilities {
        self.channels.get(channel).copied().unwrap_or(self.fallback)
    }

    /// Every registered channel name.
    pub fn names(&self) -> Vec<String> {
        self.channels.keys().cloned().collect()
    }

    /// The ladder this deployment applies when a request declares none.
    pub fn default_ladder(&self) -> &Routing {
        &self.default_ladder
    }

    /// The ladder that actually applies, snapshotted onto the request at raise time (§7.4).
    pub fn resolve_ladder(&self, declared: Option<&Routing>) -> Routing {
        match declared {
            Some(routing) if !routing.ladder.is_empty() => routing.clone(),
            Some(routing) => Routing {
                targets: if routing.targets.is_empty() {
                    self.default_ladder.targets.clone()
                } else {
                    routing.targets.clone()
                },
                ladder: self.default_ladder.ladder.clone(),
            },
            None => self.default_ladder.clone(),
        }
    }
}

/// The default ladder a deployment starts with: reach people in the app immediately, widen if
/// nobody comes.
pub fn starter_ladder() -> Routing {
    Routing {
        targets: vec![Target {
            kind: TargetKind::Anyone,
            value: "*".into(),
        }],
        ladder: vec![
            RoutingRung {
                after: IsoDuration::from_secs(0),
                channels: vec!["inapp".into()],
                to: None,
            },
            RoutingRung {
                after: IsoDuration::from_secs(5 * 60),
                channels: vec!["email".into()],
                to: None,
            },
            RoutingRung {
                after: IsoDuration::from_secs(15 * 60),
                channels: vec!["chat".into()],
                to: None,
            },
        ],
    }
}

/// The channels the reference server ships with.
///
/// None of them sends anything: this crate has no credentials, no sender reputation, and no
/// carrier history, and pretending otherwise in the open core would misrepresent what a fresh
/// deployment can actually do. What they carry is the **declaration** §7.2 requires, so the engine
/// can route and grade honestly before any adapter is wired.
pub fn starter_channels() -> Vec<ChannelDescriptor> {
    let describe = |name: &str, max_grade, can_authenticate_person| ChannelDescriptor {
        name: name.to_string(),
        capabilities: ChannelCapabilities {
            max_grade,
            can_authenticate_person,
        },
    };
    vec![
        // The person opens the request surface and authenticates there, so this is the only
        // starter channel that can carry an answer.
        describe("inapp", DeliveryGrade::Acted, true),
        describe("push", DeliveryGrade::Delivered, false),
        describe("email", DeliveryGrade::Delivered, false),
        describe("chat", DeliveryGrade::Delivered, false),
        describe("voice", DeliveryGrade::Delivered, false),
    ]
}

/// Which rungs are due at an elapsed time since the raise.
pub fn rungs_due(routing: &Routing, elapsed: IsoDuration) -> Vec<u32> {
    routing
        .ladder
        .iter()
        .enumerate()
        .filter(|(_, rung)| rung.after.as_secs() <= elapsed.as_secs())
        .map(|(i, _)| i as u32)
        .collect()
}

/// The targets a rung addresses: its own override, or the ladder's rung-0 targets.
pub fn rung_targets<'a>(routing: &'a Routing, rung: &'a RoutingRung) -> Vec<Target> {
    match &rung.to {
        Some(target) => vec![target.clone()],
        None if routing.targets.is_empty() => vec![Target {
            kind: TargetKind::Anyone,
            value: "*".into(),
        }],
        None => routing.targets.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unregistered_channel_proves_the_least_not_the_most() {
        let registry = ChannelRegistry::new(starter_channels(), starter_ladder());
        let unknown = registry.capabilities("carrier-pigeon");
        assert_eq!(unknown.max_grade, DeliveryGrade::Dispatched);
        assert!(!unknown.can_authenticate_person);
    }

    #[test]
    fn only_a_channel_that_authenticates_a_person_tops_out_at_acted() {
        let registry = ChannelRegistry::new(starter_channels(), starter_ladder());
        assert_eq!(
            registry.capabilities("inapp").max_grade,
            DeliveryGrade::Acted
        );
        assert_eq!(
            registry.capabilities("voice").max_grade,
            DeliveryGrade::Delivered
        );
        assert!(!registry.capabilities("voice").can_authenticate_person);
    }

    #[test]
    fn a_declared_ladder_wins_and_an_empty_one_falls_back() {
        let registry = ChannelRegistry::new(starter_channels(), starter_ladder());
        let declared = Routing {
            targets: vec![Target {
                kind: TargetKind::Role,
                value: "editor".into(),
            }],
            ladder: vec![RoutingRung {
                after: IsoDuration::from_secs(0),
                channels: vec!["inapp".into()],
                to: None,
            }],
        };
        assert_eq!(registry.resolve_ladder(Some(&declared)).ladder.len(), 1);
        assert_eq!(registry.resolve_ladder(None).ladder.len(), 3);
    }

    #[test]
    fn only_rungs_whose_timer_has_elapsed_are_due() {
        let ladder = starter_ladder();
        assert_eq!(rungs_due(&ladder, IsoDuration::from_secs(0)), vec![0]);
        assert_eq!(
            rungs_due(&ladder, IsoDuration::from_secs(6 * 60)),
            vec![0, 1]
        );
    }
}

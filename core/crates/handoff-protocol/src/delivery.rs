//! The DELIVERY state machine (§7).
//!
//! Delivery is a first-class tracked entity, not a side effect of a notification sweep: "we tried"
//! is a claim that has to survive being questioned. One delivery is one attempt to reach one target
//! on one channel, and a request has many of them — escalation, reminders, and channel fallback all
//! mint deliveries, never requests (I3).
//!
//! The four grades are the point of the machine. `dispatched` means our transport accepted
//! something; it is **not** evidence a person received anything. A protocol that sells receipts
//! must not blur the difference between "the API returned 200" and "a person got it" (§7.2).

use crate::error::{ErrorCode, ProtocolError, Result};
use serde::{Deserialize, Serialize};

/// What a delivery actually proves, ordered from weakest to strongest evidence (§7.2).
///
/// The `Ord` derive is load-bearing: a grade may only ever advance, and a channel may never record
/// one above what its adapter declared it can prove.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryGrade {
    /// Our transport accepted it. **This is not evidence a person received anything.**
    Dispatched,
    /// The channel reports it reached the person's endpoint.
    Delivered,
    /// The person opened the request surface, **authenticated**.
    Seen,
    /// The person answered **through this delivery**. The strongest tier, and the only one that
    /// proves a person acted rather than that a transport accepted something.
    Acted,
}

impl DeliveryGrade {
    /// Every grade, weakest first.
    pub const ALL: &'static [DeliveryGrade] =
        &[Self::Dispatched, Self::Delivered, Self::Seen, Self::Acted];

    /// The state a delivery occupies once it has reached this grade.
    pub const fn state(self) -> DeliveryState {
        match self {
            Self::Dispatched => DeliveryState::Dispatched,
            Self::Delivered => DeliveryState::Delivered,
            Self::Seen => DeliveryState::Seen,
            Self::Acted => DeliveryState::Acted,
        }
    }
}

/// What a channel adapter declares about itself (§7.2, §4.7).
///
/// Both fields are REQUIRED of every adapter. They live on the channel rather than in the core
/// because the core carries channel names and looks adapters up; it never switches on the name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelCapabilities {
    /// The best grade this channel can ever prove. A voice page that cannot authenticate a person
    /// tops out at `delivered`, which stops anyone treating a phone call as consent.
    pub max_grade: DeliveryGrade,
    /// Whether this channel can establish *who* received it (§4.7).
    ///
    /// A channel declaring `false` may deliver and may collect an intent, but produces a
    /// **provisional** answer only and MUST NOT settle a request.
    pub can_authenticate_person: bool,
}

impl ChannelCapabilities {
    /// An in-app surface: authenticated, and able to prove every grade.
    pub const IN_APP: Self = Self {
        max_grade: DeliveryGrade::Acted,
        can_authenticate_person: true,
    };
}

/// The states one delivery moves through (§7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    /// Minted by a ladder rung, not yet handed to a transport.
    Queued,
    /// Withheld by policy — quiet hours, dedupe, or missing consent. **A real outcome, not a
    /// failure**, and it must stay visible: an invisible suppression is indistinguishable from a
    /// bug.
    Suppressed,
    /// Handed to the transport.
    Sending,
    /// Waiting out a backoff before the next send attempt.
    Retrying,
    /// Every attempt was used up.
    Failed,
    /// The transport accepted it.
    Dispatched,
    /// The channel reports it reached the person's endpoint.
    Delivered,
    /// The channel reports it could not be delivered.
    Bounced,
    /// The person opened the request surface, authenticated.
    Seen,
    /// The person answered through this delivery.
    Acted,
    /// The request settled through some other delivery after this one was dispatched.
    Stale,
    /// The request settled before this delivery was dispatched.
    Cancelled,
}

impl DeliveryState {
    /// Every state, in `openapi.yaml` order.
    pub const ALL: &'static [DeliveryState] = &[
        Self::Queued,
        Self::Suppressed,
        Self::Sending,
        Self::Retrying,
        Self::Failed,
        Self::Dispatched,
        Self::Delivered,
        Self::Bounced,
        Self::Seen,
        Self::Acted,
        Self::Stale,
        Self::Cancelled,
    ];

    /// Whether no further transition is possible (§7.1).
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Suppressed
                | Self::Failed
                | Self::Bounced
                | Self::Acted
                | Self::Stale
                | Self::Cancelled
        )
    }

    /// The evidence grade this state represents, if any.
    pub const fn grade(self) -> Option<DeliveryGrade> {
        match self {
            Self::Dispatched => Some(DeliveryGrade::Dispatched),
            Self::Delivered => Some(DeliveryGrade::Delivered),
            Self::Seen => Some(DeliveryGrade::Seen),
            Self::Acted => Some(DeliveryGrade::Acted),
            Self::Queued
            | Self::Suppressed
            | Self::Sending
            | Self::Retrying
            | Self::Failed
            | Self::Bounced
            | Self::Stale
            | Self::Cancelled => None,
        }
    }
}

/// What happens to a delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryEvent {
    /// A ladder rung fired and minted this delivery.
    RungFires,
    /// Policy withheld it: quiet hours, dedupe, or missing consent.
    Suppress,
    /// Hand it to the transport, either the first time or after a backoff.
    StartSend,
    /// Evidence arrived that the delivery reached a stronger grade.
    ///
    /// One event covers all four grades rather than four events, because the rule is a property of
    /// the grade ordering — advance only, never above the channel's ceiling — and writing it once
    /// is what stops a per-grade branch appearing here.
    AdvanceGrade(DeliveryGrade),
    /// The transport failed in a way worth retrying.
    ScheduleRetry,
    /// The retry budget is spent.
    Exhausted,
    /// The channel reports permanent non-delivery.
    Bounce,
    /// The request settled before this delivery was dispatched.
    Cancel,
    /// The request settled elsewhere after this delivery was dispatched.
    MarkStale,
}

/// One accepted move of the delivery machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeliveryTransition {
    /// Where it came from. `None` is the machine's start.
    pub from: Option<DeliveryState>,
    /// Where it went.
    pub to: DeliveryState,
    /// The grade now reached, if any.
    pub grade_reached: Option<DeliveryGrade>,
}

/// Move the delivery machine. A total function: every `(state, event)` pair either yields a
/// transition or a typed error, and nothing panics.
///
/// `from` is `None` for a delivery that does not exist yet.
pub fn transition(
    from: Option<DeliveryState>,
    event: DeliveryEvent,
    channel: &ChannelCapabilities,
) -> Result<DeliveryTransition> {
    let refuse = |why: &str| {
        Err(ProtocolError::new(
            ErrorCode::InvalidRequest,
            match from {
                Some(state) => format!("delivery in `{state:?}` cannot {why}"),
                None => format!("a delivery that does not exist cannot {why}"),
            },
        ))
    };
    let ok = |to: DeliveryState| {
        Ok(DeliveryTransition {
            from,
            to,
            grade_reached: to.grade().or(from.and_then(DeliveryState::grade)),
        })
    };

    // §7.1: terminal delivery states are terminal. Checked once, before any edge is considered, so
    // no edge below can accidentally reopen one.
    if from.is_some_and(DeliveryState::is_terminal) {
        return refuse("move again: it is already terminal");
    }

    match (from, event) {
        (None, DeliveryEvent::RungFires) => ok(DeliveryState::Queued),
        (None, _) => refuse("do anything before its rung has fired"),
        (Some(_), DeliveryEvent::RungFires) => refuse("be minted twice"),

        (Some(DeliveryState::Queued), DeliveryEvent::Suppress) => ok(DeliveryState::Suppressed),
        (Some(_), DeliveryEvent::Suppress) => refuse("be suppressed once it has left the queue"),

        (Some(DeliveryState::Queued | DeliveryState::Retrying), DeliveryEvent::StartSend) => {
            ok(DeliveryState::Sending)
        }
        (Some(_), DeliveryEvent::StartSend) => refuse("start sending from here"),

        (Some(DeliveryState::Sending), DeliveryEvent::ScheduleRetry) => ok(DeliveryState::Retrying),
        (Some(_), DeliveryEvent::ScheduleRetry) => refuse("schedule a retry from here"),

        (Some(DeliveryState::Retrying), DeliveryEvent::Exhausted) => ok(DeliveryState::Failed),
        (Some(_), DeliveryEvent::Exhausted) => refuse("exhaust a retry budget it is not spending"),

        (Some(DeliveryState::Dispatched | DeliveryState::Delivered), DeliveryEvent::Bounce) => {
            ok(DeliveryState::Bounced)
        }
        (Some(_), DeliveryEvent::Bounce) => refuse("bounce before a transport accepted it"),

        // §7.1: cancelled when the request settles before dispatch, stale when it settles after.
        (
            Some(DeliveryState::Queued | DeliveryState::Sending | DeliveryState::Retrying),
            DeliveryEvent::Cancel,
        ) => ok(DeliveryState::Cancelled),
        (Some(_), DeliveryEvent::Cancel) => {
            refuse("be cancelled after dispatch; it goes stale instead")
        }

        (
            Some(DeliveryState::Dispatched | DeliveryState::Delivered | DeliveryState::Seen),
            DeliveryEvent::MarkStale,
        ) => ok(DeliveryState::Stale),
        (Some(_), DeliveryEvent::MarkStale) => {
            refuse("go stale before dispatch; it is cancelled instead")
        }

        (
            Some(
                current @ (DeliveryState::Sending
                | DeliveryState::Dispatched
                | DeliveryState::Delivered
                | DeliveryState::Seen),
            ),
            DeliveryEvent::AdvanceGrade(grade),
        ) => {
            if grade > channel.max_grade {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidRequest,
                    format!(
                        "this channel proves at most `{:?}`; recording `{grade:?}` would claim \
                         evidence it does not have",
                        channel.max_grade
                    ),
                ));
            }
            if grade == DeliveryGrade::Acted && !channel.can_authenticate_person {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidRequest,
                    "a channel that cannot authenticate a person cannot carry an answer",
                ));
            }
            if current.grade().is_some_and(|reached| grade <= reached) {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidRequest,
                    format!(
                        "delivery has already reached `{:?}`; a grade only advances",
                        current.grade().unwrap_or(grade)
                    ),
                ));
            }
            Ok(DeliveryTransition {
                from,
                to: grade.state(),
                grade_reached: Some(grade),
            })
        }
        (Some(_), DeliveryEvent::AdvanceGrade(_)) => {
            refuse("record evidence before it has been handed to a transport")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVENTS: &[DeliveryEvent] = &[
        DeliveryEvent::RungFires,
        DeliveryEvent::Suppress,
        DeliveryEvent::StartSend,
        DeliveryEvent::AdvanceGrade(DeliveryGrade::Dispatched),
        DeliveryEvent::AdvanceGrade(DeliveryGrade::Delivered),
        DeliveryEvent::AdvanceGrade(DeliveryGrade::Seen),
        DeliveryEvent::AdvanceGrade(DeliveryGrade::Acted),
        DeliveryEvent::ScheduleRetry,
        DeliveryEvent::Exhausted,
        DeliveryEvent::Bounce,
        DeliveryEvent::Cancel,
        DeliveryEvent::MarkStale,
    ];

    fn step(from: DeliveryState, event: DeliveryEvent) -> Result<DeliveryState> {
        transition(Some(from), event, &ChannelCapabilities::IN_APP).map(|t| t.to)
    }

    #[test]
    fn the_happy_path_runs_queued_to_acted() {
        let mut state = transition(None, DeliveryEvent::RungFires, &ChannelCapabilities::IN_APP)
            .expect("mint")
            .to;
        assert_eq!(state, DeliveryState::Queued);
        for (event, expected) in [
            (DeliveryEvent::StartSend, DeliveryState::Sending),
            (
                DeliveryEvent::AdvanceGrade(DeliveryGrade::Dispatched),
                DeliveryState::Dispatched,
            ),
            (
                DeliveryEvent::AdvanceGrade(DeliveryGrade::Delivered),
                DeliveryState::Delivered,
            ),
            (
                DeliveryEvent::AdvanceGrade(DeliveryGrade::Seen),
                DeliveryState::Seen,
            ),
            (
                DeliveryEvent::AdvanceGrade(DeliveryGrade::Acted),
                DeliveryState::Acted,
            ),
        ] {
            state = step(state, event).unwrap_or_else(|e| panic!("{event:?} from {state:?}: {e}"));
            assert_eq!(state, expected);
        }
        assert!(state.is_terminal());
    }

    #[test]
    fn a_transport_failure_retries_then_gives_up() {
        let mut state = DeliveryState::Queued;
        state = step(state, DeliveryEvent::StartSend).expect("send");
        state = step(state, DeliveryEvent::ScheduleRetry).expect("retry");
        assert_eq!(state, DeliveryState::Retrying);
        state = step(state, DeliveryEvent::StartSend).expect("send again");
        assert_eq!(state, DeliveryState::Sending);
        state = step(state, DeliveryEvent::ScheduleRetry).expect("retry");
        state = step(state, DeliveryEvent::Exhausted).expect("give up");
        assert_eq!(state, DeliveryState::Failed);
    }

    #[test]
    fn the_machine_is_total_and_never_panics() {
        // Exhaustive rather than sampled: the domain is finite, so nothing is left to chance.
        for &from in DeliveryState::ALL {
            for &event in EVENTS {
                let _ = transition(Some(from), event, &ChannelCapabilities::IN_APP);
            }
        }
        for &event in EVENTS {
            let _ = transition(None, event, &ChannelCapabilities::IN_APP);
        }
    }

    #[test]
    fn no_event_moves_a_terminal_delivery() {
        for &from in DeliveryState::ALL.iter().filter(|s| s.is_terminal()) {
            for &event in EVENTS {
                assert!(
                    transition(Some(from), event, &ChannelCapabilities::IN_APP).is_err(),
                    "{from:?} is terminal but accepted {event:?}"
                );
            }
        }
    }

    #[test]
    fn a_grade_only_ever_advances() {
        for &from in DeliveryState::ALL {
            let Some(reached) = from.grade() else {
                continue;
            };
            for &grade in DeliveryGrade::ALL {
                let result = step(from, DeliveryEvent::AdvanceGrade(grade));
                if grade > reached && !from.is_terminal() {
                    assert_eq!(result.expect("advance"), grade.state());
                } else {
                    assert!(result.is_err(), "{from:?} must not regrade to {grade:?}");
                }
            }
        }
    }

    #[test]
    fn a_grade_may_skip_a_rung_it_never_got_evidence_for() {
        // A person can open the surface without the channel ever reporting `delivered` — email is
        // the ordinary case. See the crate documentation's ambiguity note A-2.
        assert_eq!(
            step(
                DeliveryState::Dispatched,
                DeliveryEvent::AdvanceGrade(DeliveryGrade::Seen)
            )
            .expect("skip"),
            DeliveryState::Seen
        );
    }

    #[test]
    fn a_channel_cannot_record_a_grade_it_cannot_prove() {
        let voice = ChannelCapabilities {
            max_grade: DeliveryGrade::Delivered,
            can_authenticate_person: false,
        };
        let dispatched = transition(
            Some(DeliveryState::Sending),
            DeliveryEvent::AdvanceGrade(DeliveryGrade::Dispatched),
            &voice,
        )
        .expect("a transport accepted it");
        assert_eq!(dispatched.to, DeliveryState::Dispatched);

        for beyond in [DeliveryGrade::Seen, DeliveryGrade::Acted] {
            assert!(
                transition(
                    Some(DeliveryState::Dispatched),
                    DeliveryEvent::AdvanceGrade(beyond),
                    &voice
                )
                .is_err(),
                "a phone call must not be treated as consent"
            );
        }
    }

    #[test]
    fn a_channel_that_cannot_identify_a_person_cannot_carry_an_answer() {
        // Reaches `seen` — someone opened it — but the channel cannot say who, so `acted` is
        // refused and the answer stays provisional (§4.7, C-21).
        let anonymous = ChannelCapabilities {
            max_grade: DeliveryGrade::Acted,
            can_authenticate_person: false,
        };
        assert!(transition(
            Some(DeliveryState::Seen),
            DeliveryEvent::AdvanceGrade(DeliveryGrade::Acted),
            &anonymous
        )
        .is_err());
    }

    #[test]
    fn settling_before_dispatch_cancels_and_settling_after_goes_stale() {
        for pre in [
            DeliveryState::Queued,
            DeliveryState::Sending,
            DeliveryState::Retrying,
        ] {
            assert_eq!(
                step(pre, DeliveryEvent::Cancel).expect("cancel"),
                DeliveryState::Cancelled
            );
            assert!(step(pre, DeliveryEvent::MarkStale).is_err());
        }
        for post in [
            DeliveryState::Dispatched,
            DeliveryState::Delivered,
            DeliveryState::Seen,
        ] {
            assert_eq!(
                step(post, DeliveryEvent::MarkStale).expect("stale"),
                DeliveryState::Stale
            );
            assert!(step(post, DeliveryEvent::Cancel).is_err());
        }
    }

    #[test]
    fn suppression_is_an_outcome_of_the_queue_not_of_a_send() {
        assert_eq!(
            step(DeliveryState::Queued, DeliveryEvent::Suppress).expect("suppress"),
            DeliveryState::Suppressed
        );
        assert!(step(DeliveryState::Sending, DeliveryEvent::Suppress).is_err());
        assert!(
            DeliveryState::Suppressed.is_terminal(),
            "suppression is a real, visible outcome"
        );
    }

    #[test]
    fn states_and_grades_use_the_wire_strings() {
        assert_eq!(
            serde_json::to_value(DeliveryState::Dispatched).expect("ser"),
            "dispatched"
        );
        assert_eq!(
            serde_json::to_value(DeliveryGrade::Acted).expect("ser"),
            "acted"
        );
        assert_eq!(DeliveryState::ALL.len(), 12);
        assert_eq!(DeliveryGrade::ALL.len(), 4);
        // The grade ladder is ordered, and that ordering is what `max_grade` compares against.
        assert!(
            DeliveryGrade::Dispatched < DeliveryGrade::Delivered
                && DeliveryGrade::Delivered < DeliveryGrade::Seen
                && DeliveryGrade::Seen < DeliveryGrade::Acted
        );
    }
}

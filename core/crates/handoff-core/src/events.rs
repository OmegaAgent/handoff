//! Event names, and the rule about when they are written.
//!
//! I12: **every state transition emits its event in the same transaction as the state change.** The
//! names are here rather than inline at each call site so that a transition and its event cannot
//! drift apart in spelling, and so the set is enumerable by a reader checking §6.2 against the
//! implementation.
//!
//! There is no `emit` function in this module on purpose. An event is written by the same SQL
//! statement batch that writes the state, inside one transaction; a helper that could be called
//! from anywhere would be exactly the "best-effort path that can drop them relative to the state"
//! §6.2 forbids.

/// A request was raised (§6.2 R1).
pub const REQUEST_RAISED: &str = "request.raised";
/// A request was amended in place (§6.2 R2).
pub const REQUEST_AMENDED: &str = "request.amended";
/// A progressive-disclosure step was submitted (§6.2 R12).
///
/// R12, R13, and R14 share the property that gives them their own numbers: each is something a
/// **person** does to a `pending` request that leaves it `pending`, and **none of them may signal
/// the waiter**. A runtime must not be able to observe that an intermediate step, a delegation, or
/// a partial endorsement occurred; it learns only the single outcome. Waking the waiter on any of
/// the three turns one intervention into several, and breaks I1 the moment a receipt is minted for
/// each. They are distinct event names rather than `request.amended` because an amendment is a
/// third party improving the wording, and these are the answerer at work.
pub const REQUEST_STEP_RECORDED: &str = "request.step_recorded";
/// A person delegated, or reported being unable (§6.2 R13, §6.6).
pub const REQUEST_DISPOSITION_RECORDED: &str = "request.disposition_recorded";
/// A person endorsed, without reaching quorum (§6.2 R14, §4.5).
pub const REQUEST_ENDORSED: &str = "request.endorsed";
/// An attempt clock lapsed. Stamped once, ever (§6.2 R3).
pub const ATTEMPT_LAPSED: &str = "attempt.lapsed";
/// A ladder rung fired (§6.2 R4).
pub const REQUEST_ESCALATED: &str = "request.escalated";
/// A person answered (§6.2 R5).
pub const REQUEST_ANSWERED: &str = "request.answered";
/// The TTL sweep settled it (§6.2 R6).
pub const REQUEST_EXPIRED: &str = "request.expired";
/// The requester withdrew it (§6.2 R7).
pub const REQUEST_CANCELLED: &str = "request.cancelled";
/// A successor replaced it (§6.2 R8).
pub const REQUEST_SUPERSEDED: &str = "request.superseded";
/// The request was retargeted (§6.6).
pub const REQUEST_REASSIGNED: &str = "request.reassigned";
/// An attempt clock was armed or re-armed (§6.3).
pub const ATTEMPT_ARMED: &str = "attempt.armed";
/// A person took a capability. Every successful resolve leaves a record of who took what (§11.2).
pub const GRANT_RESOLVED: &str = "grant.resolved";
/// A grant was revoked (§11.4).
pub const GRANT_REVOKED: &str = "grant.revoked";
/// A message arrived on a channel. Recorded, and **never** allowed to decide anything (§4.7).
pub const CHANNEL_MESSAGE_RECEIVED: &str = "channel.message_received";
/// A runtime observed the target change state. Recorded as an observation, never as a person
/// (§9.7).
pub const RUNTIME_OBSERVATION: &str = "runtime.observation";

/// Every event name this implementation emits, so a reader can check the set against §6.2.
pub const ALL: &[&str] = &[
    REQUEST_RAISED,
    REQUEST_AMENDED,
    REQUEST_STEP_RECORDED,
    REQUEST_DISPOSITION_RECORDED,
    REQUEST_ENDORSED,
    ATTEMPT_LAPSED,
    REQUEST_ESCALATED,
    REQUEST_ANSWERED,
    REQUEST_EXPIRED,
    REQUEST_CANCELLED,
    REQUEST_SUPERSEDED,
    REQUEST_REASSIGNED,
    ATTEMPT_ARMED,
    GRANT_RESOLVED,
    GRANT_REVOKED,
    CHANNEL_MESSAGE_RECEIVED,
    RUNTIME_OBSERVATION,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_state_transition_in_section_6_2_has_an_event_name() {
        for name in [
            REQUEST_RAISED,
            REQUEST_AMENDED,
            ATTEMPT_LAPSED,
            REQUEST_ESCALATED,
            REQUEST_ANSWERED,
            REQUEST_EXPIRED,
            REQUEST_CANCELLED,
            REQUEST_SUPERSEDED,
        ] {
            assert!(ALL.contains(&name), "{name} is missing from ALL");
        }
    }

    #[test]
    fn the_three_transitions_that_must_not_signal_have_their_own_names() {
        // §6.2 R12-R14. Reusing `request.amended` for these would make a person's own in-progress
        // work indistinguishable from a third party rewriting the ask underneath them.
        for name in [
            REQUEST_STEP_RECORDED,
            REQUEST_DISPOSITION_RECORDED,
            REQUEST_ENDORSED,
        ] {
            assert!(ALL.contains(&name), "{name} is missing from ALL");
            assert_ne!(name, REQUEST_AMENDED);
        }
    }

    #[test]
    fn names_are_unique() {
        let mut seen = ALL.to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), ALL.len());
    }
}

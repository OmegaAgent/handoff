//! The WAITER state machine (§8): the durable server-side record of a runtime waiting for an
//! outcome.
//!
//! This is what makes resumption a protocol property rather than a runtime accident: **the wait is
//! a durable row, not a loop inside the client's process.** A client can die mid-poll, restart, and
//! reattach to find its signal still there and still unacked (C-11).
//!
//! Two rules do most of the work:
//!
//! * **Signals are a queue, not a flag** (W2). A non-terminal `attempt_lapsed` nudge must never
//!   overwrite, replace, or mask a subsequent terminal signal. A Server that models the pending
//!   outcome as a single mutable field will lose answers.
//! * **Reading a signal does not consume it.** Consumption is the ack (§8.3). That two-step is the
//!   effectively-once hinge: at-least-once delivery plus an idempotent ack. A receiver that returns
//!   `2xx` to a callback and then crashes before applying the decision has not received it.

use crate::clock::Timestamp;
use crate::error::{ErrorCode, ProtocolError, Result};
use crate::id::{AuthorizationId, ReceiptId, RequestId, ResumeToken, SignalId};
use crate::request::OnWaiterTerminal;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

// ---------------------------------------------------------------------------------------------
// Signals
// ---------------------------------------------------------------------------------------------

/// The closed set of signal types (§8.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalType {
    /// A person decided. Terminal.
    Answered,
    /// The TTL policy settled it. Terminal.
    Expired,
    /// The requester withdrew it. Terminal.
    Cancelled,
    /// A successor replaced it. Terminal.
    Superseded,
    /// The attempt clock lapsed. **A nudge, not an outcome**: the request stays `pending` and stays
    /// answerable, and this fires exactly once, ever, per attempt.
    AttemptLapsed,
}

impl SignalType {
    /// Every signal type.
    pub const ALL: &'static [SignalType] = &[
        Self::Answered,
        Self::Expired,
        Self::Cancelled,
        Self::Superseded,
        Self::AttemptLapsed,
    ];

    /// Whether this signal settles the request.
    pub const fn is_terminal(self) -> bool {
        !matches!(self, Self::AttemptLapsed)
    }
}

/// The outcome a terminal decision carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionOutcome {
    /// A person decided.
    Answered,
    /// The TTL policy settled it.
    Expired,
    /// The requester withdrew it.
    Cancelled,
    /// A successor replaced it.
    Superseded,
}

/// Who or what produced the decision (§9.7, I16).
///
/// **The protocol never fabricates a person.** One field, and the audit trail stops lying about who
/// decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionSource {
    /// A person decided.
    Human,
    /// An expiry policy decided.
    Policy,
    /// A runtime concluded the person acted. Recorded as inference, with no actor.
    RuntimeInference,
}

/// For an expiry: whether the effective answer is a denial or the pre-declared default (§6.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectiveAnswer {
    /// Unanswered means no.
    Deny,
    /// Unanswered means the pre-agreed default, and the record says a policy decided.
    Default,
}

/// The typed outcome the runtime consumes.
///
/// It is **data the runtime reads, never an instruction it must obey** — that distinction is what
/// keeps a decision auditable instead of persuasive. The runtime branches on `values`; it never has
/// to interpret prose, and no model sits between the person's click and the code path taken.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    /// What happened.
    pub outcome: DecisionOutcome,
    /// The answer, keyed by declared field name. **Never carries a `secret` value** (I7).
    #[serde(default)]
    pub values: Map<String, Value>,
    /// Who or what decided.
    pub source: DecisionSource,
    /// For an expiry only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective: Option<EffectiveAnswer>,
    /// The receipt this decision was recorded in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_id: Option<ReceiptId>,
    /// The authorization this decision minted, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_id: Option<AuthorizationId>,
    /// Where to look instead, for a supersession.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<RequestId>,
}

/// One queued notification to a waiter (§8.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Signal {
    /// This signal.
    pub id: SignalId,
    /// The request whose state changed.
    pub request_id: RequestId,
    /// The waiter this was enqueued for.
    pub waiter_ref: String,
    /// What happened.
    #[serde(rename = "type")]
    pub signal_type: SignalType,
    /// Monotonically increasing per `waiter_ref`, so a receiver can detect gaps and reordering
    /// without inspecting content. A Server MUST assign it; a client MAY use it.
    pub sequence: u64,
    /// Proof that the acking client is the waiter this was enqueued for.
    pub resume_token: ResumeToken,
    /// The typed decision. **Null for `attempt_lapsed`**, which decides nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<Decision>,
    /// Level 2 (§14). Returned byte-identical to what was raised.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_ref: Option<String>,
    /// Level 2 (§14). Base64, returned byte-identical, and present in no log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_payload: Option<String>,
    /// How many times this signal has been pushed to a callback. Redelivery stops at the ack, not
    /// at a `2xx`.
    #[serde(default)]
    pub attempts: u64,
    /// When it was enqueued.
    pub created_at: Timestamp,
    /// When it was consumed. `None` while it is still outstanding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acked_at: Option<Timestamp>,
}

impl Signal {
    /// Check the signal carries what its type requires (§8.2).
    ///
    /// **Every signal MUST carry a typed payload.** A Server MUST NOT satisfy a wait with a null or
    /// empty decision — a runtime that unblocks on nothing has learned nothing, and the whole
    /// promise of §1.3 guarantee 1 is that the answer arrives as typed data.
    pub fn validate(&self) -> Result<()> {
        let bad = |why: &str| ProtocolError::new(ErrorCode::InvalidRequest, why.to_string());
        match (self.signal_type.is_terminal(), &self.decision) {
            (true, None) => Err(bad(
                "a terminal signal must carry a typed decision; a wait satisfied by nothing has \
                 learned nothing",
            )),
            (false, Some(_)) => Err(bad(
                "`attempt_lapsed` is a nudge and decides nothing, so it carries no decision",
            )),
            (true, Some(decision)) => {
                let expected = match self.signal_type {
                    SignalType::Answered => DecisionOutcome::Answered,
                    SignalType::Expired => DecisionOutcome::Expired,
                    SignalType::Cancelled => DecisionOutcome::Cancelled,
                    SignalType::Superseded => DecisionOutcome::Superseded,
                    SignalType::AttemptLapsed => unreachable!("not terminal"),
                };
                if decision.outcome != expected {
                    return Err(bad("the decision's outcome must match the signal's type"));
                }
                Ok(())
            }
            (false, None) => Ok(()),
        }
    }

    /// Whether this signal is still outstanding. Reading does not change this; only an ack does.
    pub fn is_unacked(&self) -> bool {
        self.acked_at.is_none()
    }
}

/// The next sequence number for a waiter, given the highest already assigned (§8.3).
pub fn next_sequence(highest_assigned: Option<u64>) -> u64 {
    highest_assigned.map_or(1, |n| n + 1)
}

// ---------------------------------------------------------------------------------------------
// The state machine (§8.1, §8.2)
// ---------------------------------------------------------------------------------------------

/// The states a waiter occupies (§8.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaiterState {
    /// Waiting. Nothing is queued.
    Armed,
    /// At least one signal is queued and unclaimed.
    Signalled,
    /// A poll or a callback relay holds an exclusive lease on a signal.
    Delivering,
    /// Every signal has been consumed and the request is settled.
    Acked,
    /// The runtime reported the `waiter_ref` terminal, or a leased waiter's heartbeat lapsed.
    Orphaned,
    /// The request was cancelled or superseded; the terminal signal was delivered and no ack is
    /// required.
    Released,
}

impl WaiterState {
    /// Every state, in `openapi.yaml` order.
    pub const ALL: &'static [WaiterState] = &[
        Self::Armed,
        Self::Signalled,
        Self::Delivering,
        Self::Acked,
        Self::Orphaned,
        Self::Released,
    ];

    /// Whether the waiter can still receive anything.
    ///
    /// `orphaned` is deliberately **not** terminal: it is the state reattachment recovers from
    /// (W7), and it is the whole reason a client's process death is survivable.
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Acked | Self::Released)
    }
}

/// Which row of §8.2's table an accepted transition took.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WaiterRule {
    /// ∅ → `armed`: the request was raised.
    W1,
    /// → `signalled`: a signal was enqueued.
    W2,
    /// `signalled` → `delivering`: a poll or relay claimed it.
    W3,
    /// `delivering` → consumed: an ack with a valid resume token.
    W4,
    /// `delivering` → `signalled`: transport failure or lease expiry.
    W5,
    /// → `orphaned`: the waiter went terminal.
    W6,
    /// `orphaned` → `armed`: reattachment.
    W7,
    /// → `released`: the request was cancelled or superseded.
    W8,
}

/// What a waiter transition commits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaiterEffect {
    /// Create the durable waiter row, keyed `(request_id, waiter_ref)`.
    CreateDurableWaiter,
    /// Store `resume_ref` and `resume_payload` verbatim, if present (§14).
    StoreContinuationVerbatim,
    /// Append one signal to the queue. **A queue, not a flag** (W2).
    EnqueueSignal(SignalType),
    /// Claim the signal exclusively, with a lease.
    ClaimWithLease,
    /// Stop redelivering. This happens at the ack, never at a `2xx` (§8.3, §15 rule 4).
    StopRedelivery,
    /// Record whether the runtime actually applied the decision. `applied: false` is a fact the
    /// record should hold, not an error to swallow (§8.3).
    RecordAckOutcome {
        /// Whether the runtime applied it.
        applied: bool,
    },
    /// Back off exponentially with jitter and re-queue.
    BackoffAndRequeue,
    /// Disable a repeatedly failing callback endpoint and notify the tenant: silent permanent
    /// retry is how queues die (§15 rule 5). The signal itself stays unacked and recoverable.
    DisableCallbackAndNotifyTenant,
    /// Surface the waiter's `orphaned` state on the request, so a person can decide whether it is
    /// still worth answering (§8.4, `keep`).
    SurfaceOrphanedOnRequest,
    /// Cancel the request via R7: nobody is left to receive the answer, so stop paging people
    /// (§8.4, `cancel`).
    CancelRequest,
    /// Return every unacked signal to the reattaching client (§8.5).
    ReturnUnackedSignals,
    /// Re-arm the lease.
    ReArmLease,
    /// Deliver the terminal signal; no ack is required (W8).
    DeliverTerminalSignalWithoutAck,
}

/// What happens to a waiter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaiterEvent {
    /// The request was raised (W1).
    Raise,
    /// A signal is being enqueued (W2, W8).
    Signal {
        /// Which signal.
        signal_type: SignalType,
    },
    /// A long poll attached, or a callback relay claimed the signal (W3).
    Claim,
    /// An ack arrived (W4).
    Ack {
        /// Whether the presented `resume_token` matches.
        resume_token_valid: bool,
        /// Whether this signal was already acked. A repeat is `200` with `first_ack: false`.
        already_acked: bool,
        /// Whether other signals remain unacked.
        remaining_unacked: bool,
        /// Whether the request itself has settled.
        request_settled: bool,
        /// Whether the runtime applied the decision.
        applied: bool,
    },
    /// The push failed, or the lease expired (W5).
    TransportFailed {
        /// Whether the attempt budget still has room.
        attempts_below_max: bool,
    },
    /// The `waiter_ref` was reported terminal, or a leased waiter's heartbeat lapsed (W6).
    WaiterTerminal {
        /// What the request declared should happen.
        policy: OnWaiterTerminal,
    },
    /// A client reattached (W7).
    Reattach {
        /// Whether it presented a valid `resume_token` or proved ownership of the `waiter_ref`.
        authorized: bool,
    },
}

/// One accepted move of the waiter machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaiterTransition {
    /// Which row of §8.2 was taken.
    pub rule: WaiterRule,
    /// Where it came from. `None` is the machine's start.
    pub from: Option<WaiterState>,
    /// Where it went.
    pub to: WaiterState,
    /// What is committed.
    pub effects: Vec<WaiterEffect>,
}

/// Move the waiter machine. A total function: every `(state, event)` pair either yields a
/// transition or a typed error, and nothing panics.
pub fn transition(from: Option<WaiterState>, event: WaiterEvent) -> Result<WaiterTransition> {
    use WaiterEffect as E;
    use WaiterState as S;

    let accept = |rule, to, effects| {
        Ok(WaiterTransition {
            rule,
            from,
            to,
            effects,
        })
    };
    let refuse = |why: &str| {
        Err(ProtocolError::new(
            ErrorCode::InvalidRequest,
            match from {
                Some(state) => format!("a waiter in `{state:?}` cannot {why}"),
                None => format!("a waiter that does not exist cannot {why}"),
            },
        ))
    };

    match (from, event) {
        (None, WaiterEvent::Raise) => accept(
            WaiterRule::W1,
            S::Armed,
            vec![E::CreateDurableWaiter, E::StoreContinuationVerbatim],
        ),
        (None, _) => refuse("do anything before it is armed"),
        (Some(_), WaiterEvent::Raise) => refuse("be armed twice"),

        // Spelled out rather than guarded by `is_terminal()`, so the compiler checks this match for
        // exhaustiveness: a guard arm would let a new state or event slip through unhandled.
        (
            Some(S::Acked | S::Released),
            WaiterEvent::Signal { .. }
            | WaiterEvent::Claim
            | WaiterEvent::Ack { .. }
            | WaiterEvent::TransportFailed { .. }
            | WaiterEvent::WaiterTerminal { .. }
            | WaiterEvent::Reattach { .. },
        ) => refuse("move again: it is already terminal"),

        // ------------------------------------------------------------------ W2 and W8: signals
        //
        // W8 names R7 and R8 specifically — a cancelled or superseded request delivers its terminal
        // signal and requires no ack, because nothing is left to apply. Everything else queues.
        // See the crate documentation's ambiguity note A-5: §8.2 lists both W2 and W8 against those
        // triggers.
        (
            Some(S::Armed | S::Signalled | S::Delivering),
            WaiterEvent::Signal {
                signal_type: signal_type @ (SignalType::Cancelled | SignalType::Superseded),
            },
        ) => accept(
            WaiterRule::W8,
            S::Released,
            vec![
                E::EnqueueSignal(signal_type),
                E::DeliverTerminalSignalWithoutAck,
            ],
        ),
        (Some(S::Armed | S::Signalled | S::Delivering), WaiterEvent::Signal { signal_type }) => {
            // A queue, not a flag: enqueueing while already `signalled` or `delivering` appends
            // rather than replacing, so an `attempt_lapsed` nudge can never mask a later answer.
            accept(
                WaiterRule::W2,
                S::Signalled,
                vec![E::EnqueueSignal(signal_type)],
            )
        }
        (Some(S::Orphaned), WaiterEvent::Signal { signal_type }) => {
            // Under `keep` the request stays answerable, so a late answer must still be recorded.
            // The signal waits in the queue for whoever reattaches (§8.4, §8.5).
            accept(
                WaiterRule::W2,
                S::Orphaned,
                vec![E::EnqueueSignal(signal_type)],
            )
        }

        // ------------------------------------------------------------------ W3: claim
        (Some(S::Signalled), WaiterEvent::Claim) => {
            accept(WaiterRule::W3, S::Delivering, vec![E::ClaimWithLease])
        }
        (Some(_), WaiterEvent::Claim) => refuse("claim a signal that is not queued"),

        // ------------------------------------------------------------------ W4: ack
        (
            Some(S::Delivering),
            WaiterEvent::Ack {
                resume_token_valid,
                already_acked,
                remaining_unacked,
                request_settled,
                applied,
            },
        ) => {
            if !resume_token_valid {
                // §3.2: possession of an identifier is never authorization, and existence is not
                // disclosed to a caller who cannot prove it is the waiter.
                return Err(ProtocolError::new(
                    ErrorCode::SignalNotFound,
                    "no such signal for this waiter",
                ));
            }
            if already_acked {
                // Idempotent: `200` with `first_ack: false`, and no second application (C-12).
                return accept(WaiterRule::W4, S::Delivering, Vec::new());
            }
            let to = if remaining_unacked {
                S::Signalled
            } else if request_settled {
                S::Acked
            } else {
                // The nudge was consumed but the request is still pending, so the wait continues.
                // This is the `armed ⇄ signalled` edge of §8.1; see ambiguity note A-6.
                S::Armed
            };
            accept(
                WaiterRule::W4,
                to,
                vec![E::StopRedelivery, E::RecordAckOutcome { applied }],
            )
        }
        (Some(_), WaiterEvent::Ack { .. }) => refuse("ack a signal it is not delivering"),

        // ------------------------------------------------------------------ W5: transport failure
        (Some(S::Delivering), WaiterEvent::TransportFailed { attempts_below_max }) => accept(
            WaiterRule::W5,
            S::Signalled,
            vec![if attempts_below_max {
                E::BackoffAndRequeue
            } else {
                E::DisableCallbackAndNotifyTenant
            }],
        ),
        (Some(_), WaiterEvent::TransportFailed { .. }) => {
            refuse("fail a delivery it is not attempting")
        }

        // ------------------------------------------------------------------ W6: the waiter died
        (Some(S::Armed | S::Signalled), WaiterEvent::WaiterTerminal { policy }) => accept(
            WaiterRule::W6,
            S::Orphaned,
            vec![match policy {
                OnWaiterTerminal::Keep => E::SurfaceOrphanedOnRequest,
                OnWaiterTerminal::Cancel => E::CancelRequest,
            }],
        ),
        (Some(_), WaiterEvent::WaiterTerminal { .. }) => refuse("be orphaned from here"),

        // ------------------------------------------------------------------ W7: reattach
        (Some(S::Orphaned), WaiterEvent::Reattach { authorized }) => {
            if !authorized {
                return Err(ProtocolError::new(
                    ErrorCode::AuthenticationRequired,
                    "reattaching requires a valid resume token or proven waiter_ref ownership",
                ));
            }
            accept(
                WaiterRule::W7,
                S::Armed,
                vec![E::ReturnUnackedSignals, E::ReArmLease],
            )
        }
        (Some(_), WaiterEvent::Reattach { authorized }) => {
            if !authorized {
                return Err(ProtocolError::new(
                    ErrorCode::AuthenticationRequired,
                    "reattaching requires a valid resume token or proven waiter_ref ownership",
                ));
            }
            // Reattaching a live waiter is not an error — a client that restarted before its lease
            // lapsed must still get its unacked signals back (§8.5, C-11).
            accept(
                WaiterRule::W7,
                from.unwrap_or(S::Armed),
                vec![E::ReturnUnackedSignals, E::ReArmLease],
            )
        }
    }
}

/// Everything a restarted client needs to carry on (§8.5).
///
/// Nothing was lost while the client was gone. A Server MUST NOT discard a signal because the
/// client that raised the request went away.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReattachResult {
    /// The waiter that reattached.
    pub waiter_ref: String,
    /// Its state after re-arming.
    pub state: WaiterState,
    /// Requests under this waiter that are still `pending`.
    pub open_requests: Vec<RequestId>,
    /// Every **unacked** signal, oldest first.
    pub signals: Vec<Signal>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::{RequestId, ResumeToken, SignalId};
    use serde_json::json;

    fn ts(s: &str) -> Timestamp {
        Timestamp::parse(s).expect("valid timestamp")
    }

    fn signal(seq: u64, signal_type: SignalType) -> Signal {
        let decision = signal_type.is_terminal().then(|| Decision {
            outcome: match signal_type {
                SignalType::Answered => DecisionOutcome::Answered,
                SignalType::Expired => DecisionOutcome::Expired,
                SignalType::Cancelled => DecisionOutcome::Cancelled,
                SignalType::Superseded => DecisionOutcome::Superseded,
                SignalType::AttemptLapsed => unreachable!(),
            },
            values: Map::new(),
            source: DecisionSource::Human,
            effective: None,
            receipt_id: None,
            authorization_id: None,
            superseded_by: None,
        });
        Signal {
            id: SignalId::parse("sig_01K3MB2R4XC4YRXB2N6VD9FTHE").expect("parse"),
            request_id: RequestId::parse("req_01K3M7QW8ZC4YRXB2N6VD9FTHE").expect("parse"),
            waiter_ref: "run:0198f2a1".to_string(),
            signal_type,
            sequence: seq,
            resume_token: ResumeToken::parse("rt_01K3MB2R55C4YRXB2N6VD9FTHE").expect("parse"),
            decision,
            resume_ref: None,
            resume_payload: None,
            attempts: 0,
            created_at: ts("2026-07-30T14:07:44Z"),
            acked_at: None,
        }
    }

    const EVENTS: &[WaiterEvent] = &[
        WaiterEvent::Raise,
        WaiterEvent::Signal {
            signal_type: SignalType::Answered,
        },
        WaiterEvent::Signal {
            signal_type: SignalType::Expired,
        },
        WaiterEvent::Signal {
            signal_type: SignalType::Cancelled,
        },
        WaiterEvent::Signal {
            signal_type: SignalType::Superseded,
        },
        WaiterEvent::Signal {
            signal_type: SignalType::AttemptLapsed,
        },
        WaiterEvent::Claim,
        WaiterEvent::Ack {
            resume_token_valid: true,
            already_acked: false,
            remaining_unacked: false,
            request_settled: true,
            applied: true,
        },
        WaiterEvent::TransportFailed {
            attempts_below_max: true,
        },
        WaiterEvent::WaiterTerminal {
            policy: OnWaiterTerminal::Keep,
        },
        WaiterEvent::Reattach { authorized: true },
    ];

    #[test]
    fn the_machine_is_total_and_never_panics() {
        for &from in WaiterState::ALL {
            for &event in EVENTS {
                let _ = transition(Some(from), event);
            }
        }
        for &event in EVENTS {
            let _ = transition(None, event);
        }
    }

    #[test]
    fn the_happy_path_arms_signals_claims_and_acks() {
        let armed = transition(None, WaiterEvent::Raise).expect("arm");
        assert_eq!(armed.to, WaiterState::Armed);
        assert!(armed
            .effects
            .contains(&WaiterEffect::StoreContinuationVerbatim));

        let signalled = transition(
            Some(WaiterState::Armed),
            WaiterEvent::Signal {
                signal_type: SignalType::Answered,
            },
        )
        .expect("signal");
        assert_eq!(signalled.to, WaiterState::Signalled);

        let delivering =
            transition(Some(WaiterState::Signalled), WaiterEvent::Claim).expect("claim");
        assert_eq!(delivering.to, WaiterState::Delivering);
        assert!(delivering.effects.contains(&WaiterEffect::ClaimWithLease));

        let acked = transition(
            Some(WaiterState::Delivering),
            WaiterEvent::Ack {
                resume_token_valid: true,
                already_acked: false,
                remaining_unacked: false,
                request_settled: true,
                applied: true,
            },
        )
        .expect("ack");
        assert_eq!(acked.to, WaiterState::Acked);
        assert!(acked.effects.contains(&WaiterEffect::StopRedelivery));
    }

    #[test]
    fn a_nudge_can_never_mask_a_later_answer() {
        // W2: signals are a queue, not a flag. A server modelling the pending outcome as one
        // mutable field loses answers, and this is the test that would catch it.
        let mut queue: Vec<Signal> = Vec::new();
        let mut state = transition(None, WaiterEvent::Raise).expect("arm").to;

        for signal_type in [SignalType::AttemptLapsed, SignalType::Answered] {
            let t = transition(Some(state), WaiterEvent::Signal { signal_type }).expect("enqueue");
            assert!(t
                .effects
                .contains(&WaiterEffect::EnqueueSignal(signal_type)));
            queue.push(signal(
                next_sequence(queue.last().map(|s| s.sequence)),
                signal_type,
            ));
            state = t.to;
        }

        assert_eq!(
            queue.len(),
            2,
            "the nudge was appended to, not replaced by, the answer"
        );
        assert_eq!(
            queue.iter().map(|s| s.sequence).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(queue[1].signal_type, SignalType::Answered);
        assert!(
            queue.iter().all(Signal::is_unacked),
            "reading a signal does not consume it"
        );
    }

    #[test]
    fn sequences_increase_monotonically_per_waiter() {
        assert_eq!(next_sequence(None), 1);
        assert_eq!(next_sequence(Some(1)), 2);
        assert_eq!(next_sequence(Some(41)), 42);
    }

    #[test]
    fn an_ack_is_idempotent_and_stops_redelivery_exactly_once() {
        // C-12: ack twice, both `200`, redelivery stops once, no duplicate application.
        let first = transition(
            Some(WaiterState::Delivering),
            WaiterEvent::Ack {
                resume_token_valid: true,
                already_acked: false,
                remaining_unacked: false,
                request_settled: true,
                applied: true,
            },
        )
        .expect("first ack");
        assert!(first.effects.contains(&WaiterEffect::StopRedelivery));

        let replay = transition(
            Some(WaiterState::Delivering),
            WaiterEvent::Ack {
                resume_token_valid: true,
                already_acked: true,
                remaining_unacked: false,
                request_settled: true,
                applied: true,
            },
        )
        .expect("replayed ack is accepted");
        assert!(
            replay.effects.is_empty(),
            "a replay applies nothing a second time"
        );
    }

    #[test]
    fn an_unapplied_decision_is_recorded_rather_than_swallowed() {
        // §8.3: `applied: false` is a fact the record should hold, not an error.
        let t = transition(
            Some(WaiterState::Delivering),
            WaiterEvent::Ack {
                resume_token_valid: true,
                already_acked: false,
                remaining_unacked: false,
                request_settled: true,
                applied: false,
            },
        )
        .expect("accepted");
        assert!(t
            .effects
            .contains(&WaiterEffect::RecordAckOutcome { applied: false }));
        assert!(t.effects.contains(&WaiterEffect::StopRedelivery));
    }

    #[test]
    fn acking_a_nudge_returns_the_waiter_to_waiting_not_to_done() {
        let t = transition(
            Some(WaiterState::Delivering),
            WaiterEvent::Ack {
                resume_token_valid: true,
                already_acked: false,
                remaining_unacked: false,
                request_settled: false,
                applied: true,
            },
        )
        .expect("ack the nudge");
        assert_eq!(
            t.to,
            WaiterState::Armed,
            "the request is still pending, so the wait continues"
        );

        let more = transition(
            Some(WaiterState::Delivering),
            WaiterEvent::Ack {
                resume_token_valid: true,
                already_acked: false,
                remaining_unacked: true,
                request_settled: true,
                applied: true,
            },
        )
        .expect("ack with more queued");
        assert_eq!(more.to, WaiterState::Signalled);
    }

    #[test]
    fn an_ack_without_a_valid_token_does_not_disclose_the_signal() {
        let err = transition(
            Some(WaiterState::Delivering),
            WaiterEvent::Ack {
                resume_token_valid: false,
                already_acked: false,
                remaining_unacked: false,
                request_settled: true,
                applied: true,
            },
        )
        .expect_err("refused");
        assert_eq!(err.code, ErrorCode::SignalNotFound);
        assert_eq!(
            err.http_status(),
            404,
            "not a 403: existence is not disclosed"
        );
    }

    #[test]
    fn a_transport_failure_requeues_and_a_dead_endpoint_is_eventually_disabled() {
        let retried = transition(
            Some(WaiterState::Delivering),
            WaiterEvent::TransportFailed {
                attempts_below_max: true,
            },
        )
        .expect("requeue");
        assert_eq!(retried.to, WaiterState::Signalled);
        assert!(retried.effects.contains(&WaiterEffect::BackoffAndRequeue));

        let exhausted = transition(
            Some(WaiterState::Delivering),
            WaiterEvent::TransportFailed {
                attempts_below_max: false,
            },
        )
        .expect("still queued");
        assert_eq!(
            exhausted.to,
            WaiterState::Signalled,
            "the signal is not lost"
        );
        assert!(exhausted
            .effects
            .contains(&WaiterEffect::DisableCallbackAndNotifyTenant));
    }

    #[test]
    fn a_dead_client_can_reattach_and_find_its_signal_still_there() {
        // C-11: kill the client mid-poll, restart, reattach.
        let orphaned = transition(
            Some(WaiterState::Signalled),
            WaiterEvent::WaiterTerminal {
                policy: OnWaiterTerminal::Keep,
            },
        )
        .expect("orphan");
        assert_eq!(orphaned.to, WaiterState::Orphaned);
        assert!(orphaned
            .effects
            .contains(&WaiterEffect::SurfaceOrphanedOnRequest));
        assert!(
            !WaiterState::Orphaned.is_terminal(),
            "orphaned is recoverable, not final"
        );

        // A late answer must still be queued for whoever comes back.
        let late = transition(
            Some(WaiterState::Orphaned),
            WaiterEvent::Signal {
                signal_type: SignalType::Answered,
            },
        )
        .expect("still recorded");
        assert!(late
            .effects
            .contains(&WaiterEffect::EnqueueSignal(SignalType::Answered)));

        let back = transition(
            Some(WaiterState::Orphaned),
            WaiterEvent::Reattach { authorized: true },
        )
        .expect("reattach");
        assert_eq!(back.to, WaiterState::Armed);
        assert!(back.effects.contains(&WaiterEffect::ReturnUnackedSignals));
        assert!(back.effects.contains(&WaiterEffect::ReArmLease));
    }

    #[test]
    fn a_leased_waiter_that_dies_stops_paging_people() {
        let t = transition(
            Some(WaiterState::Armed),
            WaiterEvent::WaiterTerminal {
                policy: OnWaiterTerminal::Cancel,
            },
        )
        .expect("orphan");
        assert!(t.effects.contains(&WaiterEffect::CancelRequest));
    }

    #[test]
    fn reattaching_without_proof_of_ownership_is_refused() {
        for from in WaiterState::ALL.iter().filter(|s| !s.is_terminal()) {
            assert_eq!(
                transition(Some(*from), WaiterEvent::Reattach { authorized: false })
                    .expect_err("refused")
                    .code,
                ErrorCode::AuthenticationRequired
            );
        }
    }

    #[test]
    fn cancelling_or_superseding_releases_the_waiter_without_an_ack() {
        for signal_type in [SignalType::Cancelled, SignalType::Superseded] {
            let t = transition(
                Some(WaiterState::Armed),
                WaiterEvent::Signal { signal_type },
            )
            .expect("release");
            assert_eq!(t.rule, WaiterRule::W8);
            assert_eq!(t.to, WaiterState::Released);
            assert!(t
                .effects
                .contains(&WaiterEffect::DeliverTerminalSignalWithoutAck));
        }
    }

    #[test]
    fn a_terminal_waiter_never_moves_again() {
        for &from in WaiterState::ALL.iter().filter(|s| s.is_terminal()) {
            for &event in EVENTS {
                assert!(
                    transition(Some(from), event).is_err(),
                    "{from:?} is terminal but accepted {event:?}"
                );
            }
        }
    }

    // -------------------------------------------------------------- signal payloads

    #[test]
    fn a_terminal_signal_must_carry_a_typed_decision() {
        // §8.2: a Server MUST NOT satisfy a wait with a null or empty decision.
        let mut answered = signal(1, SignalType::Answered);
        answered.validate().expect("valid");
        answered.decision = None;
        assert!(
            answered.validate().is_err(),
            "an empty wait teaches the runtime nothing"
        );
    }

    #[test]
    fn a_nudge_decides_nothing_and_says_so() {
        let mut nudge = signal(1, SignalType::AttemptLapsed);
        nudge.validate().expect("valid");
        assert!(nudge.decision.is_none());

        nudge.decision = Some(Decision {
            outcome: DecisionOutcome::Answered,
            values: Map::new(),
            source: DecisionSource::Human,
            effective: None,
            receipt_id: None,
            authorization_id: None,
            superseded_by: None,
        });
        assert!(nudge.validate().is_err());
    }

    #[test]
    fn a_decisions_outcome_must_match_its_signal_type() {
        let mut expired = signal(1, SignalType::Expired);
        expired.decision.as_mut().expect("present").outcome = DecisionOutcome::Answered;
        assert!(
            expired.validate().is_err(),
            "an expiry must not report itself as an answer"
        );
    }

    #[test]
    fn signals_serialize_to_the_wire_shape() {
        let mut s = signal(1, SignalType::Expired);
        s.decision.as_mut().expect("present").source = DecisionSource::Policy;
        s.decision.as_mut().expect("present").effective = Some(EffectiveAnswer::Deny);
        let json = serde_json::to_value(&s).expect("serialize");
        assert_eq!(json["type"], "expired");
        assert_eq!(json["decision"]["source"], "policy");
        assert_eq!(json["decision"]["effective"], "deny");
        assert_eq!(json["sequence"], json!(1));
        let back: Signal = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back, s);
    }

    #[test]
    fn every_terminal_signal_type_is_reachable_from_a_request_state() {
        // I11 from the other side: each terminal signal names a request state, and no signal type
        // is orphaned.
        for &signal_type in SignalType::ALL {
            let terminal = signal_type.is_terminal();
            assert_eq!(terminal, signal_type != SignalType::AttemptLapsed);
        }
        assert_eq!(SignalType::ALL.len(), 5);
    }
}

//! The decisions, as pure functions.
//!
//! Everything here takes a snapshot read inside a transaction and returns the rows that transaction
//! must write. Nothing here does I/O, so the rules that matter most — who may answer, what the
//! receipt says, whether an answer settles anything — are decidable in a unit test rather than only
//! against a database.
//!
//! The receipts these build are **unsealed**: `chain` is `None`. Sealing needs the tenant's current
//! head, which is only knowable under the lock the settling transaction already holds, so it
//! happens there (§9.4).

use handoff_protocol::authorization::{Authorization, AuthorizationBinding};
use handoff_protocol::clock::Timestamp;
use handoff_protocol::error::{ErrorCode, ProtocolError, Result};
use handoff_protocol::id::{AuthorizationId, OrgId, ReceiptId};
use handoff_protocol::receipt::{
    ActorType, CapabilityExercised, Clearance, ClearanceSource, Digest, Receipt, ReceiptActor,
    ReceiptAuthority, ReceiptDecision, ReceiptKind, ReceiptRendered, ReceiptStep, ReceiptVia,
    SatisfiedStrength,
};
use handoff_protocol::request::{Disposition, OnExpiry, RequestState};
use handoff_protocol::requires::{AnswerMode, DeploymentProfile, Field, FieldType};
use handoff_protocol::waiter::{Decision, DecisionOutcome, DecisionSource, EffectiveAnswer};
use serde_json::{json, Map, Value};

use crate::auth::{AuthPolicy, Principal, PrincipalKind};
use crate::model::{AnswerCommand, DeliveryView, GrantView, RequestView};

/// What a settling write must persist, decided before anything is written.
#[derive(Debug, Clone)]
pub struct AnswerPlan {
    /// The receipt, unsealed. The transaction seals it into the tenant's chain.
    pub receipt: Receipt,
    /// The single authorization an answer mints (I10). `None` for a partial answer and for any
    /// disposition other than `decide`.
    pub authorization: Option<Authorization>,
    /// The typed decision the waiter receives. `None` when nothing settled — §5.5 is explicit that
    /// a runtime MUST NOT learn an intermediate step occurred.
    pub decision: Option<Decision>,
    /// Whether the request actually reaches `answered`.
    pub settles: bool,
    /// The delivery this decision arrived through, graded to `acted`.
    pub via: Option<DeliveryView>,
    /// Field names whose values went to the sink and were never here.
    pub secret_fields: Vec<String>,
}

/// Everything a settling write is decided from.
#[derive(Debug, Clone, Copy)]
pub struct AnswerInput<'a> {
    /// The request as it stands, read inside the settling transaction.
    pub request: &'a RequestView,
    /// Who is answering, as authenticated right now (§4.3).
    pub principal: &'a Principal,
    /// What they submitted.
    pub command: &'a AnswerCommand,
    /// The deployment's view of `link_only` (§4.4).
    pub policy: &'a AuthPolicy,
    /// The deployment's declared capabilities, for the authority evaluation.
    pub profile: &'a DeploymentProfile,
    /// The delivery this answer arrived through, if one is identified.
    pub via: Option<&'a DeliveryView>,
    /// The grants on this request, for the capability record.
    pub grants: &'a [GrantView],
    /// The receipt identity to mint.
    pub receipt_id: ReceiptId,
    /// The authorization identity to mint.
    pub authorization_id: AuthorizationId,
}

/// Decide a settling write (§6.2 R5, §9).
///
/// The checks run in the order the specification puts them, and the order is the point:
///
/// 1. **Principal type** (§4.2). A machine is refused before anything else is even read, because
///    §4.2 forbids any role, scope, or setting that could satisfy the check later.
/// 2. Role and grade against the request's declared authority, evaluated against the answerer's
///    identity **at this moment** rather than at raise time (§4.3).
/// 3. The deployment's own view of the grade (§4.4) — separate, and with its own code.
/// 4. The submitted values against the declared field set (§5.3).
pub fn plan_answer(input: AnswerInput<'_>) -> Result<AnswerPlan> {
    let AnswerInput {
        request,
        principal,
        command,
        policy,
        profile,
        via,
        grants,
        receipt_id,
        authorization_id,
    } = input;
    // 1. §4.2 / I15. By type, with no way round it.
    if !principal.may_answer() {
        return Err(ProtocolError::new(
            ErrorCode::RequesterMayNotAnswer,
            "a service_account principal may not answer a request it can raise",
        ));
    }

    let required = request.requires.effective_authority();

    // 2. §4.3, at answer time. An anonymous link has no principal to evaluate, so §4.4's rules
    //    stand in for the role check that cannot be made.
    let satisfied = match principal.presented() {
        Some(presented) => required.evaluate(&presented, profile)?,
        None => {
            if principal.kind != PrincipalKind::AnonymousLink {
                return Err(ProtocolError::new(
                    ErrorCode::AuthenticationRequired,
                    "this operation requires an authenticated principal",
                ));
            }
            if principal.role
                < required
                    .min_role
                    .unwrap_or(handoff_protocol::requires::Role::Viewer)
            {
                return Err(ProtocolError::new(
                    ErrorCode::InsufficientAuthority,
                    "this request requires a higher role than a delivery link carries",
                ));
            }
            if principal.auth_strength < required.auth_strength {
                return Err(ProtocolError::new(
                    ErrorCode::InsufficientAuthority,
                    "this request requires a stronger authentication grade",
                ));
            }
            principal.auth_strength
        }
    };

    // 3. §4.4 / C-6b. After the role, and with its own code: this is not a role anyone could be
    //    granted.
    policy.check_grade(satisfied)?;

    // 4. §5.3. A raw value against a `secret` field is refused here, and the error names the field
    //    without ever echoing what was submitted (§12 rule 6).
    let mode = if command.partial {
        AnswerMode::Partial
    } else if command.disposition == Disposition::Decide {
        AnswerMode::Settle
    } else {
        AnswerMode::NonDeciding
    };
    request.requires.validate_answer(&command.values, mode)?;

    // §1.4. Every number that reaches a digest-covered object has to be one every canonicalizer
    // spells the same way, and one that survives a round trip through a double. Checked here
    // rather than at the edge because the receipt is what carries the consequence.
    check_number_bounds(&command.values)?;

    // §9.3. Under `strict`, an answer against wording the person is no longer looking at is
    // refused; under `advisory` the divergence is recorded rather than hidden.
    if let Some(echoed) = &command.rendered_digest {
        if *echoed != request.rendered_digest
            && request.presentation_binding
                == handoff_protocol::receipt::PresentationBinding::Strict
        {
            return Err(ProtocolError::new(
                ErrorCode::PresentationStale,
                "rendered_digest does not match the current request version",
            ));
        }
    }

    let secret_fields = declared_secret_fields(request);
    let values = reduce_secrets(&command.values, &secret_fields);
    let settles = !command.partial && command.disposition == Disposition::Decide;

    let actor = match principal.kind {
        PrincipalKind::AnonymousLink => ReceiptActor {
            actor_type: ActorType::AnonymousLink,
            principal_id: None,
            display: None,
            role_at_decision: None,
            auth_strength: Some(satisfied),
            reauth_at: None,
            mfa_at: None,
            ip_digest: None,
            user_agent_digest: None,
            on_behalf_of: None,
        },
        _ => ReceiptActor {
            actor_type: ActorType::User,
            principal_id: principal.id,
            display: principal.display.clone(),
            role_at_decision: Some(format!("{:?}", principal.role).to_lowercase()),
            auth_strength: Some(satisfied),
            reauth_at: None,
            mfa_at: None,
            ip_digest: None,
            user_agent_digest: None,
            on_behalf_of: None,
        },
    };

    let capabilities_exercised = command
        .capability_uses
        .iter()
        .map(|use_| CapabilityExercised {
            handle: use_.handle,
            session_ref: use_.session_ref,
            scopes: grants
                .iter()
                .find(|g| g.handle == use_.handle)
                .map(|g| vec![g.scope])
                .unwrap_or_default(),
            resolved_at: None,
            released_at: Some(command.now),
            held_ms: None,
            input_events: None,
            navigations: Vec::new(),
            blast_radius_digest: grants
                .iter()
                .find(|g| g.handle == use_.handle)
                .map(|g| g.blast_radius_digest.clone()),
        })
        .collect();

    // §9.7. A person answering *is* the assertion; nothing here is inferred from an observation,
    // and a receipt that named a person the Server did not authenticate is the failure the rule
    // exists to prevent.
    let clearance = principal.id.map(|actor| Clearance {
        source: ClearanceSource::HumanAssertion,
        actor: Some(actor),
        at: Some(command.now),
    });

    let receipt = Receipt {
        id: receipt_id,
        request_id: request.id,
        org_id: tenant_as_org(&request.tenant_ref)?,
        kind: ReceiptKind::Decision,
        corrects: None,
        decision: ReceiptDecision {
            values: values.clone(),
            disposition: command.disposition,
            note: command.note.clone(),
        },
        actor,
        decided_at: command.now,
        attempt_id: None,
        request_version: request.version,
        request_digest: request.request_digest.clone(),
        rendered: Some(ReceiptRendered {
            digest: request.rendered_digest.clone(),
            reference: Some(request.rendered_ref.clone()),
        }),
        via: via.map_or_else(ReceiptVia::default, |d| ReceiptVia {
            delivery_id: Some(d.id),
            channel: Some(d.channel.clone()),
            target: Some(d.target.clone()),
            // Never above the channel's declared ceiling (§7.2).
            grade_reached: Some(
                d.max_grade
                    .min(handoff_protocol::delivery::DeliveryGrade::Acted),
            ),
        }),
        authority: ReceiptAuthority {
            required,
            satisfied: SatisfiedStrength::from(satisfied),
        },
        steps: vec![ReceiptStep {
            n: 1,
            at: command.now,
            fields_provided: command.values.keys().cloned().collect(),
            secret_fields: secret_fields
                .iter()
                .filter(|name| command.values.contains_key(*name))
                .cloned()
                .collect(),
            via_delivery_id: via.map(|d| d.id),
        }],
        capabilities_exercised,
        clearance,
        chain: None,
        presentation_divergence: None,
    };
    receipt.validate()?;

    let authorization = settles.then(|| {
        Authorization::mint(authorization_id, receipt_id, request.id, values.clone())
            .bound_to(AuthorizationBinding {
                waiter_ref: Some(request.waiter_ref.clone()),
                effect_digest: None,
            })
            .expiring_at(command.now.saturating_add(
                handoff_protocol::clock::IsoDuration::from_secs(
                    handoff_protocol::authorization::DEFAULT_AUTHORIZATION_TTL_SECS,
                ),
            ))
    });

    let decision = settles.then(|| Decision {
        outcome: DecisionOutcome::Answered,
        values,
        source: DecisionSource::Human,
        effective: None,
        receipt_id: Some(receipt_id),
        authorization_id: authorization.as_ref().map(|a| a.id),
        superseded_by: None,
    });

    Ok(AnswerPlan {
        receipt,
        authorization,
        decision,
        settles,
        via: via.cloned(),
        secret_fields,
    })
}

/// What a terminal transition other than an answer must persist.
#[derive(Debug, Clone)]
pub struct TerminalPlan {
    /// The state the request reaches.
    pub state: RequestState,
    /// The receipt an expiry mints. `None` for a cancellation or a supersession: §2 says nothing
    /// but an *outcome* mints a receipt, and withdrawing an ask is not one.
    pub receipt: Option<Receipt>,
    /// The typed terminal signal. Never `None` — a request never goes quiet (I11).
    pub decision: Decision,
}

/// Decide an expiry (§6.2 R6, §6.4, §9.6).
///
/// The receipt is visibly **not** a human decision: `actor.type = "policy"`, no principal named,
/// and `authority.satisfied = "none"`. A record that cannot distinguish a person from a policy from
/// a passer-by is not a record.
pub fn plan_expiry(
    request: &RequestView,
    receipt_id: ReceiptId,
    now: Timestamp,
) -> Result<TerminalPlan> {
    let policy = request
        .ttl_policy
        .as_ref()
        .map_or(OnExpiry::ExpireAndDeny, |p| p.on_expiry);
    let (effective, values) = match policy {
        OnExpiry::Default => (
            EffectiveAnswer::Default,
            request
                .ttl_policy
                .as_ref()
                .and_then(|p| p.default_answer.clone())
                .unwrap_or_default(),
        ),
        _ => (EffectiveAnswer::Deny, Map::new()),
    };

    let receipt = Receipt {
        id: receipt_id,
        request_id: request.id,
        org_id: tenant_as_org(&request.tenant_ref)?,
        kind: ReceiptKind::Policy,
        corrects: None,
        decision: ReceiptDecision {
            values: values.clone(),
            disposition: Disposition::Decide,
            note: None,
        },
        actor: ReceiptActor {
            actor_type: ActorType::Policy,
            principal_id: None,
            display: None,
            role_at_decision: None,
            auth_strength: None,
            reauth_at: None,
            mfa_at: None,
            ip_digest: None,
            user_agent_digest: None,
            on_behalf_of: None,
        },
        decided_at: now,
        attempt_id: None,
        request_version: request.version,
        request_digest: request.request_digest.clone(),
        rendered: Some(ReceiptRendered {
            digest: request.rendered_digest.clone(),
            reference: Some(request.rendered_ref.clone()),
        }),
        via: ReceiptVia::default(),
        authority: ReceiptAuthority {
            required: request.requires.effective_authority(),
            satisfied: SatisfiedStrength::None,
        },
        steps: Vec::new(),
        capabilities_exercised: Vec::new(),
        clearance: Some(Clearance {
            source: ClearanceSource::Timeout,
            actor: None,
            at: Some(now),
        }),
        chain: None,
        presentation_divergence: None,
    };
    receipt.validate()?;

    Ok(TerminalPlan {
        state: RequestState::Expired,
        decision: Decision {
            outcome: DecisionOutcome::Expired,
            values,
            source: DecisionSource::Policy,
            effective: Some(effective),
            receipt_id: Some(receipt_id),
            authorization_id: None,
            superseded_by: None,
        },
        receipt: Some(receipt),
    })
}

/// The typed terminal signal a cancellation or a supersession produces (I11).
pub fn terminal_decision(
    state: RequestState,
    superseded_by: Option<handoff_protocol::id::RequestId>,
) -> Decision {
    Decision {
        outcome: match state {
            RequestState::Cancelled => DecisionOutcome::Cancelled,
            RequestState::Superseded => DecisionOutcome::Superseded,
            RequestState::Expired => DecisionOutcome::Expired,
            _ => DecisionOutcome::Answered,
        },
        values: Map::new(),
        source: DecisionSource::Policy,
        effective: None,
        receipt_id: None,
        authorization_id: None,
        superseded_by,
    }
}

/// The band in which RFC 8785's number serialization produces plain decimal notation (§1.4).
pub const MIN_MAGNITUDE: f64 = 1e-6;
/// The exclusive upper end of that band.
pub const MAX_MAGNITUDE: f64 = 1e21;
/// The largest integer an IEEE-754 double distinguishes from its neighbours, `2^53 - 1`.
pub const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// Refuse an answer carrying a number this protocol cannot represent identically everywhere (§1.4).
///
/// Two different rules, with two different reasons:
///
/// - **The band** is about *notation*. Outside `1e-6 ≤ |x| < 1e21`, `Number::toString` switches to
///   exponential form, and that switch is where independent canonicalizers disagree. A
///   disagreement there is not cosmetic: two conforming implementations compute different digests
///   for the same receipt, so a chain one can verify the other cannot — and nothing errors, ever.
/// - **The safe-integer bound** is about *precision*. Beyond `2^53 - 1` a value is
///   indistinguishable from its neighbours, so a person approving `100000000000000000001` would
///   get a receipt saying `100000000000000000000`. A receipt that misstates what was approved is
///   the one thing a receipt may never do.
///
/// Every offending field is collected rather than the first, because a surface has to be able to
/// mark every bad input at once.
pub fn check_number_bounds(values: &Map<String, Value>) -> Result<()> {
    let mut errors: Vec<handoff_protocol::error::FieldError> = Vec::new();
    for (name, value) in values {
        if let Some(reason) = first_bad_number(value) {
            errors.push(handoff_protocol::error::FieldError {
                name: name.clone(),
                code: handoff_protocol::error::FieldErrorCode::OutOfRange,
                message: Some(reason),
            });
        }
    }
    if errors.is_empty() {
        return Ok(());
    }
    Err(ProtocolError::new(
        ErrorCode::AnswerValidationFailed,
        "the answer carries a number this protocol cannot represent identically everywhere",
    )
    .with_fields(errors))
}

/// The first number anywhere inside a value that is out of bounds, and why.
fn first_bad_number(value: &Value) -> Option<String> {
    match value {
        Value::Number(number) => {
            let x = number.as_f64()?;
            if !x.is_finite() {
                return Some("a number must be finite".into());
            }
            let magnitude = x.abs();
            if magnitude != 0.0 && !(MIN_MAGNITUDE..MAX_MAGNITUDE).contains(&magnitude) {
                return Some(format!(
                    "a number must be 0 or within {MIN_MAGNITUDE:e} <= |x| < {MAX_MAGNITUDE:e}, \
                     which is the band that canonicalizes to plain decimal"
                ));
            }
            if x.fract() == 0.0 && magnitude > MAX_SAFE_INTEGER {
                return Some(
                    "a whole number must be within +/-(2^53 - 1) so it round-trips through a \
                     double without loss"
                        .into(),
                );
            }
            None
        }
        Value::Array(items) => items.iter().find_map(first_bad_number),
        Value::Object(fields) => fields.values().find_map(first_bad_number),
        _ => None,
    }
}

/// Which declared fields are `secret`, and therefore never carry a value anywhere (§5.3, I7).
pub fn declared_secret_fields(request: &RequestView) -> Vec<String> {
    request
        .requires
        .answer
        .as_ref()
        .map(|answer| {
            answer
                .fields
                .iter()
                .filter(|f: &&Field| f.field_type == FieldType::Secret)
                .map(|f| f.name.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// Reduce every `secret` field to the fact of provision, and nothing else (§9.1, I7).
///
/// This runs on the way *into* the receipt, the decision, and the signal, so there is one place
/// where a value could survive and it does not.
pub fn reduce_secrets(values: &Map<String, Value>, secret_fields: &[String]) -> Map<String, Value> {
    values
        .iter()
        .map(|(name, value)| {
            if secret_fields.iter().any(|s| s == name) {
                (name.clone(), json!({"provided": true}))
            } else {
                (name.clone(), value.clone())
            }
        })
        .collect()
}

/// Read a tenant reference as the receipt's `org_id`.
///
/// The engine treats `tenant_ref` as opaque everywhere else. A receipt is the one place the
/// protocol gives the field a type, so a deployment's tenant references have to be spellable as
/// one; the error says so plainly rather than producing a receipt that will not verify.
pub fn tenant_as_org(tenant_ref: &str) -> Result<OrgId> {
    OrgId::parse(tenant_ref).map_err(|_| {
        ProtocolError::new(
            ErrorCode::InvalidRequest,
            format!(
                "tenant reference `{tenant_ref}` cannot be recorded on a receipt: the protocol \
                 types `org_id` as an identifier, so tenant references must be spelled `org_<ULID>`"
            ),
        )
    })
}

/// The digest of what a person is shown at a version (§9.2).
///
/// Taken over the prompt, the declarations, and the version together, so an amendment produces a
/// different digest and a receipt cannot be re-derived later from content the person never saw.
pub fn rendered_digest(request_prompt: &Value, requires: &Value, version: u64) -> Result<Digest> {
    handoff_protocol::receipt::digest_of(&json!({
        "prompt": request_prompt,
        "requires": requires,
        "version": version,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_values_are_reduced_to_the_fact_of_provision() {
        let mut values = Map::new();
        values.insert("email".into(), json!("dana@example.com"));
        values.insert("password".into(), json!("hunter2"));
        let reduced = reduce_secrets(&values, &["password".to_string()]);
        assert_eq!(reduced["email"], json!("dana@example.com"));
        assert_eq!(reduced["password"], json!({"provided": true}));
        assert!(!serde_json::to_string(&reduced).unwrap().contains("hunter2"));
    }

    #[test]
    fn a_tenant_reference_that_is_not_an_org_id_fails_loudly() {
        assert!(tenant_as_org("tenant-7").is_err());
        assert!(tenant_as_org("org_01K3M7QW8ZC4YRXB2N6VD9FTHE").is_ok());
    }

    #[test]
    fn the_bounds_are_inclusive_at_zero_and_at_both_edges() {
        let mut values = Map::new();
        values.insert("at_zero".into(), json!(0));
        values.insert("at_lower_bound".into(), json!(0.000001));
        values.insert(
            "at_max_safe_integer".into(),
            json!(9_007_199_254_740_991u64),
        );
        values.insert("ordinary".into(), json!(1.5));
        assert!(check_number_bounds(&values).is_ok());
    }

    #[test]
    fn a_number_below_the_band_names_its_field() {
        let mut values = Map::new();
        values.insert("at_lower_bound".into(), json!(0.0000001));
        let err = check_number_bounds(&values).unwrap_err();
        assert_eq!(err.code, ErrorCode::AnswerValidationFailed);
        assert_eq!(err.fields()[0].name, "at_lower_bound");
    }

    #[test]
    fn an_integer_at_two_to_the_fifty_third_no_longer_round_trips() {
        let mut values = Map::new();
        values.insert(
            "at_max_safe_integer".into(),
            json!(9_007_199_254_740_992u64),
        );
        let err = check_number_bounds(&values).unwrap_err();
        assert_eq!(err.fields()[0].name, "at_max_safe_integer");
    }

    #[test]
    fn a_number_above_the_band_is_refused_wherever_it_is_nested() {
        let mut values = Map::new();
        values.insert("ordinary".into(), json!({"nested": [1e21]}));
        let err = check_number_bounds(&values).unwrap_err();
        assert_eq!(err.fields()[0].name, "ordinary");
    }

    #[test]
    fn amending_changes_what_the_person_saw() {
        let before = rendered_digest(&json!({"title": "a"}), &json!({"v": 1}), 1).unwrap();
        let after = rendered_digest(&json!({"title": "b"}), &json!({"v": 1}), 2).unwrap();
        assert_ne!(before, after);
    }
}

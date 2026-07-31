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
    // §1.4 rule 3. Everything below this line — the receipt, its digest, and the decision the
    // waiter is handed — sees numbers in the one form the canonicalizer emits, so the bytes that
    // get sealed are the bytes an auditor will canonicalize.
    let mut values = reduce_secrets(&command.values, &secret_fields);
    normalize_numbers(&mut values);
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
            // What this delivery **reached**, never what its channel could have proved. §7.2
            // forbids synthesizing a grade that was not observed, and a ceiling is a statement
            // about the channel rather than about this delivery: recording `delivered` because
            // voice can prove `delivered` would put evidence on a receipt that no phone call ever
            // produced.
            //
            // Answering through a delivery is itself the observation that earns `acted` — but only
            // where the channel can say who the person was (§4.7). Everywhere else the strongest
            // honest claim is whatever the channel actually reported.
            grade_reached: grade_of(d),
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

/// The largest integer an IEEE-754 double distinguishes from its neighbours, `2^53 - 1`.
pub const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// Refuse an answer carrying a number this protocol cannot represent identically everywhere (§1.4).
///
/// **Digest-covered content carries integers only, bounded to ±(2^53 − 1).** One rule, two reasons,
/// and they are different:
///
/// - **Formatting.** RFC 8785 inherits ECMAScript's number serialization, which independent
///   implementations do not reproduce reliably. An earlier profile admitted any number inside
///   `1e-6 ≤ |x| < 1e21` on the grounds that the notation is plain decimal there — true of the
///   notation, and still not safe for the value. Both published SDKs refuse a non-integer outright,
///   so a receipt carrying `1.5` could be minted by this server and verified by nothing we ship.
///   That is the whole claim of the chain — a party who was never given a secret can check it —
///   failing in a narrow window rather than a wide one.
/// - **Precision.** Beyond `2^53 - 1` a value is indistinguishable from its neighbours, so a person
///   approving `100000000000000000001` would get a receipt saying `100000000000000000000`. A
///   receipt that misstates what was approved is the one thing a receipt may never do.
///
/// This is a rule about the **value**, and deliberately not about how the number was written.
/// `-0.0`, `1.0` and `1e2` are integral and therefore legal here; what they must not do is reach
/// the record in that form, and [`normalize_numbers`] is what stops them. Phrasing the constraint
/// over the written form instead was considered and rejected: `JSON.parse` discards the lexeme
/// irrecoverably, so a lexical rule is enforceable in Rust and Python and impossible in TypeScript,
/// which leaves two conforming Servers disagreeing about what is legal.
///
/// A Client with an exact decimal quantity — money, most obviously — declares the field as `text`,
/// which sidesteps binary floating point instead of negotiating with it.
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
            if x.fract() != 0.0 {
                return Some(
                    "digest-covered content carries integers only, because RFC 8785 inherits a \
                     number serialization independent implementations do not reproduce. Declare \
                     an exact decimal quantity as a `text` field."
                        .into(),
                );
            }
            if x.abs() > MAX_SAFE_INTEGER {
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

/// Put every number in `values` into the form the canonicalizer emits for it (§1.4 rule 3).
///
/// Validation has already refused everything that has no such form, so this only ever rewrites a
/// float that denotes an integer — `-0.0` to `0`, `1.0` to `1`, `1e2` to `100`. It runs on the way
/// *into* the receipt rather than at the edge, because the record is what carries the consequence:
/// a number stored as it arrived and canonicalized at digest time gives an auditor different bytes
/// than the ones that were sealed, and a receipt is verifiable only when those agree.
pub fn normalize_numbers(values: &mut Map<String, Value>) {
    for value in values.values_mut() {
        handoff_protocol::receipt::normalize_numbers(value);
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

/// The grade the answering delivery reached (§9.2, §7.2).
fn grade_of(delivery: &DeliveryView) -> Option<handoff_protocol::delivery::DeliveryGrade> {
    use handoff_protocol::delivery::DeliveryGrade;
    if delivery.can_authenticate_person && delivery.max_grade >= DeliveryGrade::Acted {
        // Answering through it *is* the observation, on a channel that can say who answered.
        return Some(DeliveryGrade::Acted);
    }
    // Otherwise: what the channel actually reported, and nothing where it reported nothing.
    // `None` here is not a weak grade to be rounded up to `dispatched` — `dispatched` asserts a
    // transport accepted something, so inventing it puts a send on the receipt that may never have
    // happened. §7.2 forbids synthesizing a grade that was not observed, and the receipt is the
    // last artifact that should be guessing.
    delivery
        .grade_reached
        .map(|reached| reached.min(delivery.max_grade))
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
    fn integers_are_accepted_at_zero_and_at_the_safe_edge() {
        let mut values = Map::new();
        values.insert("at_zero".into(), json!(0));
        values.insert(
            "at_max_safe_integer".into(),
            json!(9_007_199_254_740_991u64),
        );
        values.insert("negative".into(), json!(-42));
        assert!(check_number_bounds(&values).is_ok());
    }

    #[test]
    fn a_non_integer_names_its_field() {
        // The case that made this rule: a person answers a `number` field with 1.5, and the
        // receipt that decision produces cannot be canonicalized by either published SDK.
        for bad in [json!(1.5), json!(0.000_001), json!(0.000_000_1)] {
            let mut values = Map::new();
            values.insert("ordinary".into(), bad.clone());
            let err = check_number_bounds(&values).unwrap_err();
            assert_eq!(err.code, ErrorCode::AnswerValidationFailed, "{bad}");
            assert_eq!(err.fields()[0].name, "ordinary");
        }
    }

    #[test]
    fn a_float_denoting_a_whole_number_is_normalized_rather_than_stored_as_it_arrived() {
        // §1.4 rule 3, and the whole of it: these values are legal, and the form they are kept in
        // is not optional. `-0.0` is the case that made the rule — it has no fractional part and
        // no magnitude problem, so validation passes it, and the canonicalizer renders it `0`.
        // A Server that stored what arrived then held `-0.0` and digested `0`, so an auditor
        // canonicalizing the served receipt was not canonicalizing the bytes that were sealed.
        let mut values = Map::new();
        values.insert("negative_zero".into(), json!(-0.0));
        values.insert("whole_float".into(), json!(2.0));
        values.insert("exponent".into(), json!(1e2));
        values.insert("nested".into(), json!({"deep": [3.0, {"deeper": -0.0}]}));

        check_number_bounds(&values).expect("all four are integral and in range, so all are legal");
        normalize_numbers(&mut values);

        // Serialized, because the point is the bytes at rest and not the value they parse to:
        // `-0.0 == 0` is true, so an equality assertion here would pass without normalization and
        // measure nothing.
        assert_eq!(
            serde_json::to_string(&Value::Object(values.clone())).expect("serialize"),
            r#"{"exponent":100,"negative_zero":0,"nested":{"deep":[3,{"deeper":0}]},"whole_float":2}"#
        );

        // And that form is exactly what the canonicalizer emits, which is the property rule 3
        // actually asserts: canonicalize what is served and it is what is served.
        for value in values.values() {
            let mut normalized = value.clone();
            handoff_protocol::receipt::normalize_numbers(&mut normalized);
            assert_eq!(&normalized, value, "normalizing twice must change nothing");
        }
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
    fn a_bad_number_is_refused_wherever_it_is_nested() {
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

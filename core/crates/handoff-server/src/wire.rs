//! The wire format.
//!
//! Every response body is built here, by hand, rather than by deriving `Serialize` on a domain
//! type. That is a deliberate cost, and the reason is `null`:
//!
//! > `"receipt": null` and a missing `receipt` key are different answers to different questions.
//! > The first says "this request has settled nothing"; the second says "this representation does
//! > not discuss receipts". A client that has to tell "not yet" from "not applicable" needs the
//! > first, and `skip_serializing_if = "Option::is_none"` silently gives it the second.
//!
//! So the fields a client branches on — `receipt`, `authorization`, `answered_at`, `expires_at`,
//! `cancel_reason`, `decision`, `acked_at`, `actor.principal_id`, `chain.prev_digest` — are always
//! present, and carry an explicit `null` when there is nothing to say. The fields §11.1 forbids —
//! anything resolvable on a grant — are absent, because for those the *absence* is the contract.

use handoff_core::model::{DeliveryView, GrantView, RequestView};
use handoff_protocol::authorization::{Authorization, AuthorizationState};
use handoff_protocol::clock::Timestamp;
use handoff_protocol::error::ProtocolError;
use handoff_protocol::receipt::Receipt;
use handoff_protocol::waiter::Signal;
use serde_json::{json, Map, Value};

/// Render a timestamp, or `null`.
fn at(value: Option<Timestamp>) -> Value {
    value.map_or(Value::Null, |t| Value::String(t.to_string()))
}

fn text(value: Option<&str>) -> Value {
    value.map_or(Value::Null, |t| Value::String(t.to_string()))
}

fn name<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// One request, as `GET /requests/{id}` returns it.
pub fn request(view: &RequestView, surface_base: &str) -> Value {
    json!({
        "id": view.id.to_string(),
        "state": name(&view.state),
        "version": view.version,
        "org_id": view.tenant_ref,
        "waiter_ref": view.waiter_ref,
        "urgency": name(&view.urgency),
        "urgency_state": name(&view.urgency_state),
        "prompt": name(&view.prompt),
        "requires": name(&view.requires),
        "mode": name(&view.mode),
        "presentation_binding": name(&view.presentation_binding),
        "liveness": name(&view.liveness),
        "on_waiter_terminal": name(&view.on_waiter_terminal),
        "ttl_policy": view.ttl_policy.as_ref().map_or(Value::Null, name),
        "routing": name(&view.routing),
        "created_at": view.created_at.to_string(),
        "expires_at": at(view.expires_at),
        "attempt_expires_at": at(view.attempt_expires_at),
        "answered_at": at(view.answered_at),
        "superseded_by": view.superseded_by.map_or(Value::Null, |id| json!(id.to_string())),
        "cancel_reason": text(view.cancel_reason.as_deref()),
        // §4.6. A locator, not a capability: opening it prompts for authentication, and knowing
        // the URL grants nothing.
        "surface_url": format!("{}/r/{}", surface_base.trim_end_matches('/'), view.id),
        "deliveries": Value::Array(view.deliveries.iter().map(delivery).collect()),
        "receipt": view.receipt.as_ref().map_or(Value::Null, receipt),
        "authorization": view
            .authorization
            .as_ref()
            .map_or(Value::Null, |a| authorization(a, view.created_at)),
        "waiter": json!({
            "state": name(&view.waiter.state),
            "liveness": name(&view.waiter.liveness),
        }),
        "metadata": Value::Object(view.metadata.clone()),
    })
    // `resume_ref` and `resume_payload` are deliberately absent. §14: returned byte-identical in
    // every signal for that request, and appearing nowhere else — not in a listing, not in a
    // receipt, not in an event, not in a log line.
}

/// One delivery. `max_grade` and `can_authenticate_person` are always present, because §7.2 makes
/// both mandatory declarations and a channel that has not declared them cannot be reasoned about.
pub fn delivery(view: &DeliveryView) -> Value {
    json!({
        "id": view.id.to_string(),
        "request_id": view.request_id.to_string(),
        "channel": view.channel,
        "target": {"kind": name(&view.target.kind), "value": view.target.value},
        "rung": view.rung,
        "state": name(&view.state),
        "grade_reached": view.grade_reached.map_or(Value::Null, |g| name(&g)),
        "max_grade": name(&view.max_grade),
        "can_authenticate_person": view.can_authenticate_person,
        "attempts": Value::Array(
            view.attempts
                .iter()
                .map(|a| json!({
                    "n": a.n,
                    "started_at": a.started_at.to_string(),
                    "ended_at": at(a.ended_at),
                    "outcome": a.outcome,
                    "transport_status": text(a.transport_status.as_deref()),
                    "error": text(a.error.as_deref()),
                }))
                .collect(),
        ),
        "created_at": view.created_at.to_string(),
        "updated_at": view.updated_at.to_string(),
    })
}

/// One receipt, **exactly as it was sealed**.
///
/// This is a straight serialization of the stored record, and that is the whole design. The receipt
/// core `signing.md` §2.2 hashes is "the receipt object excluding its `chain` member" — so the
/// bytes a third party hashes are the bytes this endpoint returned. Re-rendering the receipt here,
/// even into a shape that is arguably nicer, gives an auditor an object that does not reproduce the
/// digest it carries, and the receipt's entire claim is that a party who was never given a secret
/// can check it.
///
/// So there is deliberately no field-by-field construction below. Anything a client needs to see as
/// an explicit `null` is `null` in the record itself.
pub fn receipt(value: &Receipt) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// One authorization, with the state derived at read time rather than stored.
pub fn authorization(value: &Authorization, now: Timestamp) -> Value {
    json!({
        "id": value.id.to_string(),
        "receipt_id": value.receipt_id.to_string(),
        "request_id": value.request_id.to_string(),
        "grants": Value::Object(value.grants.clone()),
        "single_use": value.single_use,
        "expires_at": at(value.expires_at),
        "bound_to": {
            "waiter_ref": text(value.bound_to.waiter_ref.as_deref()),
            "effect_digest": value
                .bound_to
                .effect_digest
                .as_ref()
                .map_or(Value::Null, |d| json!(d.to_string())),
        },
        "redemptions": Value::Array(
            value
                .redemptions
                .iter()
                .map(|r| json!({
                    "effect_key": r.effect_key,
                    "redeemed_at": r.redeemed_at.to_string(),
                }))
                .collect(),
        ),
        "state": match value.state_at(now) {
            AuthorizationState::Open => "open",
            AuthorizationState::Spent => "spent",
            AuthorizationState::Expired => "expired",
        },
    })
}

/// One signal.
///
/// `decision` is present and `null` for `attempt_lapsed`, which decides nothing (§8.3). `acked_at`
/// is present and `null` while the signal is outstanding, because reading a signal does not consume
/// it and a client has to be able to see that it has not.
pub fn signal(value: &Signal) -> Value {
    json!({
        "id": value.id.to_string(),
        "request_id": value.request_id.to_string(),
        "waiter_ref": value.waiter_ref,
        "type": name(&value.signal_type),
        "sequence": value.sequence,
        "resume_token": value.resume_token.to_string(),
        "decision": value.decision.as_ref().map_or(Value::Null, name),
        "resume_ref": text(value.resume_ref.as_deref()),
        "resume_payload": text(value.resume_payload.as_deref()),
        "attempts": value.attempts,
        "created_at": value.created_at.to_string(),
        "acked_at": at(value.acked_at),
    })
}

/// One capability grant, as the surface reads it before the person accepts.
///
/// There is no `transport` key and no `url` key, at any nesting. §11.1: the protocol MUST NOT
/// carry a resolvable address by value — not in a request, not in a receipt, not in a delivery
/// body, not in an event, not in a waiter signal, and not in any message sent to a channel.
pub fn grant(value: &GrantView) -> Value {
    json!({
        "handle": value.handle.to_string(),
        "request_id": value.request_id.to_string(),
        "type": value.capability_type,
        "scope": name(&value.scope),
        "provider": text(value.provider.as_deref()),
        "resource_ref": text(value.resource_ref.as_deref()),
        "label": text(value.label.as_deref()),
        "purpose": text(value.purpose.as_deref()),
        "optional": value.optional,
        "blast_radius": name(&value.blast_radius),
        "blast_radius_digest": value.blast_radius_digest.to_string(),
        "expires_at": value.expires_at.to_string(),
        "revoked_at": at(value.revoked_at),
        "max_holders": value.max_holders,
        "bound_principal_id": text(value.bound_principal.as_deref()),
    })
}

/// The error envelope. One shape across the whole surface (§13).
pub fn error(err: &ProtocolError) -> Value {
    let mut body = Map::new();
    body.insert("code".into(), json!(err.code.to_string()));
    body.insert("message".into(), json!(err.message));
    let context = err.context();
    if let Some(id) = &context.request_id {
        body.insert("request_id".into(), json!(id));
    }
    if let Some(id) = &context.receipt_id {
        body.insert("receipt_id".into(), json!(id));
    }
    if let Some(id) = &context.superseded_by {
        body.insert("superseded_by".into(), json!(id));
    }
    if !context.fields.is_empty() {
        body.insert(
            "fields".into(),
            Value::Array(
                context
                    .fields
                    .iter()
                    .map(|f| {
                        json!({
                            "name": f.name,
                            "code": name(&f.code),
                            // §12 rule 6: never the submitted value, not even in a validation error.
                            // Echoing the rejected value is the single most common way a secret leaks.
                            "message": text(f.message.as_deref()),
                        })
                    })
                    .collect(),
            ),
        );
    }
    json!({ "error": Value::Object(body) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use handoff_protocol::error::{ErrorCode, FieldError, FieldErrorCode};

    #[test]
    fn a_validation_error_names_the_field_and_carries_no_value() {
        let err = ProtocolError::new(
            ErrorCode::AnswerValidationFailed,
            "the answer does not satisfy",
        )
        .with_fields(vec![FieldError {
            name: "password".into(),
            code: FieldErrorCode::SecretValueNotPermitted,
            message: Some("secret values go to the sink".into()),
        }]);
        let body = error(&err);
        assert_eq!(body["error"]["code"], json!("answer_validation_failed"));
        assert_eq!(body["error"]["fields"][0]["name"], json!("password"));
        assert_eq!(
            body["error"]["fields"][0]["code"],
            json!("secret_value_not_permitted")
        );
    }

    #[test]
    fn a_conflict_carries_the_record_that_settled_it() {
        let err = ProtocolError::new(ErrorCode::AlreadyAnswered, "already answered")
            .with_receipt("rcpt_01K3MB2R4Y8ZC4YRXB2N6VD9FT");
        let body = error(&err);
        assert_eq!(
            body["error"]["receipt_id"],
            json!("rcpt_01K3MB2R4Y8ZC4YRXB2N6VD9FT")
        );
    }
}

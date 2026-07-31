//! The `/v1` surface.
//!
//! Handlers are thin on purpose. Anything that decides something lives in `handoff-core`; anything
//! that writes lives in the store, where it is inside a transaction. What is left here is parsing,
//! authentication, idempotency, and rendering — and the order those happen in, which is itself
//! load-bearing in two places:
//!
//! - **Authentication resolves the tenant** (§4.1, I13). No handler reads a tenant from a body, and
//!   `metadata.org_id` is carried verbatim and never obeyed (C-20).
//! - **The requester ≠ decider check is by principal type** (§4.2, I15) and it is made in the store
//!   before the request row is even read, so no ordering of later checks can reach past it.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{delete, get, post};
use axum::Router;
use handoff_core::auth::{Principal, PrincipalKind};
use handoff_core::capability::ProviderResource;
use handoff_core::ids;
use handoff_core::model::*;
use handoff_core::ports::{ResolveGrant, Store};
use handoff_protocol::authorization::AuthorizationState;
use handoff_protocol::clock::IsoDuration;
use handoff_protocol::error::{ErrorCode, ProtocolError, Result};
use handoff_protocol::id::{DeliveryId, GrantHandle, GrantSessionRef};
use handoff_protocol::receipt::Digest;
use handoff_protocol::request::{Disposition, RaiseRequest, RequestState};
use handoff_protocol::requires::{CapabilityScope, Target};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::http::*;
use crate::state::AppState;
use crate::wire;

/// Build the router.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/v1/requests", post(raise).get(list_requests))
        .route("/v1/requests/{request_id}", get(get_request))
        .route("/v1/requests/{request_id}/amend", post(amend))
        .route("/v1/requests/{request_id}/cancel", post(cancel))
        .route("/v1/requests/{request_id}/supersede", post(supersede))
        .route("/v1/requests/{request_id}/escalate", post(escalate))
        .route("/v1/requests/{request_id}/reassign", post(reassign))
        .route("/v1/requests/{request_id}/attempt", post(arm_attempt))
        .route("/v1/requests/{request_id}/answer", post(answer))
        .route("/v1/requests/{request_id}/receipt", get(request_receipt))
        .route(
            "/v1/requests/{request_id}/deliveries",
            get(request_deliveries),
        )
        .route("/v1/receipts", get(list_receipts))
        .route("/v1/receipts/chain-head", get(chain_head))
        .route("/v1/receipts/{receipt_id}", get(get_receipt))
        .route("/v1/waiters/{waiter_ref}/signals", get(poll_signals))
        .route("/v1/waiters/{waiter_ref}/reattach", post(reattach))
        .route("/v1/signals/{signal_id}/ack", post(ack))
        .route("/v1/signals/{signal_id}/attempts", get(signal_attempts))
        .route(
            "/v1/authorizations/{authorization_id}",
            get(get_authorization),
        )
        .route("/v1/authorizations/{authorization_id}/redeem", post(redeem))
        .route("/v1/grants/{handle}", get(get_grant).delete(revoke_grant))
        .route("/v1/grants/{handle}/sessions", post(resolve_grant))
        .route(
            "/v1/grants/{handle}/sessions/{session_ref}/renew",
            post(renew_session),
        )
        .route(
            "/v1/grants/{handle}/sessions/{session_ref}",
            delete(release_session),
        )
        .route("/v1/sinks/{sink_ref}/values", post(submit_sink_values))
        .route("/v1/deliveries/{delivery_id}", get(get_delivery))
        .route("/v1/deliveries/{delivery_id}/redeliver", post(redeliver))
        .route("/v1/deliveries/{delivery_id}/grade", post(record_grade))
        .route("/v1/meta", get(meta))
        .with_state(state)
}

// ------------------------------------------------------------------------------ authentication

/// Resolve the caller, or say which kind of failure it was.
///
/// §13: `invalid_api_key` covers absent, malformed, revoked, and expired credentials as one code,
/// deliberately — a distinct "revoked" code tells an attacker which keys once existed. A caller who
/// presented nothing at all gets `authentication_required`, which is a different fact.
async fn caller(state: &AppState, headers: &HeaderMap) -> Result<Principal> {
    let bearer = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string);
    let cookie = headers
        .get("Cookie")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.split(';')
                .map(str::trim)
                .find_map(|part| part.strip_prefix("handoff_session="))
        })
        .map(str::to_string);

    let Some(secret) = bearer.or(cookie) else {
        return Err(ProtocolError::new(
            ErrorCode::AuthenticationRequired,
            "this operation requires an authenticated caller",
        ));
    };
    // Where the credential is verified is a deployment's own business; what it resolves to is not.
    // Both paths return the same `Principal`, so nothing downstream can tell which one ran — that
    // sameness is the point, and it is what stops a hosted deployment acquiring an authorization
    // path the open core does not have.
    match &state.deployment.authenticator {
        Some(authenticator) => authenticator.authenticate(secret).await,
        None => state.store.authenticate(secret).await,
    }?
    .ok_or_else(|| ProtocolError::new(ErrorCode::InvalidApiKey, "the credential is not valid"))
}

/// A caller who must be a person: answering, resolving a grant, submitting to a sink.
fn require_person(principal: &Principal) -> Result<()> {
    if principal.kind == PrincipalKind::Machine {
        return Err(ProtocolError::new(
            ErrorCode::RequesterMayNotAnswer,
            "a service_account principal may not perform this operation",
        ));
    }
    Ok(())
}

fn parse_id<T: handoff_protocol::id::IdKind>(
    raw: &str,
    missing: ErrorCode,
) -> Result<handoff_protocol::id::Id<T>> {
    handoff_protocol::id::Id::<T>::parse(raw)
        .map_err(|_| ProtocolError::new(missing, "no such object in this tenant"))
}

// ------------------------------------------------------------------------------------- requests

async fn raise(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> ApiResult {
    let principal = caller(&state, &headers).await?;
    principal.require_scope("handoff:requests:write")?;
    let body = body_json(&body)?;
    let key = idempotency_key(&headers, &body)?;
    let digest = body_digest(&body)?;
    let now = state.now();

    // Fails closed on an unknown envelope version, field type, or capability type, and creates
    // nothing (§5.2, §19, I21, C-16).
    let mut raise = RaiseRequest::parse(&body, &state.profile)?;

    // §14 requires a Server that stores `resume_payload` to encrypt it at rest, and this
    // deployment implements no encryption at rest. §14 also states the alternative plainly: a
    // Level 1 Server MUST accept and ignore these fields, or reject them with
    // `400 invalid_request`. Rejecting is the honest one — accepting and quietly keeping a
    // runtime's private state in the clear, under a comment claiming otherwise, is the failure
    // this refusal exists to prevent. `resume_ref` is a pointer the runtime owns, carries no
    // secret, and has no such requirement, so it is still accepted and returned verbatim.
    if raise.continuation.resume_payload.is_some() && !state.config.continuation_supported {
        return Err(ProtocolError::new(
            ErrorCode::InvalidRequest,
            "this deployment does not implement encryption at rest for `resume_payload`, which \
             §14 requires of any Server that stores it, so the field is refused rather than kept \
             in the clear. `GET /meta` reports the conformance level and extensions this build \
             actually implements.",
        )
        .into());
    }

    // §11.4. Grants are minted here, server-side, and the handle a client declared is replaced by
    // one from a CSPRNG. §11.1 forbids deriving a handle from anything recomputable, and a handle
    // the caller chose is a handle the caller can predict for somebody else's request.
    let mut grants = Vec::new();
    for declaration in &mut raise.requires.capabilities {
        let provider = state.capabilities.provider(declaration.provider.as_deref());
        let blast_radius = provider.blast_radius(declaration);
        let blast_radius_digest = blast_radius.digest()?;
        let handle = ids::mint_random::<handoff_protocol::id::Grant>()?;
        declaration.handle = handle;
        declaration.blast_radius_digest = Some(blast_radius_digest.to_string());
        grants.push(GrantToMint {
            handle,
            capability_type: declaration.capability_type.clone(),
            scope: declaration.scope,
            provider: declaration.provider.clone(),
            resource_ref: declaration.resource_ref.clone(),
            label: declaration.label.clone(),
            purpose: declaration.purpose.clone(),
            optional: declaration.optional,
            blast_radius,
            blast_radius_digest,
            expires_at: now.saturating_add(declaration.ttl.unwrap_or(raise.attempt_ttl)),
            max_holders: 1,
        });
    }

    // §7.4. The ladder is deployment policy, resolved server-side and snapshotted onto the request,
    // so a policy edit mid-flight cannot retroactively change what happened.
    let routing = state.channels.resolve_ladder(raise.routing.as_ref());
    let expires_at = raise.ttl.map(|ttl| now.saturating_add(ttl));
    let dedupe_key = raise.effective_dedupe_key()?;

    let result = state
        .store
        .raise(RaiseCommand {
            principal,
            idempotency_key: key,
            body_digest: digest,
            raise,
            dedupe_key,
            routing,
            grants,
            deliveries: Vec::new(),
            expires_at,
            now,
        })
        .await?;

    Ok(Api(
        StatusCode::from_u16(result.status).unwrap_or(StatusCode::CREATED),
        wire::request(&result.request, &state.config.public_base),
    ))
}

#[derive(Debug, serde::Deserialize)]
struct ListQuery {
    waiter_ref: Option<String>,
    #[serde(default)]
    state: Vec<String>,
    limit: Option<i64>,
}

async fn list_requests(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(raw): Query<BTreeMap<String, String>>,
) -> ApiResult {
    let principal = caller(&state, &headers).await?;
    principal.require_scope("handoff:requests:read")?;
    let query = ListQuery {
        waiter_ref: raw.get("waiter_ref").cloned(),
        state: raw
            .get("state")
            .map(|s| vec![s.clone()])
            .unwrap_or_default(),
        limit: raw.get("limit").and_then(|l| l.parse().ok()),
    };
    let states: Vec<RequestState> = query
        .state
        .iter()
        .filter_map(|s| serde_json::from_value(Value::String(s.clone())).ok())
        .collect();

    let requests = state
        .store
        .list_requests(
            principal.tenant_ref.clone(),
            RequestFilter {
                waiter_ref: query.waiter_ref,
                states,
                limit: query.limit.unwrap_or(50),
            },
        )
        .await?;

    Ok(Api::ok(json!({
        "data": requests
            .iter()
            .map(|r| wire::request(r, &state.config.public_base))
            .collect::<Vec<_>>(),
        "has_more": false,
        "next_cursor": Value::Null,
    })))
}

async fn get_request(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    Query(raw): Query<BTreeMap<String, String>>,
) -> ApiResult {
    let principal = caller(&state, &headers).await?;
    principal.require_scope("handoff:requests:read")?;
    let id = parse_id::<handoff_protocol::id::Request>(&request_id, ErrorCode::RequestNotFound)?;
    let wait = raw
        .get("wait")
        .and_then(|w| w.parse::<u64>().ok())
        .unwrap_or(0);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(wait.min(30));
    loop {
        let request = state
            .store
            .get_request(principal.tenant_ref.clone(), id)
            .await?
            // §3.2 rule 3. An object in another tenant is `404`, not `403`, so that existence is
            // not disclosed.
            .ok_or_else(|| ProtocolError::new(ErrorCode::RequestNotFound, "no such request"))?;
        if request.state != RequestState::Pending || std::time::Instant::now() >= deadline {
            return Ok(Api::ok(wire::request(&request, &state.config.public_base)));
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

async fn amend(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    body: axum::body::Bytes,
) -> ApiResult {
    let principal = caller(&state, &headers).await?;
    principal.require_scope("handoff:requests:write")?;
    let body = body_json(&body)?;
    let digest = body_digest(&body)?;
    let key = idempotency_key(&headers, &body)?;
    let id = parse_id::<handoff_protocol::id::Request>(&request_id, ErrorCode::RequestNotFound)?;
    let slot = slot(
        &principal,
        "amend",
        &id.to_string(),
        key.as_deref(),
        &digest,
    );
    if let Some(replayed) = replay(state.store.as_ref(), &slot).await? {
        return Ok(replayed);
    }
    let now = state.now();

    let patch = AmendPatch {
        prompt: match body.get("prompt") {
            Some(v) if !v.is_null() => Some(serde_json::from_value(v.clone()).map_err(|e| {
                ProtocolError::new(ErrorCode::InvalidRequest, format!("`prompt`: {e}"))
            })?),
            _ => None,
        },
        requires: match body.get("requires") {
            Some(v) if !v.is_null() => Some(handoff_protocol::requires::Requires::parse(
                v,
                &state.profile,
            )?),
            _ => None,
        },
    };

    let view = state
        .store
        .amend(
            RequestCommand {
                request_id: id,
                principal: principal.clone(),
                idempotency_key: key,
                body_digest: digest,
                now,
            },
            patch,
        )
        .await?;
    let rendered = wire::request(&view, &state.config.public_base);
    remember(state.store.as_ref(), slot, StatusCode::OK, &rendered, now).await?;
    Ok(Api::ok(rendered))
}

async fn cancel(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    body: axum::body::Bytes,
) -> ApiResult {
    let principal = caller(&state, &headers).await?;
    principal.require_scope("handoff:requests:write")?;
    let body = body_json(&body)?;
    let digest = body_digest(&body)?;
    let key = idempotency_key(&headers, &body)?;
    let id = parse_id::<handoff_protocol::id::Request>(&request_id, ErrorCode::RequestNotFound)?;
    let slot = slot(
        &principal,
        "cancel",
        &id.to_string(),
        key.as_deref(),
        &digest,
    );
    if let Some(replayed) = replay(state.store.as_ref(), &slot).await? {
        return Ok(replayed);
    }
    let reason = body
        .get("reason")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ProtocolError::new(ErrorCode::InvalidRequest, "`reason` is required"))?
        .to_string();
    let now = state.now();

    let view = state
        .store
        .cancel(
            RequestCommand {
                request_id: id,
                principal,
                idempotency_key: key,
                body_digest: digest,
                now,
            },
            reason,
        )
        .await?;
    let rendered = wire::request(&view, &state.config.public_base);
    remember(state.store.as_ref(), slot, StatusCode::OK, &rendered, now).await?;
    Ok(Api::ok(rendered))
}

async fn supersede(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    body: axum::body::Bytes,
) -> ApiResult {
    let principal = caller(&state, &headers).await?;
    principal.require_scope("handoff:requests:write")?;
    let body = body_json(&body)?;
    let digest = body_digest(&body)?;
    let key = idempotency_key(&headers, &body)?;
    let id = parse_id::<handoff_protocol::id::Request>(&request_id, ErrorCode::RequestNotFound)?;
    let slot = slot(
        &principal,
        "supersede",
        &id.to_string(),
        key.as_deref(),
        &digest,
    );
    if let Some(replayed) = replay(state.store.as_ref(), &slot).await? {
        return Ok(replayed);
    }
    let by = body
        .get("by")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ProtocolError::new(ErrorCode::InvalidRequest, "`by` is required"))?;
    let by = parse_id::<handoff_protocol::id::Request>(by, ErrorCode::RequestNotFound)?;
    let now = state.now();

    let view = state
        .store
        .supersede(
            RequestCommand {
                request_id: id,
                principal,
                idempotency_key: key,
                body_digest: digest,
                now,
            },
            by,
        )
        .await?;
    let rendered = wire::request(&view, &state.config.public_base);
    remember(state.store.as_ref(), slot, StatusCode::OK, &rendered, now).await?;
    Ok(Api::ok(rendered))
}

async fn escalate(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    body: axum::body::Bytes,
) -> ApiResult {
    let principal = caller(&state, &headers).await?;
    // §7.4. Overriding or advancing the ladder needs a scope of its own: a compromised key that can
    // ask a question is a materially different blast radius from one that can page an on-call
    // engineer at 3 a.m.
    principal.require_scope("handoff:requests:route")?;
    let body = body_json(&body)?;
    let digest = body_digest(&body)?;
    let key = idempotency_key(&headers, &body)?;
    let id = parse_id::<handoff_protocol::id::Request>(&request_id, ErrorCode::RequestNotFound)?;
    let slot = slot(
        &principal,
        "escalate",
        &id.to_string(),
        key.as_deref(),
        &digest,
    );
    if let Some(replayed) = replay(state.store.as_ref(), &slot).await? {
        return Ok(replayed);
    }
    let rung = body.get("rung").and_then(|v| v.as_u64()).map(|r| r as u32);
    let now = state.now();

    let view = state
        .store
        .escalate(
            RequestCommand {
                request_id: id,
                principal,
                idempotency_key: key,
                body_digest: digest,
                now,
            },
            rung,
        )
        .await?;
    let rendered = wire::request(&view, &state.config.public_base);
    remember(state.store.as_ref(), slot, StatusCode::OK, &rendered, now).await?;
    Ok(Api::ok(rendered))
}

async fn reassign(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    body: axum::body::Bytes,
) -> ApiResult {
    let principal = caller(&state, &headers).await?;
    principal.require_scope("handoff:requests:route")?;
    let body = body_json(&body)?;
    let digest = body_digest(&body)?;
    let key = idempotency_key(&headers, &body)?;
    let id = parse_id::<handoff_protocol::id::Request>(&request_id, ErrorCode::RequestNotFound)?;
    let slot = slot(
        &principal,
        "reassign",
        &id.to_string(),
        key.as_deref(),
        &digest,
    );
    if let Some(replayed) = replay(state.store.as_ref(), &slot).await? {
        return Ok(replayed);
    }
    let to: Target = serde_json::from_value(
        body.get("to")
            .cloned()
            .ok_or_else(|| ProtocolError::new(ErrorCode::InvalidRequest, "`to` is required"))?,
    )
    .map_err(|e| ProtocolError::new(ErrorCode::InvalidRequest, format!("`to`: {e}")))?;
    let reason = body
        .get("reason")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let now = state.now();

    let view = state
        .store
        .reassign(
            RequestCommand {
                request_id: id,
                principal,
                idempotency_key: key,
                body_digest: digest,
                now,
            },
            to,
            reason,
        )
        .await?;
    let rendered = wire::request(&view, &state.config.public_base);
    remember(state.store.as_ref(), slot, StatusCode::OK, &rendered, now).await?;
    Ok(Api::ok(rendered))
}

async fn arm_attempt(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    body: axum::body::Bytes,
) -> ApiResult {
    let principal = caller(&state, &headers).await?;
    principal.require_scope("handoff:requests:write")?;
    let body = body_json(&body)?;
    let digest = body_digest(&body)?;
    let key = idempotency_key(&headers, &body)?;
    let id = parse_id::<handoff_protocol::id::Request>(&request_id, ErrorCode::RequestNotFound)?;
    let slot = slot(
        &principal,
        "attempt",
        &id.to_string(),
        key.as_deref(),
        &digest,
    );
    if let Some(replayed) = replay(state.store.as_ref(), &slot).await? {
        return Ok(replayed);
    }
    let ttl = match body.get("ttl").and_then(|v| v.as_str()) {
        Some(text) => Some(IsoDuration::parse(text)?),
        None => None,
    };
    let now = state.now();

    let view = state
        .store
        .arm_attempt(
            RequestCommand {
                request_id: id,
                principal,
                idempotency_key: key,
                body_digest: digest,
                now,
            },
            ttl,
        )
        .await?;
    let rendered = wire::request(&view, &state.config.public_base);
    remember(state.store.as_ref(), slot, StatusCode::OK, &rendered, now).await?;
    Ok(Api::ok(rendered))
}

async fn answer(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    body: axum::body::Bytes,
) -> ApiResult {
    let principal = caller(&state, &headers).await?;
    let body = body_json(&body)?;

    let values: Map<String, Value> = match body.get("values") {
        Some(Value::Object(map)) => map.clone(),
        Some(_) => {
            return Err(
                ProtocolError::new(ErrorCode::InvalidRequest, "`values` must be an object").into(),
            )
        }
        None => Map::new(),
    };

    // §1.4 rule 3, and it runs *before* the body digest on purpose. The digest is taken over the
    // canonical form, and canonicalization refuses a number outside the band — so digesting first
    // would answer a numeric bound with `400 invalid_request` and no field name, when the
    // specification requires `422 answer_validation_failed` naming the offending field.
    handoff_core::plan::check_number_bounds(&values)?;

    let digest = body_digest(&body)?;
    let key = idempotency_key(&headers, &body)?;

    let id = parse_id::<handoff_protocol::id::Request>(&request_id, ErrorCode::RequestNotFound)?;

    // §6.7 rule 3. A retried click is not a conflict: the same key as the answer that landed
    // returns `200` with the original receipt, and it does so before any state check.
    //
    // Scoped to **this** request. The same key against a different request is not a retry of this
    // answer, and replaying one here would hand the caller a decision about something else.
    let slot = slot(
        &principal,
        "answer",
        &id.to_string(),
        key.as_deref(),
        &digest,
    );
    if let Some(replayed) = replay(state.store.as_ref(), &slot).await? {
        return Ok(replayed);
    }

    let now = state.now();
    let disposition: Disposition = match body.get("disposition").and_then(|v| v.as_str()) {
        Some(text) => serde_json::from_value(Value::String(text.to_string()))
            .map_err(|_| ProtocolError::new(ErrorCode::InvalidRequest, "unknown `disposition`"))?,
        None => Disposition::Decide,
    };
    let capability_uses = body
        .get("capability_uses")
        .and_then(|v| v.as_array())
        .map(|uses| {
            uses.iter()
                .filter_map(|u| {
                    Some(CapabilityUse {
                        handle: GrantHandle::parse(u.get("handle")?.as_str()?).ok()?,
                        session_ref: GrantSessionRef::parse(u.get("session_ref")?.as_str()?)
                            .ok()?,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let result = state
        .store
        .answer(AnswerCommand {
            request_id: id,
            principal,
            idempotency_key: key,
            body_digest: digest,
            values,
            note: body
                .get("note")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            disposition,
            delegate_to: body
                .get("delegate_to")
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok()),
            via_delivery_id: body
                .get("via_delivery_id")
                .and_then(|v| v.as_str())
                .and_then(|v| DeliveryId::parse(v).ok()),
            partial: body
                .get("partial")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            capability_uses,
            rendered_digest: body
                .get("rendered_digest")
                .and_then(|v| v.as_str())
                .and_then(|v| Digest::parse(v).ok()),
            now,
        })
        .await?;

    let rendered = json!({
        "request": {
            "id": result.request.id.to_string(),
            "state": serde_json::to_value(result.request.state).unwrap_or(Value::Null),
            "answered_at": result
                .request
                .answered_at
                .map_or(Value::Null, |t| json!(t.to_string())),
        },
        "receipt": result.receipt.as_ref().map_or(Value::Null, |r| json!({
            "id": r.id.to_string(),
            "digest": r.chain.as_ref().map_or(Value::Null, |c| json!(c.digest.to_string())),
        })),
        "authorization": result.authorization.as_ref().map_or(Value::Null, |a| json!({
            "id": a.id.to_string(),
            "single_use": a.single_use,
            "expires_at": a.expires_at.map_or(Value::Null, |t| json!(t.to_string())),
        })),
    });
    remember(state.store.as_ref(), slot, StatusCode::OK, &rendered, now).await?;
    Ok(Api::ok(rendered))
}

async fn request_receipt(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
) -> ApiResult {
    let principal = caller(&state, &headers).await?;
    principal.require_scope("handoff:requests:read")?;
    let id = parse_id::<handoff_protocol::id::Request>(&request_id, ErrorCode::RequestNotFound)?;
    // §9: `404 request_not_found` while the request is still pending — a receipt exists only once
    // a decision has been made, whether by a person or by an expiry policy.
    let receipt = state
        .store
        .request_receipt(principal.tenant_ref.clone(), id)
        .await?
        .ok_or_else(|| {
            ProtocolError::new(ErrorCode::RequestNotFound, "no receipt for this request")
        })?;
    Ok(Api::ok(wire::receipt(&receipt)))
}

async fn request_deliveries(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
) -> ApiResult {
    let principal = caller(&state, &headers).await?;
    principal.require_scope("handoff:requests:read")?;
    let id = parse_id::<handoff_protocol::id::Request>(&request_id, ErrorCode::RequestNotFound)?;
    let deliveries = state
        .store
        .deliveries(principal.tenant_ref.clone(), id)
        .await?;
    Ok(Api::ok(json!({
        "data": deliveries.iter().map(wire::delivery).collect::<Vec<_>>(),
    })))
}

// ------------------------------------------------------------------------------------- receipts

async fn list_receipts(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult {
    let principal = caller(&state, &headers).await?;
    principal.require_scope("handoff:receipts:read")?;
    let export = state.store.chain(principal.tenant_ref.clone()).await?;
    Ok(Api::ok(json!({
        "data": export.receipts.iter().map(wire::receipt).collect::<Vec<_>>(),
        "has_more": false,
        "next_cursor": Value::Null,
    })))
}

async fn chain_head(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult {
    let principal = caller(&state, &headers).await?;
    principal.require_scope("handoff:receipts:read")?;
    let export = state.store.chain(principal.tenant_ref.clone()).await?;
    let head = export.head.ok_or_else(|| {
        ProtocolError::new(
            ErrorCode::RequestNotFound,
            "this tenant has no receipts yet, so it has no chain head",
        )
    })?;
    Ok(Api::ok(json!({
        "org_id": head.org_id.to_string(),
        "height": head.height,
        "head_digest": head.head_digest.to_string(),
        "as_of": head.as_of.to_string(),
    })))
}

async fn get_receipt(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(receipt_id): Path<String>,
) -> ApiResult {
    let principal = caller(&state, &headers).await?;
    principal.require_scope("handoff:receipts:read")?;
    let id = parse_id::<handoff_protocol::id::Receipt>(&receipt_id, ErrorCode::RequestNotFound)?;
    let receipt = state
        .store
        .receipt(principal.tenant_ref.clone(), id)
        .await?
        .ok_or_else(|| ProtocolError::new(ErrorCode::RequestNotFound, "no such receipt"))?;
    Ok(Api::ok(wire::receipt(&receipt)))
}

// -------------------------------------------------------------------------------------- waiters

async fn poll_signals(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(waiter_ref): Path<String>,
    Query(raw): Query<BTreeMap<String, String>>,
) -> ApiResult {
    let principal = caller(&state, &headers).await?;
    principal.require_scope("handoff:waiters:wait")?;
    let wait = raw
        .get("wait")
        .and_then(|w| w.parse::<u64>().ok())
        .unwrap_or(0);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(wait.min(30));

    loop {
        // §8.3. Reading a signal MUST NOT consume it. Consumption is the ack, and this handler
        // performs no write at all.
        let signals = state
            .store
            .signals(principal.tenant_ref.clone(), waiter_ref.clone())
            .await?;
        if !signals.is_empty() {
            return Ok(Api::ok(json!({
                "data": signals.iter().map(wire::signal).collect::<Vec<_>>(),
                "has_more": false,
            })));
        }
        if std::time::Instant::now() >= deadline {
            return if wait == 0 {
                Ok(Api::ok(json!({"data": [], "has_more": false})))
            } else {
                Ok(Api(StatusCode::NO_CONTENT, Value::Null))
            };
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

async fn reattach(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(waiter_ref): Path<String>,
) -> ApiResult {
    let principal = caller(&state, &headers).await?;
    principal.require_scope("handoff:waiters:wait")?;
    let view = state
        .store
        .reattach(principal.tenant_ref.clone(), waiter_ref)
        .await?;
    Ok(Api::ok(json!({
        "waiter_ref": view.waiter_ref,
        "state": serde_json::to_value(view.state).unwrap_or(Value::Null),
        "open_requests": view.open_requests.iter().map(|id| id.to_string()).collect::<Vec<_>>(),
        "signals": view.signals.iter().map(wire::signal).collect::<Vec<_>>(),
    })))
}

async fn ack(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(signal_id): Path<String>,
    body: axum::body::Bytes,
) -> ApiResult {
    let principal = caller(&state, &headers).await?;
    principal.require_scope("handoff:waiters:wait")?;
    let body = body_json(&body)?;
    let id = parse_id::<handoff_protocol::id::Signal>(&signal_id, ErrorCode::SignalNotFound)?;
    let token = body
        .get("resume_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ProtocolError::new(ErrorCode::InvalidRequest, "`resume_token` is required"))?
        .to_string();
    let applied = body
        .get("applied")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    // Idempotent under `signal_id` itself, not under a header (§3.5). The second ack returns `200`
    // with `first_ack: false`, because a second ack is a retry and not a second application.
    let result = state
        .store
        .ack(
            principal.tenant_ref.clone(),
            AckCommand {
                signal_id: id,
                resume_token: token,
                applied,
                reason: body
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                now: state.now(),
            },
        )
        .await?
        .ok_or_else(|| ProtocolError::new(ErrorCode::SignalNotFound, "no such signal"))?;

    Ok(Api::ok(json!({
        "acked_at": result.acked_at.to_string(),
        "first_ack": result.first_ack,
    })))
}

async fn signal_attempts(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(signal_id): Path<String>,
) -> ApiResult {
    let principal = caller(&state, &headers).await?;
    principal.require_scope("handoff:waiters:wait")?;
    let id = parse_id::<handoff_protocol::id::Signal>(&signal_id, ErrorCode::SignalNotFound)?;
    let attempts = state
        .store
        .signal_attempts(principal.tenant_ref.clone(), id)
        .await?
        .ok_or_else(|| ProtocolError::new(ErrorCode::SignalNotFound, "no such signal"))?;
    Ok(Api::ok(json!({
        "data": attempts
            .iter()
            .map(|a| json!({
                "n": a.n,
                "started_at": a.started_at.to_string(),
                "ended_at": a.ended_at.map_or(Value::Null, |t| json!(t.to_string())),
                "status_code": a.status_code.map_or(Value::Null, |c| json!(c)),
                "duration_ms": a.duration_ms.map_or(Value::Null, |d| json!(d)),
                "outcome": a.outcome,
                "error": a.error.as_deref().map_or(Value::Null, |e| json!(e)),
            }))
            .collect::<Vec<_>>(),
    })))
}

// ------------------------------------------------------------------------------- authorizations

async fn get_authorization(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(authorization_id): Path<String>,
) -> ApiResult {
    let principal = caller(&state, &headers).await?;
    principal.require_scope("handoff:requests:read")?;
    let id = parse_id::<handoff_protocol::id::Authorization>(
        &authorization_id,
        ErrorCode::AuthorizationNotFound,
    )?;
    let authorization = state
        .store
        .authorization(principal.tenant_ref.clone(), id)
        .await?
        .ok_or_else(|| {
            ProtocolError::new(ErrorCode::AuthorizationNotFound, "no such authorization")
        })?;
    Ok(Api::ok(wire::authorization(&authorization, state.now())))
}

async fn redeem(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(authorization_id): Path<String>,
    body: axum::body::Bytes,
) -> ApiResult {
    let principal = caller(&state, &headers).await?;
    principal.require_scope("handoff:authorizations:redeem")?;
    let body = body_json(&body)?;
    let id = parse_id::<handoff_protocol::id::Authorization>(
        &authorization_id,
        ErrorCode::AuthorizationNotFound,
    )?;
    let effect_key = body
        .get("effect_key")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ProtocolError::new(ErrorCode::InvalidRequest, "`effect_key` is required"))?
        .to_string();

    let now = state.now();

    // `openapi.yaml`: an authorization past `expires_at` is `409 authorization_expired`. Checked
    // here because the decision was real and is on the record — it is simply no longer spendable,
    // and saying that is different from saying it was spent or that it never existed.
    if let Some(authorization) = state
        .store
        .authorization(principal.tenant_ref.clone(), id)
        .await?
    {
        if authorization.state_at(now) == AuthorizationState::Expired {
            return Err(ProtocolError::new(
                ErrorCode::AuthorizationExpired,
                "this authorization is past its expiry and can no longer be spent",
            )
            .into());
        }
    }

    // Idempotent under `effect_key` in the body, not under a header (§3.5). That is the whole
    // point: a retried agent turn presents the same effect key and must not spend twice.
    let outcome = state
        .store
        .redeem(
            principal.tenant_ref.clone(),
            RedeemCommand {
                authorization_id: id,
                effect_key,
                effect_digest: body
                    .get("effect_digest")
                    .and_then(|v| v.as_str())
                    .and_then(|v| Digest::parse(v).ok()),
                now,
            },
        )
        .await?
        .ok_or_else(|| {
            ProtocolError::new(ErrorCode::AuthorizationNotFound, "no such authorization")
        })?;

    Ok(Api::ok(json!({
        "redeemed_at": outcome.redeemed_at.to_string(),
        "first_redemption": outcome.first_redemption,
    })))
}

// --------------------------------------------------------------------------------- capabilities

async fn get_grant(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(handle): Path<String>,
) -> ApiResult {
    let principal = caller(&state, &headers).await?;
    // §11.2: a machine principal never resolves, and never needs to read, a capability.
    require_person(&principal)?;
    let handle = parse_id::<handoff_protocol::id::Grant>(&handle, ErrorCode::CapabilityNotFound)?;
    let grant = state
        .store
        .grant(principal.tenant_ref.clone(), handle)
        .await?
        .ok_or_else(|| {
            ProtocolError::new(ErrorCode::CapabilityNotFound, "no such capability grant")
        })?;
    Ok(Api::ok(wire::grant(&grant)))
}

async fn revoke_grant(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(handle): Path<String>,
    body: axum::body::Bytes,
) -> ApiResult {
    let principal = caller(&state, &headers).await?;
    require_person(&principal)?;
    let body = body_json(&body)?;
    let handle = parse_id::<handoff_protocol::id::Grant>(&handle, ErrorCode::CapabilityNotFound)?;
    let revoked = state
        .store
        .revoke_grant(
            principal.tenant_ref.clone(),
            handle,
            body.get("reason")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            state.now(),
        )
        .await?;
    if !revoked {
        return Err(ProtocolError::new(ErrorCode::CapabilityNotFound, "no such grant").into());
    }
    Ok(Api(StatusCode::NO_CONTENT, Value::Null))
}

async fn resolve_grant(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(handle): Path<String>,
    body: axum::body::Bytes,
) -> ApiResult {
    // §11.2. Authenticated with the person's own session, never with the handle itself, and never
    // by the agent runtime.
    let principal = caller(&state, &headers).await?;
    require_person(&principal)?;
    let body = body_json(&body)?;
    let handle = parse_id::<handoff_protocol::id::Grant>(&handle, ErrorCode::CapabilityNotFound)?;

    let scopes: Vec<CapabilityScope> = body
        .get("scopes")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|s| serde_json::from_value(s.clone()).ok())
                .collect()
        })
        .unwrap_or_default();
    if scopes.is_empty() {
        return Err(ProtocolError::new(ErrorCode::InvalidRequest, "`scopes` is required").into());
    }
    let accepted = body
        .get("accepted_blast_radius_digest")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            ProtocolError::new(
                ErrorCode::InvalidRequest,
                "`accepted_blast_radius_digest` is required: a person must not be handed something \
                 other than what they were shown",
            )
        })?;
    let accepted = Digest::parse(accepted)?;

    let grant = state
        .store
        .grant(principal.tenant_ref.clone(), handle)
        .await?
        .ok_or_else(|| {
            ProtocolError::new(ErrorCode::CapabilityNotFound, "no such capability grant")
        })?;

    let session_ref = ids::mint_random::<handoff_protocol::id::GrantSession>()?;
    let session = state
        .store
        .open_grant_session(
            principal.tenant_ref.clone(),
            ResolveGrant {
                handle,
                principal: principal.clone(),
                scopes: scopes.clone(),
                accepted_blast_radius_digest: accepted,
                session_ref,
                now: state.now(),
            },
        )
        .await?;

    // The one resolvable address in a conforming system. It is minted here, bound to this single
    // session, and written to no table, no event, and no log line (§11.2, I8). A fresh nonce per
    // resolve is what makes two resolves of one grant produce two different addresses; a stable URL
    // would be a bearer value under another name.
    let provider = state.capabilities.provider(grant.provider.as_deref());
    let transport = provider.transport(
        &ProviderResource {
            capability_type: grant.capability_type.clone(),
            resource_ref: grant.resource_ref.clone(),
            scopes: scopes.clone(),
        },
        &session.session_ref.to_string(),
        &ids::random_token()?,
    );

    Ok(Api::ok(json!({
        "session_ref": session.session_ref.to_string(),
        "scopes": session
            .scopes
            .iter()
            .map(|s| serde_json::to_value(s).unwrap_or(Value::Null))
            .collect::<Vec<_>>(),
        "lease_until": session.lease_until.to_string(),
        "renew_after_ms": session.renew_after_ms,
        "blast_radius": serde_json::to_value(&grant.blast_radius).unwrap_or(Value::Null),
        "transport": {
            "kind": serde_json::to_value(transport.kind).unwrap_or(Value::Null),
            "url": transport.url,
        },
        "receipt_id": Value::Null,
    })))
}

async fn renew_session(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((handle, _session_ref)): Path<(String, String)>,
) -> ApiResult {
    let principal = caller(&state, &headers).await?;
    require_person(&principal)?;
    let handle = parse_id::<handoff_protocol::id::Grant>(&handle, ErrorCode::CapabilityNotFound)?;
    // §11.4. Renewal re-checks revocation, expiry, binding, and the caller's **current** authority,
    // so a role removed mid-session takes effect within one lease period.
    let grant = state
        .store
        .grant(principal.tenant_ref.clone(), handle)
        .await?
        .ok_or_else(|| ProtocolError::new(ErrorCode::CapabilityNotFound, "no such grant"))?;
    if grant.revoked_at.is_some() {
        return Err(
            ProtocolError::new(ErrorCode::CapabilityNotFound, "this grant was revoked").into(),
        );
    }
    let now = state.now();
    if grant.expires_at.is_at_or_before(now) {
        return Err(ProtocolError::new(ErrorCode::CapabilityExpired, "this grant expired").into());
    }
    if principal.role < grant.scope.minimum_role() {
        return Err(ProtocolError::new(
            ErrorCode::InsufficientAuthority,
            "this scope requires a higher role",
        )
        .into());
    }
    let lease_until = now.saturating_add(IsoDuration::from_secs(120));
    Ok(Api::ok(json!({
        "lease_until": lease_until.to_string(),
        "renew_after_ms": 60_000,
    })))
}

async fn release_session(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((handle, _session_ref)): Path<(String, String)>,
) -> ApiResult {
    let principal = caller(&state, &headers).await?;
    require_person(&principal)?;
    let _ = parse_id::<handoff_protocol::id::Grant>(&handle, ErrorCode::CapabilityNotFound)?;
    Ok(Api(StatusCode::NO_CONTENT, Value::Null))
}

// ---------------------------------------------------------------------------------------- sinks

async fn submit_sink_values(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(sink_ref): Path<String>,
    body: axum::body::Bytes,
) -> ApiResult {
    // §12. The person's own session, TLS, never logged, never echoed. Nothing in this handler
    // writes a value anywhere, including into a log line or an error message.
    let principal = caller(&state, &headers).await?;
    require_person(&principal)?;
    let body = body_json(&body)?;
    let values: Map<String, Value> = match body.get("values") {
        Some(Value::Object(map)) => map.clone(),
        _ => {
            return Err(
                ProtocolError::new(ErrorCode::InvalidRequest, "`values` must be an object").into(),
            )
        }
    };

    let acceptance = state
        .store
        .submit_sink_values(principal.tenant_ref.clone(), sink_ref, values)
        .await?;
    Ok(Api(
        StatusCode::ACCEPTED,
        json!({
            "accepted": acceptance.accepted,
            "state": acceptance.state.map_or(Value::Null, Value::String),
        }),
    ))
}

// ----------------------------------------------------------------------------------- deliveries

async fn get_delivery(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(delivery_id): Path<String>,
) -> ApiResult {
    let principal = caller(&state, &headers).await?;
    principal.require_scope("handoff:requests:read")?;
    let id = parse_id::<handoff_protocol::id::Delivery>(&delivery_id, ErrorCode::RequestNotFound)?;
    // §7.3 gives a delivery an ordered list of attempts, and §15.5 requires every attempt to be
    // inspectable by the tenant. A delivery you cannot inspect is one you cannot debug at 3 a.m.
    match state.store.delivery(&principal.tenant_ref, id).await? {
        Some(view) => Ok(Api::ok(wire::delivery(&view))),
        // §13: `404 …_not_found` rather than `403` wherever existence is itself sensitive. A
        // delivery in another tenant is indistinguishable from one that never existed.
        None => Err(ProtocolError::new(
            ErrorCode::RequestNotFound,
            "no such delivery in this tenant",
        )
        .into()),
    }
}

async fn redeliver(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(delivery_id): Path<String>,
) -> ApiResult {
    let principal = caller(&state, &headers).await?;
    // Routing scope, not raise scope. §7.4: a key that can ask a question and a key that can page
    // the on-call at 3 a.m. are different blast radiuses.
    principal.require_scope("handoff:requests:route")?;
    let id = parse_id::<handoff_protocol::id::Delivery>(&delivery_id, ErrorCode::RequestNotFound)?;
    let now = state.now();
    // `signing.md` §1.5 requires manual redelivery to be available to the tenant. It re-arms the
    // schedule and nothing else: a terminal delivery stays terminal, because re-delivering an
    // `acted` one would ask a person again for a decision that is already on a receipt.
    if !state
        .store
        .redeliver(&principal.tenant_ref, id, now)
        .await?
    {
        return Err(ProtocolError::new(
            ErrorCode::RequestNotFound,
            "no such delivery in this tenant, or it has already settled",
        )
        .into());
    }
    Ok(Api(
        StatusCode::ACCEPTED,
        json!({
            "delivery_id": id.to_string(),
            "queued_at": now.to_string(),
        }),
    ))
}

/// Record evidence a channel reported **after** the send returned.
///
/// A synchronous send can only report what the transport said at the moment it took the message,
/// which for a real channel is `dispatched` and nothing more. A provider's delivery receipt, or the
/// person opening the surface, arrives later — so without this route every asynchronous channel
/// stays at `dispatched` forever and the grade ladder of §7.2 is decorative.
///
/// # Who may call it, and what it cannot do
///
/// It takes its **own scope**, `handoff:deliveries:grade`, rather than reusing the routing scope.
/// §7.4 already establishes the principle: overriding a ladder needs a separate scope from raising
/// a request, because a key that can ask a question and one that can page the on-call at 3 a.m. are
/// different blast radiuses. Asserting what a person did is a third one. A deployment hands this
/// scope only to the credential its own provider webhooks authenticate as.
///
/// Three things it cannot do, in decreasing order of how much they would matter:
///
/// 1. **It cannot award `acted`.** `acted` means the person answered *through this delivery*
///    (§7.2), which is established by an answer landing and by nothing else. If this route accepted
///    it, a holder of this scope could write "they decided" onto a delivery with no decision behind
///    it — no receipt would be minted, but the delivery record is what an escalation review reads.
/// 2. **It cannot exceed the channel's declared ceiling**, because the store runs the same
///    [`transition`](handoff_protocol::delivery::transition) every other delivery write runs. A
///    caller reporting `seen` for an email delivery is refused, since email declares `delivered`.
/// 3. **It cannot move a grade backwards**, for the same reason.
///
/// The residual risk, stated rather than glossed: a holder of this scope can inflate a delivery
/// from `dispatched` to `delivered` on a channel capped there, and if that delivery is the one an
/// answer arrives through, the receipt's `via.grade_reached` carries it. That is inherent to any
/// channel-evidence ingress — the deployment is trusting its own provider webhooks — and it is why
/// the scope is separate. It cannot reach `seen` or `acted`, which are the grades that assert
/// something about a **person** rather than about a transport.
async fn record_grade(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(delivery_id): Path<String>,
    body: axum::body::Bytes,
) -> ApiResult {
    let principal = caller(&state, &headers).await?;
    principal.require_scope("handoff:deliveries:grade")?;
    let body = body_json(&body)?;
    let digest = body_digest(&body)?;
    let key = idempotency_key(&headers, &body)?;
    let id = parse_id::<handoff_protocol::id::Delivery>(&delivery_id, ErrorCode::RequestNotFound)?;
    let slot = slot(
        &principal,
        "delivery_grade",
        &id.to_string(),
        key.as_deref(),
        &digest,
    );
    if let Some(replayed) = replay(state.store.as_ref(), &slot).await? {
        return Ok(replayed);
    }

    let grade = match body.get("grade").and_then(|value| value.as_str()) {
        Some("delivered") => handoff_protocol::delivery::DeliveryGrade::Delivered,
        Some("seen") => handoff_protocol::delivery::DeliveryGrade::Seen,
        // Neither of the other two is a channel's to report. `dispatched` is the send's own claim,
        // already recorded by the attempt that made it; `acted` is the person's, and only an
        // answer establishes it.
        Some(other @ ("dispatched" | "acted")) => {
            return Err(ProtocolError::new(
                ErrorCode::InvalidRequest,
                format!(
                    "`{other}` is not a grade a channel reports: `dispatched` is recorded by the \
                     attempt that dispatched, and `acted` is established by the answer"
                ),
            )
            .into())
        }
        _ => {
            return Err(ProtocolError::new(
                ErrorCode::InvalidRequest,
                "`grade` must be `delivered` or `seen`",
            )
            .into())
        }
    };

    // Tenancy from the credential, never from the body (I13) — and never from possession of the
    // delivery id, which is an identifier and not an authorization (§4.6, I17).
    let now = state.now();
    let current = state
        .store
        .delivery(&principal.tenant_ref, id)
        .await?
        .ok_or_else(|| {
            // §13: `404 …_not_found` instead of `403` wherever existence is itself sensitive.
            ProtocolError::new(
                ErrorCode::RequestNotFound,
                "no such delivery in this tenant",
            )
        })?;

    // Already at or past this grade: nothing to record, and **not an error**. Delivery is
    // at-least-once and consumers dedupe (§16 rule 10), so a provider webhook will resend, and its
    // delivery receipt routinely lands after the person has already opened the surface. Answering
    // 4xx to a duplicate teaches a retrying provider to keep retrying forever; answering with the
    // state it is already in is both true and idempotent (I20), with or without a caller's key.
    let view = if current
        .grade_reached
        .is_some_and(|reached| reached >= grade)
    {
        current
    } else {
        state
            .store
            .advance_delivery_grade(&principal.tenant_ref, id, grade, now)
            .await?;
        state
            .store
            .delivery(&principal.tenant_ref, id)
            .await?
            .ok_or_else(|| {
                ProtocolError::new(
                    ErrorCode::RequestNotFound,
                    "no such delivery in this tenant",
                )
            })?
    };

    let rendered = wire::delivery(&view);
    remember(state.store.as_ref(), slot, StatusCode::OK, &rendered, now).await?;
    Ok(Api::ok(rendered))
}

// ----------------------------------------------------------------------------------------- meta

async fn meta(State(state): State<Arc<AppState>>) -> ApiResult {
    // Unauthenticated by design: a conformance runner and a client both use this to discover
    // support instead of assuming it (§19).
    Ok(Api::ok(json!({
        "protocol_version": handoff_protocol::PROTOCOL_VERSION,
        // The build's own version, so "did the managed tier quietly fork the core?" is a question
        // anyone can answer over HTTP against a running deployment. GOVERNANCE.md and the cutover
        // plan both name that check; they described a `/v1/version` route that does not exist, and
        // a second endpoint answering the same question is worse than one field here.
        "core_version": env!("CARGO_PKG_VERSION"),
        // §1.2 makes the advertised level normative, and a Server MUST NOT advertise Level 2
        // unless it passes C-17. Derived from what this build actually does, never a literal.
        "conformance_level": if state.config.continuation_supported { 2 } else { 1 },
        "extensions": if state.config.continuation_supported {
            vec!["continuation".to_string()]
        } else {
            Vec::<String>::new()
        },
        "field_types": ["choice", "text", "number", "boolean", "secret", "attestation", "document", "file_ref"],
        "capability_types": state.profile.capability_types.iter().collect::<Vec<_>>(),
        "channels": state.channels.names(),
        "max_wait_seconds": 30,
    })))
}

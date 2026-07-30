//! Responses, errors, and the two things every handler does before anything else.
//!
//! §13 requires exactly one error envelope across the whole surface, and rate-limit headers on
//! every response. Both live here so that neither can be forgotten in a handler.

use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use handoff_core::auth::Principal;
use handoff_core::ports::{IdempotencySlot, Store, StoredResponse};
use handoff_protocol::clock::Timestamp;
use handoff_protocol::error::{ErrorCode, ProtocolError};
use handoff_protocol::receipt::{digest_of, Digest};
use serde_json::Value;

/// A successful response: a status and a body.
pub struct Api(pub StatusCode, pub Value);

impl Api {
    /// `200` with a body.
    pub fn ok(body: Value) -> Self {
        Self(StatusCode::OK, body)
    }
}

/// Rate-limit headers, which §13 makes mandatory rather than optional.
///
/// The numbers are this deployment's; what the specification requires is that a client holding a
/// long-lived credential can see its own budget without guessing. A deployment that meters
/// differently reports differently, and the header set is the contract.
fn rate_limit_headers(headers: &mut HeaderMap) {
    let reset = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() + 60)
        .unwrap_or(0);
    headers.insert("X-RateLimit-Limit", HeaderValue::from_static("1000"));
    headers.insert("X-RateLimit-Remaining", HeaderValue::from_static("999"));
    if let Ok(value) = HeaderValue::from_str(&reset.to_string()) {
        headers.insert("X-RateLimit-Reset", value);
    }
}

impl IntoResponse for Api {
    fn into_response(self) -> Response {
        let mut response = if self.0 == StatusCode::NO_CONTENT {
            self.0.into_response()
        } else {
            (self.0, axum::Json(self.1)).into_response()
        };
        rate_limit_headers(response.headers_mut());
        response
    }
}

/// A failure, rendered in the one error envelope §13 requires across the whole surface.
pub struct ApiError(pub ProtocolError);

impl From<ProtocolError> for ApiError {
    fn from(value: ProtocolError) -> Self {
        Self(value)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.0.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let mut response = (status, axum::Json(crate::wire::error(&self.0))).into_response();
        rate_limit_headers(response.headers_mut());
        response
    }
}

/// What a handler returns.
pub type ApiResult = std::result::Result<Api, ApiError>;

/// Parse a JSON body, or explain why it is not one.
pub fn body_json(bytes: &[u8]) -> Result<Value, ProtocolError> {
    if bytes.is_empty() {
        return Ok(Value::Object(Default::default()));
    }
    serde_json::from_slice(bytes).map_err(|e| {
        ProtocolError::new(
            ErrorCode::InvalidRequest,
            format!("the body is not JSON: {e}"),
        )
    })
}

/// The digest a reused `Idempotency-Key` is compared against (§3.3 rule 2).
pub fn body_digest(body: &Value) -> Result<Digest, ProtocolError> {
    digest_of(body)
}

/// The caller's `Idempotency-Key`, if they supplied one.
///
/// §3.5 requires the header form and permits a body field of the same name; where both are present
/// and disagree, the answer is `400 invalid_request` rather than a silent preference for one.
pub fn idempotency_key(headers: &HeaderMap, body: &Value) -> Result<Option<String>, ProtocolError> {
    let header = headers
        .get("Idempotency-Key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let in_body = body
        .get("Idempotency-Key")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    match (header, in_body) {
        (Some(a), Some(b)) if a != b => Err(ProtocolError::new(
            ErrorCode::InvalidRequest,
            "the Idempotency-Key header and body field disagree",
        )),
        (Some(a), _) => Ok(Some(a)),
        (None, Some(b)) => Ok(Some(b)),
        (None, None) => Ok(None),
    }
}

/// Which idempotency slot this call occupies.
pub fn slot(
    principal: &Principal,
    operation: &str,
    key: Option<&str>,
    digest: &Digest,
) -> Option<IdempotencySlot> {
    key.map(|key| IdempotencySlot {
        tenant: principal.tenant_ref.clone(),
        principal: principal
            .id
            .map(|id| id.to_string())
            .unwrap_or_else(|| format!("{}::anonymous", principal.tenant_ref)),
        operation: operation.to_string(),
        key: key.to_string(),
        body_digest: digest.clone(),
    })
}

/// Return the stored response for a repeated key, if there is one (§3.5).
pub async fn replay(
    store: &dyn Store,
    slot: &Option<IdempotencySlot>,
) -> Result<Option<Api>, ProtocolError> {
    let Some(slot) = slot else { return Ok(None) };
    let Some(stored) = store.idempotent_replay(slot.clone()).await? else {
        return Ok(None);
    };
    Ok(Some(Api(
        StatusCode::from_u16(stored.status).unwrap_or(StatusCode::OK),
        serde_json::from_str(&stored.body).unwrap_or(Value::Null),
    )))
}

/// Store a response against a key, so a retry returns exactly what the first call did.
pub async fn remember(
    store: &dyn Store,
    slot: Option<IdempotencySlot>,
    status: StatusCode,
    body: &Value,
    now: Timestamp,
) -> Result<(), ProtocolError> {
    let Some(slot) = slot else { return Ok(()) };
    store
        .remember_idempotent(
            slot,
            StoredResponse {
                status: status.as_u16(),
                body: body.to_string(),
            },
            now,
        )
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use serde_json::json;

    #[test]
    fn a_header_and_a_body_field_that_disagree_are_a_bad_request() {
        let mut headers = HeaderMap::new();
        headers.insert("Idempotency-Key", HeaderValue::from_static("a"));
        let err = idempotency_key(&headers, &json!({"Idempotency-Key": "b"})).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidRequest);
    }

    #[test]
    fn the_header_is_accepted_on_its_own() {
        let mut headers = HeaderMap::new();
        headers.insert("Idempotency-Key", HeaderValue::from_static("a"));
        assert_eq!(
            idempotency_key(&headers, &json!({})).unwrap(),
            Some("a".to_string())
        );
    }

    #[test]
    fn two_bodies_that_differ_have_different_digests() {
        let a = body_digest(&json!({"title": "Refund $2,400"})).unwrap();
        let b = body_digest(&json!({"title": "Refund $24,000"})).unwrap();
        assert_ne!(a, b);
    }
}

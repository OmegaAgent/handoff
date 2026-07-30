//! The error taxonomy of spec §13.
//!
//! Every failure in this crate is an [`ErrorCode`] plus a human-readable message plus whatever
//! context the code is defined to carry. The code is stable and machine-readable; the message is
//! for people and a client must never parse it.
//!
//! The whole surface uses one envelope ([`ErrorEnvelope`]), because §13 says so: "A Server MUST use
//! exactly one error envelope across its whole surface."

use serde::{Deserialize, Serialize};
use std::fmt;

/// The complete, stable error taxonomy, mirroring `ErrorCode` in `openapi.yaml`.
///
/// A code's meaning MUST NOT change within a major protocol version (§13). New codes are additive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ErrorCode {
    /// The request body was malformed, or violated a stated constraint that has no more specific
    /// code (for example `forbid_requester: false`, or `on_expiry: "default"` with no
    /// `default_answer`).
    InvalidRequest,
    /// A declared answer field carried a `type` this Server does not implement. Fail closed: the
    /// request is not created and the field is never degraded to a text input (§5.3, I21).
    UnsupportedFieldType,
    /// A declared capability carried a `type` this Server does not implement (§19, I21).
    UnsupportedCapabilityType,
    /// `requires.v` named an envelope version this Server does not implement. Nothing is created
    /// and nothing is partially accepted (§5.2, C-16, I21).
    UnsupportedRequiresVersion,
    /// The API key was absent, malformed, revoked, or expired — deliberately one code, so that the
    /// response does not tell an attacker which keys once existed (§13).
    InvalidApiKey,
    /// The operation requires an authenticated principal and none was presented.
    AuthenticationRequired,
    /// The principal authenticated but lacks the scope for this operation — for example raising a
    /// request with a `routing` override without the separate routing scope (§7.4).
    InsufficientScope,
    /// The tenant is not entitled to the product or feature the operation needs.
    ProductNotEntitled,
    /// The answerer does not satisfy the authority declared on the request (§4.3, C-6).
    InsufficientAuthority,
    /// A requester (machine) principal attempted to answer. Enforced by principal *type*, never by
    /// role or configuration (§4.2, I15, C-5).
    RequesterMayNotAnswer,
    /// The caller holds a grant spanning both tenants but named the wrong one. Everywhere else,
    /// cross-tenant access is a `*_not_found` so that existence is not disclosed (§3.2).
    TenantMismatch,
    /// The deployment does not permit the `auth_strength` grade the answer was made at — in
    /// practice, `link_only` where it has not been explicitly enabled (§4.4, C-6b).
    AuthStrengthNotPermitted,
    /// No such request in the caller's tenant. Returned in preference to `403` wherever existence
    /// is itself sensitive (§3.2).
    RequestNotFound,
    /// No such capability grant in the caller's tenant.
    CapabilityNotFound,
    /// No such waiter signal in the caller's tenant.
    SignalNotFound,
    /// No such authorization in the caller's tenant.
    AuthorizationNotFound,
    /// The request already settled with an answer. Carries the existing `receipt_id` so the caller
    /// can recover without a second round trip (§6.7, I5).
    AlreadyAnswered,
    /// The request already settled by expiry.
    RequestExpired,
    /// The request already settled by cancellation.
    RequestCancelled,
    /// The request already settled by supersession. Carries `superseded_by`.
    RequestSuperseded,
    /// An amendment arrived after a person had begun answering. The caller must supersede instead
    /// (§6.2, R2).
    RequestInProgress,
    /// The same `Idempotency-Key` was replayed within its window with a different body digest. The
    /// stored request is not modified (§3.3).
    IdempotencyKeyReused,
    /// A single-use authorization was redeemed with a second, different `effect_key` (§10, I10).
    AuthorizationSpent,
    /// A redemption arrived after the authorization's `expires_at` (§10 rule 4).
    ///
    /// Distinct from [`Self::AuthorizationSpent`] on purpose, and the distinction is the whole
    /// reason the code exists: the decision was real, it is on the record, and it is simply no
    /// longer spendable. Saying "spent" would claim it was used, and saying "not found" would claim
    /// it never happened. Both are untrue, and a caller that has to tell a stale approval from a
    /// double-spend cannot do it from either.
    AuthorizationExpired,
    /// A redemption's `effect_digest` disagreed with the digest the authorization was bound to.
    /// This is what stops an approval of "refund $2,400" being spent on "refund $24,000" (§10).
    EffectDigestMismatch,
    /// The `accepted_blast_radius_digest` on a grant resolve did not match the grant's current
    /// blast radius. The person must not be handed something other than what they read (§11.5,
    /// I19).
    BlastRadiusMismatch,
    /// The grant is already held by its maximum number of holders (§11, `max_holders`).
    GrantAlreadyHeld,
    /// Under `presentation_binding: strict`, the answer's `rendered_digest` did not match what the
    /// answerer was shown. They must re-read the current request (§9.3).
    PresentationStale,
    /// The capability grant is past its `expires_at` and cannot be renewed (§11.4).
    CapabilityExpired,
    /// The submitted answer did not validate against the declared fields. Carries per-field detail
    /// so a surface can mark the offending input (§5.3, §13).
    AnswerValidationFailed,
    /// The caller exceeded its rate limit.
    RateLimited,
    /// No channel could even be attempted. **The request still exists** — failing the raise would
    /// put a channel outage inside the caller's agent (§7.3, §13).
    DeliveryUnavailable,
}

impl ErrorCode {
    /// The wire string for this code, identical to its `openapi.yaml` enum member.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::UnsupportedFieldType => "unsupported_field_type",
            Self::UnsupportedCapabilityType => "unsupported_capability_type",
            Self::UnsupportedRequiresVersion => "unsupported_requires_version",
            Self::InvalidApiKey => "invalid_api_key",
            Self::AuthenticationRequired => "authentication_required",
            Self::InsufficientScope => "insufficient_scope",
            Self::ProductNotEntitled => "product_not_entitled",
            Self::InsufficientAuthority => "insufficient_authority",
            Self::RequesterMayNotAnswer => "requester_may_not_answer",
            Self::TenantMismatch => "tenant_mismatch",
            Self::AuthStrengthNotPermitted => "auth_strength_not_permitted",
            Self::RequestNotFound => "request_not_found",
            Self::CapabilityNotFound => "capability_not_found",
            Self::SignalNotFound => "signal_not_found",
            Self::AuthorizationNotFound => "authorization_not_found",
            Self::AlreadyAnswered => "already_answered",
            Self::RequestExpired => "request_expired",
            Self::RequestCancelled => "request_cancelled",
            Self::RequestSuperseded => "request_superseded",
            Self::RequestInProgress => "request_in_progress",
            Self::IdempotencyKeyReused => "idempotency_key_reused",
            Self::AuthorizationSpent => "authorization_spent",
            Self::AuthorizationExpired => "authorization_expired",
            Self::EffectDigestMismatch => "effect_digest_mismatch",
            Self::BlastRadiusMismatch => "blast_radius_mismatch",
            Self::GrantAlreadyHeld => "grant_already_held",
            Self::PresentationStale => "presentation_stale",
            Self::CapabilityExpired => "capability_expired",
            Self::AnswerValidationFailed => "answer_validation_failed",
            Self::RateLimited => "rate_limited",
            Self::DeliveryUnavailable => "delivery_unavailable",
        }
    }

    /// The HTTP status `openapi.yaml` pairs with this code.
    ///
    /// This lives here rather than in the server so that the mapping is stated once and can be
    /// asserted by the conformance suite without standing up a transport.
    pub const fn http_status(self) -> u16 {
        match self {
            Self::InvalidRequest
            | Self::UnsupportedFieldType
            | Self::UnsupportedCapabilityType
            | Self::UnsupportedRequiresVersion => 400,
            Self::InvalidApiKey | Self::AuthenticationRequired => 401,
            Self::InsufficientScope
            | Self::ProductNotEntitled
            | Self::InsufficientAuthority
            | Self::RequesterMayNotAnswer
            | Self::TenantMismatch
            | Self::AuthStrengthNotPermitted => 403,
            Self::RequestNotFound
            | Self::CapabilityNotFound
            | Self::SignalNotFound
            | Self::AuthorizationNotFound => 404,
            Self::AlreadyAnswered
            | Self::RequestExpired
            | Self::RequestCancelled
            | Self::RequestSuperseded
            | Self::RequestInProgress
            | Self::IdempotencyKeyReused
            | Self::AuthorizationSpent
            | Self::AuthorizationExpired
            | Self::EffectDigestMismatch
            | Self::BlastRadiusMismatch
            | Self::GrantAlreadyHeld
            | Self::PresentationStale => 409,
            Self::CapabilityExpired => 410,
            Self::AnswerValidationFailed => 422,
            Self::RateLimited => 429,
            Self::DeliveryUnavailable => 503,
        }
    }

    /// Every code in the taxonomy, in `openapi.yaml` order.
    ///
    /// Used by tests that assert this crate and the wire contract enumerate the same set.
    pub const ALL: &'static [ErrorCode] = &[
        Self::InvalidRequest,
        Self::UnsupportedFieldType,
        Self::UnsupportedCapabilityType,
        Self::UnsupportedRequiresVersion,
        Self::InvalidApiKey,
        Self::AuthenticationRequired,
        Self::InsufficientScope,
        Self::ProductNotEntitled,
        Self::InsufficientAuthority,
        Self::RequesterMayNotAnswer,
        Self::TenantMismatch,
        Self::AuthStrengthNotPermitted,
        Self::RequestNotFound,
        Self::CapabilityNotFound,
        Self::SignalNotFound,
        Self::AuthorizationNotFound,
        Self::AlreadyAnswered,
        Self::RequestExpired,
        Self::RequestCancelled,
        Self::RequestSuperseded,
        Self::RequestInProgress,
        Self::IdempotencyKeyReused,
        Self::AuthorizationSpent,
        Self::AuthorizationExpired,
        Self::EffectDigestMismatch,
        Self::BlastRadiusMismatch,
        Self::GrantAlreadyHeld,
        Self::PresentationStale,
        Self::CapabilityExpired,
        Self::AnswerValidationFailed,
        Self::RateLimited,
        Self::DeliveryUnavailable,
    ];
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One field-level validation failure, so a surface can highlight the input in place (§13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldError {
    /// The declared field name that failed.
    pub name: String,
    /// Stable machine-readable reason. See [`FieldErrorCode`].
    pub code: FieldErrorCode,
    /// Human-readable detail. Never contains a submitted value (§12, I18).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// The per-field reasons an answer can fail validation.
///
/// Open by design at the wire level (`openapi.yaml` types `FieldError.code` as a string), but this
/// crate only ever emits these, so a client can branch on them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum FieldErrorCode {
    /// A `required` field was absent or null.
    Required,
    /// The submitted value's JSON type did not match the declared field type.
    WrongType,
    /// A `choice` answer named an option outside the declared set. Never a free-text fallback.
    NotAnOption,
    /// A `number` outside `min`/`max`, or a `text` longer than `max_len`.
    OutOfRange,
    /// A `secret` field carried a raw value instead of `{"provided": true}` (§5.3 rule 4, I7).
    SecretValueNotPermitted,
    /// An `attestation` field carried anything other than `true`.
    AttestationMustBeTrue,
    /// The answer carried a key that is not a declared field name (§5.3 rule 5).
    UndeclaredField,
}

/// Context a specific [`ErrorCode`] is defined to carry (§13: "`409` responses about a settled
/// request carry the settling record").
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorContext {
    /// The request this error concerns.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// The decision that already exists. Present on [`ErrorCode::AlreadyAnswered`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt_id: Option<String>,
    /// Where to send the person instead. Present on [`ErrorCode::RequestSuperseded`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
    /// Per-field detail. Present on [`ErrorCode::AnswerValidationFailed`].
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub fields: Vec<FieldError>,
}

/// The context every error without one shares. Most errors carry none.
static NO_CONTEXT: ErrorContext = ErrorContext {
    request_id: None,
    receipt_id: None,
    superseded_by: None,
    fields: Vec::new(),
};

/// A protocol-level failure: a stable code, a message for people, and the code's defined context.
///
/// The context is boxed and only allocated when something is attached. Almost every error in this
/// crate carries none, and these travel in the `Err` arm of hot paths like the state machines'
/// `transition` functions — a fat error variant would tax every successful call too.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{code}: {message}")]
pub struct ProtocolError {
    /// The stable, machine-readable code. Clients branch on this and nothing else.
    pub code: ErrorCode,
    /// Human-readable and subject to change. A client MUST NOT parse it (§13).
    pub message: String,
    context: Option<Box<ErrorContext>>,
}

impl ProtocolError {
    /// Build an error with no additional context.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            context: None,
        }
    }

    fn context_mut(&mut self) -> &mut ErrorContext {
        self.context.get_or_insert_with(Box::default)
    }

    /// Whatever context this code is defined to carry.
    pub fn context(&self) -> &ErrorContext {
        self.context.as_deref().unwrap_or(&NO_CONTEXT)
    }

    /// Per-field validation detail, present on `answer_validation_failed`.
    pub fn fields(&self) -> &[FieldError] {
        &self.context().fields
    }

    /// Attach the request this error concerns.
    #[must_use]
    pub fn with_request(mut self, request_id: impl Into<String>) -> Self {
        self.context_mut().request_id = Some(request_id.into());
        self
    }

    /// Attach the receipt that already settled the request.
    #[must_use]
    pub fn with_receipt(mut self, receipt_id: impl Into<String>) -> Self {
        self.context_mut().receipt_id = Some(receipt_id.into());
        self
    }

    /// Attach the successor request a superseded request points at.
    #[must_use]
    pub fn with_superseded_by(mut self, request_id: impl Into<String>) -> Self {
        self.context_mut().superseded_by = Some(request_id.into());
        self
    }

    /// Attach per-field validation detail.
    #[must_use]
    pub fn with_fields(mut self, fields: Vec<FieldError>) -> Self {
        self.context_mut().fields = fields;
        self
    }

    /// The HTTP status this error maps to.
    pub const fn http_status(&self) -> u16 {
        self.code.http_status()
    }

    /// Render as the single wire envelope of §13.
    pub fn to_envelope(&self) -> ErrorEnvelope {
        let context = self.context();
        ErrorEnvelope {
            error: ErrorBody {
                code: self.code,
                message: self.message.clone(),
                request_id: context.request_id.clone(),
                receipt_id: context.receipt_id.clone(),
                superseded_by: context.superseded_by.clone(),
                fields: context.fields.clone(),
                docs: None,
            },
        }
    }
}

/// The body of the one error envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorBody {
    /// The stable code.
    pub code: ErrorCode,
    /// Human-readable and subject to change.
    pub message: String,
    /// The request this error concerns, when it concerns one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// The existing decision, on `already_answered`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt_id: Option<String>,
    /// The successor, on `request_superseded`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
    /// Per-field detail, on `answer_validation_failed`.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub fields: Vec<FieldError>,
    /// A stable link explaining this code. Filled in by the server, not by this crate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docs: Option<String>,
}

/// The one error envelope used across the whole protocol surface. There is never a second shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorEnvelope {
    /// The error itself.
    pub error: ErrorBody,
}

/// Shorthand for a protocol result.
pub type Result<T> = std::result::Result<T, ProtocolError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_code_round_trips_through_its_wire_string() {
        for &code in ErrorCode::ALL {
            let json = serde_json::to_string(&code).expect("serialize");
            assert_eq!(json, format!("\"{}\"", code.as_str()));
            let back: ErrorCode = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, code);
        }
    }

    #[test]
    fn taxonomy_has_no_duplicates_and_matches_openapi_count() {
        let mut seen: Vec<&str> = ErrorCode::ALL.iter().map(|c| c.as_str()).collect();
        let total = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), total, "duplicate error code in ErrorCode::ALL");
        // openapi.yaml `ErrorCode` enumerates exactly 32 members.
        assert_eq!(total, 32);
    }

    #[test]
    fn settled_request_errors_carry_the_settling_record() {
        let err = ProtocolError::new(ErrorCode::AlreadyAnswered, "answered at 14:07:44Z")
            .with_request("req_01K3M7QW8ZC4YRXB2N6VD9FTHE")
            .with_receipt("rcpt_01K3MB2R4YC4YRXB2N6VD9FTHE");
        assert_eq!(err.http_status(), 409);
        let envelope = serde_json::to_value(err.to_envelope()).expect("serialize");
        assert_eq!(envelope["error"]["code"], "already_answered");
        assert_eq!(
            envelope["error"]["receipt_id"],
            "rcpt_01K3MB2R4YC4YRXB2N6VD9FTHE"
        );
        // Absent context is omitted rather than rendered as null.
        assert!(envelope["error"].get("superseded_by").is_none());
    }

    #[test]
    fn unknown_wire_code_fails_closed() {
        let err = serde_json::from_str::<ErrorCode>("\"teapot\"");
        assert!(
            err.is_err(),
            "an unrecognized error code must not deserialize"
        );
    }
}

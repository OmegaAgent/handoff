//! The versioned `requires` envelope (§5), and everything it declares.
//!
//! This module is where the protocol's central design decision lives: **there is no request `kind`
//! enum** (§5.1, I14). A request declares the shape of the answer, the capabilities the person must
//! be handed, and who is entitled to answer. The eight interaction patterns of §5.6 are eight
//! populations of those three declarations, and none of them adds a branch here.
//!
//! Everything in this module fails closed (I21). An unknown envelope version, an unknown field
//! type, or an unknown capability type is an error and nothing is created. A field the surface
//! cannot draw is a field the person cannot answer, and rendering it anyway produces a receipt that
//! misstates what was asked.

use crate::clock::IsoDuration;
use crate::error::{ErrorCode, FieldError, FieldErrorCode, ProtocolError, Result};
use crate::id::{GrantHandle, PrincipalId, SinkRef};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// The only `requires.v` this crate implements. Anything else fails closed (§5.2, C-16).
pub const REQUIRES_VERSION: u64 = 1;

// ---------------------------------------------------------------------------------------------
// Deployment profile
// ---------------------------------------------------------------------------------------------

/// The deployment-configurable half of validation.
///
/// These are the knobs §4.4, §4.5, and §19 leave to a deployment, gathered in one place so that a
/// Server never grows a per-case branch for them. `GET /v1/meta` reports exactly this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeploymentProfile {
    /// Whether `auth_strength: link_only` is accepted at all (§4.4, C-6b).
    ///
    /// Default `false`: a receipt that cannot say who decided is not a receipt, so a deployment has
    /// to opt in deliberately.
    pub allow_link_only: bool,
    /// The largest `quorum` this deployment can honour. Level 1 requires only `1`; a larger value
    /// than this is `400 invalid_request` rather than a silent downgrade to `1` (§4.5).
    pub max_quorum: u64,
    /// Capability types this deployment can render. An unknown type fails closed (§19, I21).
    pub capability_types: BTreeSet<String>,
}

impl Default for DeploymentProfile {
    fn default() -> Self {
        Self {
            allow_link_only: false,
            max_quorum: 1,
            capability_types: BTreeSet::new(),
        }
    }
}

impl DeploymentProfile {
    /// A profile supporting the named capability types and nothing else.
    pub fn with_capability_types<I, S>(mut self, types: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.capability_types = types.into_iter().map(Into::into).collect();
        self
    }
}

// ---------------------------------------------------------------------------------------------
// Roles and authentication strength
// ---------------------------------------------------------------------------------------------

/// A role in the owning tenancy, ordered from least to most privileged.
///
/// The `Ord` derive is load-bearing: `min_role` is a floor, and comparing roles is how §4.3 is
/// evaluated. Adding a role between two existing ones is a breaking change under §19.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// May read.
    Viewer,
    /// May decide ordinary requests.
    Editor,
    /// The administrative role. The derived floor of §4.3 raises `min_role` to this whenever a
    /// `secret` field is declared.
    Admin,
}

/// How firmly the answerer's identity was established (§4.4), ordered `link_only < session <
/// reauth < mfa`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthStrength {
    /// Possession of a single-use delivery token only. **No person is identified**; the receipt
    /// must record `actor.type = "anonymous_link"` and no principal identity (§4.4).
    LinkOnly,
    /// An authenticated principal in the tenant.
    Session,
    /// Re-entered a primary credential within the last five minutes.
    Reauth,
    /// Presented a second factor within the last five minutes.
    Mfa,
}

// ---------------------------------------------------------------------------------------------
// Field types
// ---------------------------------------------------------------------------------------------

/// The closed, versioned set of answer field types (§5.3).
///
/// Closed on purpose: a Server MUST reject an unknown `type` and MUST NOT degrade it to a text
/// input. New interaction types arrive as new members here, behind the declaration — never as a new
/// branch elsewhere (I14).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    /// The answer carries the selected option `id`, or an array of ids when `multi`.
    Choice,
    /// The answer carries a string.
    Text,
    /// The answer carries a number.
    Number,
    /// The answer carries a boolean.
    Boolean,
    /// The answer carries `{"provided": true}` **and nothing else**. The value went to the sink
    /// (§12, I7). Declaring one of these raises the authority floor (§4.3).
    Secret,
    /// The answer carries `true` and nothing else: "I did the out-of-band thing".
    Attestation,
    /// The answer carries a structured value, validated against `schema_ref`.
    Document,
    /// The answer carries an opaque handle, never bytes.
    FileRef,
}

impl FieldType {
    /// Every field type, in `openapi.yaml` order. `GET /v1/meta` reports this list.
    pub const ALL: &'static [FieldType] = &[
        Self::Choice,
        Self::Text,
        Self::Number,
        Self::Boolean,
        Self::Secret,
        Self::Attestation,
        Self::Document,
        Self::FileRef,
    ];

    /// The wire string for this type.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Choice => "choice",
            Self::Text => "text",
            Self::Number => "number",
            Self::Boolean => "boolean",
            Self::Secret => "secret",
            Self::Attestation => "attestation",
            Self::Document => "document",
            Self::FileRef => "file_ref",
        }
    }

    /// Parse a wire string, returning `None` for anything unrecognized.
    ///
    /// The caller turns `None` into `400 unsupported_field_type`. There is deliberately no
    /// fallback: see [`FieldType`].
    pub fn from_str_opt(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|t| t.as_str() == s)
    }
}

impl fmt::Display for FieldType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One selectable option on a `choice` field. `id` is what the answer carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldOption {
    /// The stable value the answer carries.
    pub id: String,
    /// What the person reads.
    pub label: String,
}

/// One thing the person is asked for. **Metadata only**: it declares the shape of an answer and
/// never carries one (§5.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Field {
    /// Stable key in the answer's `values` object. Lowercase snake case.
    pub name: String,
    /// What the person sees. Absent means the surface humanizes `name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The declared type. Drives the renderer and the validator, together with the fields below.
    #[serde(rename = "type")]
    pub field_type: FieldType,
    /// A missing required field is `422 answer_validation_failed`.
    #[serde(default)]
    pub required: bool,
    /// One line of guidance shown near the input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    /// For `choice`. An answer outside this set is a validation failure, never a free-text fallback.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<FieldOption>,
    /// For `choice` — the answer is an array of option ids rather than one id.
    #[serde(default)]
    pub multi: bool,
    /// For `text`. Enforced server-side, not only in the surface.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_len: Option<u64>,
    /// For `number`, inclusive lower bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    /// For `number`, inclusive upper bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// Pre-filled value. For `document` this is the draft the person edits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial: Option<Value>,
    /// For `document`. The answer is validated against this schema before the receipt is minted.
    ///
    /// This crate carries the reference; resolving and applying the schema needs I/O and therefore
    /// belongs to `handoff-core`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_ref: Option<String>,
    /// For `secret`. Where the value goes instead of into the answer (§12).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sink_ref: Option<SinkRef>,
}

impl Field {
    /// Check the *declaration* is coherent, independently of any answer.
    ///
    /// Only contradictions that would leave a person unable to answer are rejected here. A `choice`
    /// with no options cannot be answered at all, and options on a `text` field mean the author
    /// believed they were declaring something the renderer will not draw.
    pub fn validate_declaration(&self) -> Result<()> {
        let bad = |why: &str| {
            ProtocolError::new(
                ErrorCode::InvalidRequest,
                format!("field `{}`: {why}", self.name),
            )
        };
        if !is_snake_case_name(&self.name) {
            return Err(bad("name must match `^[a-z][a-z0-9_]{0,63}$`"));
        }
        match self.field_type {
            FieldType::Choice => {
                if self.options.is_empty() {
                    return Err(bad("a `choice` field must declare at least one option"));
                }
                let mut ids: Vec<&str> = self.options.iter().map(|o| o.id.as_str()).collect();
                let total = ids.len();
                ids.sort_unstable();
                ids.dedup();
                if ids.len() != total {
                    return Err(bad("option ids must be distinct"));
                }
            }
            _ => {
                if !self.options.is_empty() {
                    return Err(bad("only a `choice` field may declare options"));
                }
                if self.multi {
                    return Err(bad("only a `choice` field may declare `multi`"));
                }
            }
        }
        if let (Some(min), Some(max)) = (self.min, self.max) {
            if min > max {
                return Err(bad("`min` must not exceed `max`"));
            }
        }
        Ok(())
    }

    /// Validate one submitted value against this declaration.
    ///
    /// Returns the per-field errors §13 requires so a surface can mark the offending input. An
    /// empty vector means the value is acceptable.
    fn validate_value(&self, value: &Value) -> Vec<FieldError> {
        let err = |code: FieldErrorCode, message: &str| {
            vec![FieldError {
                name: self.name.clone(),
                code,
                // Never interpolate the submitted value: an error message is a log-adjacent
                // position, and §12 rule 6 forbids echoing a value even in a validation error.
                message: Some(message.to_string()),
            }]
        };

        match self.field_type {
            // A `secret` field carries the *fact* of provision and nothing else. A raw value here
            // is the failure C-7 exists to catch (§5.3 rule 4, I7).
            FieldType::Secret => {
                if value.as_object().is_some_and(|o| {
                    o.len() == 1 && o.get("provided").and_then(Value::as_bool) == Some(true)
                }) {
                    Vec::new()
                } else {
                    err(
                        FieldErrorCode::SecretValueNotPermitted,
                        "a secret field must carry exactly {\"provided\": true}; the value goes to the sink",
                    )
                }
            }
            FieldType::Attestation => {
                if value.as_bool() == Some(true) {
                    Vec::new()
                } else {
                    err(
                        FieldErrorCode::AttestationMustBeTrue,
                        "an attestation must be `true`",
                    )
                }
            }
            FieldType::Choice => self.validate_choice(value),
            FieldType::Text => match value.as_str() {
                None => err(FieldErrorCode::WrongType, "expected a string"),
                Some(s) => match self.max_len {
                    Some(limit) if s.chars().count() as u64 > limit => err(
                        FieldErrorCode::OutOfRange,
                        "longer than the declared `max_len`",
                    ),
                    _ => Vec::new(),
                },
            },
            FieldType::Number => match value.as_f64() {
                None => err(FieldErrorCode::WrongType, "expected a number"),
                Some(n) => {
                    if self.min.is_some_and(|min| n < min) || self.max.is_some_and(|max| n > max) {
                        err(FieldErrorCode::OutOfRange, "outside the declared bounds")
                    } else {
                        Vec::new()
                    }
                }
            },
            FieldType::Boolean => {
                if value.is_boolean() {
                    Vec::new()
                } else {
                    err(FieldErrorCode::WrongType, "expected a boolean")
                }
            }
            FieldType::FileRef => {
                if value.is_string() {
                    Vec::new()
                } else {
                    err(
                        FieldErrorCode::WrongType,
                        "expected an opaque handle string",
                    )
                }
            }
            // Structural validation against `schema_ref` needs to fetch the schema, which is I/O
            // and therefore `handoff-core`'s job. Here we only assert that *something* was carried.
            FieldType::Document => {
                if value.is_null() {
                    err(FieldErrorCode::WrongType, "expected a document value")
                } else {
                    Vec::new()
                }
            }
        }
    }

    fn validate_choice(&self, value: &Value) -> Vec<FieldError> {
        let known = |id: &str| self.options.iter().any(|o| o.id == id);
        let one = |code: FieldErrorCode, message: &str| {
            vec![FieldError {
                name: self.name.clone(),
                code,
                message: Some(message.to_string()),
            }]
        };

        if self.multi {
            let Some(items) = value.as_array() else {
                return one(FieldErrorCode::WrongType, "expected an array of option ids");
            };
            for item in items {
                match item.as_str() {
                    None => {
                        return one(FieldErrorCode::WrongType, "expected an array of option ids")
                    }
                    Some(id) if !known(id) => {
                        return one(FieldErrorCode::NotAnOption, "not a declared option")
                    }
                    Some(_) => {}
                }
            }
            Vec::new()
        } else {
            match value.as_str() {
                None => one(FieldErrorCode::WrongType, "expected an option id"),
                Some(id) if !known(id) => one(FieldErrorCode::NotAnOption, "not a declared option"),
                Some(_) => Vec::new(),
            }
        }
    }
}

fn is_snake_case_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    name.len() <= 64 && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

// ---------------------------------------------------------------------------------------------
// Answer specification
// ---------------------------------------------------------------------------------------------

/// Where `secret`-typed values go: a channel the runtime owns and can audit (§12 rule 4).
///
/// `ref` is modelled as an opaque bounded string rather than a typed [`SinkRef`]. See the crate
/// documentation's note on spec defect D-2: `openapi.yaml` types this as `SinkRef`, but the
/// normative fixture `use-cases/03-login-assistance.json` and spec §5.6.1 both carry
/// `"ref": "opaque:bs_4KpQ"`, which that pattern rejects. §12 calls the sink runtime-owned and
/// opaque, so the permissive reading is the one that keeps the normative fixtures parseable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValueSink {
    /// Opaque provider name. The core looks it up; it never matches on the string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Opaque operation name, meaningful to the provider only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op: Option<String>,
    /// Opaque destination reference.
    #[serde(rename = "ref")]
    pub sink_ref: String,
}

/// The shape of a valid answer. One declaration, one renderer, no per-vendor code anywhere.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnswerSpec {
    /// The declared fields.
    ///
    /// **May be empty, and empty is meaningful** (§5.3 rule 1): the whole request is then an
    /// attestation — there is nothing to type and the person acts out of band.
    #[serde(default)]
    pub fields: Vec<Field>,
    /// Where `secret` values travel instead of into the answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_sink: Option<ValueSink>,
}

/// What an answer is trying to do, which decides how strictly it is validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerMode {
    /// A settling answer (`disposition: "decide"`, `partial: false`). Every required field must be
    /// present.
    Settle,
    /// An intermediate step of a progressive-disclosure ladder (`partial: true`, §5.5).
    ///
    /// Submitted values are validated against the current field set, but absent required fields are
    /// not an error: §5.5 says "validate the submitted values", and the ladder's later steps are by
    /// definition not submitted yet. See the crate documentation's ambiguity note A-3.
    Partial,
    /// A non-deciding disposition (`delegate` or `unable`, §6.6). The request stays `pending` and
    /// nothing was decided, so requiredness cannot apply — the normative fixture
    /// `use-cases/07-reassign-escalate.json` delegates with `"values": {}`.
    NonDeciding,
}

impl AnswerSpec {
    /// Validate a submitted `values` map against the declared fields (§5.3 rules 3, 4, 5).
    ///
    /// Every violation is collected rather than short-circuited, because a surface has to be able
    /// to mark every offending input at once.
    pub fn validate_answer(&self, values: &Map<String, Value>, mode: AnswerMode) -> Result<()> {
        let mut errors: Vec<FieldError> = Vec::new();

        // Rule 5: an answer carrying a key that is not a declared field name is rejected. This is
        // what stops a compromised surface smuggling keys through to the runtime.
        for key in values.keys() {
            if !self.fields.iter().any(|f| &f.name == key) {
                errors.push(FieldError {
                    name: key.clone(),
                    code: FieldErrorCode::UndeclaredField,
                    message: Some("not a declared field on this request".to_string()),
                });
            }
        }

        for field in &self.fields {
            match values.get(&field.name) {
                None | Some(Value::Null) => {
                    if field.required && mode == AnswerMode::Settle {
                        errors.push(FieldError {
                            name: field.name.clone(),
                            code: FieldErrorCode::Required,
                            message: Some("this field is required".to_string()),
                        });
                    }
                }
                Some(value) => errors.extend(field.validate_value(value)),
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(ProtocolError::new(
                ErrorCode::AnswerValidationFailed,
                "the answer does not match the declared fields",
            )
            .with_fields(errors))
        }
    }

    /// The names of every declared `secret` field.
    ///
    /// The receipt records these names on each step, never their values (§9.2, I7).
    pub fn secret_field_names(&self) -> Vec<String> {
        self.fields
            .iter()
            .filter(|f| f.field_type == FieldType::Secret)
            .map(|f| f.name.clone())
            .collect()
    }
}

// ---------------------------------------------------------------------------------------------
// Authority
// ---------------------------------------------------------------------------------------------

/// Who this is for. One union, resolved through a single target-resolution port (§7.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    /// The kind of target.
    pub kind: TargetKind,
    /// Opaque to the protocol; meaningful to the deployment's directory.
    pub value: String,
}

/// The kinds of thing a delivery can be addressed to (§7.5).
///
/// A Server MUST resolve every kind through **one** port returning a set of principals, and MUST
/// NOT branch per kind anywhere else in the core.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    /// A specific person.
    Principal,
    /// Everyone holding a role.
    Role,
    /// A named group.
    Group,
    /// A rotation, resolved at rung-fire time rather than at raise time.
    Rotation,
    /// Anyone in scope.
    Anyone,
}

/// Who is entitled to answer, declared **on the request** and evaluated at answer time (§4.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Authority {
    /// Minimum role in the owning tenancy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_role: Option<Role>,
    /// How firmly the answerer's identity must be established. Defaults to [`AuthStrength::Session`].
    #[serde(default = "default_auth_strength")]
    pub auth_strength: AuthStrength,
    /// Who this is for. Empty means "anyone who satisfies the rest of this block".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assignees: Vec<Target>,
    /// How many distinct principals must answer before the request settles (§4.5).
    #[serde(default = "default_quorum")]
    pub quorum: u64,
    /// Always `true`. Stated rather than assumed, and a Server MUST reject `false` (§4.3).
    #[serde(default = "default_true")]
    pub forbid_requester: bool,
    /// Why this authority is required, shown to the person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

fn default_auth_strength() -> AuthStrength {
    AuthStrength::Session
}
fn default_quorum() -> u64 {
    1
}
fn default_true() -> bool {
    true
}

impl Default for Authority {
    fn default() -> Self {
        Self {
            min_role: None,
            auth_strength: AuthStrength::Session,
            assignees: Vec::new(),
            quorum: 1,
            forbid_requester: true,
            reason: None,
        }
    }
}

/// What an answerer actually presented, at the moment of the answer.
///
/// §4.3 requires authority to be evaluated against this, not against anything captured at raise
/// time — roles change, and a receipt that records a stale role records a fiction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentedAuthority {
    /// The authenticated principal.
    pub principal: PrincipalId,
    /// The role held at this moment.
    pub role: Role,
    /// The strength actually established.
    pub auth_strength: AuthStrength,
}

impl Authority {
    /// Check the declaration is one this deployment can honour (§4.3, §4.5).
    pub fn validate_declaration(&self, profile: &DeploymentProfile) -> Result<()> {
        if !self.forbid_requester {
            return Err(ProtocolError::new(
                ErrorCode::InvalidRequest,
                "`forbid_requester` is always true; a requester principal may never answer",
            ));
        }
        if self.quorum == 0 || self.quorum > profile.max_quorum {
            return Err(ProtocolError::new(
                ErrorCode::InvalidRequest,
                format!(
                    "quorum {} cannot be honoured by this deployment (maximum {})",
                    self.quorum, profile.max_quorum
                ),
            ));
        }
        if self.auth_strength == AuthStrength::LinkOnly && !profile.allow_link_only {
            return Err(ProtocolError::new(
                ErrorCode::AuthStrengthNotPermitted,
                "this deployment does not accept `link_only`",
            ));
        }
        Ok(())
    }

    /// Evaluate this authority against what an answerer presented.
    ///
    /// The order matters. The machine check comes first and is unconditional: §4.2 requires it to
    /// be enforced by principal **type**, with no role, scope, setting, or deployment mode that can
    /// satisfy a human-intervention request with a machine. Without it, an agent holding an API key
    /// approves itself and every other guarantee in the specification is decoration.
    pub fn evaluate(
        &self,
        presented: &PresentedAuthority,
        profile: &DeploymentProfile,
    ) -> Result<AuthStrength> {
        if presented.principal.is_machine() {
            return Err(ProtocolError::new(
                ErrorCode::RequesterMayNotAnswer,
                "a machine principal may never answer a human-intervention request",
            ));
        }
        if presented.auth_strength == AuthStrength::LinkOnly && !profile.allow_link_only {
            return Err(ProtocolError::new(
                ErrorCode::AuthStrengthNotPermitted,
                "this deployment does not accept `link_only`",
            ));
        }
        if presented.auth_strength < self.auth_strength {
            return Err(ProtocolError::new(
                ErrorCode::InsufficientAuthority,
                format!(
                    "this request requires `{:?}` authentication",
                    self.auth_strength
                ),
            ));
        }
        if let Some(min_role) = self.min_role {
            if presented.role < min_role {
                return Err(ProtocolError::new(
                    ErrorCode::InsufficientAuthority,
                    format!("this request requires the `{min_role:?}` role or higher"),
                ));
            }
        }
        Ok(presented.auth_strength)
    }
}

// ---------------------------------------------------------------------------------------------
// Capabilities
// ---------------------------------------------------------------------------------------------

/// What a capability session may do (§11.3).
///
/// Narrowing beyond these two is expressed through provider `constraints`, which the core carries
/// and never inspects. A Server MUST NOT grow a scope name per case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityScope {
    /// Output only; no input accepted. Enforced server-side, never by a client-side attribute.
    View,
    /// Full interactive control. Requires the administrative role.
    Drive,
}

impl CapabilityScope {
    /// The minimum role this scope requires (§11.3).
    pub const fn minimum_role(self) -> Role {
        match self {
            Self::View => Role::Viewer,
            Self::Drive => Role::Admin,
        }
    }
}

/// What the person must be handed in order to be *able* to answer (§5.4, §11).
///
/// **No resolvable address ever appears here** — not a URL, not an endpoint, not a token (§11.1,
/// I8). The handle is exchanged for a live session at an authenticated resolve endpoint, by the
/// person's own client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityDeclaration {
    /// The opaque public identifier.
    pub handle: GrantHandle,
    /// The capability kind. An open vocabulary the deployment declares; unknown types fail closed.
    #[serde(rename = "type")]
    pub capability_type: String,
    /// The maximum scope this grant can produce.
    pub scope: CapabilityScope,
    /// Opaque provider name. The core looks the provider up and calls it; it never matches on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Opaque provider resource id, handed straight back to the provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_ref: Option<String>,
    /// What this is, in the person's words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Why the person is being handed it, so accepting is an informed act.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    /// Whether the request is answerable without ever resolving this capability.
    #[serde(default)]
    pub optional: bool,
    /// How long the grant stays resolvable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl: Option<IsoDuration>,
    /// Digest of the blast radius the person will be shown. The resolve call echoes it back, so a
    /// person can never be handed something other than what they read (§11.5, I19).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blast_radius_digest: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// The envelope
// ---------------------------------------------------------------------------------------------

/// The three orthogonal declarations that replace a request `kind` enum (§5.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Requires {
    /// Envelope version. A Server that does not understand this value MUST reject the raise and
    /// create nothing (§5.2, C-16, I21).
    pub v: u64,
    /// The shape of the answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<AnswerSpec>,
    /// What the person must be handed in order to answer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<CapabilityDeclaration>,
    /// Who is entitled to answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority: Option<Authority>,
    /// Additional keys, stored verbatim and returned verbatim.
    ///
    /// §19 reserves `x-` for vendor extensions and forbids a Server from interpreting them. Nothing
    /// in this crate reads this map; it exists so that a round trip is byte-faithful.
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

impl Requires {
    /// Parse and fully validate a `requires` envelope, failing closed at every unknown (I21).
    ///
    /// The checks run in the order the error codes are defined, so the caller always gets the most
    /// specific reason:
    ///
    /// 1. `v` must be present and equal to [`REQUIRES_VERSION`], else
    ///    `400 unsupported_requires_version` (§5.2).
    /// 2. Every declared field `type` must be known, else `400 unsupported_field_type` (§5.3).
    /// 3. Every declared capability `type` must be one this deployment renders, else
    ///    `400 unsupported_capability_type` (§19).
    /// 4. The rest of the structure must parse and be internally coherent, else
    ///    `400 invalid_request`.
    ///
    /// Nothing is created on failure, and no unrecognized part is dropped to make the rest fit.
    pub fn parse(value: &Value, profile: &DeploymentProfile) -> Result<Self> {
        let object = value.as_object().ok_or_else(|| {
            ProtocolError::new(ErrorCode::InvalidRequest, "`requires` must be an object")
        })?;

        // 1. Version, before anything else is even looked at.
        match object.get("v").and_then(Value::as_u64) {
            Some(REQUIRES_VERSION) => {}
            Some(other) => {
                return Err(ProtocolError::new(
                    ErrorCode::UnsupportedRequiresVersion,
                    format!("this server implements `requires.v` {REQUIRES_VERSION}, not {other}"),
                ))
            }
            None => {
                return Err(ProtocolError::new(
                    ErrorCode::UnsupportedRequiresVersion,
                    "`requires.v` is required and must be an integer",
                ))
            }
        }

        // 2. Field types. Checked from the raw JSON so the failure is `unsupported_field_type`
        //    rather than whatever serde would say about an unknown enum variant.
        if let Some(fields) = object
            .get("answer")
            .and_then(Value::as_object)
            .and_then(|a| a.get("fields"))
            .and_then(Value::as_array)
        {
            for field in fields {
                let declared = field.get("type").and_then(Value::as_str).ok_or_else(|| {
                    ProtocolError::new(
                        ErrorCode::UnsupportedFieldType,
                        "every declared field must carry a `type`",
                    )
                })?;
                if FieldType::from_str_opt(declared).is_none() {
                    return Err(ProtocolError::new(
                        ErrorCode::UnsupportedFieldType,
                        format!(
                            "field type `{declared}` is not implemented; a field the surface \
                             cannot draw is a field nobody can answer"
                        ),
                    ));
                }
            }
        }

        // 3. Capability types, against the deployment's declared vocabulary.
        if let Some(capabilities) = object.get("capabilities").and_then(Value::as_array) {
            for capability in capabilities {
                let declared = capability
                    .get("type")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        ProtocolError::new(
                            ErrorCode::UnsupportedCapabilityType,
                            "every declared capability must carry a `type`",
                        )
                    })?;
                if !profile.capability_types.contains(declared) {
                    return Err(ProtocolError::new(
                        ErrorCode::UnsupportedCapabilityType,
                        format!(
                            "capability type `{declared}` is not implemented; a capability nobody \
                             can render is a request nobody can answer"
                        ),
                    ));
                }
            }
        }

        // 4. Structure.
        let requires: Requires = serde_json::from_value(value.clone()).map_err(|e| {
            ProtocolError::new(
                ErrorCode::InvalidRequest,
                format!("`requires` is malformed: {e}"),
            )
        })?;
        requires.validate_declaration(profile)?;
        Ok(requires)
    }

    /// Check the declaration is internally coherent and honourable by this deployment.
    pub fn validate_declaration(&self, profile: &DeploymentProfile) -> Result<()> {
        if let Some(answer) = &self.answer {
            let mut names: Vec<&str> = Vec::with_capacity(answer.fields.len());
            for field in &answer.fields {
                field.validate_declaration()?;
                names.push(&field.name);
            }
            let total = names.len();
            names.sort_unstable();
            names.dedup();
            if names.len() != total {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidRequest,
                    "field names must be distinct within one request",
                ));
            }
        }
        self.effective_authority().validate_declaration(profile)?;
        Ok(())
    }

    /// Whether any declared field is a `secret`, which is what triggers the derived floor of §4.3.
    pub fn declares_secret_field(&self) -> bool {
        self.answer
            .as_ref()
            .is_some_and(|a| a.fields.iter().any(|f| f.field_type == FieldType::Secret))
    }

    /// The authority actually enforced, after applying the **derived floor** of §4.3.
    ///
    /// If any declared field is a `secret`, the effective `min_role` rises to [`Role::Admin`] and
    /// the effective `auth_strength` to at least [`AuthStrength::Session`], whatever the client
    /// declared. A client may raise the floor further and cannot lower it.
    ///
    /// This is the mechanism that keeps §5.1 true. The stricter authority for a credential handoff
    /// is a consequence of the request's *shape*, not a hand-written branch for a "login" kind —
    /// which is why the login-assistance pattern of §5.6 needs no special case.
    pub fn effective_authority(&self) -> Authority {
        let mut authority = self.authority.clone().unwrap_or_default();
        if self.declares_secret_field() {
            authority.min_role = Some(authority.min_role.unwrap_or(Role::Admin).max(Role::Admin));
            authority.auth_strength = authority.auth_strength.max(AuthStrength::Session);
        }
        authority
    }

    /// Validate a submitted answer against the declared fields.
    ///
    /// A request with no `answer` block declares no fields, so only undeclared keys can fail.
    pub fn validate_answer(&self, values: &Map<String, Value>, mode: AnswerMode) -> Result<()> {
        static EMPTY: &AnswerSpec = &AnswerSpec {
            fields: Vec::new(),
            value_sink: None,
        };
        self.answer
            .as_ref()
            .unwrap_or(EMPTY)
            .validate_answer(values, mode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::UserId;
    use serde_json::json;

    fn profile() -> DeploymentProfile {
        DeploymentProfile::default().with_capability_types(["interactive_surface"])
    }

    fn approve_reject() -> Value {
        json!({
            "v": 1,
            "answer": {"fields": [
                {"name": "decision", "label": "Decision", "type": "choice", "required": true,
                 "options": [{"id": "approve", "label": "Refund it"},
                             {"id": "reject", "label": "Don't refund"}]},
                {"name": "note", "label": "Add a note", "type": "text", "required": false,
                 "max_len": 500}
            ]},
            "capabilities": [],
            "authority": {"min_role": "editor", "auth_strength": "session", "quorum": 1,
                          "forbid_requester": true}
        })
    }

    fn values(v: Value) -> Map<String, Value> {
        v.as_object().expect("object").clone()
    }

    // -------------------------------------------------------------- fail closed (I21, C-16)

    #[test]
    fn an_unknown_envelope_version_fails_closed() {
        let mut raw = approve_reject();
        raw["v"] = json!(2);
        let err = Requires::parse(&raw, &profile()).expect_err("v: 2 must be rejected");
        assert_eq!(err.code, ErrorCode::UnsupportedRequiresVersion);
        assert_eq!(err.http_status(), 400);
    }

    #[test]
    fn a_missing_envelope_version_fails_closed() {
        let raw = json!({"answer": {"fields": []}});
        assert_eq!(
            Requires::parse(&raw, &profile())
                .expect_err("v is required")
                .code,
            ErrorCode::UnsupportedRequiresVersion
        );
    }

    #[test]
    fn an_unknown_field_type_fails_closed_and_is_never_degraded_to_text() {
        let raw = json!({"v": 1, "answer": {"fields": [{"name": "x", "type": "hologram"}]}});
        let err = Requires::parse(&raw, &profile()).expect_err("unknown type must be rejected");
        assert_eq!(err.code, ErrorCode::UnsupportedFieldType);
    }

    #[test]
    fn an_unknown_capability_type_fails_closed() {
        let raw = json!({"v": 1, "capabilities": [
            {"handle": "hg_01K3M7QW8ZC4YRXB2N6VD9FTHG", "type": "telepathy", "scope": "view"}
        ]});
        let err =
            Requires::parse(&raw, &profile()).expect_err("unknown capability must be rejected");
        assert_eq!(err.code, ErrorCode::UnsupportedCapabilityType);
    }

    #[test]
    fn version_is_checked_before_anything_else_can_fail() {
        // A v2 envelope carrying a v2-only field type must report the version, not the field: the
        // client's problem is that it is talking to the wrong server, not that it typed a typo.
        let raw = json!({"v": 2, "answer": {"fields": [{"name": "x", "type": "hologram"}]}});
        assert_eq!(
            Requires::parse(&raw, &profile())
                .expect_err("rejected")
                .code,
            ErrorCode::UnsupportedRequiresVersion
        );
    }

    // -------------------------------------------------------------- declarations

    #[test]
    fn the_eight_field_types_round_trip_through_their_wire_strings() {
        for &t in FieldType::ALL {
            assert_eq!(FieldType::from_str_opt(t.as_str()), Some(t));
            assert_eq!(
                serde_json::to_value(t).expect("serialize"),
                json!(t.as_str())
            );
        }
        assert_eq!(FieldType::ALL.len(), 8);
        assert_eq!(
            FieldType::from_str_opt("Text"),
            None,
            "matching is exact, not case-folded"
        );
    }

    #[test]
    fn an_empty_field_list_is_legal_and_means_attestation() {
        let raw = json!({"v": 1, "answer": {"fields": []}});
        let requires = Requires::parse(&raw, &profile()).expect("empty fields are legal");
        requires
            .validate_answer(&values(json!({})), AnswerMode::Settle)
            .expect("nothing to type");
    }

    #[test]
    fn incoherent_declarations_are_refused() {
        let cases = [
            json!({"v": 1, "answer": {"fields": [{"name": "d", "type": "choice", "options": []}]}}),
            json!({"v": 1, "answer": {"fields": [
                {"name": "t", "type": "text", "options": [{"id": "a", "label": "A"}]}]}}),
            json!({"v": 1, "answer": {"fields": [{"name": "Decision", "type": "text"}]}}),
            json!({"v": 1, "answer": {"fields": [
                {"name": "d", "type": "text"}, {"name": "d", "type": "text"}]}}),
            json!({"v": 1, "answer": {"fields": [
                {"name": "n", "type": "number", "min": 10.0, "max": 1.0}]}}),
            json!({"v": 1, "answer": {"fields": [{"name": "d", "type": "choice",
                "options": [{"id": "a", "label": "A"}, {"id": "a", "label": "Again"}]}]}}),
        ];
        for raw in cases {
            let err = Requires::parse(&raw, &profile()).expect_err("must be refused");
            assert_eq!(err.code, ErrorCode::InvalidRequest, "{raw}");
        }
    }

    #[test]
    fn forbid_requester_false_is_refused() {
        let mut raw = approve_reject();
        raw["authority"]["forbid_requester"] = json!(false);
        assert_eq!(
            Requires::parse(&raw, &profile())
                .expect_err("rejected")
                .code,
            ErrorCode::InvalidRequest
        );
    }

    #[test]
    fn a_quorum_this_deployment_cannot_honour_is_refused_not_downgraded() {
        let mut raw = approve_reject();
        raw["authority"]["quorum"] = json!(3);
        let err = Requires::parse(&raw, &profile()).expect_err("rejected");
        assert_eq!(err.code, ErrorCode::InvalidRequest);
    }

    #[test]
    fn extension_keys_round_trip_verbatim_and_are_never_interpreted() {
        let mut raw = approve_reject();
        raw["x-acme"] = json!({"tier": "gold", "nested": [1, 2, 3]});
        let requires = Requires::parse(&raw, &profile()).expect("x- keys are carried");
        assert_eq!(
            requires.extensions["x-acme"],
            json!({"tier": "gold", "nested": [1, 2, 3]})
        );
        let round_tripped = serde_json::to_value(&requires).expect("serialize");
        assert_eq!(round_tripped["x-acme"], raw["x-acme"]);
    }

    // -------------------------------------------------------------- answer validation

    #[test]
    fn a_valid_answer_passes() {
        let requires = Requires::parse(&approve_reject(), &profile()).expect("parse");
        requires
            .validate_answer(
                &values(json!({"decision": "approve", "note": "Confirmed on the phone."})),
                AnswerMode::Settle,
            )
            .expect("valid");
    }

    #[test]
    fn a_choice_outside_the_declared_set_is_not_a_free_text_fallback() {
        let requires = Requires::parse(&approve_reject(), &profile()).expect("parse");
        let err = requires
            .validate_answer(&values(json!({"decision": "maybe"})), AnswerMode::Settle)
            .expect_err("rejected");
        assert_eq!(err.code, ErrorCode::AnswerValidationFailed);
        assert_eq!(err.fields()[0].code, FieldErrorCode::NotAnOption);
        assert_eq!(err.http_status(), 422);
    }

    #[test]
    fn an_undeclared_key_is_rejected_rather_than_stored() {
        let requires = Requires::parse(&approve_reject(), &profile()).expect("parse");
        let err = requires
            .validate_answer(
                &values(json!({"decision": "approve", "smuggled": "payload"})),
                AnswerMode::Settle,
            )
            .expect_err("rejected");
        assert_eq!(err.fields()[0].name, "smuggled");
        assert_eq!(err.fields()[0].code, FieldErrorCode::UndeclaredField);
    }

    #[test]
    fn a_missing_required_field_is_reported_by_name() {
        let requires = Requires::parse(&approve_reject(), &profile()).expect("parse");
        let err = requires
            .validate_answer(&values(json!({})), AnswerMode::Settle)
            .expect_err("rejected");
        assert_eq!(err.fields()[0].name, "decision");
        assert_eq!(err.fields()[0].code, FieldErrorCode::Required);
    }

    #[test]
    fn every_violation_is_reported_at_once_so_a_surface_can_mark_them_all() {
        let requires = Requires::parse(&approve_reject(), &profile()).expect("parse");
        let err = requires
            .validate_answer(
                &values(json!({"note": 42, "extra": true})),
                AnswerMode::Settle,
            )
            .expect_err("rejected");
        let codes: BTreeSet<FieldErrorCode> = err.fields().iter().map(|f| f.code).collect();
        assert!(codes.contains(&FieldErrorCode::UndeclaredField));
        assert!(codes.contains(&FieldErrorCode::Required));
        assert!(codes.contains(&FieldErrorCode::WrongType));
    }

    #[test]
    fn a_raw_value_for_a_secret_field_is_refused() {
        let raw = json!({"v": 1, "answer": {"fields": [
            {"name": "password", "type": "secret", "required": true,
             "sink_ref": "snk_01K3M7QW8ZC4YRXB2N6VD9FTHF"}]}});
        let requires = Requires::parse(&raw, &profile()).expect("parse");

        let err = requires
            .validate_answer(&values(json!({"password": "hunter2"})), AnswerMode::Settle)
            .expect_err("a raw secret must be refused");
        assert_eq!(
            err.fields()[0].code,
            FieldErrorCode::SecretValueNotPermitted
        );
        // I18: not even the error message may carry the value back out.
        let rendered = serde_json::to_string(&err.to_envelope()).expect("serialize");
        assert!(
            !rendered.contains("hunter2"),
            "a validation error must not echo the value"
        );

        requires
            .validate_answer(
                &values(json!({"password": {"provided": true}})),
                AnswerMode::Settle,
            )
            .expect("the fact of provision is what travels");
        // Anything richer than the bare fact is still a leak.
        assert!(requires
            .validate_answer(
                &values(json!({"password": {"provided": true, "length": 8}})),
                AnswerMode::Settle
            )
            .is_err());
    }

    #[test]
    fn bounds_and_lengths_are_enforced_server_side() {
        let raw = json!({"v": 1, "answer": {"fields": [
            {"name": "note", "type": "text", "max_len": 4},
            {"name": "amount", "type": "number", "min": 1.0, "max": 10.0},
            {"name": "ok", "type": "boolean"},
            {"name": "cleared", "type": "attestation"},
            {"name": "doc", "type": "file_ref"}
        ]}});
        let requires = Requires::parse(&raw, &profile()).expect("parse");
        for (bad, code) in [
            (json!({"note": "toolong"}), FieldErrorCode::OutOfRange),
            (json!({"amount": 11.0}), FieldErrorCode::OutOfRange),
            (json!({"amount": 0.5}), FieldErrorCode::OutOfRange),
            (json!({"ok": "yes"}), FieldErrorCode::WrongType),
            (
                json!({"cleared": false}),
                FieldErrorCode::AttestationMustBeTrue,
            ),
            (json!({"doc": 7}), FieldErrorCode::WrongType),
        ] {
            let err = requires
                .validate_answer(&values(bad.clone()), AnswerMode::Settle)
                .expect_err("rejected");
            assert_eq!(err.fields()[0].code, code, "{bad}");
        }
        requires
            .validate_answer(
                &values(
                    json!({"note": "fine", "amount": 10.0, "ok": true, "cleared": true,
                               "doc": "file_abc"}),
                ),
                AnswerMode::Settle,
            )
            .expect("all within bounds");
    }

    #[test]
    fn multi_choice_takes_an_array_of_declared_ids() {
        let raw = json!({"v": 1, "answer": {"fields": [
            {"name": "tags", "type": "choice", "multi": true,
             "options": [{"id": "a", "label": "A"}, {"id": "b", "label": "B"}]}]}});
        let requires = Requires::parse(&raw, &profile()).expect("parse");
        requires
            .validate_answer(&values(json!({"tags": ["a", "b"]})), AnswerMode::Settle)
            .expect("valid");
        assert!(requires
            .validate_answer(&values(json!({"tags": "a"})), AnswerMode::Settle)
            .is_err());
        assert!(requires
            .validate_answer(&values(json!({"tags": ["c"]})), AnswerMode::Settle)
            .is_err());
    }

    #[test]
    fn a_non_deciding_disposition_does_not_demand_required_fields() {
        // The normative fixture `use-cases/07-reassign-escalate.json` delegates with `values: {}`.
        let requires = Requires::parse(&approve_reject(), &profile()).expect("parse");
        requires
            .validate_answer(&values(json!({})), AnswerMode::NonDeciding)
            .expect("a delegation decides nothing, so nothing is required");
        requires
            .validate_answer(&values(json!({})), AnswerMode::Partial)
            .expect("an intermediate step has not been asked for the later fields yet");
        assert!(requires
            .validate_answer(&values(json!({})), AnswerMode::Settle)
            .is_err());
    }

    // -------------------------------------------------------------- authority (§4.2, §4.3, §4.4)

    #[test]
    fn a_secret_field_derives_the_admin_floor_without_a_per_kind_branch() {
        let raw = json!({"v": 1, "answer": {"fields": [
            {"name": "email", "type": "text", "required": true},
            {"name": "password", "type": "secret", "required": true,
             "sink_ref": "snk_01K3M7QW8ZC4YRXB2N6VD9FTHF"}]},
            "authority": {"min_role": "viewer", "auth_strength": "link_only"}});
        let requires = Requires::parse(
            &raw,
            &DeploymentProfile {
                allow_link_only: true,
                ..profile()
            },
        )
        .expect("parse");
        // Declared viewer/link_only; the *shape* of the request overrides both.
        let effective = requires.effective_authority();
        assert_eq!(effective.min_role, Some(Role::Admin));
        assert_eq!(effective.auth_strength, AuthStrength::Session);
    }

    #[test]
    fn a_client_may_raise_the_derived_floor_but_never_lower_it() {
        let raw = json!({"v": 1, "answer": {"fields": [
            {"name": "password", "type": "secret"}]},
            "authority": {"min_role": "admin", "auth_strength": "mfa"}});
        let requires = Requires::parse(&raw, &profile()).expect("parse");
        assert_eq!(
            requires.effective_authority().auth_strength,
            AuthStrength::Mfa
        );
    }

    #[test]
    fn a_machine_principal_can_never_answer_whatever_its_role() {
        let requires = Requires::parse(&approve_reject(), &profile()).expect("parse");
        let machine = PresentedAuthority {
            principal: PrincipalId::parse("sa_01J9ZP4KRTC4YRXB2N6VD9FTHE").expect("parse"),
            // Deliberately the highest role and the strongest authentication available.
            role: Role::Admin,
            auth_strength: AuthStrength::Mfa,
        };
        let err = requires
            .effective_authority()
            .evaluate(&machine, &profile())
            .expect_err("a machine may never answer");
        assert_eq!(err.code, ErrorCode::RequesterMayNotAnswer);
        assert_eq!(err.http_status(), 403);
    }

    #[test]
    fn a_person_below_the_declared_role_or_strength_is_refused() {
        let requires = Requires::parse(&approve_reject(), &profile()).expect("parse");
        let person = |role, auth_strength| PresentedAuthority {
            principal: PrincipalId::User(
                UserId::parse("usr_01J9ZP4KRTC4YRXB2N6VD9FTHE").expect("parse"),
            ),
            role,
            auth_strength,
        };
        let authority = requires.effective_authority();
        assert_eq!(
            authority
                .evaluate(&person(Role::Viewer, AuthStrength::Session), &profile())
                .expect_err("below min_role")
                .code,
            ErrorCode::InsufficientAuthority
        );
        assert_eq!(
            authority
                .evaluate(&person(Role::Editor, AuthStrength::LinkOnly), &profile())
                .expect_err("link_only is not enabled here")
                .code,
            ErrorCode::AuthStrengthNotPermitted
        );
        assert_eq!(
            authority
                .evaluate(&person(Role::Admin, AuthStrength::Mfa), &profile())
                .expect("a stronger grade satisfies a weaker requirement"),
            AuthStrength::Mfa
        );
    }

    #[test]
    fn link_only_is_refused_unless_the_deployment_opted_in() {
        // C-6b, both halves.
        let raw = json!({"v": 1, "authority": {"auth_strength": "link_only"}});
        assert_eq!(
            Requires::parse(&raw, &profile())
                .expect_err("forbidden by default")
                .code,
            ErrorCode::AuthStrengthNotPermitted
        );
        let opted_in = DeploymentProfile {
            allow_link_only: true,
            ..profile()
        };
        Requires::parse(&raw, &opted_in).expect("accepted where the deployment enabled it");
    }

    #[test]
    fn scopes_carry_their_own_minimum_role() {
        assert_eq!(CapabilityScope::View.minimum_role(), Role::Viewer);
        assert_eq!(CapabilityScope::Drive.minimum_role(), Role::Admin);
        assert!(CapabilityScope::View.minimum_role() < CapabilityScope::Drive.minimum_role());
    }

    // -------------------------------------------------------------- the eight patterns (C-22)

    #[test]
    fn all_eight_interaction_patterns_parse_with_no_kind_field_anywhere() {
        // Verbatim from `spec/fixtures/use-cases/`, reduced to the `requires` envelope. C-22 asserts
        // the full HTTP behaviour; what belongs here is that one declaration model expresses all
        // eight without the core learning a single new branch (I14).
        let patterns: Vec<(&str, Value)> = vec![
            ("approve-reject", approve_reject()),
            (
                "answer-a-question",
                json!({"v": 1, "answer": {"fields": [
                    {"name": "po_number", "label": "PO number", "type": "text", "required": true,
                     "max_len": 40}]},
                    "capabilities": [],
                    "authority": {"min_role": "viewer", "auth_strength": "session", "quorum": 1,
                                  "forbid_requester": true}}),
            ),
            (
                "login-assistance",
                json!({"v": 1, "answer": {
                    "fields": [
                        {"name": "email", "label": "Email or username", "type": "text",
                         "required": true},
                        {"name": "password", "label": "Password", "type": "secret",
                         "required": true, "sink_ref": "snk_01K3M7QW8ZC4YRXB2N6VD9FTHF"}],
                    "value_sink": {"provider": "example/browser", "op": "submit_credentials",
                                   "ref": "opaque:bs_4KpQ"}},
                    "capabilities": [{"handle": "hg_01K3M7QW8ZC4YRXB2N6VD9FTHG",
                        "type": "interactive_surface", "scope": "drive",
                        "provider": "example/browser", "resource_ref": "opaque:bs_4KpQ",
                        "optional": true, "ttl": "PT15M",
                        "label": "the browser the agent is driving",
                        "purpose": "Finish sign-in yourself if the site uses single sign-on.",
                        "blast_radius_digest": "sha256:9f2c8a1b0d3e4f5a6b7c8d9e0f1a2b3c9f2c8a1b0d3e4f5a6b7c8d9e0f1a2b3c"}],
                    "authority": {"min_role": "admin", "auth_strength": "session", "quorum": 1,
                                  "forbid_requester": true,
                                  "reason": "the resulting session outlives the run"}}),
            ),
            (
                "challenge-or-takeover",
                json!({"v": 1, "answer": {"fields": [
                    {"name": "cleared", "label": "I cleared it", "type": "attestation",
                     "required": true}]},
                    "capabilities": [{"handle": "hg_01K3M7QW8ZC4YRXB2N6VD9FTHH",
                        "type": "interactive_surface", "scope": "drive", "optional": false,
                        "ttl": "PT15M"}],
                    "authority": {"min_role": "admin", "auth_strength": "session", "quorum": 1,
                                  "forbid_requester": true}}),
            ),
            (
                "review-and-correction",
                json!({"v": 1, "answer": {"fields": [
                    {"name": "invoice", "label": "Invoice", "type": "document", "required": true,
                     "schema_ref": "https://schemas.example.com/invoice-v2.json",
                     "initial": {"vendor": "Acme Corp", "total": "2400.00"}}]},
                    "capabilities": [],
                    "authority": {"min_role": "editor", "auth_strength": "session", "quorum": 1,
                                  "forbid_requester": true}}),
            ),
            (
                "confirm-an-external-side-effect",
                json!({"v": 1, "answer": {"fields": [
                    {"name": "decision", "label": "Decision", "type": "choice", "required": true,
                     "options": [{"id": "confirm", "label": "Send it"},
                                 {"id": "cancel", "label": "Don't send"}]}]},
                    "capabilities": [],
                    "authority": {"min_role": "editor", "auth_strength": "reauth", "quorum": 1,
                                  "forbid_requester": true,
                                  "reason": "the send cannot be undone"}}),
            ),
            // Patterns 7 and 8 deliberately have no request shape: reassign/escalate is an
            // operation (§6.6) and expiry is a policy (§6.4). The envelope they ride on is an
            // ordinary one, which is the point.
            (
                "reassign-or-escalate",
                json!({"v": 1, "answer": {"fields": [
                    {"name": "decision", "type": "choice",
                     "options": [{"id": "approve", "label": "Approve"}]}]}}),
            ),
            (
                "expiry-when-nobody-answers",
                json!({"v": 1, "answer": {"fields": [
                    {"name": "decision", "type": "choice", "required": true,
                     "options": [{"id": "approve", "label": "Approve"},
                                 {"id": "reject", "label": "Reject"}]}]},
                    "capabilities": [],
                    "authority": {"min_role": "editor", "auth_strength": "session", "quorum": 1,
                                  "forbid_requester": true}}),
            ),
        ];

        assert_eq!(patterns.len(), 8);
        for (name, raw) in patterns {
            let requires = Requires::parse(&raw, &profile())
                .unwrap_or_else(|e| panic!("pattern `{name}` must parse: {e}"));
            let round_tripped = serde_json::to_value(&requires).expect("serialize");
            assert!(
                !round_tripped.to_string().contains("\"kind\":"),
                "pattern `{name}` must not carry a request kind anywhere (I14)"
            );
            assert!(
                requires.effective_authority().forbid_requester,
                "pattern `{name}` must forbid the requester"
            );
        }
    }
}

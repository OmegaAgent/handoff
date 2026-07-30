//! The REQUEST state machine (§6), request identity (§3), and what a raise declares (§5.2).
//!
//! `pending` is the only non-terminal state. `answered`, `expired`, `cancelled`, and `superseded`
//! are terminal, and every terminal transition produces a typed terminal signal to the waiter
//! (I11): **a request never goes quiet**.
//!
//! Two rules from §6 shape this module more than the rest:
//!
//! * **R5 is a conditional write.** The answer must be a state-conditional update
//!   (`… WHERE state = 'pending'`) or an equivalent compare-and-set, never a read-then-write. This
//!   crate cannot perform the write, so [`AnswerGuards::conditional_write_won`] carries its result
//!   in, and a lost race becomes `409 already_answered` rather than a silent overwrite (C-3).
//! * **R11 is a product rule, not an edge case.** A machine changing its mind a millisecond after a
//!   person acted must not discard that person's work, so a cancel or an expiry that races a landed
//!   answer loses.

use crate::clock::{IsoDuration, Timestamp};
use crate::error::{ErrorCode, ProtocolError, Result};
use crate::id::RequestId;
use crate::receipt::{canonical_json, digest_of, Digest, PresentationBinding};
use crate::requires::{DeploymentProfile, Requires, Target};
use crate::waiter::SignalType;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

// ---------------------------------------------------------------------------------------------
// Declarations carried on a raise
// ---------------------------------------------------------------------------------------------

/// What the person reads (§5.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prompt {
    /// One line, answerable on its own: it is what arrives on a lock screen and in a chat preview.
    pub title: String,
    /// Markdown prose. Rendered by the surface; never interpreted by the core.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// The things the person should look at before deciding, structured so every surface can render
    /// them natively rather than pasting a URL into prose.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<Evidence>,
}

/// How one piece of supporting material should be rendered.
///
/// Unlike a field type, an unknown evidence kind is **not** fatal: `openapi.yaml` says it renders as
/// a labelled link or is ignored, never as an error. Evidence is context, and a person who cannot
/// see one attachment can still answer; a field they cannot draw means they cannot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    /// An external locator, opened as the reader's own authenticated action.
    Link,
    /// Tabular data.
    Table,
    /// An image.
    Image,
    /// A structured document.
    Document,
    /// Plain text.
    Text,
}

/// One piece of supporting material.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    /// How to render it.
    pub kind: EvidenceKind,
    /// What this evidence is, in the person's words.
    pub label: String,
    /// An external locator. **Never a capability** — opening it is the reader's own action (§4.6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Inline content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    /// An opaque handle to content held elsewhere, resolved by the surface.
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

/// How hard to try. The client declares urgency; **the Server decides the channel** (§7.4).
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Urgency {
    /// Lowest.
    Low,
    /// The default.
    #[default]
    Normal,
    /// Raised.
    High,
    /// Highest.
    Critical,
}

/// Whether a person is expected to be working the request right now (§6.3).
///
/// **A label and a sort key, never a filter.** A `pending` request whose attempt lapsed must stay
/// listed and answerable, or "always resumable" is true in the database and false in the inbox
/// (I4, C-9).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UrgencyState {
    /// A person is expected to be on it right now.
    #[default]
    Attention,
    /// The attempt lapsed and nobody is actively working it.
    Waiting,
}

/// Where the wait lives (§8.4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Liveness {
    /// A server-side parked run. The answer is worth recording even if the caller is gone.
    #[default]
    Durable,
    /// A live client process holding a long poll.
    Leased,
}

/// What to do when the waiter goes terminal or its lease lapses (§8.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnWaiterTerminal {
    /// Preserve the ask so a late answer is still recorded. The default for `durable`.
    Keep,
    /// Stop paging people about work nobody is left to receive. The default for `leased`.
    Cancel,
}

impl Liveness {
    /// The default disposition for a waiter of this kind (§8.4).
    pub const fn default_on_waiter_terminal(self) -> OnWaiterTerminal {
        match self {
            Self::Durable => OnWaiterTerminal::Keep,
            Self::Leased => OnWaiterTerminal::Cancel,
        }
    }
}

/// Whether the runtime must redeem before performing the effect (§10.1).
///
/// Both modes are the same code path with a different declared property. **Blocking is a property a
/// request declares; it MUST NOT be a policy the platform imposes on a class of actions** — that is
/// an interception gate, and it is outside this protocol.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// The runtime proceeds; the decision is typed input.
    #[default]
    Advisory,
    /// The runtime must redeem the authorization before the effect.
    Gated,
}

/// What an answerer did, as opposed to what they decided (§6.6).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    /// Settled the request and minted an authorization.
    #[default]
    Decide,
    /// Handed the decision on. The request stays `pending`; a delegation is not a decision.
    Delegate,
    /// Was asked, engaged, and could not complete the ask. An honest fact worth recording, and
    /// better than silence that looks like inattention.
    Unable,
}

/// What happens at `expires_at` (§6.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnExpiry {
    /// Advance the ladder and extend the TTL, falling through to the deployment's terminal policy
    /// once rungs are exhausted.
    Escalate,
    /// Settle as `expired` with `effective: "deny"` — unanswered means no.
    ExpireAndDeny,
    /// Settle as `expired` carrying the pre-declared `default_answer`, with a receipt that records
    /// `actor.type = "policy"` so no audit mistakes it for consent.
    Default,
    /// Never expire; re-remind on a cadence.
    Park,
}

/// The TTL policy declared on a request (§6.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TtlPolicy {
    /// What to do at `expires_at`.
    pub on_expiry: OnExpiry,
    /// Required when `on_expiry` is `default`, and **declared at raise time** — before anyone knew
    /// the person would go quiet. That is what makes it a pre-agreement rather than a convenient
    /// assumption made after the fact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_answer: Option<Map<String, Value>>,
    /// For `park` only: how often to re-page while the request stays open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reminder_every: Option<IsoDuration>,
}

impl TtlPolicy {
    /// The policy that applies when a request declares none (§6.4).
    ///
    /// With a TTL: escalate, falling through to the deployment's terminal policy. Without one, the
    /// request does not expire and the policy is irrelevant. The general rule the default
    /// expresses: **fail toward a typed terminal answer, never toward silence, and when the typed
    /// answer must be guessed, guess "no."**
    pub fn default_for(has_ttl: bool) -> Option<Self> {
        has_ttl.then_some(Self {
            on_expiry: OnExpiry::Escalate,
            default_answer: None,
            reminder_every: None,
        })
    }

    /// Check the policy is complete enough to run.
    pub fn validate(&self) -> Result<()> {
        if self.on_expiry == OnExpiry::Default && self.default_answer.is_none() {
            return Err(ProtocolError::new(
                ErrorCode::InvalidRequest,
                "`on_expiry: \"default\"` requires a `default_answer` declared at raise time",
            ));
        }
        Ok(())
    }
}

/// One step of an escalation ladder, fired `after` the request was raised (§7.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutingRung {
    /// How long after the raise this rung fires.
    pub after: IsoDuration,
    /// Channel names. An **open vocabulary**: the core carries the string and looks up an adapter.
    pub channels: Vec<String>,
    /// Who this rung reaches, when it differs from the request's targets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<Target>,
}

/// A per-request override of the deployment's escalation policy (§7.4).
///
/// Overriding the ladder MUST require a **separate scope** from raising a request: a compromised key
/// that can ask a question is a materially different blast radius from one that can page an on-call
/// engineer at 3 a.m.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Routing {
    /// Rung-0 targets.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<Target>,
    /// Ordered rungs. **A rung mints deliveries, never a new request** (I3).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ladder: Vec<RoutingRung>,
}

/// Where to POST signals, for runtimes whose wait lives server-side (§15).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Callback {
    /// HTTPS endpoint. Signed per delivery.
    pub url: String,
    /// Handle for the signing secret. Two may be active during a rotation overlap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_ref: Option<String>,
}

/// The Level 2 `continuation` extension (§14).
///
/// **The Server stores a pointer or a blob. It never stores meaning.** Nothing in this crate
/// dereferences, parses, or interprets either field, and no method here does anything with them
/// except carry them. Execution resumption is a property of the runtime, not of this protocol
/// (§1.3), and this crate deliberately implements none of it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Continuation {
    /// A URI the runtime owns. Stored verbatim, never dereferenced.
    pub resume_ref: Option<String>,
    /// Opaque bytes, at most 64 KiB. Base64 on the wire; encrypted at rest by the Server.
    pub resume_payload: Option<Vec<u8>>,
}

/// The largest `resume_payload` §14 permits.
pub const MAX_RESUME_PAYLOAD_BYTES: usize = 64 * 1024;

impl Continuation {
    /// Whether anything was declared at all.
    pub fn is_empty(&self) -> bool {
        self.resume_ref.is_none() && self.resume_payload.is_none()
    }
}

/// Everything a caller declares when it asks for a person (§5.2).
///
/// Note what is **not** here: no channel, no recipient address, and no request `kind`.
///
/// There is deliberately no derived `Deserialize`. The only way in is [`RaiseRequest::parse`],
/// which fails closed on an unknown envelope version, field type, or capability type. A derived
/// impl would be a second door into the same room with the lock left off.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RaiseRequest {
    /// The caller's opaque grouping key for a unit of runtime work (§3.4).
    pub waiter_ref: String,
    /// Where the wait lives.
    pub liveness: Liveness,
    /// How hard to try.
    pub urgency: Urgency,
    /// What the person reads.
    pub prompt: Prompt,
    /// What the request needs.
    pub requires: Requires,
    /// How long the ask stays worth answering. **Absent means it never expires** (§6.3).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl: Option<IsoDuration>,
    /// What happens at `expires_at`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl_policy: Option<TtlPolicy>,
    /// The attempt window (§6.3). Defaults to `PT15M`, uniformly: a Server MUST NOT vary it by
    /// interaction type.
    pub attempt_ttl: IsoDuration,
    /// What to do when the waiter goes terminal.
    pub on_waiter_terminal: OnWaiterTerminal,
    /// A per-request ladder override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routing: Option<Routing>,
    /// Advisory or gated.
    pub mode: Mode,
    /// How strictly the answer must match what was shown.
    pub presentation_binding: PresentationBinding,
    /// Ask-once key. When absent the Server derives one (§3.3).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dedupe_key: Option<String>,
    /// The Level 2 continuation fields, carried and never interpreted.
    #[serde(skip)]
    pub continuation: Continuation,
    /// Where to POST signals.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callback: Option<Callback>,
    /// Caller-owned annotations, stored and returned verbatim.
    ///
    /// **The core never switches on anything in here** — including a `hint` key, which is
    /// explicitly non-normative (§5.1).
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub metadata: Map<String, Value>,
    /// Route to a sandbox destination instead of to real people.
    pub test_mode: bool,
}

/// The RECOMMENDED default attempt window (§6.3).
pub const DEFAULT_ATTEMPT_TTL: IsoDuration = IsoDuration::from_secs(15 * 60);

impl RaiseRequest {
    /// Parse and fully validate a raise, failing closed at every unknown (I21).
    ///
    /// `requires` is validated first and on its own terms, so the caller gets
    /// `unsupported_requires_version` or `unsupported_field_type` rather than a structural
    /// complaint about something further down the body.
    pub fn parse(value: &Value, profile: &DeploymentProfile) -> Result<Self> {
        let object = value.as_object().ok_or_else(|| {
            ProtocolError::new(ErrorCode::InvalidRequest, "a raise body must be an object")
        })?;
        let invalid = |why: String| ProtocolError::new(ErrorCode::InvalidRequest, why);

        for key in object.keys() {
            if !RAISE_KEYS.contains(&key.as_str()) {
                return Err(invalid(format!("`{key}` is not a field of a raise")));
            }
        }

        let requires_raw = object
            .get("requires")
            .ok_or_else(|| invalid("`requires` is required".to_string()))?;
        let requires = Requires::parse(requires_raw, profile)?;

        let waiter_ref = object
            .get("waiter_ref")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("`waiter_ref` is required".to_string()))?
            .to_string();
        if waiter_ref.is_empty() || waiter_ref.len() > 512 {
            return Err(invalid("`waiter_ref` must be 1..=512 bytes".to_string()));
        }

        let prompt: Prompt = object
            .get("prompt")
            .ok_or_else(|| invalid("`prompt` is required".to_string()))
            .and_then(|p| {
                serde_json::from_value(p.clone()).map_err(|e| invalid(format!("`prompt`: {e}")))
            })?;
        if prompt.title.is_empty() || prompt.title.chars().count() > 200 {
            return Err(invalid(
                "`prompt.title` must be 1..=200 characters".to_string(),
            ));
        }

        // `null` and absent mean the same thing everywhere in this body: take the default.
        let optional = |name: &str| -> Result<Option<Value>> {
            Ok(match object.get(name) {
                None | Some(Value::Null) => None,
                Some(v) => Some(v.clone()),
            })
        };

        let liveness: Liveness = match optional("liveness")? {
            None => Liveness::default(),
            Some(v) => {
                serde_json::from_value(v).map_err(|e| invalid(format!("`liveness`: {e}")))?
            }
        };
        let urgency: Urgency = match optional("urgency")? {
            None => Urgency::default(),
            Some(v) => serde_json::from_value(v).map_err(|e| invalid(format!("`urgency`: {e}")))?,
        };
        let ttl: Option<IsoDuration> = match optional("ttl")? {
            None => None,
            Some(v) => Some(serde_json::from_value(v).map_err(|e| invalid(format!("`ttl`: {e}")))?),
        };
        let ttl_policy: Option<TtlPolicy> = match optional("ttl_policy")? {
            None => TtlPolicy::default_for(ttl.is_some()),
            Some(v) => {
                Some(serde_json::from_value(v).map_err(|e| invalid(format!("`ttl_policy`: {e}")))?)
            }
        };
        if let Some(policy) = &ttl_policy {
            policy.validate()?;
        }
        let attempt_ttl: IsoDuration = match optional("attempt_ttl")? {
            None => DEFAULT_ATTEMPT_TTL,
            Some(v) => {
                serde_json::from_value(v).map_err(|e| invalid(format!("`attempt_ttl`: {e}")))?
            }
        };
        let on_waiter_terminal: OnWaiterTerminal = match optional("on_waiter_terminal")? {
            None => liveness.default_on_waiter_terminal(),
            Some(v) => serde_json::from_value(v)
                .map_err(|e| invalid(format!("`on_waiter_terminal`: {e}")))?,
        };
        let routing: Option<Routing> = match optional("routing")? {
            None => None,
            Some(v) => {
                Some(serde_json::from_value(v).map_err(|e| invalid(format!("`routing`: {e}")))?)
            }
        };
        let mode: Mode = match optional("mode")? {
            None => Mode::default(),
            Some(v) => serde_json::from_value(v).map_err(|e| invalid(format!("`mode`: {e}")))?,
        };
        let presentation_binding: PresentationBinding = match optional("presentation_binding")? {
            None => PresentationBinding::default(),
            Some(v) => serde_json::from_value(v)
                .map_err(|e| invalid(format!("`presentation_binding`: {e}")))?,
        };
        let dedupe_key: Option<String> = match optional("dedupe_key")? {
            None => None,
            Some(v) => {
                let key = v
                    .as_str()
                    .ok_or_else(|| invalid("`dedupe_key` must be a string".to_string()))?;
                if key.len() > 255 {
                    return Err(invalid(
                        "`dedupe_key` must be at most 255 bytes".to_string(),
                    ));
                }
                Some(key.to_string())
            }
        };
        let callback: Option<Callback> = match optional("callback")? {
            None => None,
            Some(v) => {
                Some(serde_json::from_value(v).map_err(|e| invalid(format!("`callback`: {e}")))?)
            }
        };
        let metadata = match optional("metadata")? {
            None => Map::new(),
            Some(Value::Object(m)) => m,
            Some(_) => return Err(invalid("`metadata` must be an object".to_string())),
        };
        let test_mode = match optional("test_mode")? {
            None => false,
            Some(Value::Bool(b)) => b,
            Some(_) => return Err(invalid("`test_mode` must be a boolean".to_string())),
        };

        let continuation = Continuation {
            resume_ref: match optional("resume_ref")? {
                None => None,
                Some(v) => Some(
                    v.as_str()
                        .filter(|s| s.len() <= 2048)
                        .ok_or_else(|| {
                            invalid(
                                "`resume_ref` must be a string of at most 2048 bytes".to_string(),
                            )
                        })?
                        .to_string(),
                ),
            },
            resume_payload: match optional("resume_payload")? {
                None => None,
                Some(v) => {
                    let encoded = v.as_str().ok_or_else(|| {
                        invalid("`resume_payload` must be a base64 string".to_string())
                    })?;
                    let bytes = base64_decode(encoded)?;
                    if bytes.len() > MAX_RESUME_PAYLOAD_BYTES {
                        return Err(invalid(format!(
                            "`resume_payload` is {} bytes; the limit is {MAX_RESUME_PAYLOAD_BYTES}",
                            bytes.len()
                        )));
                    }
                    Some(bytes)
                }
            },
        };

        Ok(Self {
            waiter_ref,
            liveness,
            urgency,
            prompt,
            requires,
            ttl,
            ttl_policy,
            attempt_ttl,
            on_waiter_terminal,
            routing,
            mode,
            presentation_binding,
            dedupe_key,
            continuation,
            callback,
            metadata,
            test_mode,
        })
    }

    /// The `dedupe_key` this raise collapses on, derived when the caller supplied none (§3.3).
    ///
    /// `sha256( waiter_ref ‖ canonical_json(requires) ‖ canonical_json(prompt.title) )`, with `‖`
    /// concatenating UTF-8 bytes with no separator. Deriving it means collapse-on-retry holds even
    /// for callers that never think about idempotency.
    pub fn effective_dedupe_key(&self) -> Result<String> {
        if let Some(key) = &self.dedupe_key {
            return Ok(key.clone());
        }
        let mut bytes = self.waiter_ref.as_bytes().to_vec();
        bytes.extend_from_slice(&canonical_json(
            &serde_json::to_value(&self.requires).map_err(|e| {
                ProtocolError::new(ErrorCode::InvalidRequest, format!("`requires`: {e}"))
            })?,
        )?);
        bytes.extend_from_slice(&canonical_json(&Value::String(self.prompt.title.clone()))?);
        Ok(Digest::sha256(&bytes).to_string())
    }

    /// The digest a receipt records as `request_digest` (§9.2).
    pub fn digest(&self) -> Result<Digest> {
        digest_of(&serde_json::to_value(self).map_err(|e| {
            ProtocolError::new(
                ErrorCode::InvalidRequest,
                format!("raise is not serializable: {e}"),
            )
        })?)
    }
}

/// Every key a raise body may carry. Anything else is `400 invalid_request`.
const RAISE_KEYS: &[&str] = &[
    "waiter_ref",
    "liveness",
    "urgency",
    "prompt",
    "requires",
    "ttl",
    "ttl_policy",
    "attempt_ttl",
    "on_waiter_terminal",
    "routing",
    "mode",
    "presentation_binding",
    "dedupe_key",
    "resume_ref",
    "resume_payload",
    "callback",
    "metadata",
    "test_mode",
];

/// Decode standard base64 with padding, strictly.
///
/// Non-canonical input is refused so that §14's "returned **byte-identical**" holds without storing
/// the original spelling: every accepted encoding is the one [`base64_encode`] would produce.
pub fn base64_decode(s: &str) -> Result<Vec<u8>> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bad = || {
        ProtocolError::new(
            ErrorCode::InvalidRequest,
            "`resume_payload` must be canonical, padded base64",
        )
    };
    let bytes = s.as_bytes();
    if bytes.len() % 4 != 0 {
        return Err(bad());
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        let padding = chunk.iter().filter(|&&c| c == b'=').count();
        if padding > 2 || (padding > 0 && chunk[..4 - padding].contains(&b'=')) {
            return Err(bad());
        }
        let mut acc: u32 = 0;
        for (i, &c) in chunk.iter().enumerate() {
            let value = if c == b'=' {
                0
            } else {
                ALPHABET.iter().position(|&a| a == c).ok_or_else(bad)? as u32
            };
            acc |= value << (18 - 6 * i);
        }
        for i in 0..(3 - padding) {
            out.push(((acc >> (16 - 8 * i)) & 0xff) as u8);
        }
        // Reject encodings whose padding bits are not zero: they have two spellings, and only one
        // of them round-trips.
        if padding > 0 && (acc & ((1 << (8 * padding)) - 1)) != 0 {
            return Err(bad());
        }
    }
    Ok(out)
}

/// Encode bytes as standard padded base64, the canonical spelling [`base64_decode`] accepts.
pub fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let mut acc: u32 = 0;
        for (i, &b) in chunk.iter().enumerate() {
            acc |= u32::from(b) << (16 - 8 * i);
        }
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((acc >> (18 - 6 * i)) & 0x3f) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Request identity (§3.3)
// ---------------------------------------------------------------------------------------------

/// What an `Idempotency-Key` matched, if anything (§3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdempotencyMatch {
    /// Whether the stored request's body digest equals this raise's.
    pub same_body_digest: bool,
}

/// Why a raise resolved to an existing request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameRequestReason {
    /// The same `Idempotency-Key` within its window, with the same body. **Retry safety.**
    IdempotencyKey,
    /// A `pending` request already carries this `dedupe_key`. **Ask-once.**
    DedupeKey,
}

/// What `POST /v1/requests` should do with this raise (§3.3).
///
/// The `201` / `200` distinction is contract: it is how a client tells "I asked" from "I already
/// asked".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RaiseOutcome {
    /// A new request. Responds `201` and runs R1.
    New,
    /// The same request. Responds `200` with the stored representation in its *current* state — a
    /// retried raise after a person has already answered returns the answered request and its
    /// receipt, and does **not** re-ask.
    Existing {
        /// Which of the two keys collapsed it.
        reason: SameRequestReason,
        /// Whether `prompt` and `requires` merge forward and the version increments (rule 3).
        merge_forward: bool,
    },
}

impl RaiseOutcome {
    /// The HTTP status this outcome responds with.
    pub const fn http_status(self) -> u16 {
        match self {
            Self::New => 201,
            Self::Existing { .. } => 200,
        }
    }
}

/// Decide whether this raise is a new request or an existing one (§3.3).
///
/// The four rules are evaluated in exactly the specified order, which is what makes the three keys
/// of §3.1 stay distinct instead of collapsing into one.
pub fn resolve_raise(
    idempotency: Option<IdempotencyMatch>,
    pending_request_with_same_dedupe_key: bool,
) -> Result<RaiseOutcome> {
    match idempotency {
        Some(IdempotencyMatch {
            same_body_digest: true,
        }) => Ok(RaiseOutcome::Existing {
            reason: SameRequestReason::IdempotencyKey,
            merge_forward: false,
        }),
        Some(IdempotencyMatch {
            same_body_digest: false,
        }) => Err(ProtocolError::new(
            ErrorCode::IdempotencyKeyReused,
            "this idempotency key was used with a different body; the stored request is unchanged",
        )),
        None if pending_request_with_same_dedupe_key => Ok(RaiseOutcome::Existing {
            reason: SameRequestReason::DedupeKey,
            merge_forward: true,
        }),
        None => Ok(RaiseOutcome::New),
    }
}

// ---------------------------------------------------------------------------------------------
// The state machine (§6.1, §6.2)
// ---------------------------------------------------------------------------------------------

/// The states a request occupies (§6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestState {
    /// The only non-terminal state. A person still owes us an answer.
    Pending,
    /// A person decided.
    Answered,
    /// The TTL elapsed and the policy settled it.
    Expired,
    /// The requester withdrew it.
    Cancelled,
    /// A materially different ask replaced it.
    Superseded,
}

impl RequestState {
    /// Every state, in `openapi.yaml` order.
    pub const ALL: &'static [RequestState] = &[
        Self::Pending,
        Self::Answered,
        Self::Expired,
        Self::Cancelled,
        Self::Superseded,
    ];

    /// Whether a Server MUST NOT transition out of this state.
    pub const fn is_terminal(self) -> bool {
        !matches!(self, Self::Pending)
    }

    /// The terminal signal this state produces to the waiter, if it is terminal (I11).
    pub const fn terminal_signal(self) -> Option<SignalType> {
        match self {
            Self::Pending => None,
            Self::Answered => Some(SignalType::Answered),
            Self::Expired => Some(SignalType::Expired),
            Self::Cancelled => Some(SignalType::Cancelled),
            Self::Superseded => Some(SignalType::Superseded),
        }
    }

    /// The `409` a settled request answers a late write with (§6.7 rule 2).
    pub const fn settled_error_code(self) -> Option<ErrorCode> {
        match self {
            Self::Pending => None,
            Self::Answered => Some(ErrorCode::AlreadyAnswered),
            Self::Expired => Some(ErrorCode::RequestExpired),
            Self::Cancelled => Some(ErrorCode::RequestCancelled),
            Self::Superseded => Some(ErrorCode::RequestSuperseded),
        }
    }
}

/// Which row of §6.2's table an accepted transition took.
///
/// The `R` numbers are the specification's and are stable. The two named variants are moves §5.5
/// and §6.6 describe in prose but give no numbered row; see the crate documentation's spec defect
/// D-3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransitionRule {
    /// ∅ → `pending`: raise.
    R1,
    /// `pending` → `pending`: amend.
    R2,
    /// `pending` → `pending`: the attempt clock lapsed.
    R3,
    /// `pending` → `pending`: escalate to the next rung.
    R4,
    /// `pending` → `answered`: a person decided.
    R5,
    /// `pending` → `expired`: the TTL swept.
    R6,
    /// `pending` → `cancelled`.
    R7,
    /// `pending` → `superseded`.
    R8,
    /// `answered` → `answered`: a duplicate answer under the same idempotency key. No state change.
    R9,
    /// §5.5 — a progressive-disclosure step. `pending` → `pending`, and the waiter is deliberately
    /// **not** signalled: the runtime must not learn that an intermediate step occurred.
    ProgressiveStep,
    /// §6.6 — `delegate` or `unable`. `pending` → `pending`; a delegation is not a decision.
    NonDecidingDisposition,
    /// §4.5 — a partial answer toward a quorum. `pending` → `pending`, recorded as an endorsement.
    QuorumEndorsement,
}

/// What a transition commits, **in one atomic transaction** with the state change (§6.2, I12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestEffect {
    /// Insert the request row.
    InsertRequest,
    /// Register the waiter as `armed`, keyed `(request_id, waiter_ref)`.
    RegisterWaiterArmed,
    /// Resolve the routing policy and snapshot it onto the request, so a policy edit mid-flight
    /// cannot retroactively change what happened (§7.4).
    SnapshotRoutingPolicy,
    /// Enqueue the rung-0 deliveries. A raise MUST NOT block on delivery (§7.3).
    EnqueueRungZeroDeliveries,
    /// Mint one capability grant per declared capability, server-side (§11.4).
    MintCapabilityGrants,
    /// Merge `prompt` and `requires` forward from the newer raise.
    MergeDeclarationsForward,
    /// Bump `version`. A receipt records the version the person actually saw.
    IncrementVersion,
    /// Re-render open deliveries against the new version.
    ReRenderOpenDeliveries,
    /// Stamp the attempt lapse **once, ever**.
    StampAttemptLapseOnce,
    /// Set the request's urgency label.
    SetUrgencyState(UrgencyState),
    /// Re-list the request in every inbox. A lapsed attempt changes urgency, never visibility (I4).
    RelistInEveryInbox,
    /// Mint the deliveries for a ladder rung. **A rung mints deliveries, never a request** (I3).
    MintRungDeliveries,
    /// Extend `expires_at` by the rung's grant.
    ExtendExpiry,
    /// Amend `requires.answer.fields` to the next step's field set (§5.5).
    AmendFieldSetToNextStep,
    /// Re-arm the attempt clock **fresh**, never inheriting the remaining time (§5.5, §6.3).
    RearmAttemptClockFresh,
    /// Append a step record to the eventual receipt (§5.5).
    AppendReceiptStep,
    /// Record a delegation or an inability on the receipt-to-be (§6.6).
    RecordDisposition(Disposition),
    /// Record one endorsement toward quorum (§4.5).
    RecordEndorsement,
    /// Mint the decision RECEIPT.
    MintReceipt,
    /// Mint the single AUTHORIZATION this answer produces (I10).
    MintAuthorization,
    /// Mint a **policy** receipt, with `actor.type = "policy"` (§9.6).
    MintPolicyReceipt,
    /// Link `superseded_by` and its inverse.
    LinkSupersededBy,
    /// Enqueue exactly one signal to the waiter.
    SignalWaiter(SignalType),
    /// Cancel every open delivery.
    CancelOpenDeliveries,
    /// Revoke every open capability grant (§11.4).
    RevokeOpenGrants,
    /// Emit the state event. Exactly one per state change, in the same transaction (I12).
    EmitEvent(&'static str),
}

/// The guards §6.2 places on an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnswerGuards {
    /// Whether the submitted answer validated against `requires.answer` (§5.3).
    pub answer_valid: bool,
    /// Whether the answerer satisfied the declared authority, evaluated at answer time (§4.3).
    pub authority_satisfied: bool,
    /// Whether quorum is met by this answer (§4.5).
    pub quorum_met: bool,
    /// The result of the state-conditional write (`… WHERE state = 'pending'`).
    ///
    /// `false` means another writer settled the request between the read and the write. Answering
    /// is first-writer-wins, so this answer loses and gets `409 already_answered` (I5, C-3).
    pub conditional_write_won: bool,
    /// What the answerer did.
    pub disposition: Disposition,
    /// Whether this is an intermediate step of a progressive-disclosure ladder (§5.5).
    pub partial: bool,
    /// Whether this retry carries the same `Idempotency-Key` as the answer that already landed. A
    /// retried click is not a conflict (§6.7 rule 3).
    pub same_idempotency_key_as_landed_answer: bool,
}

impl Default for AnswerGuards {
    fn default() -> Self {
        Self {
            answer_valid: true,
            authority_satisfied: true,
            quorum_met: true,
            conditional_write_won: true,
            disposition: Disposition::Decide,
            partial: false,
            same_idempotency_key_as_landed_answer: false,
        }
    }
}

/// What happens to a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestEvent {
    /// A caller raised it (R1).
    Raise {
        /// Whether the caller holds the write scope.
        caller_holds_write_scope: bool,
        /// Whether the tenant is entitled to the product.
        tenant_is_entitled: bool,
    },
    /// The requester amended it (R2).
    Amend {
        /// Whether any delivery has reached `acted`. Once a person has begun answering, an
        /// amendment must be refused and the caller must supersede instead (§6.5).
        any_delivery_acted: bool,
        /// Whether the caller is the original requester or shares the `waiter_ref`.
        caller_owns_request: bool,
    },
    /// The attempt clock lapsed (R3).
    AttemptLapse {
        /// Whether `attempt_expires_at <= now`.
        deadline_passed: bool,
        /// Whether the lapse was already notified. It is stamped **once, ever**.
        already_notified: bool,
        /// Whether the waiter is `armed`.
        waiter_armed: bool,
    },
    /// A ladder rung fired (R4).
    Escalate {
        /// Whether a further rung exists.
        further_rung_exists: bool,
    },
    /// A person answered (R5, R9, R10).
    Answer(AnswerGuards),
    /// The TTL sweep ran (R6).
    TtlSweep {
        /// Whether `expires_at <= now`.
        deadline_passed: bool,
        /// Whether a further ladder rung exists.
        further_rung_exists: bool,
        /// The declared policy.
        policy: OnExpiry,
    },
    /// The requester cancelled it, or the waiter died under `on_waiter_terminal: "cancel"` (R7).
    Cancel {
        /// Whether the caller is the requester principal or shares the `waiter_ref`.
        caller_owns_request: bool,
        /// Whether this cancel comes from a waiter that went terminal (§8.4).
        from_waiter_terminal: bool,
    },
    /// A successor request replaced it (R8).
    Supersede {
        /// Whether the successor exists.
        successor_exists: bool,
        /// Whether the successor is in the same tenant. Cross-tenant supersession is not a thing
        /// (§3.2).
        successor_same_tenant: bool,
        /// Whether the successor is itself `pending`.
        successor_pending: bool,
    },
}

/// One accepted move of the request machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestTransition {
    /// Which row of §6.2 was taken.
    pub rule: TransitionRule,
    /// Where it came from. `None` is the machine's start.
    pub from: Option<RequestState>,
    /// Where it went.
    pub to: RequestState,
    /// What is committed atomically with the state change.
    pub effects: Vec<RequestEffect>,
}

impl RequestTransition {
    /// Whether this transition actually changed the state.
    pub fn changes_state(&self) -> bool {
        self.from != Some(self.to)
    }
}

/// Move the request machine. A total function: every `(state, event)` pair either yields a
/// transition or a typed error, and nothing panics.
///
/// `from` is `None` for a request that does not exist yet.
pub fn transition(from: Option<RequestState>, event: RequestEvent) -> Result<RequestTransition> {
    use RequestEffect as E;

    let accept = |rule, to, effects| {
        Ok(RequestTransition {
            rule,
            from,
            to,
            effects,
        })
    };

    match (from, event) {
        // ------------------------------------------------------------------ R1: ∅ → pending
        (
            None,
            RequestEvent::Raise {
                caller_holds_write_scope,
                tenant_is_entitled,
            },
        ) => {
            if !caller_holds_write_scope {
                return Err(ProtocolError::new(
                    ErrorCode::InsufficientScope,
                    "raising a request requires the write scope",
                ));
            }
            if !tenant_is_entitled {
                return Err(ProtocolError::new(
                    ErrorCode::ProductNotEntitled,
                    "this tenant is not entitled to raise requests",
                ));
            }
            accept(
                TransitionRule::R1,
                RequestState::Pending,
                vec![
                    E::InsertRequest,
                    E::RegisterWaiterArmed,
                    E::SnapshotRoutingPolicy,
                    E::EnqueueRungZeroDeliveries,
                    E::MintCapabilityGrants,
                    E::EmitEvent("request.raised"),
                ],
            )
        }
        (None, _) => Err(ProtocolError::new(
            ErrorCode::RequestNotFound,
            "no such request; only a raise can create one",
        )),
        (Some(_), RequestEvent::Raise { .. }) => Err(ProtocolError::new(
            ErrorCode::InvalidRequest,
            "this request already exists; a repeat raise is resolved by `resolve_raise`",
        )),

        // ------------------------------------------------------------------ from a terminal state
        //
        // R9, R10, and R11 all live here. The person's answer wins every race (§6.5, I5).
        (Some(RequestState::Answered), RequestEvent::Answer(guards)) => {
            if guards.same_idempotency_key_as_landed_answer {
                // R9: a retried click is not a conflict. No state change; `200` with the original
                // receipt.
                accept(TransitionRule::R9, RequestState::Answered, Vec::new())
            } else {
                // R10: a conflicting second answer changes nothing.
                Err(ProtocolError::new(
                    ErrorCode::AlreadyAnswered,
                    "this request was already answered; the existing receipt stands",
                ))
            }
        }
        // Spelled out rather than guarded by `is_terminal()`, so that the compiler checks this
        // match for exhaustiveness. A guard arm would let a new state or event slip through
        // silently, which is exactly the class of bug a total function is supposed to rule out.
        (
            Some(
                state @ (RequestState::Answered
                | RequestState::Expired
                | RequestState::Cancelled
                | RequestState::Superseded),
            ),
            RequestEvent::Amend { .. }
            | RequestEvent::AttemptLapse { .. }
            | RequestEvent::Escalate { .. }
            | RequestEvent::Answer(_)
            | RequestEvent::TtlSweep { .. }
            | RequestEvent::Cancel { .. }
            | RequestEvent::Supersede { .. },
        ) => {
            // R11 for a cancel or an expiry racing a landed answer, and §6.7 rule 2 for everything
            // else: a settled request answers with a *specific* code so the caller can recover
            // without a second round trip.
            let code = state
                .settled_error_code()
                .unwrap_or(ErrorCode::InvalidRequest);
            Err(ProtocolError::new(
                code,
                format!("this request is already `{}`", state_name(state)),
            ))
        }

        // ------------------------------------------------------------------ R2: amend
        (
            Some(RequestState::Pending),
            RequestEvent::Amend {
                any_delivery_acted,
                caller_owns_request,
            },
        ) => {
            if !caller_owns_request {
                return Err(ProtocolError::new(
                    ErrorCode::RequestNotFound,
                    "only the requester or a caller sharing the waiter_ref may amend",
                ));
            }
            if any_delivery_acted {
                return Err(ProtocolError::new(
                    ErrorCode::RequestInProgress,
                    "someone has begun answering; supersede instead of amending",
                ));
            }
            accept(
                TransitionRule::R2,
                RequestState::Pending,
                vec![
                    E::MergeDeclarationsForward,
                    E::IncrementVersion,
                    E::ReRenderOpenDeliveries,
                    E::EmitEvent("request.amended"),
                ],
            )
        }

        // ------------------------------------------------------------------ R3: attempt lapse
        (
            Some(RequestState::Pending),
            RequestEvent::AttemptLapse {
                deadline_passed,
                already_notified,
                waiter_armed,
            },
        ) => {
            if !deadline_passed {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidRequest,
                    "the attempt has not lapsed yet",
                ));
            }
            if already_notified {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidRequest,
                    "this attempt lapse was already stamped; it fires once, ever",
                ));
            }
            if !waiter_armed {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidRequest,
                    "the waiter is not armed, so there is nobody to nudge",
                ));
            }
            accept(
                TransitionRule::R3,
                RequestState::Pending,
                vec![
                    E::StampAttemptLapseOnce,
                    E::SignalWaiter(SignalType::AttemptLapsed),
                    E::SetUrgencyState(UrgencyState::Waiting),
                    // I4: the request returns to every inbox. A lapsed attempt changes urgency,
                    // never visibility — the specific failure this split exists to prevent is a
                    // data layer that stays resumable behind a surface that shows nothing to click.
                    E::RelistInEveryInbox,
                    E::EmitEvent("attempt.lapsed"),
                ],
            )
        }

        // ------------------------------------------------------------------ R4: escalate
        (
            Some(RequestState::Pending),
            RequestEvent::Escalate {
                further_rung_exists,
            },
        ) => {
            if !further_rung_exists {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidRequest,
                    "the escalation ladder has no further rung",
                ));
            }
            accept(
                TransitionRule::R4,
                RequestState::Pending,
                vec![
                    E::MintRungDeliveries,
                    E::ExtendExpiry,
                    E::EmitEvent("request.escalated"),
                ],
            )
        }

        // ------------------------------------------------------------------ R5 and its neighbours
        (Some(RequestState::Pending), RequestEvent::Answer(guards)) => {
            if !guards.answer_valid {
                return Err(ProtocolError::new(
                    ErrorCode::AnswerValidationFailed,
                    "the answer does not match the declared fields",
                ));
            }
            if !guards.authority_satisfied {
                return Err(ProtocolError::new(
                    ErrorCode::InsufficientAuthority,
                    "the answerer does not satisfy the authority this request declares",
                ));
            }
            if guards.partial {
                // §5.5: one request, amended in place. The waiter is deliberately not signalled —
                // the runtime must not learn that an intermediate step occurred.
                return accept(
                    TransitionRule::ProgressiveStep,
                    RequestState::Pending,
                    vec![
                        E::AmendFieldSetToNextStep,
                        E::IncrementVersion,
                        E::RearmAttemptClockFresh,
                        E::AppendReceiptStep,
                        E::EmitEvent("request.amended"),
                    ],
                );
            }
            if guards.disposition != Disposition::Decide {
                // §6.6: a delegation is not a decision, and "unable to do it" is a disposition,
                // not a new state.
                return accept(
                    TransitionRule::NonDecidingDisposition,
                    RequestState::Pending,
                    vec![
                        E::RecordDisposition(guards.disposition),
                        E::EmitEvent("request.disposition_recorded"),
                    ],
                );
            }
            if !guards.quorum_met {
                // §4.5: partial answers are endorsements and the request stays `pending`.
                return accept(
                    TransitionRule::QuorumEndorsement,
                    RequestState::Pending,
                    vec![E::RecordEndorsement, E::EmitEvent("request.endorsed")],
                );
            }
            if !guards.conditional_write_won {
                // I5: first-writer-wins. Another answer landed between the read and the write.
                return Err(ProtocolError::new(
                    ErrorCode::AlreadyAnswered,
                    "another answer settled this request first; the existing receipt stands",
                ));
            }
            accept(
                TransitionRule::R5,
                RequestState::Answered,
                vec![
                    E::MintReceipt,
                    E::MintAuthorization,
                    E::SignalWaiter(SignalType::Answered),
                    E::CancelOpenDeliveries,
                    E::RevokeOpenGrants,
                    E::EmitEvent("request.answered"),
                ],
            )
        }

        // ------------------------------------------------------------------ R6: TTL sweep
        (
            Some(RequestState::Pending),
            RequestEvent::TtlSweep {
                deadline_passed,
                further_rung_exists,
                policy,
            },
        ) => {
            if !deadline_passed {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidRequest,
                    "the request has not reached its expiry",
                ));
            }
            match policy {
                OnExpiry::Park => Err(ProtocolError::new(
                    ErrorCode::InvalidRequest,
                    "a parked request never expires; it waits until someone answers",
                )),
                OnExpiry::Escalate if further_rung_exists => accept(
                    TransitionRule::R4,
                    RequestState::Pending,
                    vec![
                        E::MintRungDeliveries,
                        E::ExtendExpiry,
                        E::EmitEvent("request.escalated"),
                    ],
                ),
                // §6.4: `escalate` is terminal only once rungs are exhausted, and it then falls
                // through to the deployment's terminal policy.
                OnExpiry::Escalate | OnExpiry::ExpireAndDeny | OnExpiry::Default => accept(
                    TransitionRule::R6,
                    RequestState::Expired,
                    vec![
                        E::MintPolicyReceipt,
                        E::SignalWaiter(SignalType::Expired),
                        E::CancelOpenDeliveries,
                        E::RevokeOpenGrants,
                        E::EmitEvent("request.expired"),
                    ],
                ),
            }
        }

        // ------------------------------------------------------------------ R7: cancel
        (
            Some(RequestState::Pending),
            RequestEvent::Cancel {
                caller_owns_request,
                from_waiter_terminal,
            },
        ) => {
            if !caller_owns_request && !from_waiter_terminal {
                return Err(ProtocolError::new(
                    ErrorCode::RequestNotFound,
                    "only the requester or a caller sharing the waiter_ref may cancel",
                ));
            }
            accept(
                TransitionRule::R7,
                RequestState::Cancelled,
                vec![
                    E::SignalWaiter(SignalType::Cancelled),
                    E::CancelOpenDeliveries,
                    E::RevokeOpenGrants,
                    E::EmitEvent("request.cancelled"),
                ],
            )
        }

        // ------------------------------------------------------------------ R8: supersede
        (
            Some(RequestState::Pending),
            RequestEvent::Supersede {
                successor_exists,
                successor_same_tenant,
                successor_pending,
            },
        ) => {
            if !successor_exists || !successor_same_tenant {
                // §3.2: existence in another tenant is not disclosed.
                return Err(ProtocolError::new(
                    ErrorCode::RequestNotFound,
                    "the successor request does not exist in this tenant",
                ));
            }
            if !successor_pending {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidRequest,
                    "the successor request must itself be pending",
                ));
            }
            accept(
                TransitionRule::R8,
                RequestState::Superseded,
                vec![
                    E::LinkSupersededBy,
                    E::CancelOpenDeliveries,
                    E::RevokeOpenGrants,
                    E::SignalWaiter(SignalType::Superseded),
                    E::EmitEvent("request.superseded"),
                ],
            )
        }
    }
}

const fn state_name(state: RequestState) -> &'static str {
    match state {
        RequestState::Pending => "pending",
        RequestState::Answered => "answered",
        RequestState::Expired => "expired",
        RequestState::Cancelled => "cancelled",
        RequestState::Superseded => "superseded",
    }
}

/// The two clocks a Server maintains (§6.3), evaluated against an injected instant.
///
/// They are independent on purpose. The attempt clock bounds "a specific person is expected to be
/// doing this right now"; the request clock bounds "this ask is still worth answering". Only the
/// second is terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deadlines {
    /// When the ask stops being worth answering. `None` means it never expires (§6.3).
    pub expires_at: Option<Timestamp>,
    /// When the current attempt lapses. `None` until an attempt is armed.
    pub attempt_expires_at: Option<Timestamp>,
}

impl Deadlines {
    /// Whether the attempt has lapsed at `now`.
    pub fn attempt_lapsed(&self, now: Timestamp) -> bool {
        self.attempt_expires_at
            .is_some_and(|at| at.is_at_or_before(now))
    }

    /// Whether the request has expired at `now`.
    pub fn expired(&self, now: Timestamp) -> bool {
        self.expires_at.is_some_and(|at| at.is_at_or_before(now))
    }

    /// Arm a fresh attempt window from `now`, never inheriting a near-expired countdown (§5.5).
    #[must_use]
    pub fn arm_attempt(self, now: Timestamp, attempt_ttl: IsoDuration) -> Self {
        Self {
            attempt_expires_at: Some(now.saturating_add(attempt_ttl)),
            ..self
        }
    }
}

/// A reference to the successor of a superseded request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SupersededBy(pub RequestId);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::{Clock, ManualClock};
    use serde_json::json;

    fn profile() -> DeploymentProfile {
        DeploymentProfile::default().with_capability_types(["interactive_surface"])
    }

    fn raise_body() -> Value {
        json!({
            "waiter_ref": "run:0198f2a1",
            "urgency": "high",
            "prompt": {"title": "Refund $2,400 to Acme Corp?",
                       "body": "Invoice **INV-8821** was double-charged on 2026-07-28."},
            "requires": {"v": 1, "answer": {"fields": [
                {"name": "decision", "type": "choice", "required": true,
                 "options": [{"id": "approve", "label": "Refund it"},
                             {"id": "reject", "label": "Don't refund"}]}]},
                "capabilities": [],
                "authority": {"min_role": "editor", "auth_strength": "session"}},
            "ttl": "PT4H",
            "ttl_policy": {"on_expiry": "expire_and_deny"},
            "metadata": {"hint": "approval"}
        })
    }

    const EVENTS: &[RequestEvent] = &[
        RequestEvent::Raise {
            caller_holds_write_scope: true,
            tenant_is_entitled: true,
        },
        RequestEvent::Amend {
            any_delivery_acted: false,
            caller_owns_request: true,
        },
        RequestEvent::AttemptLapse {
            deadline_passed: true,
            already_notified: false,
            waiter_armed: true,
        },
        RequestEvent::Escalate {
            further_rung_exists: true,
        },
        RequestEvent::Answer(AnswerGuards {
            answer_valid: true,
            authority_satisfied: true,
            quorum_met: true,
            conditional_write_won: true,
            disposition: Disposition::Decide,
            partial: false,
            same_idempotency_key_as_landed_answer: false,
        }),
        RequestEvent::TtlSweep {
            deadline_passed: true,
            further_rung_exists: false,
            policy: OnExpiry::ExpireAndDeny,
        },
        RequestEvent::Cancel {
            caller_owns_request: true,
            from_waiter_terminal: false,
        },
        RequestEvent::Supersede {
            successor_exists: true,
            successor_same_tenant: true,
            successor_pending: true,
        },
    ];

    // -------------------------------------------------------------- raise parsing

    #[test]
    fn a_raise_parses_and_applies_the_documented_defaults() {
        let raise = RaiseRequest::parse(&raise_body(), &profile()).expect("parse");
        assert_eq!(raise.waiter_ref, "run:0198f2a1");
        assert_eq!(raise.urgency, Urgency::High);
        assert_eq!(raise.liveness, Liveness::Durable);
        assert_eq!(
            raise.on_waiter_terminal,
            OnWaiterTerminal::Keep,
            "durable defaults to keep"
        );
        assert_eq!(raise.attempt_ttl, DEFAULT_ATTEMPT_TTL);
        assert_eq!(raise.mode, Mode::Advisory);
        assert_eq!(raise.presentation_binding, PresentationBinding::Advisory);
        assert!(!raise.test_mode);
        assert_eq!(
            raise.metadata["hint"],
            json!("approval"),
            "carried verbatim, never branched on"
        );
    }

    #[test]
    fn a_leased_waiter_defaults_to_cancelling_rather_than_paging_nobody() {
        let mut body = raise_body();
        body["liveness"] = json!("leased");
        let raise = RaiseRequest::parse(&body, &profile()).expect("parse");
        assert_eq!(raise.on_waiter_terminal, OnWaiterTerminal::Cancel);
    }

    #[test]
    fn an_absent_ttl_means_the_request_never_expires() {
        let mut body = raise_body();
        body.as_object_mut().expect("object").remove("ttl");
        body.as_object_mut().expect("object").remove("ttl_policy");
        let raise = RaiseRequest::parse(&body, &profile()).expect("parse");
        assert!(raise.ttl.is_none());
        assert!(
            raise.ttl_policy.is_none(),
            "with no deadline the policy is irrelevant"
        );
    }

    #[test]
    fn a_ttl_with_no_declared_policy_defaults_to_escalating() {
        let mut body = raise_body();
        body.as_object_mut().expect("object").remove("ttl_policy");
        let raise = RaiseRequest::parse(&body, &profile()).expect("parse");
        assert_eq!(
            raise.ttl_policy.expect("defaulted").on_expiry,
            OnExpiry::Escalate
        );
    }

    #[test]
    fn default_expiry_without_a_declared_default_answer_is_refused() {
        let mut body = raise_body();
        body["ttl_policy"] = json!({"on_expiry": "default"});
        let err = RaiseRequest::parse(&body, &profile()).expect_err("rejected");
        assert_eq!(err.code, ErrorCode::InvalidRequest);
        body["ttl_policy"] =
            json!({"on_expiry": "default", "default_answer": {"decision": "reject"}});
        RaiseRequest::parse(&body, &profile()).expect("a pre-agreed default is fine");
    }

    #[test]
    fn a_raise_fails_closed_on_an_unknown_key_or_a_bad_envelope() {
        let mut body = raise_body();
        body["kind"] = json!("approval");
        assert_eq!(
            RaiseRequest::parse(&body, &profile())
                .expect_err("no kind field exists")
                .code,
            ErrorCode::InvalidRequest
        );

        let mut body = raise_body();
        body["requires"]["v"] = json!(2);
        assert_eq!(
            RaiseRequest::parse(&body, &profile())
                .expect_err("rejected")
                .code,
            ErrorCode::UnsupportedRequiresVersion
        );
    }

    #[test]
    fn a_derived_dedupe_key_is_stable_and_content_addressed() {
        let raise = RaiseRequest::parse(&raise_body(), &profile()).expect("parse");
        let key = raise.effective_dedupe_key().expect("derive");
        assert!(key.starts_with("sha256:"));
        assert_eq!(key, raise.effective_dedupe_key().expect("derive again"));

        // A different title is a different ask, so it must not collapse onto the first.
        let mut other = raise_body();
        other["prompt"]["title"] = json!("Refund $24,000 to Acme Corp?");
        let other = RaiseRequest::parse(&other, &profile()).expect("parse");
        assert_ne!(key, other.effective_dedupe_key().expect("derive"));

        // A supplied key wins over the derivation.
        let mut supplied = raise_body();
        supplied["dedupe_key"] = json!("refund:inv-8821");
        let supplied = RaiseRequest::parse(&supplied, &profile()).expect("parse");
        assert_eq!(
            supplied.effective_dedupe_key().expect("derive"),
            "refund:inv-8821"
        );
    }

    // -------------------------------------------------------------- §14 continuation

    #[test]
    fn continuation_fields_are_carried_verbatim_and_never_interpreted() {
        let payload = b"\x00\x01\x02opaque runtime state\xff";
        let mut body = raise_body();
        body["resume_ref"] = json!("s3://runtime-checkpoints/0198f2a1");
        body["resume_payload"] = json!(base64_encode(payload));

        let raise = RaiseRequest::parse(&body, &profile()).expect("parse");
        assert_eq!(
            raise.continuation.resume_ref.as_deref(),
            Some("s3://runtime-checkpoints/0198f2a1")
        );
        assert_eq!(
            raise.continuation.resume_payload.as_deref(),
            Some(&payload[..])
        );
        // Byte-identical on the way back out.
        assert_eq!(
            base64_encode(
                raise
                    .continuation
                    .resume_payload
                    .as_deref()
                    .expect("present")
            ),
            body["resume_payload"].as_str().expect("string")
        );
        // §14 and I18: continuation state appears nowhere in the request's own representation.
        let serialized = serde_json::to_string(&raise).expect("serialize");
        assert!(!serialized.contains("resume_payload"));
        assert!(!serialized.contains("s3://runtime-checkpoints"));
    }

    #[test]
    fn an_oversized_or_non_canonical_resume_payload_is_refused() {
        let mut body = raise_body();
        body["resume_payload"] = json!(base64_encode(&vec![0u8; MAX_RESUME_PAYLOAD_BYTES + 1]));
        assert!(RaiseRequest::parse(&body, &profile()).is_err());

        for bad in ["not base64!", "QQ", "QQ=", "QR==", "====", "A==="] {
            body["resume_payload"] = json!(bad);
            assert!(
                RaiseRequest::parse(&body, &profile()).is_err(),
                "`{bad}` must be refused"
            );
        }
    }

    #[test]
    fn base64_round_trips_every_length_remainder() {
        for len in 0..8usize {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
            let encoded = base64_encode(&bytes);
            assert_eq!(base64_decode(&encoded).expect("decode"), bytes, "{encoded}");
        }
    }

    // -------------------------------------------------------------- §3.3 identity

    #[test]
    fn the_three_keys_stay_distinct() {
        // Rule 1: same key, same body → the same request, `200`, no re-ask (C-1).
        let same = resolve_raise(
            Some(IdempotencyMatch {
                same_body_digest: true,
            }),
            false,
        )
        .expect("idempotent");
        assert_eq!(
            same,
            RaiseOutcome::Existing {
                reason: SameRequestReason::IdempotencyKey,
                merge_forward: false
            }
        );
        assert_eq!(same.http_status(), 200);

        // Rule 2: same key, different body → `409`, and the stored request is untouched.
        assert_eq!(
            resolve_raise(
                Some(IdempotencyMatch {
                    same_body_digest: false
                }),
                false
            )
            .expect_err("reused")
            .code,
            ErrorCode::IdempotencyKeyReused
        );

        // Rule 3: no key, a pending request with the same dedupe key → collapse and merge forward
        // (C-2).
        assert_eq!(
            resolve_raise(None, true).expect("dedupe"),
            RaiseOutcome::Existing {
                reason: SameRequestReason::DedupeKey,
                merge_forward: true
            }
        );

        // Rule 4: otherwise a new request, `201`.
        assert_eq!(resolve_raise(None, false).expect("new"), RaiseOutcome::New);
        assert_eq!(RaiseOutcome::New.http_status(), 201);

        // An idempotency key wins over a live dedupe key: the rules are ordered, not merged.
        assert_eq!(
            resolve_raise(
                Some(IdempotencyMatch {
                    same_body_digest: true
                }),
                true
            )
            .expect("ordered"),
            RaiseOutcome::Existing {
                reason: SameRequestReason::IdempotencyKey,
                merge_forward: false
            }
        );
    }

    // -------------------------------------------------------------- the machine

    #[test]
    fn the_machine_is_total_and_never_panics() {
        for &from in RequestState::ALL {
            for &event in EVENTS {
                let _ = transition(Some(from), event);
            }
        }
        for &event in EVENTS {
            let _ = transition(None, event);
        }
    }

    #[test]
    fn every_state_changing_transition_emits_exactly_one_event() {
        // I12: every state transition emits its event in the same transaction as the state change.
        let mut checked = 0;
        for from in [None]
            .into_iter()
            .chain(RequestState::ALL.iter().copied().map(Some))
        {
            for &event in EVENTS {
                let Ok(t) = transition(from, event) else {
                    continue;
                };
                let emitted = t
                    .effects
                    .iter()
                    .filter(|e| matches!(e, RequestEffect::EmitEvent(_)))
                    .count();
                if t.changes_state() {
                    assert_eq!(emitted, 1, "{:?} → {:?} via {:?}", t.from, t.to, t.rule);
                    checked += 1;
                } else {
                    assert!(emitted <= 1, "{:?} emitted {emitted} events", t.rule);
                }
            }
        }
        assert!(
            checked >= 4,
            "the state-changing rows must actually be exercised"
        );
    }

    #[test]
    fn every_terminal_transition_signals_the_waiter() {
        // I11: a request never goes quiet.
        for &event in EVENTS {
            let Ok(t) = transition(Some(RequestState::Pending), event) else {
                continue;
            };
            if !t.to.is_terminal() {
                continue;
            }
            let expected =
                t.to.terminal_signal()
                    .expect("terminal states have a signal");
            assert!(
                t.effects.contains(&RequestEffect::SignalWaiter(expected)),
                "{:?} must signal `{expected:?}`",
                t.rule
            );
        }
    }

    #[test]
    fn a_terminal_request_never_transitions_again() {
        for &from in RequestState::ALL.iter().filter(|s| s.is_terminal()) {
            for &event in EVENTS {
                let result = transition(Some(from), event);
                match result {
                    // R9 is the single exception, and it changes nothing.
                    Ok(t) => {
                        assert_eq!(t.rule, TransitionRule::R9);
                        assert!(!t.changes_state());
                        assert!(t.effects.is_empty());
                    }
                    // A repeat raise is a caller mistake — identity is `resolve_raise`'s job, not
                    // the machine's — so it is a 400. Every other refusal is a conflict with a
                    // request that has already settled, and carries the record that settled it.
                    Err(e) if matches!(event, RequestEvent::Raise { .. }) => {
                        assert_eq!(e.code, ErrorCode::InvalidRequest, "{from:?} + {event:?}")
                    }
                    Err(e) => assert_eq!(e.http_status(), 409, "{from:?} + {event:?}: {e}"),
                }
            }
        }
    }

    #[test]
    fn a_settled_request_names_the_reason_it_settled() {
        // §6.7 rule 2: a *specific* code, so a client can recover without a second round trip.
        for (state, expected) in [
            (RequestState::Answered, ErrorCode::AlreadyAnswered),
            (RequestState::Expired, ErrorCode::RequestExpired),
            (RequestState::Cancelled, ErrorCode::RequestCancelled),
            (RequestState::Superseded, ErrorCode::RequestSuperseded),
        ] {
            let err = transition(
                Some(state),
                RequestEvent::Cancel {
                    caller_owns_request: true,
                    from_waiter_terminal: false,
                },
            )
            .expect_err("terminal");
            assert_eq!(err.code, expected, "{state:?}");
        }
    }

    #[test]
    fn the_persons_answer_beats_a_racing_cancel_or_expiry() {
        // R11, and it is a product rule: a machine changing its mind a millisecond after a person
        // acted must not discard that person's work.
        for racing in [
            RequestEvent::Cancel {
                caller_owns_request: true,
                from_waiter_terminal: false,
            },
            RequestEvent::TtlSweep {
                deadline_passed: true,
                further_rung_exists: false,
                policy: OnExpiry::ExpireAndDeny,
            },
        ] {
            let err =
                transition(Some(RequestState::Answered), racing).expect_err("the answer wins");
            assert_eq!(err.code, ErrorCode::AlreadyAnswered);
        }
    }

    #[test]
    fn answering_is_first_writer_wins() {
        // C-3: two concurrent answers, one receipt.
        let winner = transition(
            Some(RequestState::Pending),
            RequestEvent::Answer(AnswerGuards::default()),
        )
        .expect("the first writer settles it");
        assert_eq!(winner.rule, TransitionRule::R5);
        assert_eq!(winner.to, RequestState::Answered);
        assert!(winner.effects.contains(&RequestEffect::MintReceipt));
        assert!(winner.effects.contains(&RequestEffect::MintAuthorization));

        let loser = transition(
            Some(RequestState::Pending),
            RequestEvent::Answer(AnswerGuards {
                conditional_write_won: false,
                ..Default::default()
            }),
        )
        .expect_err("the second writer loses");
        assert_eq!(loser.code, ErrorCode::AlreadyAnswered);
    }

    #[test]
    fn a_retried_click_is_not_a_conflict_but_a_different_answer_is() {
        // §6.7 rules 3 and 4: no last-write-wins, and no `2xx` carrying a failure flag.
        let retry = transition(
            Some(RequestState::Answered),
            RequestEvent::Answer(AnswerGuards {
                same_idempotency_key_as_landed_answer: true,
                ..Default::default()
            }),
        )
        .expect("R9");
        assert_eq!(retry.rule, TransitionRule::R9);
        assert!(!retry.changes_state());

        let conflict = transition(
            Some(RequestState::Answered),
            RequestEvent::Answer(AnswerGuards::default()),
        )
        .expect_err("R10");
        assert_eq!(conflict.code, ErrorCode::AlreadyAnswered);
        assert_eq!(conflict.http_status(), 409);
    }

    #[test]
    fn an_answer_is_validated_and_authorized_before_anything_is_written() {
        for (guards, expected) in [
            (
                AnswerGuards {
                    answer_valid: false,
                    ..Default::default()
                },
                ErrorCode::AnswerValidationFailed,
            ),
            (
                AnswerGuards {
                    authority_satisfied: false,
                    ..Default::default()
                },
                ErrorCode::InsufficientAuthority,
            ),
        ] {
            let err = transition(Some(RequestState::Pending), RequestEvent::Answer(guards))
                .expect_err("refused");
            assert_eq!(err.code, expected);
        }
    }

    #[test]
    fn a_progressive_step_amends_in_place_and_never_signals_the_waiter() {
        // §5.5: password-then-OTP is one request, and the agent never learns there was a step.
        let t = transition(
            Some(RequestState::Pending),
            RequestEvent::Answer(AnswerGuards {
                partial: true,
                ..Default::default()
            }),
        )
        .expect("progressive step");
        assert_eq!(t.to, RequestState::Pending);
        assert!(t.effects.contains(&RequestEffect::RearmAttemptClockFresh));
        assert!(t.effects.contains(&RequestEffect::AppendReceiptStep));
        assert!(!t
            .effects
            .iter()
            .any(|e| matches!(e, RequestEffect::SignalWaiter(_))));
        assert!(!t.effects.contains(&RequestEffect::MintReceipt));
    }

    #[test]
    fn a_delegation_is_not_a_decision() {
        for disposition in [Disposition::Delegate, Disposition::Unable] {
            let t = transition(
                Some(RequestState::Pending),
                RequestEvent::Answer(AnswerGuards {
                    disposition,
                    ..Default::default()
                }),
            )
            .expect("recorded");
            assert_eq!(
                t.to,
                RequestState::Pending,
                "{disposition:?} must not settle the request"
            );
            assert!(t
                .effects
                .contains(&RequestEffect::RecordDisposition(disposition)));
            assert!(!t.effects.contains(&RequestEffect::MintReceipt));
        }
    }

    #[test]
    fn an_unmet_quorum_records_an_endorsement_and_stays_pending() {
        let t = transition(
            Some(RequestState::Pending),
            RequestEvent::Answer(AnswerGuards {
                quorum_met: false,
                ..Default::default()
            }),
        )
        .expect("endorsement");
        assert_eq!(t.to, RequestState::Pending);
        assert!(t.effects.contains(&RequestEffect::RecordEndorsement));
    }

    #[test]
    fn an_attempt_lapse_changes_urgency_and_never_visibility() {
        // C-9, I4. This is the failure the two-clock split exists to prevent.
        let t = transition(
            Some(RequestState::Pending),
            RequestEvent::AttemptLapse {
                deadline_passed: true,
                already_notified: false,
                waiter_armed: true,
            },
        )
        .expect("lapse");
        assert_eq!(t.to, RequestState::Pending, "the request stays answerable");
        assert!(t
            .effects
            .contains(&RequestEffect::SetUrgencyState(UrgencyState::Waiting)));
        assert!(t.effects.contains(&RequestEffect::RelistInEveryInbox));
        assert!(t
            .effects
            .contains(&RequestEffect::SignalWaiter(SignalType::AttemptLapsed)));
    }

    #[test]
    fn an_attempt_lapse_fires_once_ever() {
        assert!(transition(
            Some(RequestState::Pending),
            RequestEvent::AttemptLapse {
                deadline_passed: true,
                already_notified: true,
                waiter_armed: true,
            },
        )
        .is_err());
    }

    #[test]
    fn amending_after_someone_has_begun_answering_is_refused() {
        // §6.5: if a change touches any field the answer is about, it is a supersession.
        let err = transition(
            Some(RequestState::Pending),
            RequestEvent::Amend {
                any_delivery_acted: true,
                caller_owns_request: true,
            },
        )
        .expect_err("refused");
        assert_eq!(err.code, ErrorCode::RequestInProgress);
    }

    #[test]
    fn escalation_mints_deliveries_and_never_a_request() {
        // I3, C-14: three rungs, still one request and one receipt.
        for _ in 0..3 {
            let t = transition(
                Some(RequestState::Pending),
                RequestEvent::Escalate {
                    further_rung_exists: true,
                },
            )
            .expect("rung fires");
            assert_eq!(t.to, RequestState::Pending);
            assert!(t.effects.contains(&RequestEffect::MintRungDeliveries));
            assert!(!t.effects.contains(&RequestEffect::InsertRequest));
            assert!(!t.effects.contains(&RequestEffect::MintReceipt));
        }
    }

    #[test]
    fn expiry_policies_do_what_they_say() {
        let sweep = |policy, further_rung_exists| {
            transition(
                Some(RequestState::Pending),
                RequestEvent::TtlSweep {
                    deadline_passed: true,
                    further_rung_exists,
                    policy,
                },
            )
        };

        // `escalate` is not terminal while rungs remain, and terminal once they are exhausted.
        assert_eq!(
            sweep(OnExpiry::Escalate, true).expect("rung").to,
            RequestState::Pending
        );
        assert_eq!(
            sweep(OnExpiry::Escalate, false).expect("exhausted").to,
            RequestState::Expired
        );

        for policy in [OnExpiry::ExpireAndDeny, OnExpiry::Default] {
            let t = sweep(policy, false).expect("expires");
            assert_eq!(t.to, RequestState::Expired);
            assert!(t.effects.contains(&RequestEffect::MintPolicyReceipt));
            assert!(t
                .effects
                .contains(&RequestEffect::SignalWaiter(SignalType::Expired)));
        }

        // `park` never expires; it waits until someone answers.
        assert!(sweep(OnExpiry::Park, false).is_err());
        // And a sweep that runs early does nothing.
        assert!(transition(
            Some(RequestState::Pending),
            RequestEvent::TtlSweep {
                deadline_passed: false,
                further_rung_exists: false,
                policy: OnExpiry::ExpireAndDeny,
            },
        )
        .is_err());
    }

    #[test]
    fn supersession_requires_a_pending_successor_in_the_same_tenant() {
        let base = |successor_same_tenant, successor_pending| RequestEvent::Supersede {
            successor_exists: true,
            successor_same_tenant,
            successor_pending,
        };
        assert_eq!(
            transition(Some(RequestState::Pending), base(false, true))
                .expect_err("cross-tenant")
                .code,
            // §3.2: existence in another tenant is not disclosed.
            ErrorCode::RequestNotFound
        );
        assert_eq!(
            transition(Some(RequestState::Pending), base(true, false))
                .expect_err("settled")
                .code,
            ErrorCode::InvalidRequest
        );
        let t = transition(Some(RequestState::Pending), base(true, true)).expect("supersede");
        assert!(t.effects.contains(&RequestEffect::LinkSupersededBy));
    }

    #[test]
    fn a_raise_needs_the_write_scope_and_an_entitled_tenant() {
        assert_eq!(
            transition(
                None,
                RequestEvent::Raise {
                    caller_holds_write_scope: false,
                    tenant_is_entitled: true
                }
            )
            .expect_err("no scope")
            .code,
            ErrorCode::InsufficientScope
        );
        assert_eq!(
            transition(
                None,
                RequestEvent::Raise {
                    caller_holds_write_scope: true,
                    tenant_is_entitled: false
                }
            )
            .expect_err("not entitled")
            .code,
            ErrorCode::ProductNotEntitled
        );
    }

    #[test]
    fn nothing_but_a_raise_creates_a_request() {
        for &event in EVENTS
            .iter()
            .filter(|e| !matches!(e, RequestEvent::Raise { .. }))
        {
            assert_eq!(
                transition(None, event).expect_err("no request").code,
                ErrorCode::RequestNotFound
            );
        }
    }

    // -------------------------------------------------------------- the two clocks (§6.3)

    #[test]
    fn the_attempt_clock_and_the_request_clock_are_independent() {
        let clock = ManualClock::new(Timestamp::parse("2026-07-30T14:00:00Z").expect("parse"));
        let deadlines = Deadlines {
            expires_at: Some(clock.now().saturating_add(IsoDuration::from_hours(4))),
            attempt_expires_at: None,
        }
        .arm_attempt(clock.now(), DEFAULT_ATTEMPT_TTL);

        clock.advance(IsoDuration::from_mins(15));
        assert!(deadlines.attempt_lapsed(clock.now()), "the attempt is up");
        assert!(
            !deadlines.expired(clock.now()),
            "the ask is still worth answering"
        );

        // Re-arming takes a fresh window rather than inheriting the remainder (§5.5).
        let rearmed = deadlines.arm_attempt(clock.now(), DEFAULT_ATTEMPT_TTL);
        assert!(!rearmed.attempt_lapsed(clock.now()));

        clock.advance(IsoDuration::from_hours(4));
        assert!(rearmed.expired(clock.now()));
    }

    #[test]
    fn a_request_with_no_ttl_never_expires() {
        let clock = ManualClock::new(Timestamp::parse("2026-07-30T14:00:00Z").expect("parse"));
        let deadlines = Deadlines {
            expires_at: None,
            attempt_expires_at: None,
        };
        clock.advance(IsoDuration::from_secs(60 * 60 * 24 * 365));
        assert!(!deadlines.expired(clock.now()));
        assert!(
            !deadlines.attempt_lapsed(clock.now()),
            "an unarmed attempt has not lapsed"
        );
    }
}

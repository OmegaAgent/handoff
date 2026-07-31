//! RECEIPT (§9): the immutable record of an outcome, its canonical form, and the per-tenant hash
//! chain that makes it tamper-evident.
//!
//! A receipt answers six questions — what was decided, who decided, when, **what they saw**,
//! through what, and under what authority — and it is minted in the same transaction as the state
//! change it records (§9.1). Nothing in this crate can write one asynchronously, because nothing in
//! this crate can write anything.
//!
//! # Canonicalization
//!
//! [`canonical_json`] implements the RFC 8785 (JCS) subset the protocol needs. It is deterministic
//! and byte-stable, because the chain depends on it: two runs over the same logical receipt must
//! produce identical bytes or a verifier will report tampering that never happened.
//!
//! Two deliberate narrowings, both fail-closed:
//!
//! * **Numbers must be integral in value and within ±(2^53 − 1)**, and are rejected rather than
//!   serialized approximately. Floating point is exactly where JSON serializers start disagreeing,
//!   and a receipt nobody can re-canonicalize is a receipt nobody can verify. A number that is
//!   integral but written as a float — `-0.0`, `1.0`, `1e2` — is legal, and [`normalize_numbers`]
//!   puts it into the form this function emits *before* anything digests, stores or serves it
//!   (§1.4 rule 3). Canonicalizing at digest time while storing what arrived is how the two came
//!   apart, and a receipt is verifiable only when those bytes are the same bytes.
//! * **Object keys are sorted by UTF-16 code unit**, as JCS specifies, not by Rust's byte-wise
//!   string order. The two agree for ASCII and diverge above the basic multilingual plane.

use crate::clock::Timestamp;
use crate::delivery::DeliveryGrade;
use crate::error::{ErrorCode, ProtocolError, Result};
use crate::id::{
    DeliveryId, GrantHandle, GrantSessionRef, OrgId, PrincipalId, ReceiptId, RequestId,
};
use crate::request::Disposition;
use crate::requires::{AuthStrength, Authority, CapabilityScope, Target};
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};
use sha2::{Digest as _, Sha256};
use std::fmt;

// ---------------------------------------------------------------------------------------------
// Digests
// ---------------------------------------------------------------------------------------------

/// An algorithm-prefixed hash, `sha256:<lowercase hex>` (§1.4).
///
/// The algorithm prefix is part of the value so the hash can be migrated without ambiguity: a
/// receipt written under one algorithm stays verifiable after the default changes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Digest(String);

impl Digest {
    /// The algorithm this crate mints.
    pub const ALGORITHM: &'static str = "sha256";

    /// Hash `bytes` with SHA-256.
    pub fn sha256(bytes: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let mut out = String::with_capacity(Self::ALGORITHM.len() + 1 + 64);
        out.push_str(Self::ALGORITHM);
        out.push(':');
        for byte in hasher.finalize() {
            out.push_str(&format!("{byte:02x}"));
        }
        Self(out)
    }

    /// Parse `<algorithm>:<hex>`, per the `Digest` pattern in `openapi.yaml`.
    pub fn parse(s: &str) -> Result<Self> {
        let bad = || {
            ProtocolError::new(
                ErrorCode::InvalidRequest,
                format!("`{s}` is not an algorithm-prefixed digest"),
            )
        };
        let (algorithm, hex) = s.split_once(':').ok_or_else(bad)?;
        if algorithm.is_empty()
            || !algorithm
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(bad());
        }
        if !(32..=128).contains(&hex.len())
            || !hex
                .chars()
                .all(|c| c.is_ascii_digit() || matches!(c, 'a'..='f'))
        {
            return Err(bad());
        }
        Ok(Self(s.to_string()))
    }

    /// The algorithm portion.
    pub fn algorithm(&self) -> &str {
        self.0.split_once(':').map_or("", |(a, _)| a)
    }

    /// The whole `algorithm:hex` string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for Digest {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Digest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Digest::parse(&raw).map_err(de::Error::custom)
    }
}

// ---------------------------------------------------------------------------------------------
// Canonical JSON (RFC 8785 subset)
// ---------------------------------------------------------------------------------------------

/// The largest integer an IEEE-754 double distinguishes from its neighbours, `2^53 − 1`.
pub const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// Serialize `value` to its canonical UTF-8 form.
///
/// See the module documentation for the deliberate narrowing: digest-covered content carries
/// **integers only**, bounded to ±(2^53 − 1). Every digest defined by the protocol is taken over
/// the output of this function (§1.4).
pub fn canonical_json(value: &Value) -> Result<Vec<u8>> {
    let mut out = String::new();
    write_canonical(value, &mut out)?;
    Ok(out.into_bytes())
}

/// Hash `value`'s canonical form. This is the shape every `*_digest` field in the protocol takes.
pub fn digest_of(value: &Value) -> Result<Digest> {
    Ok(Digest::sha256(&canonical_json(value)?))
}

fn write_canonical(value: &Value, out: &mut String) -> Result<()> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(n) => write_number(n, out)?,
        Value::String(s) => write_string(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out)?;
            }
            out.push(']');
        }
        Value::Object(map) => {
            // JCS orders members by the UTF-16 code units of their names.
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|a, b| {
                a.encode_utf16()
                    .collect::<Vec<u16>>()
                    .cmp(&b.encode_utf16().collect::<Vec<u16>>())
            });
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(key, out);
                out.push(':');
                write_canonical(&map[key], out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

fn write_number(n: &serde_json::Number, out: &mut String) -> Result<()> {
    // The safe-integer bound applies to every arm, not only the floating-point one. An integer
    // arriving as `i64` used to short-circuit straight to output, so a value beyond 2^53 − 1
    // canonicalized happily here while the answer layer rejected it — the two disagreed, and the
    // one that mattered for a digest was the permissive one.
    const SAFE: i64 = 9_007_199_254_740_991;
    if let Some(i) = n.as_i64() {
        if i.abs() > SAFE {
            return Err(unsafe_integer(&i.to_string()));
        }
        out.push_str(&i.to_string());
        return Ok(());
    }
    if let Some(u) = n.as_u64() {
        if u > SAFE as u64 {
            return Err(unsafe_integer(&u.to_string()));
        }
        out.push_str(&u.to_string());
        return Ok(());
    }
    let f = n.as_f64().ok_or_else(|| {
        ProtocolError::new(
            ErrorCode::InvalidRequest,
            "a number that is not representable as f64",
        )
    })?;
    if !f.is_finite() {
        return Err(ProtocolError::new(
            ErrorCode::InvalidRequest,
            "non-finite numbers have no canonical JSON form",
        ));
    }
    if f == 0.0 {
        // Covers negative zero, which ECMAScript renders as `0` and Rust as `-0`. This is the
        // *definition* of `-0.0`'s canonical form, not a repair of it — which is why §1.4 rule 3
        // requires the stored form to be this rather than the form that arrived.
        out.push('0');
        return Ok(());
    }
    // Digest-covered content carries **integers only**, bounded to ±(2^53 − 1).
    //
    // An earlier profile admitted any number in `1e-6 ≤ |x| < 1e21`, on the reasoning that
    // `Number::toString` produces plain decimal there. That reasoning holds for the *notation* and
    // still leaves the *value* unsafe: RFC 8785 inherits ECMAScript number formatting, which is
    // precisely the thing independent implementations do not reproduce reliably, and asking three
    // languages to agree on float formatting is how a fourth ends up disagreeing. Both published
    // SDKs refuse non-integers outright, and a receipt only one implementation can canonicalize is
    // a receipt nobody can verify — which is the whole claim the chain exists to make.
    //
    // Integers are exact in IEEE-754 up to 2^53 − 1 and render identically everywhere, so the two
    // old bounds collapse into one rule: no fractional part, and inside the safe-integer range. A
    // Client carrying an exact decimal quantity — money, most obviously — sends it as `text`,
    // which sidesteps binary floating point rather than negotiating with it.
    if f.fract() != 0.0 {
        return Err(ProtocolError::new(
            ErrorCode::InvalidRequest,
            format!(
                "{f} is not an integer, and digest-covered content carries integers only: RFC 8785 \
                 inherits a number serialization independent implementations do not reproduce. \
                 Carry an exact decimal quantity as `text`."
            ),
        ));
    }
    if f.abs() > MAX_SAFE_INTEGER {
        return Err(unsafe_integer(&format!("{f}")));
    }
    // Written as a plain integer: no exponent, no fractional part, no locale, nothing to disagree
    // about.
    out.push_str(&format!("{}", f as i64));
    Ok(())
}

/// Rewrite every number to the exact form [`canonical_json`] emits for it (§1.4 rule 3).
///
/// Canonicalizing is not enough on its own, and `-0.0` is the proof. The canonicalizer renders it
/// `0`, correctly; a Server that stored the number *as it arrived* then held one byte sequence and
/// digested another, and a receipt is verifiable only when the bytes an auditor canonicalizes are
/// the bytes that were sealed. One published SDK verified that receipt and the other refused it,
/// which is the disagreement the chain exists to make impossible.
///
/// So the normal form is imposed on the record rather than filtered at the door. `-0.0`, `1.0` and
/// `1e2` are integral in value and therefore legal; they are stored and served as `0`, `1` and
/// `100`. A number this leaves alone is one [`canonical_json`] will refuse — a fractional part or a
/// magnitude past ±(2^53 − 1) — and refusing it is validation's job, not this function's. Applying
/// this to a value costs nothing when there is nothing to change and makes the property checkable
/// from outside: canonicalize what a Server served and compare it with what it served.
pub fn normalize_numbers(value: &mut Value) {
    match value {
        Value::Number(number) => {
            if let Some(normalized) = normalized_number(number) {
                *number = normalized;
            }
        }
        Value::Array(items) => items.iter_mut().for_each(normalize_numbers),
        Value::Object(members) => members.values_mut().for_each(normalize_numbers),
        _ => {}
    }
}

/// The integer form of a float that denotes one, or `None` when there is nothing to normalize.
fn normalized_number(number: &serde_json::Number) -> Option<serde_json::Number> {
    if !number.is_f64() {
        return None;
    }
    let f = number.as_f64()?;
    if !f.is_finite() || f.fract() != 0.0 || f.abs() > MAX_SAFE_INTEGER {
        return None;
    }
    // `f as i64` renders `-0.0` as `0`, which is exactly what the canonicalizer emits for it.
    Some(serde_json::Number::from(f as i64))
}

/// The one message every arm of [`write_number`] uses for an out-of-range integer.
fn unsafe_integer(rendered: &str) -> ProtocolError {
    ProtocolError::new(
        ErrorCode::InvalidRequest,
        format!(
            "{rendered} is outside ±(2^53 − 1), beyond which a value cannot be distinguished from \
             its neighbours, so a receipt would record a figure nobody entered"
        ),
    )
}

fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{9}' => out.push_str("\\t"),
            '\u{a}' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\u{d}' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

// ---------------------------------------------------------------------------------------------
// Receipt content
// ---------------------------------------------------------------------------------------------

/// Which of the three kinds of record this receipt is (§9.5, §9.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptKind {
    /// A person decided.
    Decision,
    /// An expiry policy decided, and the receipt says so plainly.
    Policy,
    /// This receipt amends the one named in `corrects`; the original stays exactly as it was.
    Correction,
}

/// Who decided (§9.2).
///
/// `policy` and `runtime` are first-class actor types precisely so a machine outcome can never be
/// mistaken for consent. A Server MUST NOT record `user` unless it authenticated a person.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorType {
    /// An authenticated person.
    User,
    /// An expiry policy (§6.4, §9.6).
    Policy,
    /// A runtime, where a deployment auto-answered from an observation (§9.7).
    Runtime,
    /// Possession of a single-use delivery token, with no person identified (§4.4).
    AnonymousLink,
}

/// The strength actually established, including the honest `none` a policy receipt records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SatisfiedStrength {
    /// Nobody authenticated. This is what an expiry receipt records (§9.6).
    None,
    /// A single-use delivery token only.
    LinkOnly,
    /// An authenticated principal.
    Session,
    /// A freshly re-entered primary credential.
    Reauth,
    /// A second factor.
    Mfa,
}

impl From<AuthStrength> for SatisfiedStrength {
    fn from(value: AuthStrength) -> Self {
        match value {
            AuthStrength::LinkOnly => Self::LinkOnly,
            AuthStrength::Session => Self::Session,
            AuthStrength::Reauth => Self::Reauth,
            AuthStrength::Mfa => Self::Mfa,
        }
    }
}

/// What was decided. `secret` fields appear only as `{"provided": true}` (§9.2, I7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptDecision {
    /// The typed answer, keyed by declared field name.
    #[serde(default)]
    pub values: Map<String, Value>,
    /// Whether the answerer decided, delegated, or reported being unable.
    pub disposition: Disposition,
    /// The person's own words, verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Who decided, with the attestation to back it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptActor {
    /// The kind of actor. Never `user` unless a person was authenticated.
    #[serde(rename = "type")]
    pub actor_type: ActorType,
    /// The principal, where there is one.
    ///
    /// Always serialized, `null` when the actor is not a person. §4.4 and §9.7 both turn on the
    /// difference between "nobody was identified" and "not recorded", and an absent key cannot
    /// express the first. It is also part of the hashed receipt core, so it must be present for a
    /// third party to reproduce the digest.
    #[serde(default)]
    pub principal_id: Option<PrincipalId>,
    /// Display name frozen at decision time, so a later rename does not rewrite history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
    /// The role that justified the decision, frozen at the moment it was made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_at_decision: Option<String>,
    /// The grade the answerer authenticated at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_strength: Option<AuthStrength>,
    /// When a primary credential was last re-entered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reauth_at: Option<Timestamp>,
    /// When a second factor was last presented.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mfa_at: Option<Timestamp>,
    /// Salted digest, not an address: enough to correlate a disputed session, not a surveillance
    /// record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip_digest: Option<Digest>,
    /// Salted digest of the user agent, for the same reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent_digest: Option<Digest>,
    /// Set when a service account acted for a named person, so delegation is visible rather than
    /// collapsed (§4.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_behalf_of: Option<PrincipalId>,
}

impl ReceiptActor {
    /// Check the actor record is internally honest (§9.2, I16).
    ///
    /// The rule that matters: an actor that is not a `user` MUST NOT carry a principal identity.
    /// Recording a person the Server did not authenticate is the exact failure §9.7 forbids, and it
    /// is not detectable later — the receipt would simply read as consent.
    pub fn validate(&self) -> Result<()> {
        let bad = |why: &str| ProtocolError::new(ErrorCode::InvalidRequest, why.to_string());
        match self.actor_type {
            ActorType::User => {
                if self.principal_id.is_none() {
                    return Err(bad(
                        "an actor of type `user` must name the principal it authenticated",
                    ));
                }
                if !self.principal_id.is_some_and(PrincipalId::is_person) {
                    return Err(bad(
                        "an actor of type `user` must be a person, not a machine",
                    ));
                }
            }
            ActorType::Policy | ActorType::Runtime | ActorType::AnonymousLink => {
                if self.principal_id.is_some() {
                    return Err(bad(
                        "only an actor of type `user` may carry a principal identity; recording \
                         one otherwise fabricates a person",
                    ));
                }
            }
        }
        Ok(())
    }
}

/// What this person actually saw: a digest plus a retained copy (§9.2).
///
/// A Server MUST compute the digest over the request **as presented at the step decided on**, and
/// MUST NOT re-derive it later from the request's current content. Thirty-two bytes converts "we
/// have a log" into "the log cannot quietly be rewritten to say something else was approved".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptRendered {
    /// Digest of the rendering.
    pub digest: Digest,
    /// Opaque pointer to the retained render. Never a public URL.
    ///
    /// Serialized as `ref`, which is what `openapi.yaml` and the published
    /// `fixtures/signing/receipt-core.json` both spell it. The Rust field cannot use that name.
    #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

/// Through which delivery the decision arrived, and how strong that evidence is (§9.2, §7.2).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptVia {
    /// The delivery answered through.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_id: Option<DeliveryId>,
    /// The channel name, carried verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    /// The target that delivery was addressed to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<Target>,
    /// The strongest evidence tier that delivery reached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grade_reached: Option<DeliveryGrade>,
}

/// What the request demanded and what the answerer actually presented — both, so the two can be
/// compared later (§9.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptAuthority {
    /// The authority the request declared, after the derived floor of §4.3.
    pub required: Authority,
    /// The strength actually established.
    pub satisfied: SatisfiedStrength,
}

/// One rung of a progressive-disclosure ladder (§5.5). Names which fields were provided, never
/// their values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptStep {
    /// Step number, from 1.
    pub n: u64,
    /// When this step was submitted, on the Server's clock.
    pub at: Timestamp,
    /// Field names only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields_provided: Vec<String>,
    /// Which of those were `secret`. Names only — the values went to the sink and were never here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secret_fields: Vec<String>,
    /// The delivery this step arrived through.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via_delivery_id: Option<DeliveryId>,
}

/// A capability the answerer held while answering: **presence and effect, never content** (§11.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityExercised {
    /// The grant handle.
    pub handle: GrantHandle,
    /// The session produced by resolving it.
    pub session_ref: GrantSessionRef,
    /// The scopes granted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<CapabilityScope>,
    /// When the session was resolved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<Timestamp>,
    /// When it was released.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released_at: Option<Timestamp>,
    /// Real held duration, derived from the lease record rather than an optimistic claim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_ms: Option<u64>,
    /// A **count** of input events. No payloads, ever: a person driving a live surface types real
    /// passwords into it, and an input log would recreate inside the audit trail exactly the
    /// exposure §12 exists to prevent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_events: Option<u64>,
    /// Ordered top-level origins visited.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub navigations: Vec<String>,
    /// Digest of the blast radius the person accepted (§11.5, I19). The full content may be
    /// personal data and MUST NOT be required to live in the receipt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blast_radius_digest: Option<Digest>,
}

/// How the system knows an out-of-band act was completed (§9.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClearanceSource {
    /// A person said so.
    HumanAssertion,
    /// A runtime concluded it from an observation. Recorded as inference, never laundered into a
    /// human fact.
    RuntimeInference,
    /// A deadline elapsed.
    Timeout,
}

/// Clearance provenance. **Clearance MUST be asserted, never inferred** (§9.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Clearance {
    /// How it was established.
    pub source: ClearanceSource,
    /// Null for anything but a human assertion: there is no principal to name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<PrincipalId>,
    /// When.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Timestamp>,
}

impl Clearance {
    /// Check that an inferred or timed-out clearance names no person (§9.7, I16).
    ///
    /// A Server MUST NOT fabricate a person. This is the check that makes that mechanical.
    pub fn validate(&self) -> Result<()> {
        if self.source != ClearanceSource::HumanAssertion && self.actor.is_some() {
            return Err(ProtocolError::new(
                ErrorCode::InvalidRequest,
                "only `human_assertion` clearance may name an actor; inference must not fabricate \
                 a person",
            ));
        }
        if self.source == ClearanceSource::HumanAssertion && self.actor.is_none() {
            return Err(ProtocolError::new(
                ErrorCode::InvalidRequest,
                "`human_assertion` clearance must name the person who asserted it",
            ));
        }
        Ok(())
    }
}

/// This receipt's position in the tenant's hash chain (§9.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChainLink {
    /// One-based position in the chain. See the crate documentation's ambiguity note A-4.
    pub height: u64,
    /// The previous receipt's digest, in the same tenant. `None` only for the first receipt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev_digest: Option<Digest>,
    /// This receipt's digest, taken over its canonical form **including** `prev_digest`.
    pub digest: Digest,
}

/// The tamper-evidence anchor an external verifier records and later re-checks (§9.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChainHead {
    /// The tenant this chain belongs to. Chains are per-tenant (§9.4).
    pub org_id: OrgId,
    /// The number of receipts in the chain. It only ever increases.
    pub height: u64,
    /// The digest of the most recent receipt.
    pub head_digest: Digest,
    /// When this head was exported.
    pub as_of: Timestamp,
}

/// An immutable record of an outcome (§9).
///
/// There is no update path on this type by design. Corrections are new receipts (§9.5), and the two
/// other layers of §9.4 — application and storage — live outside this crate. What this crate can
/// enforce is the third: any edit to a historical receipt changes its digest and therefore breaks
/// every link after it. See [`verify_chain`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    /// This receipt.
    pub id: ReceiptId,
    /// The request it settles.
    pub request_id: RequestId,
    /// The owning tenant. Chains are per-tenant, so this is part of the chain's identity.
    pub org_id: OrgId,
    /// Decision, policy, or correction.
    pub kind: ReceiptKind,
    /// The receipt this one amends, for a correction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corrects: Option<ReceiptId>,
    /// What was decided.
    pub decision: ReceiptDecision,
    /// Who decided.
    pub actor: ReceiptActor,
    /// Server clock at commit. Never a client-supplied time (§1.4).
    pub decided_at: Timestamp,
    /// Which attempt window the decision landed in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<String>,
    /// The version of the request that was decided on.
    pub request_version: u64,
    /// Digest of the request as it stood.
    pub request_digest: Digest,
    /// What the person actually saw.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rendered: Option<ReceiptRendered>,
    /// Through which delivery the decision arrived.
    #[serde(default)]
    pub via: ReceiptVia,
    /// What was required and what was satisfied.
    pub authority: ReceiptAuthority,
    /// The progressive-disclosure ladder as one intervention (§5.5).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<ReceiptStep>,
    /// Capabilities the answerer held.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities_exercised: Vec<CapabilityExercised>,
    /// How an out-of-band act was established as complete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clearance: Option<Clearance>,
    /// This receipt's chain link. `None` until it is sealed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain: Option<ChainLink>,
    /// Whether the answer's `rendered_digest` diverged from what was shown, under
    /// `presentation_binding: advisory` (§9.3). Absent means no divergence was observed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation_divergence: Option<PresentationDivergence>,
}

impl Receipt {
    /// Check the record is internally honest, before it is sealed into the chain.
    pub fn validate(&self) -> Result<()> {
        self.actor.validate()?;
        if let Some(clearance) = &self.clearance {
            clearance.validate()?;
        }
        match self.kind {
            ReceiptKind::Correction if self.corrects.is_none() => {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidRequest,
                    "a correction must name the receipt it corrects",
                ))
            }
            ReceiptKind::Decision | ReceiptKind::Policy if self.corrects.is_some() => {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidRequest,
                    "only a correction may name a corrected receipt",
                ))
            }
            _ => {}
        }
        // §9.6: an expiry receipt is visibly not a human decision.
        if self.kind == ReceiptKind::Policy {
            if self.actor.actor_type != ActorType::Policy {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidRequest,
                    "a policy receipt must record `actor.type = \"policy\"`",
                ));
            }
            if self.authority.satisfied != SatisfiedStrength::None {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidRequest,
                    "a policy receipt must record `authority.satisfied = \"none\"`",
                ));
            }
        }
        Ok(())
    }

    /// **Step 1 of `signing.md` §2.2** — the receipt core: this receipt with its `chain` member
    /// removed entirely.
    ///
    /// The whole member goes, not just `digest`. `height` and `prev_digest` are inputs to step 2
    /// and must not also be inside the hashed object, or the two steps would double-count them and
    /// no independent implementation of §2.2 would agree with this one.
    pub fn canonical_form(&self) -> Result<Value> {
        let mut value = serde_json::to_value(self).map_err(|e| {
            ProtocolError::new(
                ErrorCode::InvalidRequest,
                format!("receipt is not serializable: {e}"),
            )
        })?;
        if let Some(object) = value.as_object_mut() {
            object.remove("chain");
        }
        Ok(value)
    }

    /// `lowercase_hex(SHA-256(receipt_core))`, **without** a `sha256:` prefix (§2.2 step 1).
    pub fn core_hash(&self) -> Result<String> {
        let bytes = canonical_json(&self.canonical_form()?)?;
        let mut hex = String::with_capacity(64);
        for byte in Sha256::digest(&bytes) {
            hex.push_str(&format!("{byte:02x}"));
        }
        Ok(hex)
    }

    /// Seal this receipt into a tenant's chain, immediately after `previous`.
    ///
    /// Passing `None` starts a chain. The returned receipt carries its [`ChainLink`] and must not
    /// be modified afterwards: any change alters the digest and invalidates every later link, which
    /// is precisely the tamper-evidence §9.4 asks for.
    pub fn seal(mut self, previous: Option<&ChainLink>) -> Result<Self> {
        self.validate()?;
        let height = previous.map_or(1, |p| p.height + 1);
        // §2.2: for the first receipt in a tenant the predecessor is 64 ASCII zeros. It is stored,
        // not merely substituted during the computation, so that a party holding **one** receipt
        // can verify it without being handed the chain — which is the whole point of the receipt.
        let prev_digest = previous
            .map(|p| p.digest.clone())
            .unwrap_or_else(genesis_prev_digest);
        let digest = chain_digest(height, &prev_digest, &self.core_hash()?);
        self.chain = Some(ChainLink {
            height,
            prev_digest: Some(prev_digest),
            digest,
        });
        Ok(self)
    }
}

/// The predecessor of the first receipt in a tenant: 64 ASCII zeros, prefixed (`signing.md` §2.2).
pub fn genesis_prev_digest() -> Digest {
    Digest::parse(&format!("sha256:{}", "0".repeat(64)))
        .expect("64 zeros is a well-formed sha256 digest")
}

/// **Step 2 of `signing.md` §2.2** — the chain digest.
///
/// ```text
/// chain_input  = height ‖ LF ‖ prev_digest ‖ LF ‖ core_hash
/// chain.digest = "sha256:" ‖ lowercase_hex( SHA-256( chain_input ) )
/// ```
///
/// `prev_digest` carries its `sha256:` prefix; `core_hash` does not. `height` is inside the input
/// so an entry cannot be excised and the remaining entries re-linked without detection.
pub fn chain_digest(height: u64, prev_digest: &Digest, core_hash: &str) -> Digest {
    let input = format!("{height}\n{prev_digest}\n{core_hash}");
    Digest::sha256(input.as_bytes())
}

/// Walk a tenant's receipts in order, recompute every digest, and return the exportable head.
///
/// Altering any historical receipt — a value, an actor, a timestamp — changes its digest, which
/// breaks the `prev_digest` of the next link, which breaks the head. This gives tamper-evidence
/// with no key management at all (§9.4). This is the crate-testable half of C-15; the other half is
/// asserted from the storage layer, because the application is inside the threat model.
pub fn verify_chain(receipts: &[Receipt], as_of: Timestamp) -> Result<Option<ChainHead>> {
    let mut previous: Option<&ChainLink> = None;
    let mut org_id: Option<OrgId> = None;

    for (index, receipt) in receipts.iter().enumerate() {
        let position = index + 1;
        let broken = |why: String| {
            ProtocolError::new(
                ErrorCode::InvalidRequest,
                format!(
                    "receipt chain broken at position {position} ({}): {why}",
                    receipt.id
                ),
            )
        };

        match org_id {
            None => org_id = Some(receipt.org_id),
            Some(expected) if expected != receipt.org_id => {
                return Err(broken(
                    "the chain is per-tenant and this receipt belongs to another".to_string(),
                ))
            }
            Some(_) => {}
        }

        let link = receipt
            .chain
            .as_ref()
            .ok_or_else(|| broken("receipt is unsealed and carries no chain link".to_string()))?;

        if link.height != position as u64 {
            return Err(broken(format!(
                "expected height {position}, found {}",
                link.height
            )));
        }
        // §2.2: the predecessor of the first receipt is 64 ASCII zeros, and it is stored, so this
        // reads the same field for every position rather than special-casing the genesis.
        let expected_prev = previous.map_or_else(genesis_prev_digest, |p| p.digest.clone());
        if link.prev_digest.as_ref() != Some(&expected_prev) {
            return Err(broken(
                "prev_digest does not name the previous receipt".to_string(),
            ));
        }
        let recomputed = chain_digest(link.height, &expected_prev, &receipt.core_hash()?);
        if recomputed != link.digest {
            return Err(broken(
                "the recorded digest does not match the receipt's content; it has been altered"
                    .to_string(),
            ));
        }
        previous = Some(link);
    }

    Ok(match (previous, org_id) {
        (Some(link), Some(org_id)) => Some(ChainHead {
            org_id,
            height: link.height,
            head_digest: link.digest.clone(),
            as_of,
        }),
        _ => None,
    })
}

// ---------------------------------------------------------------------------------------------
// Presentation binding (§9.3)
// ---------------------------------------------------------------------------------------------

/// How strictly an answer must match what the answerer was shown (§9.3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresentationBinding {
    /// The answer is accepted and the divergence is recorded on the receipt.
    #[default]
    Advisory,
    /// A divergent answer is rejected; the person must re-read the current request.
    Strict,
}

/// A recorded mismatch between what the answerer echoed and what they were shown.
///
/// Only produced under [`PresentationBinding::Advisory`]; under `strict` the answer is refused
/// instead. Either way the receipt says which mode applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresentationDivergence {
    /// The digest the answerer's client echoed.
    pub answered_with: Digest,
    /// The digest of what the Server rendered for them.
    pub shown: Digest,
}

/// Compare the answer's echoed `rendered_digest` against what was shown (§9.3).
///
/// Returns the divergence to record, if any. A Server MUST record whichever mode applied.
pub fn check_presentation(
    binding: PresentationBinding,
    shown: &Digest,
    answered_with: Option<&Digest>,
) -> Result<Option<PresentationDivergence>> {
    match answered_with {
        // A client that echoes nothing has asserted nothing to diverge from. Under `strict` the
        // Server cannot establish that the person read the current wording, so it must refuse.
        None => match binding {
            PresentationBinding::Advisory => Ok(None),
            PresentationBinding::Strict => Err(ProtocolError::new(
                ErrorCode::PresentationStale,
                "this request binds the presentation; the answer must echo the rendered digest",
            )),
        },
        Some(echoed) if echoed == shown => Ok(None),
        Some(echoed) => match binding {
            PresentationBinding::Advisory => Ok(Some(PresentationDivergence {
                answered_with: echoed.clone(),
                shown: shown.clone(),
            })),
            PresentationBinding::Strict => Err(ProtocolError::new(
                ErrorCode::PresentationStale,
                "the request changed since it was rendered; please re-read it",
            )),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::UserId;
    use crate::requires::Role;
    use serde_json::json;

    fn ts(s: &str) -> Timestamp {
        Timestamp::parse(s).expect("valid timestamp")
    }
    fn org() -> OrgId {
        OrgId::parse("org_01K0A2XFV8C4YRXB2N6VD9FTHE").expect("parse")
    }
    fn person() -> PrincipalId {
        PrincipalId::User(UserId::parse("usr_01J9ZP4KRTC4YRXB2N6VD9FTHE").expect("parse"))
    }

    fn receipt(id: &str) -> Receipt {
        Receipt {
            id: ReceiptId::parse(id).expect("parse"),
            request_id: RequestId::parse("req_01K3M7QW8ZC4YRXB2N6VD9FTHE").expect("parse"),
            org_id: org(),
            kind: ReceiptKind::Decision,
            corrects: None,
            decision: ReceiptDecision {
                values: json!({"decision": "approve"})
                    .as_object()
                    .expect("object")
                    .clone(),
                disposition: Disposition::Decide,
                note: None,
            },
            actor: ReceiptActor {
                actor_type: ActorType::User,
                principal_id: Some(person()),
                display: Some("Dana Okafor".to_string()),
                role_at_decision: Some("editor".to_string()),
                auth_strength: Some(AuthStrength::Session),
                reauth_at: None,
                mfa_at: None,
                ip_digest: None,
                user_agent_digest: None,
                on_behalf_of: None,
            },
            decided_at: ts("2026-07-30T14:07:44Z"),
            attempt_id: None,
            request_version: 1,
            request_digest: Digest::sha256(b"the request"),
            rendered: Some(ReceiptRendered {
                digest: Digest::sha256(b"what dana saw"),
                reference: Some("render:rcpt_01K3MB2R4YC4YRXB2N6VD9FTHE".to_string()),
            }),
            via: ReceiptVia::default(),
            authority: ReceiptAuthority {
                required: Authority {
                    min_role: Some(Role::Editor),
                    ..Authority::default()
                },
                satisfied: SatisfiedStrength::Session,
            },
            steps: Vec::new(),
            capabilities_exercised: Vec::new(),
            clearance: None,
            chain: None,
            presentation_divergence: None,
        }
    }

    // -------------------------------------------------------------- canonicalization

    #[test]
    fn canonicalizing_the_same_logical_value_twice_is_byte_identical() {
        // The same receipt built through two different key orders must canonicalize identically,
        // or a verifier reports tampering that never happened.
        let one = json!({"b": 1, "a": {"z": true, "y": [1, 2]}, "c": "x"});
        let other = json!({"c": "x", "a": {"y": [1, 2], "z": true}, "b": 1});
        let first = canonical_json(&one).expect("canonical");
        let second = canonical_json(&other).expect("canonical");
        assert_eq!(first, second);
        assert_eq!(
            String::from_utf8(first.clone()).expect("utf8"),
            r#"{"a":{"y":[1,2],"z":true},"b":1,"c":"x"}"#
        );
        // And it is stable across repeated runs of the same input.
        assert_eq!(canonical_json(&one).expect("canonical"), first);
    }

    #[test]
    fn canonical_keys_are_ordered_by_utf16_code_unit_not_by_utf8_bytes() {
        // U+1D400 encodes as the surrogate pair D835 DC00, so it sorts *before* U+FF3A in UTF-16
        // code-unit order. In UTF-8 bytes it sorts after (F0… against EF…). The two orderings
        // genuinely disagree on this pair, which is what makes the test discriminating.
        let (mathematical, full_width) = ("\u{1D400}", "\u{FF3A}");
        assert!(
            full_width < mathematical,
            "Rust's byte-wise order puts them the other way round"
        );

        let value = json!({mathematical: 1, full_width: 2});
        let rendered = String::from_utf8(canonical_json(&value).expect("canonical")).expect("utf8");
        assert!(
            rendered.find(mathematical) < rendered.find(full_width),
            "JCS orders by UTF-16 code unit: {rendered}"
        );
    }

    #[test]
    fn canonical_strings_escape_only_what_json_requires() {
        let value = json!({"k": "a\"b\\c\nd\u{1}e\u{00e9}"});
        let rendered = String::from_utf8(canonical_json(&value).expect("canonical")).expect("utf8");
        assert_eq!(rendered, "{\"k\":\"a\\\"b\\\\c\\nd\\u0001e\u{00e9}\"}");
    }

    #[test]
    fn normalizing_makes_the_stored_form_the_form_that_gets_canonicalized() {
        // §1.4 rule 3, stated as the check an outsider can run: canonicalize what a Server serves
        // and you get back what it served. That fails whenever a number is kept in one form and
        // digested in another, which is precisely what `-0.0` did.
        let mut served = json!({
            "negative_zero": -0.0,
            "whole_float": 2400.0,
            "exponent": 1e2,
            "already_integer": 7,
            "nested": {"deep": [3.0, {"deeper": -0.0}]},
        });
        normalize_numbers(&mut served);

        // Serialized rather than compared as values: `-0.0 == 0` is true, so a value comparison
        // would pass with or without normalization and would be measuring nothing.
        assert_eq!(
            serde_json::to_string(&served).expect("serialize"),
            r#"{"already_integer":7,"exponent":100,"negative_zero":0,"nested":{"deep":[3,{"deeper":0}]},"whole_float":2400}"#
        );

        // The property itself: the bytes at rest and the canonical bytes are the same bytes.
        assert_eq!(
            canonical_json(&served).expect("canonical"),
            serde_json::to_vec(&served).expect("serialize"),
        );

        // A number with no canonical form is left alone rather than mangled into one — refusing it
        // is validation's job, and `canonical_json` still does refuse it.
        let mut fractional = json!({"amount": 1.5});
        normalize_numbers(&mut fractional);
        assert_eq!(fractional, json!({"amount": 1.5}));
        assert!(canonical_json(&fractional).is_err());
    }

    #[test]
    fn numbers_are_deterministic_or_refused_never_approximated() {
        for (value, expected) in [
            (json!(0), "0"),
            (json!(-0.0), "0"),
            (json!(2400), "2400"),
            // The same number, whether the client wrote it as an integer or a float.
            (json!(2400.0), "2400"),
            (json!(1e2), "100"),
            (json!(-17), "-17"),
            (json!(9_007_199_254_740_991i64), "9007199254740991"),
            (json!(-9_007_199_254_740_991i64), "-9007199254740991"),
        ] {
            let rendered =
                String::from_utf8(canonical_json(&value).expect("canonical")).expect("utf8");
            assert_eq!(rendered, expected, "{value}");
        }

        // Digest-covered content carries integers only. A non-integer is refused rather than
        // rendered, because RFC 8785 inherits ECMAScript number formatting and both published SDKs
        // refuse to canonicalize one at all — a receipt only this implementation can canonicalize
        // is a receipt nobody can verify.
        for non_integer in [json!(1.5), json!(0.000_001), json!(-2.25), json!(1e-7)] {
            assert!(
                canonical_json(&non_integer).is_err(),
                "{non_integer} must be refused, not rendered"
            );
        }

        // And beyond the safe-integer range a value cannot be told from its neighbours, so a
        // receipt would record a figure nobody entered.
        for unsafe_integer in [json!(9_007_199_254_740_992i64), json!(1e21), json!(-1e21)] {
            assert!(
                canonical_json(&unsafe_integer).is_err(),
                "{unsafe_integer} must be refused, not approximated"
            );
        }
    }

    #[test]
    fn a_digest_is_algorithm_prefixed_and_parses_back() {
        let digest = Digest::sha256(b"handoff");
        assert_eq!(digest.algorithm(), "sha256");
        assert_eq!(digest.as_str().len(), "sha256:".len() + 64);
        assert_eq!(Digest::parse(digest.as_str()).expect("parse"), digest);
        for bad in [
            "",
            "sha256",
            "sha256:",
            "sha256:XYZ",
            "SHA256:abcdef",
            "sha256:abc",
        ] {
            assert!(Digest::parse(bad).is_err(), "`{bad}` must not parse");
        }
        // Serde uses the wire string.
        assert_eq!(
            serde_json::to_value(&digest).expect("serialize"),
            json!(digest.as_str())
        );
    }

    // ------------------------------------------- the published vectors of `signing.md` §2.5
    //
    // These constants are the specification's, not this implementation's. That is the entire
    // point: the previous construction was self-consistent — it sealed and verified with the same
    // code, so every test passed — while computing a digest no independent implementation of §2.2
    // could reproduce. A check that shares code with the producer proves only self-consistency.
    // These numbers come from outside the crate and cannot be satisfied by agreeing with itself.

    /// `sha256(receipt_core)` from §2.5.
    const VECTOR_CORE_HASH: &str =
        "2763f39ef8a61d493106d3db302ec36cae5c024ca3da3a019d483ccc29704ad1";
    /// The height from §2.5.
    const VECTOR_HEIGHT: u64 = 4211;
    /// The published `chain.digest` for that core hash at that height.
    const VECTOR_CHAIN_DIGEST: &str =
        "sha256:919f8870391849de4e7b1d5b249ccbaaa7d5a7d3f500f5571c5a92dd0c3909db";

    #[test]
    fn the_published_chain_digest_vector_reproduces_exactly() {
        let digest = chain_digest(VECTOR_HEIGHT, &genesis_prev_digest(), VECTOR_CORE_HASH);
        assert_eq!(digest.as_str(), VECTOR_CHAIN_DIGEST);
    }

    #[test]
    fn the_genesis_predecessor_is_sixty_four_zeros() {
        assert_eq!(
            genesis_prev_digest().as_str(),
            "sha256:0000000000000000000000000000000000000000000000000000000000000000"
        );
    }

    #[test]
    fn the_chain_input_is_three_fields_separated_by_two_line_feeds() {
        // Written out from §2.2's prose rather than by calling `chain_digest`, so this fails if the
        // construction is ever changed to hash something else — which is exactly what had happened.
        let expected = {
            let input = format!(
                "{VECTOR_HEIGHT}\nsha256:{}\n{VECTOR_CORE_HASH}",
                "0".repeat(64)
            );
            assert_eq!(input.matches('\n').count(), 2);
            Digest::sha256(input.as_bytes())
        };
        assert_eq!(expected.as_str(), VECTOR_CHAIN_DIGEST);
        assert_eq!(
            chain_digest(VECTOR_HEIGHT, &genesis_prev_digest(), VECTOR_CORE_HASH),
            expected
        );
    }

    #[test]
    fn the_receipt_core_excludes_the_whole_chain_member_not_just_the_digest() {
        // §2.2 step 1 removes `chain` entirely. Leaving `height` or `prev_digest` inside the hashed
        // object double-counts them, because step 2 already covers both — and that alone is enough
        // for no other implementation to agree with this one.
        let sealed = receipt("rcpt_01K3MB2R4YC4YRXB2N6VD9FTHE")
            .seal(None)
            .expect("seal");
        let core = sealed.canonical_form().expect("core");
        assert!(
            core.get("chain").is_none(),
            "the receipt core carries no chain member at all"
        );
        // And the core hash is over that object, unchanged by which chain link it was sealed into.
        let unsealed_core = receipt("rcpt_01K3MB2R4YC4YRXB2N6VD9FTHE")
            .canonical_form()
            .expect("core");
        assert_eq!(core, unsealed_core);
    }

    // -------------------------------------------------------------- the chain (§9.4, C-15)

    #[test]
    fn a_sealed_chain_verifies_and_exports_a_head() {
        let first = receipt("rcpt_01K3MB2R4YC4YRXB2N6VD9FTHE")
            .seal(None)
            .expect("seal");
        let link = first.chain.clone().expect("sealed");
        assert_eq!(link.height, 1);
        // `signing.md` §2.2: the predecessor of the first receipt in a tenant is 64 ASCII zeros,
        // and it is **stored** rather than substituted at verification time. A party holding one
        // receipt and nothing else has to be able to verify it, and it cannot if the field it needs
        // is absent.
        assert_eq!(
            link.prev_digest.as_ref(),
            Some(&genesis_prev_digest()),
            "the first receipt names the genesis predecessor"
        );

        let second = receipt("rcpt_01K3MB2R4ZC4YRXB2N6VD9FTHE")
            .seal(Some(&link))
            .expect("seal");
        let chain = vec![first, second];
        let head = verify_chain(&chain, ts("2026-07-30T15:00:00Z"))
            .expect("verifies")
            .expect("head");
        assert_eq!(head.height, 2);
        assert_eq!(head.org_id, org());
        assert_eq!(
            head.head_digest,
            chain[1].chain.as_ref().expect("sealed").digest
        );
    }

    #[test]
    fn altering_any_historical_receipt_invalidates_the_head() {
        let first = receipt("rcpt_01K3MB2R4YC4YRXB2N6VD9FTHE")
            .seal(None)
            .expect("seal");
        let second = receipt("rcpt_01K3MB2R4ZC4YRXB2N6VD9FTHE")
            .seal(Some(first.chain.as_ref().expect("sealed")))
            .expect("seal");
        let good = vec![first, second];
        let baseline = verify_chain(&good, ts("2026-07-30T15:00:00Z"))
            .expect("ok")
            .expect("head");

        // Rewrite history: the same decision, quietly changed to a rejection.
        let mut tampered = good.clone();
        tampered[0]
            .decision
            .values
            .insert("decision".to_string(), json!("reject"));
        let err = verify_chain(&tampered, ts("2026-07-30T15:00:00Z")).expect_err("must not verify");
        assert!(err.message.contains("altered"), "{}", err.message);

        // Re-sealing the altered receipt to hide the edit changes the head, which an external
        // verifier holding the old head detects immediately.
        let resealed_first = tampered[0].clone().seal(None).expect("seal");
        let resealed_second = tampered[1]
            .clone()
            .seal(Some(resealed_first.chain.as_ref().expect("sealed")))
            .expect("seal");
        let resealed = vec![resealed_first, resealed_second];
        let after = verify_chain(&resealed, ts("2026-07-30T15:00:00Z"))
            .expect("ok")
            .expect("head");
        assert_ne!(after.head_digest, baseline.head_digest);
    }

    #[test]
    fn a_chain_with_a_broken_link_is_rejected() {
        let first = receipt("rcpt_01K3MB2R4YC4YRXB2N6VD9FTHE")
            .seal(None)
            .expect("seal");
        let orphan = receipt("rcpt_01K3MB2R4ZC4YRXB2N6VD9FTHE")
            .seal(None)
            .expect("seal");
        // `orphan` claims height 1 and no predecessor, but sits at position 2.
        assert!(verify_chain(&[first, orphan], ts("2026-07-30T15:00:00Z")).is_err());
    }

    #[test]
    fn chains_are_per_tenant() {
        let first = receipt("rcpt_01K3MB2R4YC4YRXB2N6VD9FTHE")
            .seal(None)
            .expect("seal");
        let mut foreign = receipt("rcpt_01K3MB2R4ZC4YRXB2N6VD9FTHE");
        foreign.org_id = OrgId::parse("org_01K0A2XFV8C4YRXB2N6VD9FTHF").expect("parse");
        let foreign = foreign
            .seal(Some(first.chain.as_ref().expect("sealed")))
            .expect("seal");
        let err =
            verify_chain(&[first, foreign], ts("2026-07-30T15:00:00Z")).expect_err("rejected");
        assert!(err.message.contains("per-tenant"), "{}", err.message);
    }

    #[test]
    fn an_empty_chain_has_no_head() {
        assert!(verify_chain(&[], ts("2026-07-30T15:00:00Z"))
            .expect("ok")
            .is_none());
    }

    #[test]
    fn the_digest_covers_prev_digest_so_reordering_is_detectable() {
        let first = receipt("rcpt_01K3MB2R4YC4YRXB2N6VD9FTHE");
        let alone = first.clone().seal(None).expect("seal");
        let after_another = first
            .seal(Some(&ChainLink {
                height: 1,
                prev_digest: None,
                digest: Digest::sha256(b"some other receipt"),
            }))
            .expect("seal");
        assert_ne!(
            alone.chain.expect("sealed").digest,
            after_another.chain.expect("sealed").digest,
            "the same content in a different chain position must hash differently"
        );
    }

    // -------------------------------------------------------------- honesty rules

    #[test]
    fn a_non_human_actor_may_not_carry_a_principal_identity() {
        let mut r = receipt("rcpt_01K3MB2R4YC4YRXB2N6VD9FTHE");
        r.kind = ReceiptKind::Policy;
        r.actor.actor_type = ActorType::Policy;
        r.authority.satisfied = SatisfiedStrength::None;
        // The principal is left in place: this is the fabrication §9.7 forbids.
        assert!(
            r.clone().seal(None).is_err(),
            "a policy receipt must not name a person"
        );
        r.actor.principal_id = None;
        r.seal(None).expect("an honest policy receipt seals");
    }

    #[test]
    fn a_policy_receipt_must_say_it_was_a_policy() {
        let mut r = receipt("rcpt_01K3MB2R4YC4YRXB2N6VD9FTHE");
        r.kind = ReceiptKind::Policy;
        r.actor = ReceiptActor {
            actor_type: ActorType::User,
            principal_id: Some(person()),
            display: None,
            role_at_decision: None,
            auth_strength: None,
            reauth_at: None,
            mfa_at: None,
            ip_digest: None,
            user_agent_digest: None,
            on_behalf_of: None,
        };
        assert!(r.clone().seal(None).is_err());

        r.actor.actor_type = ActorType::Policy;
        r.actor.principal_id = None;
        // Still wrong: a policy satisfied no authority and must say so.
        assert!(r.clone().seal(None).is_err());
        r.authority.satisfied = SatisfiedStrength::None;
        r.seal(None).expect("now honest");
    }

    #[test]
    fn a_user_actor_must_be_a_person_not_a_machine() {
        let mut r = receipt("rcpt_01K3MB2R4YC4YRXB2N6VD9FTHE");
        r.actor.principal_id =
            Some(PrincipalId::parse("sa_01J9ZP4KRTC4YRXB2N6VD9FTHE").expect("parse"));
        assert!(r.seal(None).is_err(), "a service account is not a `user`");
    }

    #[test]
    fn clearance_is_asserted_never_inferred() {
        let mut r = receipt("rcpt_01K3MB2R4YC4YRXB2N6VD9FTHE");
        r.clearance = Some(Clearance {
            source: ClearanceSource::RuntimeInference,
            actor: Some(person()),
            at: Some(ts("2026-07-30T14:07:44Z")),
        });
        assert!(
            r.clone().seal(None).is_err(),
            "inference must not name a person"
        );

        r.clearance = Some(Clearance {
            source: ClearanceSource::RuntimeInference,
            actor: None,
            at: Some(ts("2026-07-30T14:07:44Z")),
        });
        r.clone()
            .seal(None)
            .expect("inference with no actor is honest");

        r.clearance = Some(Clearance {
            source: ClearanceSource::HumanAssertion,
            actor: None,
            at: Some(ts("2026-07-30T14:07:44Z")),
        });
        assert!(
            r.seal(None).is_err(),
            "a human assertion must name the person who made it"
        );
    }

    #[test]
    fn a_correction_is_a_new_receipt_that_names_the_original() {
        let original = receipt("rcpt_01K3MB2R4YC4YRXB2N6VD9FTHE")
            .seal(None)
            .expect("seal");
        let mut correction = receipt("rcpt_01K3MB2R4ZC4YRXB2N6VD9FTHE");
        correction.kind = ReceiptKind::Correction;
        assert!(
            correction.clone().seal(None).is_err(),
            "a correction must say what it corrects"
        );

        correction.corrects = Some(original.id);
        let correction = correction
            .seal(Some(original.chain.as_ref().expect("sealed")))
            .expect("seal");
        // Both stay in the chain; the original is untouched (I2, §9.5).
        let head = verify_chain(&[original.clone(), correction], ts("2026-07-30T15:00:00Z"))
            .expect("verifies")
            .expect("head");
        assert_eq!(head.height, 2);
        assert_eq!(original.decision.values["decision"], json!("approve"));
    }

    // -------------------------------------------------------------- presentation binding (§9.3)

    #[test]
    fn presentation_binding_records_or_refuses_a_divergence() {
        let shown = Digest::sha256(b"version 2");
        let stale = Digest::sha256(b"version 1");

        assert!(
            check_presentation(PresentationBinding::Advisory, &shown, Some(&shown))
                .expect("match")
                .is_none()
        );

        let divergence = check_presentation(PresentationBinding::Advisory, &shown, Some(&stale))
            .expect("advisory accepts")
            .expect("but records");
        assert_eq!(divergence.answered_with, stale);
        assert_eq!(divergence.shown, shown);

        let err = check_presentation(PresentationBinding::Strict, &shown, Some(&stale))
            .expect_err("strict refuses");
        assert_eq!(err.code, ErrorCode::PresentationStale);
        assert_eq!(err.http_status(), 409);

        // Echoing nothing is acceptable under advisory and refused under strict.
        assert!(
            check_presentation(PresentationBinding::Advisory, &shown, None)
                .expect("ok")
                .is_none()
        );
        assert_eq!(
            check_presentation(PresentationBinding::Strict, &shown, None)
                .expect_err("refused")
                .code,
            ErrorCode::PresentationStale
        );
    }

    #[test]
    fn a_receipt_round_trips_through_the_wire_shape() {
        let sealed = receipt("rcpt_01K3MB2R4YC4YRXB2N6VD9FTHE")
            .seal(None)
            .expect("seal");
        let json = serde_json::to_value(&sealed).expect("serialize");
        assert_eq!(json["kind"], "decision");
        assert_eq!(json["actor"]["type"], "user");
        assert_eq!(json["decided_at"], "2026-07-30T14:07:44Z");
        assert_eq!(json["chain"]["height"], 1);
        let back: Receipt = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back, sealed);
    }
}

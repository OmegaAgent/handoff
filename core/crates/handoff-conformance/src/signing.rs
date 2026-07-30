//! Callback signature verification and the C-18 checks.
//!
//! The scheme is normative in `spec/signing.md` §1 and is implemented here in full:
//!
//! ```text
//! canonical = version ‖ LF ‖ timestamp ‖ LF ‖ delivery_id ‖ LF ‖ body_sha256_hex
//! signature = lowercase_hex( HMAC_SHA256( secret_utf8, canonical_utf8 ) )
//! ```
//!
//! Three of C-18's checks — replay onto a different delivery, a one-byte body change, a stale
//! timestamp — are really assertions about **what the canonical string binds**. They are evaluated
//! by taking a genuine callback the Server sent, altering exactly one element, and requiring that
//! verification now fails. That tests the Server's construction, not the runner's arithmetic: a
//! Server that signed only the body would produce a signature that survives being lifted onto
//! another delivery, and this is what would catch it.
//!
//! The unit tests at the bottom replay `signing.md` §1.6's published vectors, so the verifier is
//! itself checked against the specification before it is allowed to judge anyone.

use crate::callback::Captured;
use crate::case::CallbackCheck;
use crate::profile::CallbackConfig;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// The receiver-enforced freshness window, `signing.md` §1.3.2.
pub const FRESHNESS_WINDOW_SECONDS: i64 = 300;

/// Evaluate one check against the callbacks captured so far.
pub fn check(
    check: CallbackCheck,
    captured: &[Captured],
    config: &CallbackConfig,
) -> Result<(), String> {
    match check {
        CallbackCheck::Signed => signed(captured),
        CallbackCheck::SequenceMonotonicPerWaiter => sequence_monotonic(captured),
        CallbackCheck::RedeliversUntilAcked => redelivers(captured),
        CallbackCheck::CarriesNoResolvableUrl => no_resolvable_url(captured),
        CallbackCheck::TenancyNotDerivableFromBody => tenancy_not_in_body(captured),
        CallbackCheck::SignatureVerifies => signature_verifies(captured, config),
        CallbackCheck::OneByteTamperRejected => one_byte_tamper(captured, config),
        CallbackCheck::ReplayOntoOtherDeliveryRejected => replay_other_delivery(captured, config),
        CallbackCheck::StaleTimestampRejected => stale_timestamp(captured, config),
        CallbackCheck::RotationOverlapBothVerify => rotation_overlap(captured, config),
    }
}

// ---------------------------------------------------------------- the scheme

/// The canonical string of `signing.md` §1.2. Exactly three line feeds and no trailing newline.
pub fn canonical_string(version: &str, timestamp: i64, delivery_id: &str, body: &[u8]) -> String {
    format!(
        "{version}\n{timestamp}\n{delivery_id}\n{}",
        hex(&Sha256::digest(body))
    )
}

/// `lowercase_hex(HMAC_SHA256(secret_utf8, canonical_utf8))`.
pub fn sign(secret: &str, canonical: &str) -> String {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret.as_bytes())
        .expect("HMAC accepts a key of any length");
    mac.update(canonical.as_bytes());
    hex(&mac.finalize().into_bytes())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The parts of `Handoff-Signature`: the timestamp and every offered `v1` value.
#[derive(Debug, Clone)]
pub struct SignatureHeader {
    /// The `t=` value, Unix seconds.
    pub timestamp: i64,
    /// Every `v1=` value, in the order offered. Two during a rotation overlap.
    pub v1: Vec<String>,
}

/// Parse `t=<unix_seconds>,v1=<hex>[,v1=<hex>]`.
pub fn parse_signature_header(raw: &str) -> Result<SignatureHeader, String> {
    let mut timestamp = None;
    let mut v1 = Vec::new();
    for part in raw.split(',') {
        match part.trim().split_once('=') {
            Some(("t", value)) => {
                timestamp = Some(
                    value
                        .parse::<i64>()
                        .map_err(|_| format!("`t={value}` is not Unix seconds in ASCII decimal"))?,
                )
            }
            Some(("v1", value)) => v1.push(value.to_string()),
            _ => return Err(format!("malformed Handoff-Signature element {part:?}")),
        }
    }
    let timestamp = timestamp.ok_or("Handoff-Signature carries no `t=` element")?;
    if v1.is_empty() {
        return Err("Handoff-Signature carries no `v1=` element".to_string());
    }
    Ok(SignatureHeader { timestamp, v1 })
}

/// Verify one callback the way `signing.md` §1.3 requires a receiver to.
pub fn verify(captured: &Captured, secrets: &[String], now: i64) -> Result<(), String> {
    let header = |name: &str| -> Result<&String, String> {
        captured
            .headers
            .get(&name.to_lowercase())
            .ok_or_else(|| format!("no `{name}` header (signing.md §1.1 requires it)"))
    };

    let signature = parse_signature_header(header("Handoff-Signature")?)?;
    let version = header("Handoff-Version")?;
    let delivery = header("Handoff-Delivery")?;

    if (now - signature.timestamp).abs() > FRESHNESS_WINDOW_SECONDS {
        return Err(format!(
            "timestamp {} is outside the {FRESHNESS_WINDOW_SECONDS}s freshness window at {now}",
            signature.timestamp
        ));
    }

    let canonical = canonical_string(
        version,
        signature.timestamp,
        delivery,
        captured.body.as_bytes(),
    );
    for secret in secrets {
        let expected = sign(secret, &canonical);
        if signature
            .v1
            .iter()
            .any(|offered| constant_time_eq(offered, &expected))
        {
            return Ok(());
        }
    }
    Err("no active secret produces any of the offered v1 values".to_string())
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

// ---------------------------------------------------------------- the checks

fn active_secrets(config: &CallbackConfig) -> Result<Vec<String>, String> {
    let mut secrets = Vec::new();
    if let Some(s) = &config.secret {
        secrets.push(s.clone());
    }
    if let Some(s) = &config.secret_previous {
        secrets.push(s.clone());
    }
    if secrets.is_empty() {
        return Err(
            "the profile supplies no callback signing secret under `callback: secret:`. \
             signing.md §1.4: the identifier IS the secret, and verification is impossible \
             without it."
                .to_string(),
        );
    }
    Ok(secrets)
}

fn newest(captured: &[Captured]) -> Result<&Captured, String> {
    captured
        .last()
        .ok_or_else(|| "no callbacks arrived, so nothing was demonstrated".to_string())
}

fn signed(captured: &[Captured]) -> Result<(), String> {
    if captured.is_empty() {
        return Err("no callbacks arrived, so nothing about signing was demonstrated".to_string());
    }
    let required = [
        "handoff-signature",
        "handoff-delivery",
        "handoff-signal",
        "handoff-version",
        "handoff-sequence",
        "handoff-idempotency-key",
        "content-type",
    ];
    for (i, c) in captured.iter().enumerate() {
        let missing: Vec<&str> = required
            .iter()
            .copied()
            .filter(|h| !c.headers.contains_key(*h))
            .collect();
        if !missing.is_empty() {
            return Err(format!(
                "callback {i} is missing {missing:?}. signing.md §1.1 marks every one of them \
                 MUST.\n      headers seen: {:?}",
                c.headers.keys().collect::<Vec<_>>()
            ));
        }
        parse_signature_header(&c.headers["handoff-signature"])
            .map_err(|e| format!("callback {i}: {e}"))?;

        let body_sequence = c.json().get("sequence").and_then(|v| v.as_i64());
        let header_sequence = c.headers["handoff-sequence"].parse::<i64>().ok();
        if body_sequence.is_none() {
            return Err(format!(
                "callback {i} carries no integer `sequence` in the body"
            ));
        }
        if body_sequence != header_sequence {
            return Err(format!(
                "callback {i}: Handoff-Sequence is {header_sequence:?} but the body says \
                 {body_sequence:?}. signing.md §1.1 — the header is a convenience mirror and a \
                 receiver MUST reject a mismatch, so a Server must not produce one."
            ));
        }
        if c.headers["handoff-idempotency-key"] != c.headers["handoff-delivery"] {
            return Err(format!(
                "callback {i}: Handoff-Idempotency-Key must be the delivery identifier, so a \
                 receiver can dedupe without parsing the body (signing.md §1.1)"
            ));
        }
    }
    Ok(())
}

fn sequence_monotonic(captured: &[Captured]) -> Result<(), String> {
    if captured.is_empty() {
        return Err("no callbacks arrived, so no sequence could be observed".to_string());
    }
    let mut highest: BTreeMap<String, i64> = BTreeMap::new();
    let mut seen: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    for (i, c) in captured.iter().enumerate() {
        let doc = c.json();
        let waiter = doc
            .get("waiter_ref")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                format!("callback {i} carries no `waiter_ref`, so its sequence has no scope")
            })?
            .to_string();
        let sequence = doc
            .get("sequence")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| format!("callback {i} carries no integer `sequence` (§8.3)"))?;

        let already = seen.entry(waiter.clone()).or_default();
        if already.contains(&sequence) {
            // A redelivery repeats its sequence. That is expected, not a violation.
            continue;
        }
        already.push(sequence);
        if let Some(previous) = highest.insert(waiter.clone(), sequence) {
            if sequence <= previous {
                return Err(format!(
                    "waiter `{waiter}` produced sequence {sequence} after {previous}. §8.3 \
                     requires it to increase monotonically per waiter so a receiver can detect a \
                     gap or a reordering."
                ));
            }
        }
    }
    Ok(())
}

fn redelivers(captured: &[Captured]) -> Result<(), String> {
    let mut per_signal: BTreeMap<String, usize> = BTreeMap::new();
    for c in captured {
        if let Some(id) = c.headers.get("handoff-signal") {
            *per_signal.entry(id.clone()).or_default() += 1;
        } else if let Some(id) = c.json().get("id").and_then(|v| v.as_str()) {
            *per_signal.entry(id.to_string()).or_default() += 1;
        }
    }
    if per_signal.values().any(|&n| n >= 2) {
        Ok(())
    } else {
        Err(format!(
            "no signal was pushed more than once even though the receiver returned 2xx and never \
             acked. §15.4 and signing.md §1.3: a 2xx marks a callback dispatched, it does not \
             consume the signal — consumption is the ack, and redelivery must continue until one \
             arrives.\n      deliveries per signal: {per_signal:?}"
        ))
    }
}

fn no_resolvable_url(captured: &[Captured]) -> Result<(), String> {
    let hits: Vec<usize> = captured
        .iter()
        .enumerate()
        .filter(|(_, c)| c.body.contains("http://") || c.body.contains("https://"))
        .map(|(i, _)| i)
        .collect();
    if hits.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "callback(s) {hits:?} carry a resolvable address. signing.md §1.5: a callback MUST NOT \
             carry a capability handle's resolved address, a bearer URL, or a secret value — \
             identifiers and typed values only (I8, I18)."
        ))
    }
}

fn tenancy_not_in_body(captured: &[Captured]) -> Result<(), String> {
    for (i, c) in captured.iter().enumerate() {
        let doc = c.json();
        for key in ["org_id", "tenant_id", "tenant", "organization_id"] {
            if doc.get(key).is_some() {
                return Err(format!(
                    "callback {i} carries `{key}` in its body. §15.3 and I13 — a valid signature \
                     proves the SENDER, never the TENANT; a receiver must resolve tenancy from its \
                     own stored state keyed on the endpoint or the secret. A tenant identifier in \
                     the body is an invitation to read it from there."
                ));
            }
        }
    }
    Ok(())
}

fn signature_verifies(captured: &[Captured], config: &CallbackConfig) -> Result<(), String> {
    let secrets = active_secrets(config)?;
    let now = unix_now();
    for (i, c) in captured.iter().enumerate() {
        verify(c, &secrets, now).map_err(|e| {
            format!("callback {i} does not verify against the configured secret: {e}")
        })?;
    }
    Ok(())
}

fn one_byte_tamper(captured: &[Captured], config: &CallbackConfig) -> Result<(), String> {
    let secrets = active_secrets(config)?;
    let genuine = newest(captured)?;
    let now = unix_now();
    verify(genuine, &secrets, now)
        .map_err(|e| format!("the genuine callback must verify first, and it does not: {e}"))?;

    let mut tampered = genuine.clone();
    tampered.body.push(' ');
    match verify(&tampered, &secrets, now) {
        Err(_) => Ok(()),
        Ok(()) => Err(
            "a body altered by one byte still verifies. signing.md §1.2 — the signed string covers \
             sha256 of the exact transmitted bytes; a signature that survives a body change is not \
             covering the body."
                .to_string(),
        ),
    }
}

fn replay_other_delivery(captured: &[Captured], config: &CallbackConfig) -> Result<(), String> {
    let secrets = active_secrets(config)?;
    let genuine = newest(captured)?;
    let now = unix_now();
    verify(genuine, &secrets, now)
        .map_err(|e| format!("the genuine callback must verify first, and it does not: {e}"))?;

    let mut replayed = genuine.clone();
    let original = replayed
        .headers
        .get("handoff-delivery")
        .cloned()
        .unwrap_or_default();
    replayed
        .headers
        .insert("handoff-delivery".to_string(), format!("{original}X"));
    match verify(&replayed, &secrets, now) {
        Err(_) => Ok(()),
        Ok(()) => Err(
            "the same signature verifies against a different delivery. signing.md §1.2 — \
             delivery_id is inside the signed string precisely so a valid signature cannot be \
             lifted onto a different delivery of the same payload."
                .to_string(),
        ),
    }
}

fn stale_timestamp(captured: &[Captured], config: &CallbackConfig) -> Result<(), String> {
    let secrets = active_secrets(config)?;
    let genuine = newest(captured)?;
    let now = unix_now();
    verify(genuine, &secrets, now)
        .map_err(|e| format!("the genuine callback must verify first, and it does not: {e}"))?;

    // Re-sign correctly at a timestamp one second outside the window. The signature is valid; only
    // the freshness is not, which is exactly the case signing.md §1.6 names.
    let header = parse_signature_header(&genuine.headers["handoff-signature"])?;
    let stale_t = header.timestamp - (FRESHNESS_WINDOW_SECONDS + 1);
    let version = genuine.headers["handoff-version"].clone();
    let delivery = genuine.headers["handoff-delivery"].clone();
    let canonical = canonical_string(&version, stale_t, &delivery, genuine.body.as_bytes());

    let mut stale = genuine.clone();
    stale.headers.insert(
        "handoff-signature".to_string(),
        format!("t={stale_t},v1={}", sign(&secrets[0], &canonical)),
    );
    match verify(&stale, &secrets, now) {
        Err(_) => Ok(()),
        Ok(()) => Err(format!(
            "a correctly signed callback {}s old still verifies. signing.md §1.3.2 — the \
             freshness window is {FRESHNESS_WINDOW_SECONDS} seconds and it is receiver-enforced.",
            FRESHNESS_WINDOW_SECONDS + 1
        )),
    }
}

fn rotation_overlap(captured: &[Captured], config: &CallbackConfig) -> Result<(), String> {
    let (Some(current), Some(previous)) = (&config.secret, &config.secret_previous) else {
        return Err(
            "this check needs two active secrets. signing.md §1.4 — rotation is an overlap, not a \
             cutover: set `callback.secret` and `callback.secret_previous` in the profile to the \
             two secrets currently active on the endpoint."
                .to_string(),
        );
    };
    let genuine = newest(captured)?;
    let now = unix_now();
    let header = parse_signature_header(&genuine.headers["handoff-signature"])?;

    if header.v1.len() < 2 {
        return Err(format!(
            "the callback offered {} signature(s) while two secrets are active. signing.md §1.4.2 \
             — while two secrets are active the Server MUST sign with both and emit both as \
             separate v1= elements in one header, so there is no window in which valid callbacks \
             fail.",
            header.v1.len()
        ));
    }
    for (label, secret) in [("current", current), ("previous", previous)] {
        verify(genuine, std::slice::from_ref(secret), now).map_err(|e| {
            format!("the callback does not verify under the {label} secret during the overlap: {e}")
        })?;
    }
    Ok(())
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every constant below is published in signing.md §1.6 "Worked test vectors — callbacks".
    const BODY: &str = r#"{"created_at":"2026-07-30T14:07:44Z","decision":{"authorization_id":"auth_01K3MB2R4Z8ZC4YRXB2N6VD9FT","outcome":"answered","receipt_id":"rcpt_01K3MB2R4Y8ZC4YRXB2N6VD9FT","source":"human","values":{"decision":"approve","note":"Confirmed with Acme on the phone."}},"id":"sig_01K3MB2R4X8ZC4YRXB2N6VD9FT","request_id":"req_01K3M7QW8ZC4YRXB2N6VD9FTHE","resume_payload":null,"resume_ref":null,"resume_token":"rt_01K3MB2R558ZC4YRXB2N6VD9FT","sequence":1,"type":"answered","waiter_ref":"run:0198f2a1"}"#;
    const SECRET_A: &str = "whsec_2f8a91c4e7b3d05a6c1e9f47b28d3a05";
    const SECRET_B: &str = "whsec_9d41c07be5a2f36819b4d0e7c5a81f62";
    const TIMESTAMP: i64 = 1785592064;
    const DELIVERY: &str = "dlv_01K3MB2R6C8ZC4YRXB2N6VD9FT";
    const BODY_SHA: &str = "fbd6ec4cacc7cb9c9371d2791f946535e3d391a0594a92b5a3a27dd34f5e94fa";
    const SIG_A: &str = "cae13126f8dcd1e918376aa373be2757db7281a3e5aaed2d83d716537e03de80";
    const SIG_B: &str = "d86b3740bad654e46c1349614523a476be0eb7d6a30a798b2d475374f36c57eb";
    const SIG_OTHER_DELIVERY: &str =
        "9a674a003d0507ad13369a6bd82713769116a276ec57f26eb2637b2af00f8e68";

    #[test]
    fn body_length_and_hash_match_the_published_vector() {
        assert_eq!(BODY.len(), 493);
        assert_eq!(hex(&Sha256::digest(BODY.as_bytes())), BODY_SHA);
    }

    #[test]
    fn the_canonical_string_is_exactly_three_line_feeds() {
        let canonical = canonical_string("1", TIMESTAMP, DELIVERY, BODY.as_bytes());
        assert_eq!(canonical.matches('\n').count(), 3);
        assert!(!canonical.ends_with('\n'));
        assert_eq!(canonical, format!("1\n{TIMESTAMP}\n{DELIVERY}\n{BODY_SHA}"));
    }

    #[test]
    fn both_published_signatures_reproduce_exactly() {
        let canonical = canonical_string("1", TIMESTAMP, DELIVERY, BODY.as_bytes());
        assert_eq!(sign(SECRET_A, &canonical), SIG_A);
        assert_eq!(sign(SECRET_B, &canonical), SIG_B);
    }

    #[test]
    fn the_replay_vector_reproduces_exactly() {
        let other = "dlv_01K3MB2R6D8ZC4YRXB2N6VD9FT";
        let canonical = canonical_string("1", TIMESTAMP, other, BODY.as_bytes());
        assert_eq!(sign(SECRET_A, &canonical), SIG_OTHER_DELIVERY);
    }

    fn vector_callback() -> Captured {
        Captured {
            target: "/handoff-callback".into(),
            headers: BTreeMap::from([
                (
                    "handoff-signature".to_string(),
                    format!("t={TIMESTAMP},v1={SIG_A},v1={SIG_B}"),
                ),
                ("handoff-delivery".to_string(), DELIVERY.to_string()),
                (
                    "handoff-signal".to_string(),
                    "sig_01K3MB2R4X8ZC4YRXB2N6VD9FT".to_string(),
                ),
                ("handoff-version".to_string(), "1".to_string()),
                ("handoff-sequence".to_string(), "1".to_string()),
                ("handoff-idempotency-key".to_string(), DELIVERY.to_string()),
                ("content-type".to_string(), "application/json".to_string()),
            ]),
            body: BODY.to_string(),
            at: TIMESTAMP as u64,
        }
    }

    #[test]
    fn the_published_header_verifies_under_either_secret() {
        let c = vector_callback();
        assert!(verify(&c, &[SECRET_A.to_string()], TIMESTAMP).is_ok());
        assert!(verify(&c, &[SECRET_B.to_string()], TIMESTAMP).is_ok());
    }

    #[test]
    fn all_four_negative_vectors_are_rejected() {
        let secrets = [SECRET_A.to_string(), SECRET_B.to_string()];

        let mut tampered = vector_callback();
        tampered.body = BODY.replace("approve", "reject");
        assert!(
            verify(&tampered, &secrets, TIMESTAMP).is_err(),
            "tampered body"
        );

        let mut replayed = vector_callback();
        replayed.headers.insert(
            "handoff-delivery".into(),
            "dlv_01K3MB2R6D8ZC4YRXB2N6VD9FT".into(),
        );
        assert!(
            verify(&replayed, &secrets, TIMESTAMP).is_err(),
            "replay onto another delivery"
        );

        let stale = vector_callback();
        assert!(
            verify(&stale, &secrets, TIMESTAMP + 301).is_err(),
            "stale timestamp"
        );

        let retired = vector_callback();
        assert!(
            verify(&retired, &["whsec_never_issued".to_string()], TIMESTAMP).is_err(),
            "retired secret"
        );
    }

    #[test]
    fn the_freshness_boundary_is_inclusive_at_300_seconds() {
        let c = vector_callback();
        let secrets = [SECRET_A.to_string()];
        assert!(verify(&c, &secrets, TIMESTAMP + 300).is_ok());
        assert!(verify(&c, &secrets, TIMESTAMP + 301).is_err());
    }

    #[test]
    fn a_sequence_that_goes_backwards_is_a_violation() {
        let mut a = vector_callback();
        a.body = r#"{"id":"sig_1","waiter_ref":"run:a","sequence":2}"#.into();
        let mut b = vector_callback();
        b.body = r#"{"id":"sig_2","waiter_ref":"run:a","sequence":1}"#.into();
        assert!(sequence_monotonic(&[a, b]).is_err());
    }

    #[test]
    fn a_repeated_sequence_is_a_redelivery_not_a_violation() {
        let c = vector_callback();
        assert!(sequence_monotonic(&[c.clone(), c.clone()]).is_ok());
        assert!(redelivers(&[c.clone(), c]).is_ok());
    }

    #[test]
    fn a_tenant_identifier_in_the_body_fails_the_check() {
        let mut c = vector_callback();
        c.body = r#"{"id":"sig_1","org_id":"org_a","sequence":1}"#.into();
        assert!(tenancy_not_in_body(&[c]).is_err());
    }
}

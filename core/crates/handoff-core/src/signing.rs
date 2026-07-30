//! Callback signatures: the canonical string, the header, and the verifier.
//!
//! `signing.md` §1 specifies this normatively and publishes worked vectors. Everything here is
//! checked against those published constants rather than against itself, because a signing scheme
//! that only agrees with its own tests is a scheme that has never crossed a trust boundary.
//!
//! The scheme lives in this crate rather than in the server so that the sender, the receiver, and
//! any adapter that signs an outbound request all compute the same bytes. Two implementations of a
//! canonical string is one implementation too many.
//!
//! > **A valid signature proves the SENDER. It never proves the TENANT.**
//!
//! Nothing here returns, derives, or accepts a tenant. §0 of `signing.md` and I13 require a
//! receiver to resolve tenancy from its own stored state keyed on the endpoint or the secret, and
//! the way to make that hard to get wrong is to give the verifier no tenant to hand back.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

/// The signature scheme version this module implements, as it appears in `Handoff-Version` and in
/// the `v1=` element name.
pub const SIGNATURE_VERSION: &str = "1";

/// The freshness window, in seconds (`signing.md` §1.3 rule 2). Receiver-enforced.
pub const FRESHNESS_WINDOW_SECS: i64 = 300;

/// Lowercase hex, the only encoding this scheme uses.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

/// `SHA-256` over the exact bytes of the body as transmitted, as 64 lowercase hex characters.
///
/// Over the **bytes on the wire**, never over a re-encoding of the parsed object. That is one of
/// the two traps `signing.md` §3 names, and it produces a digest that is stable in one
/// implementation and wrong across two.
pub fn body_sha256_hex(body: &[u8]) -> String {
    hex(&Sha256::digest(body))
}

/// The canonical string of `signing.md` §1.2.
///
/// ```text
/// canonical = version ‖ LF ‖ timestamp ‖ LF ‖ delivery_id ‖ LF ‖ body_sha256_hex
/// ```
///
/// Exactly three line feeds, no trailing newline. `delivery_id` is inside it so that a valid
/// signature cannot be lifted onto a different delivery of the same payload, and the body **hash**
/// rather than the body is signed so a receiver can verify before buffering.
pub fn canonical_string(timestamp: i64, delivery_id: &str, body: &[u8]) -> String {
    format!(
        "{SIGNATURE_VERSION}\n{timestamp}\n{delivery_id}\n{}",
        body_sha256_hex(body)
    )
}

fn mac(secret: &str, canonical: &str) -> Vec<u8> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret.as_bytes())
        .expect("HMAC-SHA-256 accepts a key of any length");
    mac.update(canonical.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

/// One `v1=` value: `lowercase_hex(HMAC_SHA256(secret_utf8, canonical_utf8))`.
///
/// The secret is used **exactly as issued, including its `whsec_` prefix** (`signing.md` §1.2).
pub fn signature(secret: &str, timestamp: i64, delivery_id: &str, body: &[u8]) -> String {
    hex(&mac(
        secret,
        &canonical_string(timestamp, delivery_id, body),
    ))
}

/// The whole `Handoff-Signature` header value, signed under every active secret.
///
/// Rotation is an **overlap, not a cutover** (`signing.md` §1.4): while two secrets are active
/// every callback is signed with both and carries both as separate `v1=` elements, so there is no
/// window in which a valid callback fails verification and no flag day for the receiver.
pub fn sign(secrets: &[String], timestamp: i64, delivery_id: &str, body: &[u8]) -> String {
    let canonical = canonical_string(timestamp, delivery_id, body);
    let mut header = format!("t={timestamp}");
    for secret in secrets {
        header.push_str(&format!(",v1={}", hex(&mac(secret, &canonical))));
    }
    header
}

/// Why a callback was refused.
///
/// Typed rather than a bare `false`, because "rejected" and "rejected *for this reason*" are
/// different things at 3 a.m., and because the negative vectors in `signing.md` §1.6 are each a
/// distinct failure that a receiver should be able to tell apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejected {
    /// `Handoff-Signature` could not be parsed, or carried no `v1=` element.
    MalformedHeader,
    /// Outside the freshness window (`signing.md` §1.3 rule 2).
    StaleTimestamp {
        /// How far outside, in seconds.
        skew_secs: i64,
    },
    /// No active secret produced any of the supplied signatures. Covers a tampered body, a
    /// signature lifted from another delivery, and a retired secret — deliberately one code,
    /// because telling an attacker which of the three they achieved is telling them how to iterate.
    NoActiveSecretMatches,
    /// The body's `sequence` disagrees with `Handoff-Sequence` (`signing.md` §1.3 rule 6).
    SequenceMismatch,
}

impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MalformedHeader => f.write_str("the Handoff-Signature header is malformed"),
            Self::StaleTimestamp { skew_secs } => {
                write!(
                    f,
                    "the timestamp is {skew_secs}s outside the freshness window"
                )
            }
            Self::NoActiveSecretMatches => f.write_str("no active secret verifies this signature"),
            Self::SequenceMismatch => {
                f.write_str("Handoff-Sequence disagrees with the body's sequence")
            }
        }
    }
}

/// The `t=` value and the `v1=` values of a `Handoff-Signature` header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSignature {
    /// Unix time in seconds, as the sender stamped it.
    pub timestamp: i64,
    /// Every supplied signature, in header order. More than one means a rotation overlap.
    pub signatures: Vec<String>,
}

/// Parse `t=<unix_seconds>,v1=<hex>[,v1=<hex>]`.
pub fn parse_signature_header(header: &str) -> Result<ParsedSignature, Rejected> {
    let mut timestamp = None;
    let mut signatures = Vec::new();
    for part in header.split(',') {
        let (key, value) = part
            .trim()
            .split_once('=')
            .ok_or(Rejected::MalformedHeader)?;
        match key {
            "t" => {
                timestamp = Some(
                    value
                        .parse::<i64>()
                        .map_err(|_| Rejected::MalformedHeader)?,
                )
            }
            "v1" => signatures.push(value.to_string()),
            // An unknown element is skipped rather than refused: `v2=` alongside `v1=` is exactly
            // how `signing.md` §1.2 intends the algorithm to change without a flag day.
            _ => {}
        }
    }
    match (timestamp, signatures.is_empty()) {
        (Some(timestamp), false) => Ok(ParsedSignature {
            timestamp,
            signatures,
        }),
        _ => Err(Rejected::MalformedHeader),
    }
}

/// Verify a received callback, performing every check of `signing.md` §1.3 in order.
///
/// `raw_body` MUST be the bytes as received, before any parsing or re-serialization. `delivery_id`
/// and the version come from the **headers**, never from values found inside the body.
///
/// Returns the secret index that verified, so a receiver can tell which of an overlapping pair is
/// still in use before retiring one.
pub fn verify(
    raw_body: &[u8],
    signature_header: &str,
    delivery_id: &str,
    active_secrets: &[String],
    now_secs: i64,
    window_secs: i64,
) -> Result<usize, Rejected> {
    let parsed = parse_signature_header(signature_header)?;

    let skew = now_secs - parsed.timestamp;
    if skew.abs() > window_secs {
        return Err(Rejected::StaleTimestamp {
            skew_secs: skew.abs() - window_secs,
        });
    }

    let canonical = canonical_string(parsed.timestamp, delivery_id, raw_body);
    for (index, secret) in active_secrets.iter().enumerate() {
        for supplied in &parsed.signatures {
            let Some(bytes) = unhex(supplied) else {
                continue;
            };
            // Constant-time: `verify_slice` compares under the hood without an early return, so a
            // timing measurement cannot walk the signature one byte at a time.
            let mut candidate = <Hmac<Sha256> as Mac>::new_from_slice(secret.as_bytes())
                .expect("HMAC-SHA-256 accepts a key of any length");
            candidate.update(canonical.as_bytes());
            if candidate.verify_slice(&bytes).is_ok() {
                return Ok(index);
            }
        }
    }
    Err(Rejected::NoActiveSecretMatches)
}

/// Check `Handoff-Sequence` against the body's own `sequence` (`signing.md` §1.3 rule 6).
///
/// The header is a convenience mirror; the body field is authoritative because the body hash covers
/// it. A receiver that uses the header MUST check it and MUST reject a mismatch.
pub fn check_sequence(header_value: &str, body_sequence: u64) -> Result<(), Rejected> {
    match header_value.parse::<u64>() {
        Ok(value) if value == body_sequence => Ok(()),
        _ => Err(Rejected::SequenceMismatch),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every constant below is published in `signing.md` §1.6. The point of repeating them here is
    // that this file is then checked against the specification rather than against itself.
    const BODY: &str = r#"{"created_at":"2026-07-30T14:07:44Z","decision":{"authorization_id":"auth_01K3MB2R4Z8ZC4YRXB2N6VD9FT","outcome":"answered","receipt_id":"rcpt_01K3MB2R4Y8ZC4YRXB2N6VD9FT","source":"human","values":{"decision":"approve","note":"Confirmed with Acme on the phone."}},"id":"sig_01K3MB2R4X8ZC4YRXB2N6VD9FT","request_id":"req_01K3M7QW8ZC4YRXB2N6VD9FTHE","resume_payload":null,"resume_ref":null,"resume_token":"rt_01K3MB2R558ZC4YRXB2N6VD9FT","sequence":1,"type":"answered","waiter_ref":"run:0198f2a1"}"#;
    const SECRET_A: &str = "whsec_2f8a91c4e7b3d05a6c1e9f47b28d3a05";
    const SECRET_B: &str = "whsec_9d41c07be5a2f36819b4d0e7c5a81f62";
    const TIMESTAMP: i64 = 1785592064;
    const DELIVERY: &str = "dlv_01K3MB2R6C8ZC4YRXB2N6VD9FT";
    const OTHER_DELIVERY: &str = "dlv_01K3MB2R6D8ZC4YRXB2N6VD9FT";
    const SIG_A: &str = "cae13126f8dcd1e918376aa373be2757db7281a3e5aaed2d83d716537e03de80";
    const SIG_B: &str = "d86b3740bad654e46c1349614523a476be0eb7d6a30a798b2d475374f36c57eb";

    fn secrets(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_published_body_length_and_digest_reproduce() {
        assert_eq!(BODY.len(), 493, "signing.md §1.6 publishes 493 bytes");
        assert_eq!(
            body_sha256_hex(BODY.as_bytes()),
            "fbd6ec4cacc7cb9c9371d2791f946535e3d391a0594a92b5a3a27dd34f5e94fa"
        );
    }

    #[test]
    fn the_canonical_string_has_exactly_three_line_feeds_and_no_trailing_newline() {
        let canonical = canonical_string(TIMESTAMP, DELIVERY, BODY.as_bytes());
        assert_eq!(canonical.matches('\n').count(), 3);
        assert!(!canonical.ends_with('\n'));
        assert!(!canonical.contains("\r\n"), "LF, never CRLF");
        assert_eq!(
            canonical,
            "1\n1785592064\ndlv_01K3MB2R6C8ZC4YRXB2N6VD9FT\n\
             fbd6ec4cacc7cb9c9371d2791f946535e3d391a0594a92b5a3a27dd34f5e94fa"
        );
    }

    #[test]
    fn both_published_signatures_reproduce_exactly() {
        assert_eq!(
            signature(SECRET_A, TIMESTAMP, DELIVERY, BODY.as_bytes()),
            SIG_A
        );
        assert_eq!(
            signature(SECRET_B, TIMESTAMP, DELIVERY, BODY.as_bytes()),
            SIG_B
        );
    }

    #[test]
    fn a_rotation_overlap_emits_both_and_either_one_verifies() {
        let header = sign(
            &secrets(&[SECRET_A, SECRET_B]),
            TIMESTAMP,
            DELIVERY,
            BODY.as_bytes(),
        );
        assert_eq!(header.matches("v1=").count(), 2, "signing.md §1.4 rule 2");
        assert!(header.contains(SIG_A) && header.contains(SIG_B));

        // The receiver holds one secret or the other, and both work. That is the whole point:
        // there is no window in which a valid callback fails, and no flag day.
        for held in [SECRET_A, SECRET_B] {
            assert_eq!(
                verify(
                    BODY.as_bytes(),
                    &header,
                    DELIVERY,
                    &secrets(&[held]),
                    TIMESTAMP,
                    FRESHNESS_WINDOW_SECS
                ),
                Ok(0),
                "a receiver holding only {held} could not verify during the overlap"
            );
        }
    }

    #[test]
    fn a_retired_secret_no_longer_verifies() {
        // signing.md §1.6, fourth negative vector, and §1.4: a receiver MUST NOT keep a removed
        // secret in its active set, so removing it from the list must be sufficient.
        let header = sign(&secrets(&[SECRET_B]), TIMESTAMP, DELIVERY, BODY.as_bytes());
        assert_eq!(
            verify(
                BODY.as_bytes(),
                &header,
                DELIVERY,
                &secrets(&[SECRET_A]),
                TIMESTAMP,
                FRESHNESS_WINDOW_SECS
            ),
            Err(Rejected::NoActiveSecretMatches)
        );
    }

    #[test]
    fn one_altered_byte_is_rejected() {
        let header = sign(&secrets(&[SECRET_A]), TIMESTAMP, DELIVERY, BODY.as_bytes());
        let tampered = BODY.replace("approve", "reject");
        assert_eq!(
            body_sha256_hex(tampered.as_bytes()),
            "8d1b25a370b6de9d1a504ca1acfe97dc7abe10d4c12b0d33dfaf74f5114eb019",
            "signing.md §1.6 publishes the tampered digest"
        );
        assert_eq!(
            verify(
                tampered.as_bytes(),
                &header,
                DELIVERY,
                &secrets(&[SECRET_A]),
                TIMESTAMP,
                FRESHNESS_WINDOW_SECS
            ),
            Err(Rejected::NoActiveSecretMatches)
        );
    }

    #[test]
    fn a_signature_cannot_be_lifted_onto_another_delivery() {
        let header = sign(&secrets(&[SECRET_A]), TIMESTAMP, DELIVERY, BODY.as_bytes());
        assert_eq!(
            verify(
                BODY.as_bytes(),
                &header,
                OTHER_DELIVERY,
                &secrets(&[SECRET_A]),
                TIMESTAMP,
                FRESHNESS_WINDOW_SECS
            ),
            Err(Rejected::NoActiveSecretMatches),
            "the delivery id is inside the signed string precisely to stop this"
        );
        assert_eq!(
            signature(SECRET_A, TIMESTAMP, OTHER_DELIVERY, BODY.as_bytes()),
            "9a674a003d0507ad13369a6bd82713769116a276ec57f26eb2637b2af00f8e68",
            "signing.md §1.6 publishes the valid signature for that other delivery"
        );
    }

    #[test]
    fn a_stale_timestamp_is_rejected_even_though_the_signature_is_valid() {
        // signing.md §1.6, third negative vector: 301 seconds earlier, signature recomputed and
        // genuinely valid. Freshness is a separate check from authenticity, and this is why.
        let stale = TIMESTAMP - 301;
        let header = sign(&secrets(&[SECRET_A]), stale, DELIVERY, BODY.as_bytes());
        assert_eq!(
            verify(
                BODY.as_bytes(),
                &header,
                DELIVERY,
                &secrets(&[SECRET_A]),
                stale,
                FRESHNESS_WINDOW_SECS
            ),
            Ok(0),
            "at its own timestamp it is a valid signature"
        );
        assert_eq!(
            verify(
                BODY.as_bytes(),
                &header,
                DELIVERY,
                &secrets(&[SECRET_A]),
                TIMESTAMP,
                FRESHNESS_WINDOW_SECS
            ),
            Err(Rejected::StaleTimestamp { skew_secs: 1 })
        );
        // And the boundary itself: 300 seconds is inside the window, 301 is not.
        let edge = sign(
            &secrets(&[SECRET_A]),
            TIMESTAMP - 300,
            DELIVERY,
            BODY.as_bytes(),
        );
        assert!(verify(
            BODY.as_bytes(),
            &edge,
            DELIVERY,
            &secrets(&[SECRET_A]),
            TIMESTAMP,
            FRESHNESS_WINDOW_SECS
        )
        .is_ok());
    }

    #[test]
    fn a_future_timestamp_is_just_as_stale() {
        // `|now − t| > 300`, not `now − t > 300`. A clock ahead of ours is not a licence.
        let header = sign(
            &secrets(&[SECRET_A]),
            TIMESTAMP + 600,
            DELIVERY,
            BODY.as_bytes(),
        );
        assert!(matches!(
            verify(
                BODY.as_bytes(),
                &header,
                DELIVERY,
                &secrets(&[SECRET_A]),
                TIMESTAMP,
                FRESHNESS_WINDOW_SECS
            ),
            Err(Rejected::StaleTimestamp { .. })
        ));
    }

    #[test]
    fn a_malformed_header_is_refused_before_anything_else() {
        for header in [
            "",
            "v1=deadbeef",
            "t=notanumber,v1=deadbeef",
            "t=1785592064",
        ] {
            assert_eq!(
                verify(
                    BODY.as_bytes(),
                    header,
                    DELIVERY,
                    &secrets(&[SECRET_A]),
                    TIMESTAMP,
                    FRESHNESS_WINDOW_SECS
                ),
                Err(Rejected::MalformedHeader),
                "{header:?} parsed"
            );
        }
    }

    #[test]
    fn an_unknown_element_is_skipped_so_the_algorithm_can_change_without_a_flag_day() {
        let header = format!("t={TIMESTAMP},v2=notimplementedyet,v1={SIG_A}");
        assert_eq!(
            verify(
                BODY.as_bytes(),
                &header,
                DELIVERY,
                &secrets(&[SECRET_A]),
                TIMESTAMP,
                FRESHNESS_WINDOW_SECS
            ),
            Ok(0)
        );
    }

    #[test]
    fn the_sequence_header_must_agree_with_the_body() {
        assert_eq!(check_sequence("1", 1), Ok(()));
        assert_eq!(check_sequence("2", 1), Err(Rejected::SequenceMismatch));
        assert_eq!(check_sequence("", 1), Err(Rejected::SequenceMismatch));
    }

    #[test]
    fn hex_round_trips_and_refuses_a_ragged_string() {
        assert_eq!(unhex("00ff10"), Some(vec![0x00, 0xff, 0x10]));
        assert_eq!(unhex("abc"), None);
        assert_eq!(unhex("zz"), None);
    }
}

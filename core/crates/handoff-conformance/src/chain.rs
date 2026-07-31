//! An independent implementation of the receipt chain (§9.4, `signing.md` §2.2, RFC 8785).
//!
//! # Why the suite computes this itself
//!
//! Every other property in this suite is observable over HTTP. Chain integrity looks like it is
//! not — recomputing a digest needs a canonicalizer — and for two rounds the suite asked the
//! deployment to run its own verifier and print the head it arrived at. A hostile review then
//! produced a complete green run against a deployment with no verifier at all: the head digest was
//! handed to the hook as an argument, and the only value that was not handed over was a small
//! integer a hook can brute-force by printing four hundred lines. The lesson generalizes past this
//! one hook: **a claim the suite can compute, the suite must compute.** Asking the party under test
//! for the answer measures the party's willingness to answer.
//!
//! So this module implements the two-step hash of `signing.md` §2.2 and the canonicalization of
//! RFC 8785, and the runner walks the chain over the receipts a case read from the deployment's own
//! HTTP surface. Nothing is narrated; a deployment that has not implemented the chain cannot
//! produce receipts whose stored digests are the hashes this module computes.
//!
//! # Do not "simplify" this by calling `handoff_protocol`
//!
//! This is the warning the next author needs, because the closed loop is shorter and looks
//! cleaner: **this module must never call `handoff_protocol::receipt::verify_chain`,
//! `canonical_json`, or any part of the reference implementation.** If the suite verifies a receipt
//! with the same code that built it, a green result means the implementation agrees with itself and
//! nothing more — and the two release-blocking defects found in review 3 are exactly what that
//! blindness costs. Both were receipts the reference server minted, stored and happily re-verified,
//! and that a published SDK reported as forged: one because two canonicalizers ordered members
//! differently, one because a float reached a digest-covered position. A closed loop cannot see
//! either. `Cargo.toml` refuses the dependency for the same reason; this comment exists because a
//! refusal without a reason gets removed by someone tidying up.
//!
//! The other half of that seam is `scripts/verify-minted-receipts.sh`, which hands the same
//! receipts to the two **published SDKs**. That is the check for *our* divergence; this module is
//! what a third party runs against their own server, where it is genuinely independent code.
//!
//! # Written to the standard, not to our prose
//!
//! Ordering follows **RFC 8785 §3.2.3**: object members are sorted by the **UTF-16 code units** of
//! their names, compared as unsigned 16-bit values. Not code points.
//!
//! This distinction is why this module cites the RFC rather than `signing.md`. Until the correction
//! landed, §3 of `signing.md` said "sorted by code point" while naming RFC 8785 in the same
//! sentence, and its published reference verifier implemented the wrong one — so an implementation
//! written faithfully from that prose would have reproduced the defect this module exists to catch,
//! and C-26 would have passed against a server carrying it. An independent implementation of a
//! wrong specification is not an independent check; it is a second copy of the same error.
//!
//! The two orderings agree for every name below U+D800 and diverge above it: a non-BMP character
//! encodes as a surrogate pair beginning 0xD800, so it sorts *below* every BMP character above
//! U+D7FF while its code point sorts above them. A `document` field carries caller-chosen object
//! keys into `decision.values` and from there into the hashed core, so anyone who can answer a
//! request can reach this — it is not a theoretical corner. C-26 answers with `U+1F600` and
//! `U+FF01` as keys of one object for exactly that reason.
//!
//! # Numbers: the form at rest, not the value
//!
//! §1.4 admits integers within ±(2^53 − 1) in digest-covered content, and requires a digest-covered
//! number to be **stored and served in the form the canonicalizer emits**. `1.0`, `-0.0` and `1e2`
//! are accepted at the door and normalized to `1`, `0` and `100`; `1.5` is refused outright.
//!
//! So this module refuses a float literal in a **served** receipt rather than normalizing it. That
//! is the point: a Server that digests `0` and serves `0.0` has minted a record whose bytes nobody
//! else can hash back to its digest, and silently normalizing here would hide precisely that. The
//! rule is about the form at rest, so the check has to be about the form at rest.

use serde_json::{Map, Value};
use sha2::{Digest as _, Sha256};

/// The predecessor of the first receipt in a tenant: 64 ASCII zeros, prefixed (`signing.md` §2.2).
pub const GENESIS_PREV_DIGEST: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";

/// The largest magnitude a digest-covered integer may carry, ±(2^53 − 1) (§1.4).
const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

/// The head a walk arrived at: what an external verifier records and later re-checks (§9.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    /// Number of receipts in the chain, which is the height of its last link.
    pub height: u64,
    /// The last receipt's `chain.digest`.
    pub head_digest: String,
    /// The tenant the chain belongs to, when the receipts carry one.
    pub org_id: Option<String>,
    /// Every receipt id in the walk, in chain order.
    pub ids: Vec<String>,
}

/// RFC 8785 (JCS) canonical bytes, restricted to what §1.4 admits in a digest-covered object.
///
/// A number that is not an integer literal is an error rather than a serialization, because
/// rendering it is how two implementations produce different bytes for the same receipt.
pub fn canonical_json(value: &Value) -> Result<Vec<u8>, String> {
    let mut out = String::new();
    write_value(value, "", &mut out)?;
    Ok(out.into_bytes())
}

fn write_value(value: &Value, at: &str, out: &mut String) -> Result<(), String> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(n) => write_number(n, at, out)?,
        Value::String(s) => write_string(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(item, &format!("{at}[{i}]"), out)?;
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<(Vec<u16>, &String)> = map
                .keys()
                .map(|k| (k.encode_utf16().collect(), k))
                .collect();
            keys.sort();
            out.push('{');
            for (i, (_, key)) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(key, out);
                out.push(':');
                let child = if at.is_empty() {
                    (*key).clone()
                } else {
                    format!("{at}.{key}")
                };
                write_value(&map[*key], &child, out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

fn write_number(n: &serde_json::Number, at: &str, out: &mut String) -> Result<(), String> {
    let where_ = if at.is_empty() { "<root>" } else { at };
    if let Some(i) = n.as_i64() {
        if i.abs() > MAX_SAFE_INTEGER {
            return Err(format!(
                "`{where_}` is {i}, outside ±(2^53 − 1); §1.4 bounds every digest-covered integer \
                 so that two implementations agree about it"
            ));
        }
        out.push_str(&i.to_string());
        return Ok(());
    }
    if let Some(u) = n.as_u64() {
        if u > MAX_SAFE_INTEGER as u64 {
            return Err(format!(
                "`{where_}` is {u}, outside ±(2^53 − 1); §1.4 bounds every digest-covered integer \
                 so that two implementations agree about it"
            ));
        }
        out.push_str(&u.to_string());
        return Ok(());
    }
    Err(format!(
        "`{where_}` is served as {n}, and §1.4 requires a digest-covered number to be stored and \
         served in the form the canonicalizer emits — a plain integer literal, no decimal point and \
         no exponent. `1.0` and `-0.0` are accepted at the door and normalized to `1` and `0`; what \
         is served here was not. The bytes this Server digested and the bytes it served are \
         therefore different bytes, so its receipt hashes to its stored digest for nobody but \
         itself — which is the one thing a chain cannot survive."
    ))
}

fn write_string(text: &str, out: &mut String) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // RFC 8785 §3.2.2.2: everything else below 0x20 as \u00xx, lowercase hex. Everything at
            // or above it is emitted as itself, so the output is UTF-8 and never \u-escaped ASCII.
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn hex(bytes: impl AsRef<[u8]>) -> String {
    let mut out = String::with_capacity(bytes.as_ref().len() * 2);
    for byte in bytes.as_ref() {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Step 1 of §2.2 — `lowercase_hex(SHA-256(receipt_core))`, the receipt **without** its `chain`
/// member, canonicalized. No `sha256:` prefix; step 2 concatenates it raw.
pub fn core_hash(receipt: &Value) -> Result<String, String> {
    let object = receipt
        .as_object()
        .ok_or("a receipt must be a JSON object".to_string())?;
    let mut core: Map<String, Value> = object.clone();
    // The whole member goes, not only `digest`. `height` and `prev_digest` are inputs to step 2,
    // and hashing them here as well would double-count them — two implementations reading §2.2
    // differently on this point produce different digests for identical content.
    core.remove("chain");
    Ok(hex(Sha256::digest(canonical_json(&Value::Object(core))?)))
}

/// Step 2 of §2.2 — `"sha256:" ‖ lowercase_hex(SHA-256(height ‖ LF ‖ prev_digest ‖ LF ‖ core_hash))`.
pub fn chain_digest(height: u64, prev_digest: &str, core_hash: &str) -> String {
    format!(
        "sha256:{}",
        hex(Sha256::digest(
            format!("{height}\n{prev_digest}\n{core_hash}").as_bytes()
        ))
    )
}

fn member<'a>(receipt: &'a Value, name: &str) -> Option<&'a Value> {
    receipt.get(name).filter(|v| !v.is_null())
}

fn id_of(receipt: &Value, position: usize) -> String {
    member(receipt, "id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| format!("<the receipt at position {position}, which carries no id>"))
}

/// Walk a tenant's receipts, recompute every digest from the receipt's own content, and return the
/// head — or say which link disagreed and how.
///
/// The receipts are the JSON documents the deployment served. They are ordered by `chain.height`
/// here rather than trusted to arrive ordered, and the heights must then be exactly `1..=n`: a gap
/// is an excised receipt, and a duplicate is two receipts claiming one position.
///
/// An empty walk is an **error**, not a vacuous success. "Every receipt verified" over no receipts
/// is the emptiest true statement in this suite, and the one a broken listing produces.
pub fn verify(receipts: &[Value]) -> Result<Head, String> {
    if receipts.is_empty() {
        return Err(
            "the deployment served no receipts, so there is nothing to walk. An empty chain \
             verifies trivially and proves nothing; the case that asked for this walk minted a \
             receipt first, so an empty listing is itself the failure."
                .to_string(),
        );
    }

    let mut links: Vec<(u64, usize, &Value)> = Vec::with_capacity(receipts.len());
    for (index, receipt) in receipts.iter().enumerate() {
        let chain = member(receipt, "chain").ok_or_else(|| {
            format!(
                "{} carries no `chain` member, so it was never sealed into a chain (§9.4)",
                id_of(receipt, index)
            )
        })?;
        let height = chain.get("height").and_then(Value::as_u64).ok_or_else(|| {
            format!(
                "{}: `chain.height` is absent or not a number",
                id_of(receipt, index)
            )
        })?;
        links.push((height, index, receipt));
    }
    links.sort_by_key(|(height, index, _)| (*height, *index));

    for (position, (height, index, receipt)) in links.iter().enumerate() {
        let expected = position as u64 + 1;
        if *height != expected {
            return Err(format!(
                "the walk expected height {expected} and found {height} ({}). §2.2 makes height \
                 1-based and part of the hashed input, so a chain with a gap, a duplicate or a \
                 zero-based count is not the chain the head was exported from.",
                id_of(receipt, *index)
            ));
        }
    }

    let mut org_id: Option<String> = None;
    let mut previous: Option<String> = None;
    let mut ids = Vec::with_capacity(links.len());

    for (height, index, receipt) in &links {
        let id = id_of(receipt, *index);
        let this_org = member(receipt, "org_id")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        match (&org_id, &this_org) {
            (None, some) => org_id = some.clone(),
            (Some(first), Some(other)) if first != other => {
                return Err(format!(
                    "{id} belongs to {other} and the chain so far belongs to {first}; chains are \
                     per-tenant (§9.4), so these are two chains interleaved"
                ))
            }
            _ => {}
        }

        let chain = receipt.get("chain").unwrap_or(&Value::Null);
        let stored_digest = chain
            .get("digest")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{id}: `chain.digest` is absent or not a string"))?;
        let expected_prev = previous
            .clone()
            .unwrap_or_else(|| GENESIS_PREV_DIGEST.to_string());
        // `prev_digest` is stored rather than implied, so that a party holding one receipt can
        // verify it without being handed the chain. Where a deployment omits it on the first
        // receipt the genesis constant still stands there, and the digest is computed over that —
        // what is never permitted is a stored value naming something other than the predecessor.
        if let Some(stored_prev) = chain.get("prev_digest").and_then(Value::as_str) {
            if stored_prev != expected_prev {
                return Err(format!(
                    "{id} at height {height} records prev_digest {stored_prev}, and its \
                     predecessor's digest is {expected_prev}. The link does not name the receipt \
                     before it, so the history this chain describes is not the history it holds."
                ));
            }
        }

        let core = core_hash(receipt).map_err(|why| {
            format!(
                "{id} cannot be canonicalized, so no party can verify it: {why}\n      \
                 This is the failure a receipt exists to prevent — the Server minted a record whose \
                 digest only the Server can reproduce."
            )
        })?;
        let recomputed = chain_digest(*height, &expected_prev, &core);
        if recomputed != stored_digest {
            return Err(format!(
                "{id} at height {height} stores digest {stored_digest}, and recomputing it from \
                 the receipt's own content gives {recomputed}.\n      core_hash={core} \
                 prev_digest={expected_prev}\n      Either the receipt was altered after it was \
                 sealed, or this deployment's canonicalization disagrees with `signing.md` §2.2 — \
                 in which case its receipts verify nowhere but here."
            ));
        }
        previous = Some(stored_digest.to_string());
        ids.push(id);
    }

    Ok(Head {
        height: links.len() as u64,
        head_digest: previous.unwrap_or_default(),
        org_id,
        ids,
    })
}

/// The first receipt of a chain, as §2.2 defines it: height 1, and the genesis predecessor stored
/// rather than implied.
///
/// Both halves are asserted because both are silent when wrong. A chain enumerated from 0 verifies
/// against itself perfectly and seals identical content over different bytes than a 1-based one,
/// since `height` leads the hashed chain input — a verifier reading heights off the receipts rather
/// than off its own loop counter never notices. And a first receipt with no stored `prev_digest`
/// verifies in a walk and cannot be verified alone, which is the case a receipt exists for.
pub fn verify_genesis(receipts: &[Value]) -> Result<(), String> {
    let first = receipts
        .iter()
        .min_by_key(|r| {
            r.get("chain")
                .and_then(|c| c.get("height"))
                .and_then(Value::as_u64)
                .unwrap_or(u64::MAX)
        })
        .ok_or("there are no receipts, so there is no first receipt".to_string())?;
    let id = id_of(first, 0);
    let chain = member(first, "chain").ok_or_else(|| format!("{id} carries no `chain` member"))?;

    match chain.get("height").and_then(Value::as_u64) {
        Some(1) => {}
        other => {
            return Err(format!(
                "the first receipt in this chain ({id}) is at height {}, and §2.2 makes height \
                 1-based: height counts receipts, so an exported head at height n is the nth \
                 receipt. Height 0 is the position of the predecessor the first receipt does not \
                 have, and the 64 ASCII zeros stand there instead.",
                other.map_or("<absent>".to_string(), |h| h.to_string())
            ))
        }
    }
    match chain.get("prev_digest").and_then(Value::as_str) {
        Some(GENESIS_PREV_DIGEST) => Ok(()),
        Some(other) => Err(format!(
            "the first receipt in this chain ({id}) records prev_digest {other}, and §2.2 requires \
             64 ASCII zeros prefixed `sha256:` — it has a predecessor it cannot have"
        )),
        None => Err(format!(
            "the first receipt in this chain ({id}) stores no `prev_digest`. §2.2 requires the \
             genesis value to be stored rather than substituted during the computation, so that a \
             party holding one receipt can verify it without being handed the chain."
        )),
    }
}

/// Verify one receipt on its own, the way a party holding a single receipt has to.
///
/// §2.2 stores `prev_digest` rather than implying it precisely so this is possible: the digest is
/// recomputed from the receipt's own `chain.height`, its own stored `prev_digest`, and the hash of
/// its own content. A receipt that only verifies as part of a walk is one an auditor cannot check
/// without being handed the whole tenant.
pub fn verify_standalone(receipt: &Value) -> Result<(), String> {
    let id = id_of(receipt, 0);
    let chain = member(receipt, "chain")
        .ok_or_else(|| format!("{id} carries no `chain` member, so it was never sealed (§9.4)"))?;
    let height = chain
        .get("height")
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{id}: `chain.height` is absent or not a number"))?;
    let stored_digest = chain
        .get("digest")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{id}: `chain.digest` is absent or not a string"))?;
    let prev = chain.get("prev_digest").and_then(Value::as_str).ok_or_else(|| {
        format!(
            "{id} stores no `chain.prev_digest`, so it cannot be verified on its own. §2.2 requires \
             the predecessor's digest to be stored — 64 ASCII zeros for the first receipt in a \
             tenant — because a receipt an auditor can only check by being handed the entire chain \
             is not the portable record §9 describes."
        )
    })?;

    let core = core_hash(receipt)
        .map_err(|why| format!("{id} cannot be canonicalized, so no party can verify it: {why}"))?;
    let recomputed = chain_digest(height, prev, &core);
    if recomputed != stored_digest {
        return Err(format!(
            "{id} stores digest {stored_digest}, and recomputing it from the receipt alone — \
             height {height}, its own prev_digest, and the hash of its own content — gives \
             {recomputed}"
        ));
    }
    Ok(())
}

/// Rewrite one receipt the way an attacker with storage access would, for the guard that proves the
/// walk above can fail.
///
/// A verification nobody has watched fail is an instrument of unknown sensitivity. This alters a
/// digest-covered member of one receipt and hands the list back so the caller can require the walk
/// to break; it errors rather than returning if the alteration did not change the canonical bytes,
/// because a "tamper" that tampers with nothing is the same vacuity one level down.
pub fn rewrite_one(receipts: &[Value], index: usize) -> Result<Vec<Value>, String> {
    let mut altered: Vec<Value> = receipts.to_vec();
    let target = altered
        .get_mut(index)
        .ok_or_else(|| format!("no receipt at position {index} to rewrite"))?;
    let before = core_hash(target)?;
    let object = target
        .as_object_mut()
        .ok_or("a receipt must be a JSON object".to_string())?;
    // `decided_at` is on every receipt and is covered by the core hash. Where it is somehow absent
    // a member is added instead: what matters is that the hashed bytes move.
    match object.get("decided_at").and_then(Value::as_str) {
        Some(_) => {
            object.insert(
                "decided_at".to_string(),
                Value::String("2000-01-01T00:00:00Z".to_string()),
            );
        }
        None => {
            object.insert("decided_at".to_string(), Value::String("2000".to_string()));
        }
    }
    let after = core_hash(target)?;
    if before == after {
        return Err(format!(
            "rewriting receipt {index} did not change its canonical bytes, so requiring the chain \
             to break afterwards would prove nothing"
        ));
    }
    Ok(altered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The published vectors are the authority `signing.md` §3 names, so this implementation is
    /// checked against them rather than against itself. The file holds the canonical form of a real
    /// receipt core: canonicalizing the parsed document must reproduce it **byte for byte**, which
    /// tests member order, number rendering and string escaping in one assertion.
    #[test]
    fn the_published_receipt_vectors_reproduce() {
        let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let Some(root) = crate::find_repo_root(&manifest) else {
            return;
        };
        let path = root.join("spec/fixtures/signing/receipt-core.json");
        let raw = std::fs::read(&path).expect("the published receipt core is readable");
        let parsed: Value = serde_json::from_slice(&raw).expect("it is JSON");

        let canonical = canonical_json(&parsed).expect("it canonicalizes");
        assert_eq!(
            canonical,
            raw,
            "canonicalizing the published core must reproduce its exact bytes ({} vs {})",
            canonical.len(),
            raw.len()
        );
        assert_eq!(canonical.len(), 1125, "signing.md §2.5 states the length");

        let core = hex(Sha256::digest(&canonical));
        assert_eq!(
            core, "2763f39ef8a61d493106d3db302ec36cae5c024ca3da3a019d483ccc29704ad1",
            "signing.md §2.5 states sha256(receipt_core)"
        );
        assert_eq!(
            chain_digest(4211, GENESIS_PREV_DIGEST, &core),
            "sha256:919f8870391849de4e7b1d5b249ccbaaa7d5a7d3f500f5571c5a92dd0c3909db",
            "signing.md §2.5 states the chain digest at height 4211"
        );
    }

    /// The trap that made a shipped SDK report server-minted receipts as forged. `！` is U+FF01 and
    /// `😀` is U+1F600: by code point the emoji sorts last, by UTF-16 code units it sorts first,
    /// because its surrogate pair begins 0xD83D. RFC 8785 says code units.
    #[test]
    fn members_sort_by_utf16_code_units_not_code_points() {
        let doc = json!({"！": 1, "😀": 2, "a": 0});
        let canonical = String::from_utf8(canonical_json(&doc).unwrap()).unwrap();
        assert_eq!(canonical, "{\"a\":0,\"😀\":2,\"！\":1}");
    }

    #[test]
    fn a_float_literal_in_a_served_receipt_is_refused_wherever_it_sits() {
        // Normalizing quietly here would hide the defect this exists to find: a Server that
        // digests `0` and serves `0.0` has a receipt only it can verify. §1.4's rule is about the
        // form at rest, so this is a check on the form at rest.
        for bad in [json!(-0.0), json!(0.0), json!(1.5), json!(1e21)] {
            let doc = json!({"decision": {"values": {"amount": bad.clone()}}});
            let err = canonical_json(&doc).expect_err("a float is not canonicalizable");
            assert!(err.contains("decision.values.amount"), "{err}");
            assert!(err.contains("in the form the canonicalizer emits"), "{err}");
        }
        // Not a rule against numbers: both ends of the safe range are exactly what a conforming
        // Server serves, including the boundary values C-26 answers with.
        for good in [
            json!(0),
            json!(-1),
            json!(9007199254740991i64),
            json!(-9007199254740991i64),
        ] {
            canonical_json(&json!({"n": good})).expect("an integer literal canonicalizes");
        }
        assert!(canonical_json(&json!({"n": 9007199254740992i64})).is_err());
        assert!(canonical_json(&json!({"n": -9007199254740992i64})).is_err());
    }

    #[test]
    fn control_characters_escape_the_way_rfc_8785_says() {
        let doc = json!({"note": "a\u{8}b\tc\nd\u{1}e\"f\\g"});
        let canonical = String::from_utf8(canonical_json(&doc).unwrap()).unwrap();
        assert_eq!(canonical, r#"{"note":"a\bb\tc\nd\u0001e\"f\\g"}"#);
        // Non-ASCII is emitted as itself, never as a \u escape: JCS output is UTF-8.
        let unicode = String::from_utf8(canonical_json(&json!({"t": "é😀"})).unwrap()).unwrap();
        assert_eq!(unicode, "{\"t\":\"é😀\"}");
    }

    fn sealed(chain: Vec<Value>) -> Vec<Value> {
        let mut out = Vec::new();
        let mut prev = GENESIS_PREV_DIGEST.to_string();
        for (i, mut receipt) in chain.into_iter().enumerate() {
            let height = i as u64 + 1;
            let core = core_hash(&receipt).unwrap();
            let digest = chain_digest(height, &prev, &core);
            receipt.as_object_mut().unwrap().insert(
                "chain".to_string(),
                json!({"height": height, "prev_digest": prev, "digest": digest}),
            );
            prev = digest;
            out.push(receipt);
        }
        out
    }

    fn three() -> Vec<Value> {
        sealed(vec![
            json!({"id": "rk_1", "org_id": "org_a", "decided_at": "2026-07-30T10:00:00Z"}),
            json!({"id": "rk_2", "org_id": "org_a", "decided_at": "2026-07-30T11:00:00Z"}),
            json!({"id": "rk_3", "org_id": "org_a", "decided_at": "2026-07-30T12:00:00Z"}),
        ])
    }

    #[test]
    fn a_well_formed_chain_verifies_and_reports_its_head() {
        let head = verify(&three()).expect("it verifies");
        assert_eq!(head.height, 3);
        assert_eq!(head.ids, vec!["rk_1", "rk_2", "rk_3"]);
        assert!(head.head_digest.starts_with("sha256:"));
    }

    #[test]
    fn every_rewritten_receipt_breaks_the_walk() {
        // The guard the runner applies live, asserted here per entry rather than once for the
        // loop: a guard covering a loop is not a guard covering its iterations.
        let chain = three();
        for index in 0..chain.len() {
            let altered = rewrite_one(&chain, index).expect("the rewrite moves the bytes");
            let err = verify(&altered).expect_err("a rewritten history must not verify");
            assert!(err.contains("recomputing it"), "{err}");
        }
    }

    #[test]
    fn the_genesis_link_is_pinned_at_height_one_with_the_stored_zeros() {
        assert!(verify_genesis(&three()).is_ok());

        // A chain enumerated from 0. Every digest in it agrees with every other, so `verify` has
        // nothing to complain about — which is exactly why this is asserted separately.
        let mut zero_based = three();
        for receipt in &mut zero_based {
            let height = receipt["chain"]["height"].as_u64().unwrap();
            receipt["chain"]["height"] = json!(height - 1);
        }
        let err = verify_genesis(&zero_based).expect_err("height is 1-based");
        assert!(err.contains("1-based"), "{err}");

        // A first receipt that implies its predecessor rather than storing it.
        let mut implied = three();
        implied[0]["chain"]
            .as_object_mut()
            .unwrap()
            .remove("prev_digest");
        let err = verify_genesis(&implied).expect_err("the genesis value is stored");
        assert!(err.contains("stores no `prev_digest`"), "{err}");

        // A first receipt claiming a predecessor it cannot have.
        let mut inherited = three();
        inherited[0]["chain"]["prev_digest"] = json!("sha256:{}".replace("{}", &"a".repeat(64)));
        assert!(verify_genesis(&inherited).is_err());
    }

    #[test]
    fn an_empty_walk_is_a_failure_not_a_pass() {
        assert!(verify(&[]).is_err());
    }

    #[test]
    fn an_excised_receipt_is_found_by_its_height() {
        let mut chain = three();
        chain.remove(1);
        let err = verify(&chain).expect_err("a gap is not a chain");
        assert!(err.contains("expected height 2"), "{err}");
    }

    #[test]
    fn a_prev_digest_naming_the_wrong_receipt_is_refused() {
        let mut chain = three();
        chain[2]["chain"]["prev_digest"] = json!(GENESIS_PREV_DIGEST);
        let err = verify(&chain).expect_err("a re-linked chain is not a chain");
        assert!(err.contains("does not name the receipt before it"), "{err}");
    }

    #[test]
    fn two_tenants_interleaved_are_not_one_chain() {
        let mut chain = three();
        chain[1]["org_id"] = json!("org_b");
        assert!(verify(&chain).is_err());
    }

    #[test]
    fn a_receipt_nobody_can_canonicalize_fails_the_walk() {
        // The N-2 shape: a float in a digest-covered position. The server that minted it can
        // reproduce its own digest; nobody else can, which is the whole failure.
        let mut chain = three();
        chain[0]["decision"] = json!({"values": {"amount": -0.0}});
        let err = verify(&chain).expect_err("a receipt nobody can verify does not verify");
        assert!(err.contains("cannot be canonicalized"), "{err}");
    }
}

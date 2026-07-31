//! A receipt minted by this server, verified by an implementation that shares no code with it.
//!
//! The previous chain construction passed every test while computing a digest no conforming
//! verifier could reproduce, and it passed because the check and the thing checked were one
//! implementation: C-15 verified the chain with the same Rust that produced it, so any
//! self-consistent construction was accepted. Both published SDKs implement `signing.md` §2.2 and
//! returned false for every real receipt.
//!
//! So this file deliberately **does not call** `verify_chain`, `chain_digest`, `core_hash`, or
//! `canonical_form`. §2.2 is written out below from the specification's own words, over the JSON the
//! API actually returned — which is the position a third party is in. An auditor has the receipt and
//! the spec, and nothing else.

use super::harness::*;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

/// RFC 8785 canonicalization, enough for the value types a receipt contains.
///
/// `serde_json::Value` holds objects in a `BTreeMap`, so members come out sorted by byte order, and
/// for the ASCII keys this protocol uses that is the same order as JCS's sort by UTF-16 code unit.
/// `to_string` emits no insignificant whitespace. The protocol constrains numbers to the band where
/// every serializer agrees (§1.4), which is what makes this short enough to be worth writing twice.
fn canonical_bytes(value: &Value) -> Vec<u8> {
    serde_json::to_string(value)
        .expect("a receipt is serializable")
        .into_bytes()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// §2.2 step 1 — the receipt core: the receipt object **excluding** its `chain` member.
fn core_hash(receipt: &Value) -> String {
    let mut core: Map<String, Value> = receipt.as_object().expect("a receipt is an object").clone();
    core.remove("chain");
    hex(&Sha256::digest(canonical_bytes(&Value::Object(core))))
}

/// §2.2 step 2 — `height ‖ LF ‖ prev_digest ‖ LF ‖ core_hash`, hashed and prefixed.
fn chain_digest(height: u64, prev_digest: &str, core_hash: &str) -> String {
    let input = format!("{height}\n{prev_digest}\n{core_hash}");
    format!("sha256:{}", hex(&Sha256::digest(input.as_bytes())))
}

#[tokio::test]
async fn a_receipt_from_this_server_verifies_under_an_independent_reading_of_the_spec() {
    let deployment = Deployment::start("chain").await;

    // Three receipts, so the chain is linked and not just a single genesis entry.
    let mut receipts = Vec::new();
    for n in 0..3 {
        let (status, raised) = post(
            &deployment.base,
            "/requests",
            MACHINE_A,
            &format!("chain-raise-{n}"),
            raise_body(&format!("run:chain-{n}"), "Refund $2,400 to Acme Corp?"),
        )
        .await;
        assert_eq!(status, 201, "{raised}");
        let id = raised["id"].as_str().unwrap().to_string();

        let (status, answered) = post(
            &deployment.base,
            &format!("/requests/{id}/answer"),
            EDITOR_A,
            &format!("chain-answer-{n}"),
            serde_json::json!({"values": {"decision": "approve"}}),
        )
        .await;
        assert_eq!(status, 200, "{answered}");

        // Read it back through the API. This, and not an internal value, is what an auditor holds.
        let (status, receipt) = get(
            &deployment.base,
            &format!("/requests/{id}/receipt"),
            MACHINE_A,
        )
        .await;
        assert_eq!(status, 200, "{receipt}");
        receipts.push(receipt);
    }

    let genesis = format!("sha256:{}", "0".repeat(64));
    let mut previous = genesis.clone();

    for (index, receipt) in receipts.iter().enumerate() {
        let chain = &receipt["chain"];
        let height = chain["height"].as_u64().expect("a height");
        let stored_prev = chain["prev_digest"].as_str().unwrap_or_else(|| {
            panic!(
                "receipt {index} carries no prev_digest string; a party holding one receipt cannot \
                 verify it without that field"
            )
        });
        let stored_digest = chain["digest"].as_str().expect("a digest");

        assert_eq!(
            stored_prev, previous,
            "receipt {index} must name the previous receipt, and the first must name the 64-zero \
             genesis predecessor (§2.2)"
        );

        let expected = chain_digest(height, stored_prev, &core_hash(receipt));
        assert_eq!(
            expected, stored_digest,
            "receipt {index} does not verify under §2.2 as written out from the specification. \
             The server and a conforming verifier disagree about this receipt's digest, so nobody \
             outside this process can check it."
        );

        previous = stored_digest.to_string();
    }

    // And the property the whole construction exists for: altering any historical receipt must
    // invalidate it. Done here on the JSON an auditor holds, not in the store.
    let mut tampered = receipts[0].clone();
    tampered["decision"]["values"]["decision"] = serde_json::json!("reject");
    let recomputed = chain_digest(
        tampered["chain"]["height"].as_u64().unwrap(),
        tampered["chain"]["prev_digest"].as_str().unwrap(),
        &core_hash(&tampered),
    );
    assert_ne!(
        recomputed,
        tampered["chain"]["digest"].as_str().unwrap(),
        "rewriting what was decided must break the digest, or the chain is decoration"
    );
}

/// The genesis predecessor is stored, not implied.
///
/// A verifier handed **one** receipt has no chain to walk and must read `prev_digest` from the
/// record. `openapi.yaml` describes it as null for the first receipt in an org; §2.2 defines the
/// first receipt's predecessor as 64 ASCII zeros, and only the second lets a single receipt be
/// verified on its own.
#[tokio::test]
async fn the_first_receipt_in_a_tenant_names_the_genesis_predecessor() {
    let deployment = Deployment::start("genesis").await;

    let (status, raised) = post(
        &deployment.base,
        "/requests",
        MACHINE_A,
        "genesis-raise",
        raise_body(
            "run:genesis",
            "Approve the first thing this tenant ever decided",
        ),
    )
    .await;
    assert_eq!(status, 201, "{raised}");
    let id = raised["id"].as_str().unwrap().to_string();

    let (status, answered) = post(
        &deployment.base,
        &format!("/requests/{id}/answer"),
        EDITOR_A,
        "genesis-answer",
        serde_json::json!({"values": {"decision": "approve"}}),
    )
    .await;
    assert_eq!(status, 200, "{answered}");

    let (_, receipt) = get(
        &deployment.base,
        &format!("/requests/{id}/receipt"),
        MACHINE_A,
    )
    .await;
    assert_eq!(
        receipt["chain"]["prev_digest"].as_str(),
        Some(format!("sha256:{}", "0".repeat(64)).as_str())
    );
    assert_eq!(receipt["chain"]["height"].as_u64(), Some(1));

    let expected = chain_digest(
        1,
        receipt["chain"]["prev_digest"].as_str().unwrap(),
        &core_hash(&receipt),
    );
    assert_eq!(expected, receipt["chain"]["digest"].as_str().unwrap());
}

/// A receipt whose decision carries the numeric boundaries, verified out of process.
///
/// This is the hole R-1's fix left open: the chain construction was right, and a receipt whose
/// `decision.values` held `1.5` still could not be canonicalized by either published SDK — so the
/// receipt a real answer produced was unverifiable by every client we ship. Digest-covered content
/// now carries integers only, and the boundary values are exercised here so that cannot reopen
/// quietly.
#[tokio::test]
async fn a_receipt_carrying_boundary_integers_verifies_out_of_process() {
    let deployment = Deployment::start("chainnum").await;

    let (status, raised) = post(
        &deployment.base,
        "/requests",
        MACHINE_A,
        "chainnum-raise",
        serde_json::json!({
            "waiter_ref": "run:chain-numbers",
            "prompt": {"title": "Record the reconciliation figures"},
            "requires": {
                "v": 1,
                "answer": {"fields": [
                    {"name": "at_zero", "type": "number", "required": true},
                    {"name": "at_max_safe_integer", "type": "number", "required": true},
                    {"name": "negative", "type": "number", "required": true}
                ]},
                "capabilities": [],
                "authority": {"min_role": "editor", "auth_strength": "session"}
            }
        }),
    )
    .await;
    assert_eq!(status, 201, "{raised}");
    let id = raised["id"].as_str().unwrap().to_string();

    // A non-integer is refused before any of this, naming the field.
    let (status, refused) = post(
        &deployment.base,
        &format!("/requests/{id}/answer"),
        EDITOR_A,
        "chainnum-fraction",
        serde_json::json!({"values": {
            "at_zero": 0, "at_max_safe_integer": 9_007_199_254_740_991i64, "negative": 1.5
        }}),
    )
    .await;
    assert_eq!(status, 422, "{refused}");
    assert_eq!(refused["error"]["code"], "answer_validation_failed");
    assert_eq!(refused["error"]["fields"][0]["name"], "negative");

    let (status, answered) = post(
        &deployment.base,
        &format!("/requests/{id}/answer"),
        EDITOR_A,
        "chainnum-answer",
        serde_json::json!({"values": {
            "at_zero": 0,
            "at_max_safe_integer": 9_007_199_254_740_991i64,
            "negative": -9_007_199_254_740_991i64
        }}),
    )
    .await;
    assert_eq!(status, 200, "{answered}");

    let (_, receipt) = get(
        &deployment.base,
        &format!("/requests/{id}/receipt"),
        MACHINE_A,
    )
    .await;

    // The values survived exactly, and the receipt verifies under §2.2 read from the spec.
    let values = &receipt["decision"]["values"];
    assert_eq!(values["at_zero"], serde_json::json!(0));
    assert_eq!(
        values["at_max_safe_integer"],
        serde_json::json!(9_007_199_254_740_991i64)
    );
    assert_eq!(
        values["negative"],
        serde_json::json!(-9_007_199_254_740_991i64)
    );

    let expected = chain_digest(
        receipt["chain"]["height"].as_u64().unwrap(),
        receipt["chain"]["prev_digest"].as_str().unwrap(),
        &core_hash(&receipt),
    );
    assert_eq!(
        expected,
        receipt["chain"]["digest"].as_str().unwrap(),
        "a receipt carrying the numeric boundaries must verify out of process, or a person \
         answering a number field produces a record nobody can check"
    );
}

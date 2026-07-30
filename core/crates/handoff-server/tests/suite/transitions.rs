//! The transitions a person makes that leave a request `pending`, and the codes they must produce.
//!
//! §6.2 R12, R13, and R14 share one property and it is the reason they are numbered: each is
//! something a **person** does to a `pending` request that leaves it `pending`, and **none of them
//! may signal the waiter**. A runtime must not be able to observe that an intermediate step, a
//! delegation, or a partial endorsement occurred; it learns only the single outcome. An
//! implementation that wakes the waiter on any of the three has turned one intervention into
//! several, and breaks I1 as soon as a receipt is minted for each.
//!
//! None of this is covered by a conformance case, which is exactly why it is tested here: an
//! uncased normative requirement is the one that quietly rots.

use super::harness::*;
use sqlx::Row;

/// R12 — a progressive-disclosure step.
#[tokio::test]
async fn a_partial_answer_records_a_step_and_never_wakes_the_waiter() {
    let deployment = Deployment::start("r12", 18104).await;
    let waiter = "run:r12-progressive";

    let (status, raised) = post(
        &deployment.base,
        "/requests",
        MACHINE_A,
        "r12-raise",
        serde_json::json!({
            "waiter_ref": waiter,
            "liveness": "durable",
            "prompt": {"title": "Sign in to app.example.com"},
            "requires": {
                "v": 1,
                "answer": {"fields": [
                    {"name": "email", "type": "text", "required": true},
                    {"name": "code", "type": "text", "required": true}
                ]},
                "capabilities": [],
                "authority": {"min_role": "editor", "auth_strength": "session"}
            }
        }),
    )
    .await;
    assert_eq!(status, 201, "{raised}");
    let id = raised["id"].as_str().unwrap().to_string();

    // The first rung of the ladder: an email now, the code later. One request, not two asks.
    let (status, stepped) = post(
        &deployment.base,
        &format!("/requests/{id}/answer"),
        EDITOR_A,
        "r12-step",
        serde_json::json!({"values": {"email": "dana@example.com"}, "partial": true}),
    )
    .await;
    assert_eq!(status, 200, "a partial step is accepted: {stepped}");
    assert!(
        stepped["receipt"].is_null(),
        "§2 — nothing but an outcome mints a receipt, and a step is not an outcome"
    );

    let (_, request) = get(&deployment.base, &format!("/requests/{id}"), MACHINE_A).await;
    assert_eq!(
        request["state"], "pending",
        "R12 leaves the request pending"
    );
    assert!(request["receipt"].is_null());

    // The load-bearing assertion. The runtime must not be able to tell that a step happened.
    let (status, signals) = get(
        &deployment.base,
        &format!("/waiters/{}/signals", urlencode(waiter)),
        MACHINE_A,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        signals["data"].as_array().map(Vec::len),
        Some(0),
        "R12 MUST NOT signal the waiter: a runtime that learns of an intermediate step has had one \
         intervention turned into several"
    );

    let pool = deployment.pool().await;
    assert!(
        events_for(&pool, &id)
            .await
            .contains(&"request.step_recorded".to_string()),
        "R12 emits request.step_recorded, not request.amended: an amendment is a third party \
         improving the wording, and this is the answerer at work"
    );

    // §5.5 rule 4: the attempt clock is re-armed **fresh**, never inheriting the tail of the
    // previous step's window.
    assert!(
        !request["attempt_expires_at"].is_null(),
        "the next step gets a whole attempt window"
    );
}

/// R13 — a delegation.
#[tokio::test]
async fn a_delegation_is_recorded_mints_deliveries_and_never_wakes_the_waiter() {
    let deployment = Deployment::start("r13", 18105).await;
    let waiter = "run:r13-delegate";

    let (status, raised) = post(
        &deployment.base,
        "/requests",
        MACHINE_A,
        "r13-raise",
        raise_body(waiter, "Approve the $180 overage?"),
    )
    .await;
    assert_eq!(status, 201, "{raised}");
    let id = raised["id"].as_str().unwrap().to_string();
    let deliveries_before = raised["deliveries"].as_array().map(Vec::len).unwrap_or(0);

    let (status, delegated) = post(
        &deployment.base,
        &format!("/requests/{id}/answer"),
        EDITOR_A,
        "r13-delegate",
        serde_json::json!({
            "values": {},
            "disposition": "delegate",
            "delegate_to": {"kind": "role", "value": "admin"},
            "note": "Above my limit."
        }),
    )
    .await;
    assert_eq!(status, 200, "{delegated}");
    assert!(
        delegated["receipt"].is_null(),
        "§6.6 — a Server MUST NOT treat a delegation as a decision"
    );

    let (_, request) = get(&deployment.base, &format!("/requests/{id}"), MACHINE_A).await;
    assert_eq!(
        request["state"], "pending",
        "it stays pending until somebody decides"
    );
    assert!(request["receipt"].is_null());

    let (status, signals) = get(
        &deployment.base,
        &format!("/waiters/{}/signals", urlencode(waiter)),
        MACHINE_A,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        signals["data"].as_array().map(Vec::len),
        Some(0),
        "R13 MUST NOT signal the waiter"
    );

    // §6.6: the decision is handed on, so somebody new has to be reached.
    let (_, deliveries) = get(
        &deployment.base,
        &format!("/requests/{id}/deliveries"),
        MACHINE_A,
    )
    .await;
    assert!(
        deliveries["data"].as_array().map(Vec::len).unwrap_or(0) > deliveries_before,
        "a delegation mints deliveries to the new target, or nobody is asked"
    );

    let pool = deployment.pool().await;
    assert!(
        events_for(&pool, &id)
            .await
            .contains(&"request.disposition_recorded".to_string()),
        "R13 emits request.disposition_recorded"
    );

    // The disposition, the actor, and the target are on the record (§6.2 R13).
    let row = sqlx::query(
        "select disposition, principal_ref, delegate_kind, delegate_value \
         from handoff_request_dispositions where tenant_ref = $1 and request_id = $2",
    )
    .bind(ORG_A)
    .bind(&id)
    .fetch_one(&pool)
    .await
    .expect("the delegation was recorded");
    assert_eq!(row.get::<String, _>("disposition"), "delegate");
    assert_eq!(row.get::<String, _>("delegate_kind"), "role");
    assert_eq!(row.get::<String, _>("delegate_value"), "admin");
    assert!(
        row.get::<String, _>("principal_ref").starts_with("usr_"),
        "the person who delegated is named"
    );

    // And an authorized principal can still decide it afterwards — the delegation blocked nothing.
    let (status, answered) = post(
        &deployment.base,
        &format!("/requests/{id}/answer"),
        EDITOR_A,
        "r13-decide",
        serde_json::json!({"values": {"decision": "approve"}}),
    )
    .await;
    assert_eq!(status, 200, "{answered}");
    assert_eq!(answered["request"]["state"], "answered");
    assert!(
        !answered["receipt"].is_null(),
        "I1 — one intervention, one receipt, however many people it passed through"
    );
}

/// An authorization that was real, is on the record, and is no longer spendable.
#[tokio::test]
async fn redeeming_an_expired_authorization_says_expired_and_not_spent() {
    let deployment = Deployment::start("authexp", 18106).await;

    let (status, raised) = post(
        &deployment.base,
        "/requests",
        MACHINE_A,
        "authexp-raise",
        raise_body("run:auth-expired", "Refund $2,400 to Acme Corp?"),
    )
    .await;
    assert_eq!(status, 201, "{raised}");
    let id = raised["id"].as_str().unwrap().to_string();

    let (status, answered) = post(
        &deployment.base,
        &format!("/requests/{id}/answer"),
        EDITOR_A,
        "authexp-answer",
        serde_json::json!({"values": {"decision": "approve"}}),
    )
    .await;
    assert_eq!(status, 200, "{answered}");
    let authorization = answered["authorization"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Move its expiry into the past. An authorization is not a receipt: it has no immutability
    // trigger, because `expires_at` is exactly the kind of thing an operator may need to move.
    let pool = deployment.pool().await;
    sqlx::query(
        "update handoff_authorizations set expires_at = now() - interval '1 hour' where id = $1",
    )
    .bind(&authorization)
    .execute(&pool)
    .await
    .expect("expire the authorization");

    let (status, error) = post(
        &deployment.base,
        &format!("/authorizations/{authorization}/redeem"),
        MACHINE_A,
        "authexp-redeem",
        serde_json::json!({"effect_key": "refund:inv-8821"}),
    )
    .await;
    assert_eq!(status, 409, "{error}");
    assert_eq!(
        error["error"]["code"], "authorization_expired",
        "the three codes this would otherwise collapse into each assert something false: spent \
         says the decision was used, not_found says it never existed, and invalid_request says the \
         caller sent something malformed"
    );

    // The decision itself is untouched: it happened, and the record still says so.
    let (status, still_there) = get(
        &deployment.base,
        &format!("/authorizations/{authorization}"),
        MACHINE_A,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(still_there["state"], "expired");
    assert_eq!(still_there["grants"]["decision"], "approve");
}

/// §1.4 — a duration whose length depends on when you start it is refused, never approximated.
#[tokio::test]
async fn a_ttl_measured_in_months_is_rejected_rather_than_guessed() {
    let deployment = Deployment::start("durations", 18107).await;

    let mut body = raise_body("run:durations", "A request with an unfixed deadline");
    body["ttl"] = serde_json::json!("P1M");
    let (status, error) = post(&deployment.base, "/requests", MACHINE_A, "dur-month", body).await;
    assert_eq!(
        status, 400,
        "P1M is 28 to 31 days depending on when the clock starts, so it is not a deadline: {error}"
    );

    // Weeks are exactly seven days, so they are permitted.
    let mut body = raise_body("run:durations-week", "A request with a fixed deadline");
    body["ttl"] = serde_json::json!("P2W");
    let (status, raised) = post(&deployment.base, "/requests", MACHINE_A, "dur-week", body).await;
    assert_eq!(status, 201, "{raised}");
    assert!(
        !raised["expires_at"].is_null(),
        "a fixed-length TTL produces a deadline"
    );
}

async fn events_for(pool: &sqlx::PgPool, request_id: &str) -> Vec<String> {
    sqlx::query_scalar("select type from handoff_events where request_id = $1 order by id")
        .bind(request_id)
        .fetch_all(pool)
        .await
        .expect("read the event record")
}

fn urlencode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// An `Idempotency-Key` is retry safety for one call against **one object**.
///
/// §3.1 makes the key mean "a repeat returns the identical request *and its stored response*". If
/// the object is missing from the key's scope, answering request B with the key already used on
/// request A returns A's receipt and A's authorization with `200`, and B is never answered — the
/// caller is handed a decision about a different thing and told it succeeded. The agent then spends
/// A's authorization on an effect named for B.
///
/// `README.md` states the property this protects: "The answer is bound to the specific thing it was
/// shown against. It does not generalize to the next call, the next run, or a similar-looking
/// request."
#[tokio::test]
async fn one_key_reused_on_a_different_request_does_not_replay_the_first_answer() {
    let deployment = Deployment::start("idemobj", 18108).await;

    let mut ids = Vec::new();
    for n in ["one", "two"] {
        let (status, raised) = post(
            &deployment.base,
            "/requests",
            MACHINE_A,
            &format!("idemobj-raise-{n}"),
            raise_body(&format!("run:idemobj-{n}"), "Refund $2,400 to Acme Corp?"),
        )
        .await;
        assert_eq!(status, 201, "{raised}");
        ids.push(raised["id"].as_str().unwrap().to_string());
    }
    let (first, second) = (&ids[0], &ids[1]);
    assert_ne!(first, second);

    // Identical key, identical body — the only thing that differs is the request being answered.
    let answer = serde_json::json!({"values": {"decision": "approve"}});

    let (status, one) = post(
        &deployment.base,
        &format!("/requests/{first}/answer"),
        EDITOR_A,
        "SHARED-ACROSS-OBJECTS",
        answer.clone(),
    )
    .await;
    assert_eq!(status, 200, "{one}");
    let first_receipt = one["receipt"]["id"].as_str().unwrap().to_string();

    let (status, two) = post(
        &deployment.base,
        &format!("/requests/{second}/answer"),
        EDITOR_A,
        "SHARED-ACROSS-OBJECTS",
        answer,
    )
    .await;
    assert_eq!(status, 200, "{two}");
    assert_eq!(
        two["request"]["id"].as_str(),
        Some(second.as_str()),
        "the second answer must settle the second request, not replay the first"
    );
    assert_ne!(
        two["receipt"]["id"].as_str().unwrap(),
        first_receipt,
        "each request has its own receipt (I1); replaying the first one here would let an agent \
         spend the first decision on an effect named for the second"
    );

    // The assertion that actually catches the bug: the second request really did settle.
    let (_, settled) = get(&deployment.base, &format!("/requests/{second}"), MACHINE_A).await;
    assert_eq!(
        settled["state"], "answered",
        "before the object was in the idempotency scope this request stayed `pending` while the \
         caller was told the answer had landed"
    );

    // And the retry that the key does exist for still works, on the object it was used against.
    let (status, retried) = post(
        &deployment.base,
        &format!("/requests/{first}/answer"),
        EDITOR_A,
        "SHARED-ACROSS-OBJECTS",
        serde_json::json!({"values": {"decision": "approve"}}),
    )
    .await;
    assert_eq!(status, 200, "{retried}");
    assert_eq!(
        retried["receipt"]["id"].as_str().unwrap(),
        first_receipt,
        "a genuine retry of the same call against the same object still replays (§6.7 rule 3)"
    );
}

/// The same, for a withdrawal — where the consequence is an operator believing a request is off.
#[tokio::test]
async fn one_key_reused_on_a_different_cancel_does_not_replay_the_first() {
    let deployment = Deployment::start("idemcancel", 18109).await;

    let mut ids = Vec::new();
    for n in ["one", "two"] {
        let (status, raised) = post(
            &deployment.base,
            "/requests",
            MACHINE_A,
            &format!("idemcancel-raise-{n}"),
            raise_body(&format!("run:idemcancel-{n}"), "Send the contract?"),
        )
        .await;
        assert_eq!(status, 201, "{raised}");
        ids.push(raised["id"].as_str().unwrap().to_string());
    }

    let reason = serde_json::json!({"reason": "no longer needed"});
    for id in &ids {
        let (status, cancelled) = post(
            &deployment.base,
            &format!("/requests/{id}/cancel"),
            MACHINE_A,
            "SHARED-CANCEL-KEY",
            reason.clone(),
        )
        .await;
        assert_eq!(status, 200, "{cancelled}");
        assert_eq!(
            cancelled["id"].as_str(),
            Some(id.as_str()),
            "each cancel must return the request it cancelled"
        );
    }

    for id in &ids {
        let (_, request) = get(&deployment.base, &format!("/requests/{id}"), MACHINE_A).await;
        assert_eq!(
            request["state"], "cancelled",
            "an operator who called this request off must find it called off, not still live"
        );
    }
}

/// §14 requires a Server that stores `resume_payload` to encrypt it at rest.
///
/// This deployment implements no encryption at rest, so it refuses the field instead of keeping a
/// runtime's private state in the clear. §14 sanctions exactly that: a Level 1 Server MUST accept
/// and ignore these fields, or reject them with `400 invalid_request`.
///
/// The second half is the part worth having a test for at all: `GET /meta` must report the level
/// this build actually implements. A hardcoded level beside a code path that stored payloads
/// anyway is how a Server advertises something nobody measured.
#[tokio::test]
async fn continuation_state_is_refused_rather_than_stored_unprotected() {
    let deployment = Deployment::start("continuation", 18110).await;

    let (status, meta) = get(&deployment.base, "/meta", MACHINE_A).await;
    assert_eq!(status, 200);
    assert_eq!(
        meta["conformance_level"], 1,
        "§1.2 — a Server MUST NOT advertise Level 2 unless it passes C-17"
    );
    assert_eq!(
        meta["extensions"].as_array().map(Vec::len),
        Some(0),
        "the continuation extension is not implemented, so it is not advertised"
    );

    let mut body = raise_body("run:continuation", "Approve the batch?");
    body["resume_payload"] = serde_json::json!("Q09ORk9STUFOQ0UtT1BBUVVFLUJMT0I=");
    let (status, refused) = post(
        &deployment.base,
        "/requests",
        MACHINE_A,
        "continuation-payload",
        body,
    )
    .await;
    assert_eq!(status, 400, "{refused}");
    assert_eq!(refused["error"]["code"], "invalid_request");

    // Nothing was created: §19's fail-closed rule applies here too.
    let (_, listed) = get(
        &deployment.base,
        "/requests?waiter_ref=run%3Acontinuation",
        MACHINE_A,
    )
    .await;
    assert_eq!(
        listed["data"].as_array().map(Vec::len),
        Some(0),
        "a refused raise creates nothing"
    );

    // And the store holds no payload for anyone, which is the property the refusal buys.
    let pool = deployment.pool().await;
    let stored: i64 = sqlx::query_scalar(
        "select count(*) from handoff_requests where resume_payload is not null",
    )
    .fetch_one(&pool)
    .await
    .expect("count stored payloads");
    assert_eq!(
        stored, 0,
        "no continuation payload is persisted in the clear"
    );

    // `resume_ref` is a pointer the runtime owns. It carries no secret and §14 places no
    // encryption requirement on it, so it is still accepted.
    let mut body = raise_body("run:continuation-ref", "Approve the batch?");
    body["resume_ref"] = serde_json::json!("s3://checkpoints/run/step-14.msgpack");
    let (status, accepted) = post(
        &deployment.base,
        "/requests",
        MACHINE_A,
        "continuation-ref",
        body,
    )
    .await;
    assert_eq!(status, 201, "{accepted}");
}

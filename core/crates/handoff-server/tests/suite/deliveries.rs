//! Grades that arrive after the send returned, and the ones nobody is allowed to claim.
//!
//! A synchronous `deliver` can only report what the transport said at the moment it took the
//! message, which for a real channel is `dispatched` and nothing more. Everything that makes the
//! grade ladder worth having — the provider's delivery receipt, the person opening the surface —
//! arrives later, through an ingress the deployment owns. Without this route every asynchronous
//! channel stays at `dispatched` forever and §7.2 is decorative.
//!
//! The interesting assertions are the refusals. `acted` means the person answered **through this
//! delivery**, which is established by an answer landing and by nothing else, so a channel must not
//! be able to assert it: accepting it here would let a caller write "they decided" onto a delivery
//! with no decision behind it.

use crate::harness::{get, post, raise_body, Deployment, MACHINE_A};

/// POST that returns the raw status and body without an idempotency key.
async fn post_plain(
    base: &str,
    path: &str,
    token: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let response = reqwest::Client::new()
        .post(format!("{base}{path}"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&body)
        .send()
        .await
        .expect("the server answers");
    let status = response.status().as_u16();
    let json = response.json().await.unwrap_or(serde_json::Value::Null);
    (status, json)
}

#[tokio::test]
async fn a_channel_reports_later_evidence_but_never_claims_the_person_decided() {
    let deployment = Deployment::start("grades", 18109).await;

    let (status, raised) = post(
        &deployment.base,
        "/requests",
        MACHINE_A,
        "grade-1",
        raise_body("run:grades", "Approve the release?"),
    )
    .await;
    assert_eq!(status, 201, "{raised}");
    let request = raised["id"].as_str().expect("an id").to_string();

    // The in-app delivery is dispatched by the worker: our own API accepted it, which is a real
    // claim and the weakest one.
    let mut delivery = String::new();
    for _ in 0..100 {
        let (_, list) = get(
            &deployment.base,
            &format!("/requests/{request}/deliveries"),
            MACHINE_A,
        )
        .await;
        if let Some(one) = list["data"]
            .as_array()
            .and_then(|d| d.iter().find(|x| x["state"] == "dispatched"))
        {
            delivery = one["id"].as_str().expect("an id").to_string();
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(!delivery.is_empty(), "no delivery ever reached dispatched");

    // ---- `seen`: the person opened the surface and authenticated. A real advance.
    let (status, graded) = post_plain(
        &deployment.base,
        &format!("/deliveries/{delivery}/grade"),
        MACHINE_A,
        serde_json::json!({"grade": "seen"}),
    )
    .await;
    assert_eq!(status, 200, "{graded}");
    assert_eq!(graded["state"], "seen");
    assert_eq!(graded["grade_reached"], "seen");

    // ---- Reporting it again changes nothing and is not an error. Delivery is at-least-once and
    // consumers dedupe (§16 rule 10), so a provider webhook resends; answering 4xx to a duplicate
    // teaches it to retry forever.
    let (status, repeated) = post_plain(
        &deployment.base,
        &format!("/deliveries/{delivery}/grade"),
        MACHINE_A,
        serde_json::json!({"grade": "seen"}),
    )
    .await;
    assert_eq!(status, 200, "a repeated report is idempotent: {repeated}");
    assert_eq!(repeated["grade_reached"], "seen");
    assert_eq!(
        repeated["attempts"], graded["attempts"],
        "a duplicate must not append an attempt"
    );

    // ---- A grade already behind us is the same case, not a conflict. An SES delivery receipt
    // routinely lands after the person has already opened the surface, and the delivery has
    // genuinely reached at least `delivered` — so this is true, idempotent, and not a rejection.
    let (status, late) = post_plain(
        &deployment.base,
        &format!("/deliveries/{delivery}/grade"),
        MACHINE_A,
        serde_json::json!({"grade": "delivered"}),
    )
    .await;
    assert_eq!(status, 200, "{late}");
    assert_eq!(
        late["grade_reached"], "seen",
        "§7.1 — a delivery MUST NOT move backwards down the ladder"
    );
    assert_eq!(late["state"], "seen");

    // ---- The refusals that matter.
    for (grade, why) in [
        (
            "acted",
            "`acted` means the person answered through this delivery, and only an answer \
             establishes that",
        ),
        (
            "dispatched",
            "`dispatched` is the send's own claim, already recorded by the attempt that made it",
        ),
    ] {
        let (status, refused) = post_plain(
            &deployment.base,
            &format!("/deliveries/{delivery}/grade"),
            MACHINE_A,
            serde_json::json!({ "grade": grade }),
        )
        .await;
        assert_eq!(status, 400, "{why}: {refused}");
    }

    // ---- And the request itself is untouched by any of it: recording that somebody *saw* an ask
    // is not recording that they answered it (I1, §7.2).
    let (_, view) = get(&deployment.base, &format!("/requests/{request}"), MACHINE_A).await;
    assert_eq!(view["state"], "pending");
    assert!(view["receipt"].is_null());
}

#[tokio::test]
async fn a_grade_report_replays_under_an_idempotency_key() {
    // I20: every mutating operation is idempotent under a caller-supplied key. The advance itself
    // is already idempotent by construction, so this checks the other half — that a retry with the
    // key returns the *stored* response rather than re-deriving one.
    let deployment = Deployment::start("grades-key", 18111).await;
    let (_, raised) = post(
        &deployment.base,
        "/requests",
        MACHINE_A,
        "grade-key-1",
        raise_body("run:grades-key", "Approve the release?"),
    )
    .await;
    let request = raised["id"].as_str().expect("an id").to_string();

    let mut delivery = String::new();
    for _ in 0..100 {
        let (_, list) = get(
            &deployment.base,
            &format!("/requests/{request}/deliveries"),
            MACHINE_A,
        )
        .await;
        if let Some(one) = list["data"]
            .as_array()
            .and_then(|d| d.iter().find(|x| x["state"] == "dispatched"))
        {
            delivery = one["id"].as_str().expect("an id").to_string();
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(!delivery.is_empty(), "no delivery ever reached dispatched");

    let (first_status, first) = post(
        &deployment.base,
        &format!("/deliveries/{delivery}/grade"),
        MACHINE_A,
        "grade-key-seen",
        serde_json::json!({"grade": "seen"}),
    )
    .await;
    let (again_status, again) = post(
        &deployment.base,
        &format!("/deliveries/{delivery}/grade"),
        MACHINE_A,
        "grade-key-seen",
        serde_json::json!({"grade": "seen"}),
    )
    .await;

    assert_eq!(first_status, 200, "{first}");
    assert_eq!(again_status, first_status);
    assert_eq!(
        again, first,
        "the replay returns what the first call returned"
    );
}

#[tokio::test]
async fn a_delivery_in_another_tenant_is_indistinguishable_from_one_that_never_existed() {
    let deployment = Deployment::start("grades-iso", 18110).await;
    let (status, refused) = post_plain(
        &deployment.base,
        "/deliveries/dlv_01K3MB2R6C8ZC4YRXB2N6VD9FT/grade",
        crate::harness::MACHINE_B,
        serde_json::json!({"grade": "seen"}),
    )
    .await;
    // §13: `404 …_not_found` instead of `403` wherever existence is itself sensitive.
    assert_eq!(status, 404, "{refused}");
}

#[tokio::test]
async fn a_receipt_never_names_a_delivery_that_was_never_sent() {
    // The defect this guards: the default ladder's second rung is `email`, a scaffold that
    // transmits nothing and ends `suppressed` — and it is the newest delivery by the time anyone
    // answers. Taking the newest open delivery put an email nobody sent onto the receipt as the one
    // the person answered through, graded `dispatched`, which asserts a transport accepted it.
    //
    // A receipt is the artifact the whole product exists to make trustworthy, so a false claim here
    // is the worst one available.
    let deployment = Deployment::start("receipt-via", 18112).await;

    let mut body = raise_body("run:receipt-via", "Approve the release?");
    body["routing"] = serde_json::json!({
        "targets": [{"kind": "role", "value": "editor"}],
        "ladder": [
            {"after": "PT0S", "channels": ["inapp"]},
            {"after": "PT0S", "channels": ["email"]},
        ],
    });
    let (status, raised) = post(&deployment.base, "/requests", MACHINE_A, "via-1", body).await;
    assert_eq!(status, 201, "{raised}");
    let request = raised["id"].as_str().expect("an id").to_string();

    // Wait until the suppressed email is on the record, so the wrong answer is available to pick.
    let mut suppressed = false;
    for _ in 0..100 {
        let (_, list) = get(
            &deployment.base,
            &format!("/requests/{request}/deliveries"),
            MACHINE_A,
        )
        .await;
        suppressed = list["data"]
            .as_array()
            .is_some_and(|d| d.iter().any(|x| x["state"] == "suppressed"));
        if suppressed {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        suppressed,
        "the email rung never suppressed, so this proves nothing"
    );

    let (status, answered) = post(
        &deployment.base,
        &format!("/requests/{request}/answer"),
        crate::harness::EDITOR_A,
        "via-answer",
        serde_json::json!({"values": {"decision": "approve"}}),
    )
    .await;
    assert_eq!(status, 200, "{answered}");

    let (_, receipt) = get(
        &deployment.base,
        &format!("/requests/{request}/receipt"),
        MACHINE_A,
    )
    .await;
    assert_eq!(
        receipt["via"]["channel"], "inapp",
        "the answer arrived through the in-app surface, not through an email that was suppressed"
    );
    assert_eq!(
        receipt["via"]["grade_reached"], "acted",
        "§7.2 — the grade the answering delivery reached, not one invented for a delivery that \
         never left the queue"
    );
}

//! The wait survives `kill -9`.
//!
//! §8 makes resumption a protocol property rather than a runtime accident by putting the wait in a
//! durable server-side row instead of a loop inside the client's process. A predecessor project
//! demonstrated only a clean shutdown, which proves nothing: a clean shutdown runs handlers, drains
//! work, and flushes. `SIGKILL` runs nothing. This test is the difference.

use super::harness::*;
use sqlx::Row;

#[tokio::test]
async fn a_parked_request_its_waiter_and_its_unacked_signal_all_survive_sigkill() {
    let mut deployment = Deployment::start("kill9").await;
    let waiter = "run:kill9-durability";

    // A request nobody has answered yet, with a durable waiter: the wait is worth keeping even if
    // the process that raised it is gone.
    let (status, raised) = post(
        &deployment.base,
        "/requests",
        MACHINE_A,
        "kill9-raise",
        raise_body(waiter, "Approve the ledger adjustment?"),
    )
    .await;
    assert_eq!(status, 201, "raise: {raised}");
    let request_id = raised["id"].as_str().expect("a request id").to_string();

    // A person answers, which enqueues exactly one signal. Nothing acks it, so it is still owed to
    // whichever process comes back for it.
    let (status, answered) = post(
        &deployment.base,
        &format!("/requests/{request_id}/answer"),
        EDITOR_A,
        "kill9-answer",
        serde_json::json!({"values": {"decision": "approve"}}),
    )
    .await;
    assert_eq!(status, 200, "answer: {answered}");
    let receipt_id = answered["receipt"]["id"]
        .as_str()
        .expect("a receipt id")
        .to_string();

    let (status, before) = get(
        &deployment.base,
        &format!("/waiters/{}/signals", urlencode(waiter)),
        MACHINE_A,
    )
    .await;
    assert_eq!(status, 200);
    let signal_id = before["data"][0]["id"]
        .as_str()
        .expect("a signal")
        .to_string();
    assert!(
        before["data"][0]["acked_at"].is_null(),
        "nothing has acked it"
    );

    // Open a long poll and kill the process while it is held. This is the shape that loses data in
    // an implementation where the wait lives in the process: the poll is in flight, the signal has
    // been read but not consumed, and there is no shutdown path.
    let polling = tokio::spawn({
        let url = format!(
            "{}/waiters/{}/signals?wait=30",
            deployment.base,
            urlencode(waiter)
        );
        async move {
            let _ = reqwest::Client::new()
                .get(url)
                .header("Authorization", format!("Bearer {MACHINE_A}"))
                .send()
                .await;
        }
    });
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    deployment.kill_nine();
    let _ = polling.await;

    // Restart. Nothing was handed over; the new process reads what the old one committed.
    deployment.spawn().await;

    let (status, request) = get(
        &deployment.base,
        &format!("/requests/{request_id}"),
        MACHINE_A,
    )
    .await;
    assert_eq!(status, 200, "the request survived");
    assert_eq!(request["state"], "answered", "the state survived");
    assert_eq!(
        request["receipt"]["id"],
        receipt_id.as_str(),
        "the receipt survived"
    );

    let (status, after) = get(
        &deployment.base,
        &format!("/waiters/{}/signals", urlencode(waiter)),
        MACHINE_A,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        after["data"][0]["id"],
        signal_id.as_str(),
        "the same signal is still queued: reading it never consumed it, and neither did the crash"
    );
    assert!(
        after["data"][0]["acked_at"].is_null(),
        "and it is still unacked, so redelivery is still owed"
    );

    // The waiter row itself, below the API. §8: the wait was never in the client's process — and
    // this shows it was never in the server's either.
    let pool = deployment.pool().await;
    let rows =
        sqlx::query("select state from handoff_waiters where tenant_ref = $1 and waiter_ref = $2")
            .bind(ORG_A)
            .bind(waiter)
            .fetch_all(&pool)
            .await
            .expect("read the waiter");
    assert_eq!(rows.len(), 1, "the waiter row survived the kill");
    assert_eq!(rows[0].get::<String, _>("state"), "signalled");

    // And the ack still works afterwards, which is the point of any of it surviving.
    let token = after["data"][0]["resume_token"]
        .as_str()
        .expect("a resume token");
    let (status, acked) = post(
        &deployment.base,
        &format!("/signals/{signal_id}/ack"),
        MACHINE_A,
        "kill9-ack",
        serde_json::json!({"resume_token": token, "applied": true}),
    )
    .await;
    assert_eq!(status, 200, "ack: {acked}");
    assert_eq!(
        acked["first_ack"], true,
        "the crash did not consume the signal"
    );
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

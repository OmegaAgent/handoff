//! Callbacks against a receiver that misbehaves the way real ones do.
//!
//! The properties under test are the ones a happy path never reaches. A receiver that answers 200
//! every time proves nothing about §15.4, because the interesting claim is that a 200 **does not**
//! consume the signal: redelivery stops at the ack and nowhere else. So this receiver fails first,
//! succeeds second, and is still pushed to afterwards — and only the ack ends it.
//!
//! The signature assertions are here rather than only in `handoff-core` because the unit tests
//! check the scheme against the published vectors, and these check that the bytes this server
//! actually put on a socket verify under it. Those are different claims, and only the second one
//! catches a header assembled correctly and sent wrong.

use handoff_core::signing;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::harness::{get, post, Deployment, EDITOR_A, MACHINE_A};

/// The two secrets of `signing.md` §1.6, both active. Rotation is an overlap, not a cutover.
const SECRET_A: &str = "whsec_2f8a91c4e7b3d05a6c1e9f47b28d3a05";
const SECRET_B: &str = "whsec_9d41c07be5a2f36819b4d0e7c5a81f62";

/// One callback, exactly as it arrived.
#[derive(Debug, Clone)]
struct Captured {
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl Captured {
    fn header(&self, name: &str) -> &str {
        self.headers
            .get(name)
            .map(String::as_str)
            .unwrap_or_default()
    }
}

/// A receiver whose responses are scripted, and which remembers everything it was sent.
#[derive(Default)]
struct Receiver {
    captured: Mutex<Vec<Captured>>,
    statuses: Mutex<Vec<u16>>,
}

async fn receive(
    axum::extract::State(receiver): axum::extract::State<Arc<Receiver>>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> axum::http::StatusCode {
    receiver.captured.lock().expect("the log").push(Captured {
        headers: headers
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_ascii_lowercase(),
                    value.to_str().unwrap_or_default().to_string(),
                )
            })
            .collect(),
        body: body.to_vec(),
    });
    let mut statuses = receiver.statuses.lock().expect("the script");
    let status = if statuses.is_empty() {
        200
    } else {
        statuses.remove(0)
    };
    axum::http::StatusCode::from_u16(status).expect("a real status")
}

async fn start_receiver(statuses: Vec<u16>) -> (Arc<Receiver>, String) {
    let receiver = Arc::new(Receiver {
        captured: Mutex::new(Vec::new()),
        statuses: Mutex::new(statuses),
    });
    let app = axum::Router::new()
        .route("/hook", axum::routing::post(receive))
        .with_state(Arc::clone(&receiver));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a port");
    let url = format!("http://{}/hook", listener.local_addr().expect("an address"));
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (receiver, url)
}

async fn wait_for(receiver: &Receiver, at_least: usize, label: &str) -> Vec<Captured> {
    for _ in 0..200 {
        let captured = receiver.captured.lock().expect("the log").clone();
        if captured.len() >= at_least {
            return captured;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let seen = receiver.captured.lock().expect("the log").len();
    panic!("waited for {at_least} callbacks ({label}) and saw {seen}");
}

#[tokio::test]
async fn a_receiver_that_fails_then_succeeds_is_pushed_until_it_acks() {
    let (receiver, callback_url) = start_receiver(vec![500, 200]).await;
    let deployment = Deployment::start_with(
        "callbacks",
        &[(
            "HANDOFF_CALLBACK_SECRETS",
            &format!("{SECRET_A},{SECRET_B}"),
        )],
    )
    .await;

    let waiter = "run:callback-chaos";
    let mut body = crate::harness::raise_body(waiter, "Approve the release?");
    body["callback"] = serde_json::json!({"url": callback_url});
    let (status, raised) = post(&deployment.base, "/requests", MACHINE_A, "cb-1", body).await;
    assert_eq!(status, 201, "{raised}");
    let request = raised["id"].as_str().expect("an id").to_string();

    let (status, _) = post(
        &deployment.base,
        &format!("/requests/{request}/answer"),
        EDITOR_A,
        "cb-answer",
        serde_json::json!({"values": {"decision": "approve"}}),
    )
    .await;
    assert_eq!(status, 200);

    // Two pushes: the 500 is retried, and the 200 is *also* retried, because a 2xx marks the
    // callback dispatched and does not consume the signal (§15.4).
    let captured = wait_for(&receiver, 2, "the 500 and the 200").await;

    // ---- The identity a receiver dedupes on is stable across the retries of one push.
    let deliveries: Vec<&str> = captured
        .iter()
        .map(|c| c.header("handoff-delivery"))
        .collect();
    assert!(
        deliveries.windows(2).all(|pair| pair[0] == pair[1]),
        "signing.md §1.3 rule 7 has a receiver dedupe on Handoff-Delivery so a repeated delivery \
         does not apply a decision twice. A delivery id that changes per attempt makes every retry \
         look new, and a conforming receiver then applies the same decision once per retry: {deliveries:?}"
    );
    for one in &captured {
        assert_eq!(
            one.header("handoff-idempotency-key"),
            one.header("handoff-delivery"),
            "signing.md §1.1 — the idempotency key is the delivery id, so a receiver can dedupe \
             without parsing the body"
        );
        assert_eq!(one.header("handoff-version"), "1");
        assert!(!one.header("handoff-signal").is_empty());
    }

    // ---- Rotation: both secrets are active, both are emitted, either one verifies.
    let first = &captured[0];
    let signature = first.header("handoff-signature");
    assert_eq!(
        signature.matches("v1=").count(),
        2,
        "signing.md §1.4 rule 2 — while two secrets are active the Server MUST sign with both, so \
         there is no window in which a valid callback fails: {signature}"
    );
    let stamped = signing::parse_signature_header(signature)
        .expect("the header parses")
        .timestamp;
    for held in [SECRET_A, SECRET_B] {
        assert!(
            signing::verify(
                &first.body,
                signature,
                first.header("handoff-delivery"),
                &[held.to_string()],
                stamped,
                signing::FRESHNESS_WINDOW_SECS,
            )
            .is_ok(),
            "a receiver holding only {held} could not verify during the overlap"
        );
    }

    // ---- Replay: the same genuine callback, presented outside the freshness window.
    assert!(
        matches!(
            signing::verify(
                &first.body,
                signature,
                first.header("handoff-delivery"),
                &[SECRET_A.to_string()],
                stamped + signing::FRESHNESS_WINDOW_SECS + 1,
                signing::FRESHNESS_WINDOW_SECS,
            ),
            Err(signing::Rejected::StaleTimestamp { .. })
        ),
        "signing.md §1.3 rule 2 — a signature stays valid forever; freshness is what stops it being \
         replayed, and it is a separate check"
    );

    // ---- Replay onto another delivery, and a one-byte change to the body.
    assert!(signing::verify(
        &first.body,
        signature,
        "dlv_01K3MB2R6D8ZC4YRXB2N6VD9FT",
        &[SECRET_A.to_string()],
        stamped,
        signing::FRESHNESS_WINDOW_SECS,
    )
    .is_err());
    let mut tampered = first.body.clone();
    let last = tampered.len() - 2;
    tampered[last] ^= 0x01;
    assert!(signing::verify(
        &tampered,
        signature,
        first.header("handoff-delivery"),
        &[SECRET_A.to_string()],
        stamped,
        signing::FRESHNESS_WINDOW_SECS,
    )
    .is_err());

    // ---- The signal is still unacked, and every attempt is inspectable (§15.5).
    let (status, signals) = get(
        &deployment.base,
        &format!("/waiters/{}/signals", waiter.replace(':', "%3A")),
        MACHINE_A,
    )
    .await;
    assert_eq!(status, 200, "{signals}");
    let signal = signals["data"][0]["id"]
        .as_str()
        .expect("a signal")
        .to_string();
    let token = signals["data"][0]["resume_token"]
        .as_str()
        .expect("a resume token")
        .to_string();
    assert!(
        signals["data"][0]["acked_at"].is_null(),
        "two 2xx responses must not have consumed it"
    );

    let (status, attempts) = get(
        &deployment.base,
        &format!("/signals/{signal}/attempts"),
        MACHINE_A,
    )
    .await;
    assert_eq!(status, 200);
    assert!(
        attempts["data"].as_array().map(Vec::len).unwrap_or(0) >= 2,
        "§15.5 — every attempt MUST be inspectable by the tenant: {attempts}"
    );

    // ---- The ack, and only the ack, stops it. And acking twice applies nothing twice (I9).
    let before = receiver.captured.lock().expect("the log").len();
    let (status, acked) = post(
        &deployment.base,
        &format!("/signals/{signal}/ack"),
        MACHINE_A,
        "cb-ack",
        serde_json::json!({"resume_token": token, "applied": true}),
    )
    .await;
    assert_eq!(status, 200, "{acked}");
    assert_eq!(acked["first_ack"], true);

    let (status, again) = post(
        &deployment.base,
        &format!("/signals/{signal}/ack"),
        MACHINE_A,
        "cb-ack-again",
        serde_json::json!({"resume_token": token, "applied": true}),
    )
    .await;
    assert_eq!(
        status, 200,
        "a second ack is a retry, not an error: {again}"
    );
    assert_eq!(
        again["first_ack"], false,
        "I9 — the ack is idempotent: the second one must not be a second application"
    );

    // Long enough for several backoffs to have elapsed had anything still been owed.
    tokio::time::sleep(std::time::Duration::from_secs(6)).await;
    let after = receiver.captured.lock().expect("the log").len();
    assert_eq!(
        after,
        before,
        "§8.2 W4 — the ack stops redelivery. {} further push(es) arrived after it",
        after - before
    );
}

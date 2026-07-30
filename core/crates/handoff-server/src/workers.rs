//! The two background loops, and the signature that protects one of them.
//!
//! Neither loop is where a state change is decided. The sweep calls one store method per
//! transition, and that method commits the state and its event together (I12) — "update the row,
//! then publish" is exactly the shape a background job makes tempting and §6.2 forbids.

use handoff_core::model::CallbackAttemptView;
use handoff_core::ports::{CallbackJob, Store};
use handoff_protocol::clock::{IsoDuration, Timestamp};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::sync::Arc;

use crate::state::AppState;

/// Sweep attempt lapses, ladder rungs, and TTL expiries on a fixed interval.
pub async fn sweep_loop(state: Arc<AppState>) {
    let interval = std::time::Duration::from_millis(state.config.sweep_interval_ms.max(50));
    loop {
        tokio::time::sleep(interval).await;
        match state.store.sweep(state.now()).await {
            Ok(report) => {
                if report.attempts_lapsed > 0
                    || report.requests_expired > 0
                    || report.rungs_fired > 0
                {
                    tracing::info!(
                        attempts_lapsed = report.attempts_lapsed,
                        requests_expired = report.requests_expired,
                        rungs_fired = report.rungs_fired,
                        "sweep"
                    );
                }
            }
            Err(error) => tracing::error!(%error, "sweep failed"),
        }
    }
}

/// Push signals to registered callbacks, and keep pushing until an ack arrives.
///
/// §15.4 and §8.3: a `2xx` from a receiver marks the callback **dispatched**; it does not consume
/// the signal. A receiver that returns `2xx` and then crashes before applying the decision has not
/// received it, so redelivery continues until an explicit ack — which is why this loop reschedules
/// on success as well as on failure.
pub async fn callback_loop(state: Arc<AppState>) {
    let client = match reqwest::Client::builder()
        // §15.7: every attempt has a request timeout. A hung receiver must not hold a worker.
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            tracing::error!(%error, "no HTTP client; callbacks are disabled");
            return;
        }
    };

    loop {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let now = state.now();
        let job = match state.store.claim_callback(now).await {
            Ok(Some(job)) => job,
            Ok(None) => continue,
            Err(error) => {
                tracing::error!(%error, "cannot claim a callback");
                continue;
            }
        };
        if let Err(error) = deliver(&state, &client, job, now).await {
            tracing::error!(%error, "callback delivery failed");
        }
    }
}

async fn deliver(
    state: &AppState,
    client: &reqwest::Client,
    job: CallbackJob,
    now: Timestamp,
) -> handoff_protocol::error::Result<()> {
    // The body is serialized once and signed over those exact bytes. Re-serializing before hashing
    // is one of the two traps `signing.md` §3 names, and it produces a signature that is stable in
    // one implementation and wrong across two.
    let body = job.body.to_string();
    let timestamp = now.to_millis() / 1000;
    let signature = sign(
        &state.config.callback_secrets,
        timestamp,
        &job.delivery_id.to_string(),
        body.as_bytes(),
    );

    let started = std::time::Instant::now();
    let response = client
        .post(&job.url)
        .header("Content-Type", "application/json")
        .header("Handoff-Signature", signature)
        .header("Handoff-Delivery", job.delivery_id.to_string())
        .header("Handoff-Signal", job.signal_id.to_string())
        .header("Handoff-Version", "1")
        .header("Handoff-Sequence", job.sequence.to_string())
        // The delivery identifier, so a receiver can dedupe without parsing the body.
        .header("Handoff-Idempotency-Key", job.delivery_id.to_string())
        .body(body)
        .send()
        .await;
    let duration_ms = started.elapsed().as_millis() as i64;

    let (status_code, outcome, error) = match response {
        Ok(response) => {
            let status = response.status().as_u16();
            let outcome = if response.status().is_success() {
                "accepted"
            } else if status >= 500 {
                "transient_failure"
            } else {
                "permanent_failure"
            };
            (Some(status as i32), outcome, None)
        }
        Err(e) if e.is_timeout() => (None, "timeout", Some(e.to_string())),
        Err(e) => (None, "transient_failure", Some(e.to_string())),
    };

    let backoff = handoff_store_postgres::store::backoff_seconds(job.attempt);
    let next = now.saturating_add(IsoDuration::from_secs(backoff as u64));
    state
        .store
        .record_callback_attempt(
            job.clone(),
            CallbackAttemptView {
                n: job.attempt,
                started_at: now,
                ended_at: Some(state.now()),
                status_code,
                duration_ms: Some(duration_ms),
                outcome: outcome.to_string(),
                error,
            },
            Some(next),
        )
        .await
}

/// The canonical string of `signing.md` §1.2, signed under every active secret.
///
/// `delivery_id` is inside the signed string so that a valid signature cannot be lifted onto a
/// different delivery of the same payload, and the **body hash** rather than the body is signed so
/// a receiver can verify before buffering.
///
/// While two secrets are active the header carries both as separate `v1=` elements (§1.4.2), so
/// there is no window in which a valid callback fails verification during a rotation.
pub fn sign(secrets: &[String], timestamp: i64, delivery_id: &str, body: &[u8]) -> String {
    let canonical = format!(
        "1\n{timestamp}\n{delivery_id}\n{}",
        hex(&Sha256::digest(body))
    );
    let mut header = format!("t={timestamp}");
    for secret in secrets {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret.as_bytes())
            .expect("HMAC accepts a key of any length");
        mac.update(canonical.as_bytes());
        header.push_str(&format!(",v1={}", hex(&mac.finalize().into_bytes())));
    }
    header
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every constant here is published in `signing.md` §1.6, so this verifies the implementation
    // against the specification rather than against itself.
    const BODY: &str = r#"{"created_at":"2026-07-30T14:07:44Z","decision":{"authorization_id":"auth_01K3MB2R4Z8ZC4YRXB2N6VD9FT","outcome":"answered","receipt_id":"rcpt_01K3MB2R4Y8ZC4YRXB2N6VD9FT","source":"human","values":{"decision":"approve","note":"Confirmed with Acme on the phone."}},"id":"sig_01K3MB2R4X8ZC4YRXB2N6VD9FT","request_id":"req_01K3M7QW8ZC4YRXB2N6VD9FTHE","resume_payload":null,"resume_ref":null,"resume_token":"rt_01K3MB2R558ZC4YRXB2N6VD9FT","sequence":1,"type":"answered","waiter_ref":"run:0198f2a1"}"#;
    const SECRET_A: &str = "whsec_2f8a91c4e7b3d05a6c1e9f47b28d3a05";
    const SECRET_B: &str = "whsec_9d41c07be5a2f36819b4d0e7c5a81f62";
    const TIMESTAMP: i64 = 1785592064;
    const DELIVERY: &str = "dlv_01K3MB2R6C8ZC4YRXB2N6VD9FT";

    #[test]
    fn the_published_signature_reproduces_exactly() {
        let header = sign(
            &[SECRET_A.to_string()],
            TIMESTAMP,
            DELIVERY,
            BODY.as_bytes(),
        );
        assert_eq!(
            header,
            "t=1785592064,v1=cae13126f8dcd1e918376aa373be2757db7281a3e5aaed2d83d716537e03de80"
        );
    }

    #[test]
    fn a_rotation_overlap_emits_both_secrets_in_one_header() {
        let header = sign(
            &[SECRET_A.to_string(), SECRET_B.to_string()],
            TIMESTAMP,
            DELIVERY,
            BODY.as_bytes(),
        );
        assert_eq!(header.matches("v1=").count(), 2);
        assert!(header.contains("cae13126f8dcd1e918376aa373be2757db7281a3e5aaed2d83d716537e03de80"));
        assert!(header.contains("d86b3740bad654e46c1349614523a476be0eb7d6a30a798b2d475374f36c57eb"));
    }

    #[test]
    fn the_delivery_identifier_is_inside_the_signed_string() {
        let one = sign(
            &[SECRET_A.to_string()],
            TIMESTAMP,
            DELIVERY,
            BODY.as_bytes(),
        );
        let other = sign(
            &[SECRET_A.to_string()],
            TIMESTAMP,
            "dlv_01K3MB2R6D8ZC4YRXB2N6VD9FT",
            BODY.as_bytes(),
        );
        assert_ne!(one, other);
        assert!(other.contains("9a674a003d0507ad13369a6bd82713769116a276ec57f26eb2637b2af00f8e68"));
    }

    #[test]
    fn one_altered_byte_changes_the_signature() {
        let genuine = sign(
            &[SECRET_A.to_string()],
            TIMESTAMP,
            DELIVERY,
            BODY.as_bytes(),
        );
        let tampered = sign(
            &[SECRET_A.to_string()],
            TIMESTAMP,
            DELIVERY,
            BODY.replace("approve", "reject").as_bytes(),
        );
        assert_ne!(genuine, tampered);
    }
}

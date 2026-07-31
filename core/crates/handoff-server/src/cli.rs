//! Maintenance subcommands.
//!
//! Three of these exist because three conformance requirements are below the HTTP API by
//! construction, and the specification says so: §9.4's chain verification, §4.7's inbound channel
//! surface (which the protocol deliberately does not define), and §9.7's runtime observation. Each
//! is a real operator tool, not a test fixture — an operator who wants to know whether their
//! receipts still verify runs the same command the suite does.

use handoff_core::ports::Store;
use handoff_protocol::clock::Timestamp;
use handoff_protocol::error::{ErrorCode, ProtocolError, Result};
use handoff_protocol::id::RequestId;
use handoff_protocol::receipt::{verify_chain, Receipt};
use handoff_store_postgres::PgStore;
use sqlx::Row;

fn now() -> Timestamp {
    Timestamp::from_millis(chrono::Utc::now().timestamp_millis())
        .unwrap_or_else(|| Timestamp::from_millis(0).expect("epoch is representable"))
}

/// Re-walk every tenant's receipt chain and report whether each still verifies.
///
/// This is the **open verifier** `GOVERNANCE.md` requires: a pure function over stored receipts,
/// recomputing each digest from `height`, the predecessor's digest, and the receipt's own core
/// hash. Any historical alteration changes that receipt's core hash, which changes its digest,
/// which invalidates every digest after it and therefore the exported head.
pub async fn verify_chains(store: &PgStore, tenant: Option<&str>) -> Result<bool> {
    let tenants: Vec<String> = match tenant {
        Some(one) => vec![one.to_string()],
        None => sqlx::query_scalar("select distinct tenant_ref from handoff_receipts order by 1")
            .fetch_all(store.pool())
            .await
            .map_err(|e| ProtocolError::new(ErrorCode::InvalidRequest, e.to_string()))?,
    };

    let mut all_verified = true;
    for tenant in &tenants {
        let rows =
            sqlx::query("select body from handoff_receipts where tenant_ref = $1 order by height")
                .bind(tenant)
                .fetch_all(store.pool())
                .await
                .map_err(|e| ProtocolError::new(ErrorCode::InvalidRequest, e.to_string()))?;

        let mut receipts = Vec::with_capacity(rows.len());
        let mut unparseable = 0usize;
        for row in &rows {
            let body: serde_json::Value = row.get("body");
            match serde_json::from_value::<Receipt>(body) {
                Ok(receipt) => receipts.push(receipt),
                Err(e) => {
                    println!("{tenant}: BROKEN — a stored receipt no longer parses: {e}");
                    all_verified = false;
                    unparseable += 1;
                }
            }
        }

        // A tenant with an unreadable receipt is broken, and must not then be reported OK for the
        // rows that happen to survive. Verifying the parseable subset would print a head that is
        // not the head, over a shorter chain that verifies perfectly — and an operator or a script
        // grepping this tool's output for OK would find it. Tail truncation is exactly the attack
        // §9.4 says an unanchored chain cannot detect; printing OK over a subset would hand the
        // same result to someone who had not even truncated carefully.
        if unparseable > 0 {
            println!(
                "{tenant}: BROKEN — {unparseable} of {} receipt(s) unreadable; not verifying the \
                 remainder, because a chain over the survivors is a different chain",
                rows.len()
            );
            continue;
        }

        match verify_chain(&receipts, now()) {
            Ok(Some(head)) => println!(
                "{tenant}: OK — {} receipt(s), head {} at height {}",
                receipts.len(),
                head.head_digest,
                head.height
            ),
            Ok(None) => println!("{tenant}: OK — no receipts yet"),
            Err(e) => {
                println!("{tenant}: BROKEN — {e}");
                all_verified = false;
            }
        }
    }
    Ok(all_verified)
}

/// Print one event type per line for a request, oldest first.
///
/// The protocol defines events in §6.2 but publishes no endpoint that lists them, so there is
/// nothing for a black-box client to read. This is how an operator — and C-23 — sees the event
/// record at all.
pub async fn dump_events(store: &PgStore, request_id: &str) -> Result<()> {
    let rows = sqlx::query("select type from handoff_events where request_id = $1 order by id")
        .bind(request_id)
        .fetch_all(store.pool())
        .await
        .map_err(|e| ProtocolError::new(ErrorCode::InvalidRequest, e.to_string()))?;
    for row in rows {
        println!("{}", row.get::<String, _>("type"));
    }
    Ok(())
}

/// Inject a message that arrived on a channel.
///
/// The protocol defines the outbound delivery model but **not** an inbound channel-adapter surface,
/// so a deployment supplies one. This one records the message as a provisional answer and settles
/// nothing: §4.7 forbids deriving a decision from message content, however authenticated the
/// channel, and the way to make that true is to have no code that could.
pub async fn inject_channel_message(
    store: &PgStore,
    request_id: &str,
    channel: &str,
    text: &str,
) -> Result<()> {
    let tenant = tenant_of(store, request_id).await?;
    let id = RequestId::parse(request_id)?;
    let recorded = store
        .record_channel_message(tenant, id, channel.to_string(), text.to_string(), now())
        .await?;
    if !recorded {
        return Err(ProtocolError::new(
            ErrorCode::RequestNotFound,
            "no such request",
        ));
    }
    println!("recorded a provisional {channel} message; the request was not settled");
    Ok(())
}

/// Record that a runtime observed the target page change.
///
/// §9.7: clearance MUST be asserted, never inferred. An observation is recorded as an observation
/// and produces no receipt — a Server MUST NOT fabricate a person.
pub async fn observe_page_change(store: &PgStore, request_id: &str) -> Result<()> {
    let tenant = tenant_of(store, request_id).await?;
    let id = RequestId::parse(request_id)?;
    let recorded = store
        .record_observation(
            tenant,
            id,
            "the runtime observed the target page change; nobody asserted anything".to_string(),
            now(),
        )
        .await?;
    if !recorded {
        return Err(ProtocolError::new(
            ErrorCode::RequestNotFound,
            "no such request",
        ));
    }
    println!("recorded an observation; no clearance was asserted and no receipt was minted");
    Ok(())
}

/// Canonicalize a document per RFC 8785 and report its bytes and digest.
///
/// This exists as an operator tool for the same reason C-24 asks for it: two implementations that
/// disagree about number formatting or member ordering compute different digests for the same
/// receipt, and nothing errors — the chain simply stops verifying for somebody, later, with no way
/// to establish which side was right. Running this against the published fixtures is how a
/// deployment shows its canonicalization agrees with everyone else's.
///
/// Two output modes, because there are two jobs:
///
/// - **Filter** (`--json`): stdout is the canonical bytes and nothing else, with no trailing
///   newline, so the output can be piped, diffed, or compared byte for byte against a fixture.
/// - **Report** (`--path`): the canonical bytes, then `bytes=<n>`, then `sha256=<hex>`, which is
///   what an operator checking a fixture against `signing.md` actually wants to read.
pub fn canonicalize(document: &serde_json::Value, report: bool) -> Result<()> {
    use sha2::{Digest, Sha256};

    let bytes = handoff_protocol::receipt::canonical_json(document)?;
    let text = String::from_utf8(bytes.clone()).map_err(|e| {
        ProtocolError::new(
            ErrorCode::InvalidRequest,
            format!("canonical output is not UTF-8: {e}"),
        )
    })?;
    if !report {
        print!("{text}");
        return Ok(());
    }
    println!("{text}");
    println!("bytes={}", bytes.len());
    println!("sha256={:x}", Sha256::digest(&bytes));
    Ok(())
}

/// Read a document for [`canonicalize`], from a path or from an inline argument.
pub fn read_document(path: Option<&str>, inline: Option<&str>) -> Result<serde_json::Value> {
    let text = match (path, inline) {
        (Some(path), _) => std::fs::read_to_string(path).map_err(|e| {
            ProtocolError::new(
                ErrorCode::InvalidRequest,
                format!("cannot read {path}: {e}"),
            )
        })?,
        (None, Some(inline)) => inline.to_string(),
        (None, None) => {
            return Err(ProtocolError::new(
                ErrorCode::InvalidRequest,
                "canonicalize needs --path or --json",
            ))
        }
    };
    serde_json::from_str(&text)
        .map_err(|e| ProtocolError::new(ErrorCode::InvalidRequest, format!("not JSON: {e}")))
}

/// Which tenant a request belongs to.
///
/// An operator tool runs outside any request's authentication, so it reads the tenant from the row
/// rather than from an argument. That is the same rule as I13 seen from the other side: tenancy
/// comes from stored state, never from something the caller typed.
async fn tenant_of(store: &PgStore, request_id: &str) -> Result<String> {
    sqlx::query_scalar("select tenant_ref from handoff_requests where id = $1")
        .bind(request_id)
        .fetch_optional(store.pool())
        .await
        .map_err(|e| ProtocolError::new(ErrorCode::InvalidRequest, e.to_string()))?
        .ok_or_else(|| ProtocolError::new(ErrorCode::RequestNotFound, "no such request"))
}

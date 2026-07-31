//! The outbox and the reconciler, against a real Postgres.
//!
//! Everything else in this crate is tested against fakes, because the control plane it talks to does
//! not exist. The outbox is different: its whole claim is about **durability**, and durability is
//! not a property a fake can demonstrate. A queue that keeps rows in a `Vec` proves nothing about a
//! queue that has to survive a process dying between a remote write and an ack.
//!
//! These use a disposable `handoff_managed_*` database created and dropped per run, following the
//! same convention as the open server's own suite.

use crate::events::summarize;
use crate::fixtures;
use crate::outbox::{Destination, Outbox, STUCK_AFTER};
use crate::reconciler::{ReceiptSource, Reconciler};
use handoff_core::ports::BoxFuture;
use handoff_core::seam::{AuditEvent, EventSink, MeterReading, MeterSink};
use handoff_protocol::error::{ErrorCode, ProtocolError, Result};
use handoff_protocol::receipt::Receipt;
use sqlx::{Executor, PgPool, Row};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn admin_url() -> String {
    std::env::var("HANDOFF_TEST_ADMIN_URL")
        .unwrap_or_else(|_| "postgres://omega:omega@localhost:5432/postgres".to_string())
}

/// A database that exists for one test and is dropped afterwards.
struct Disposable {
    name: String,
    pool: PgPool,
}

impl Disposable {
    async fn create(label: &str) -> Self {
        // Named for the test, so a leaked database says which one leaked it.
        let name = format!("handoff_managed_{label}");
        let admin = PgPool::connect(&admin_url())
            .await
            .expect("a local Postgres for the managed suite");
        admin
            .execute(format!(r#"drop database if exists "{name}" with (force)"#).as_str())
            .await
            .expect("drop any leftover");
        admin
            .execute(format!(r#"create database "{name}""#).as_str())
            .await
            .expect("create the disposable database");
        admin.close().await;

        let url = admin_url().replace("/postgres", &format!("/{name}"));
        let pool = PgPool::connect(&url).await.expect("connect");
        Self { name, pool }
    }

    async fn drop_it(self) {
        let name = self.name;
        self.pool.close().await;
        let admin = PgPool::connect(&admin_url()).await.expect("admin");
        admin
            .execute(format!(r#"drop database if exists "{name}" with (force)"#).as_str())
            .await
            .expect("drop");
        admin.close().await;
    }
}

/// A sink that fails on demand and counts what it was handed.
struct Flaky {
    fails: bool,
    seen: AtomicUsize,
}

impl Flaky {
    fn new(fails: bool) -> Self {
        Self {
            fails,
            seen: AtomicUsize::new(0),
        }
    }

    fn answer(&self, n: usize) -> Result<()> {
        self.seen.fetch_add(n, Ordering::SeqCst);
        if self.fails {
            return Err(ProtocolError::new(
                ErrorCode::DeliveryUnavailable,
                "the control plane is unreachable",
            ));
        }
        Ok(())
    }
}

impl EventSink for Flaky {
    fn append(&self, events: Vec<AuditEvent>) -> BoxFuture<'_, Result<()>> {
        let n = events.len();
        Box::pin(async move { self.answer(n) })
    }
}

impl MeterSink for Flaky {
    fn record(&self, readings: Vec<MeterReading>) -> BoxFuture<'_, Result<()>> {
        let n = readings.len();
        Box::pin(async move { self.answer(n) })
    }
}

/// A receipt source backed by a fixed chain.
struct FixedChain(Vec<Receipt>);

impl ReceiptSource for FixedChain {
    fn receipts_since(
        &self,
        _tenant: String,
        after: Option<String>,
    ) -> BoxFuture<'_, Result<Vec<Receipt>>> {
        let receipts = self.0.clone();
        Box::pin(async move {
            let Some(after) = after else {
                return Ok(receipts);
            };
            match receipts.iter().position(|r| r.id.to_string() == after) {
                Some(index) => Ok(receipts.into_iter().skip(index + 1).collect()),
                None => Ok(Vec::new()),
            }
        })
    }
}

/// Make every deferred row due again, so a retry ladder can be walked without sleeping.
async fn make_everything_due(pool: &PgPool) {
    pool.execute("update handoff_managed_outbox set next_attempt_at = now() - interval '1 hour', leased_until = null where acked_at is null")
        .await
        .expect("reset the schedule");
}

async fn attempts_of(pool: &PgPool, dedupe_key: &str) -> i32 {
    sqlx::query("select attempts from handoff_managed_outbox where dedupe_key = $1")
        .bind(dedupe_key)
        .fetch_one(pool)
        .await
        .expect("row")
        .try_get("attempts")
        .expect("attempts")
}

#[tokio::test]
async fn a_row_the_control_plane_refuses_is_retried_and_never_dropped() {
    let db = Disposable::create("retention").await;
    let outbox = Outbox::open(db.pool.clone()).await.expect("open");
    let summary = summarize(&fixtures::sealed_receipt()).expect("summarize");
    let key = "handoff.receipt.recorded.v1:rcpt_01K3M7QW8ZC4YRXB2N6VD9FTH0";

    let queued = outbox
        .enqueue(
            Destination::Event,
            fixtures::ORG,
            key,
            serde_json::to_value(&summary).expect("json"),
            summary.occurred_at.to_datetime(),
        )
        .await
        .expect("enqueue");
    assert!(queued);

    // Queueing the same summary again is a no-op, not a second row. A reconciler that re-reads a
    // receipt it already saw must not double-count anyone.
    assert!(!outbox
        .enqueue(
            Destination::Event,
            fixtures::ORG,
            key,
            serde_json::to_value(&summary).expect("json"),
            summary.occurred_at.to_datetime(),
        )
        .await
        .expect("enqueue"));
    assert_eq!(outbox.pending().await.expect("pending"), 1);

    // Walk the ladder past the reporting threshold. The row is deferred every time and removed
    // never — this is the B-25 failure, inverted.
    let down = Flaky::new(true);
    for expected in 1..=STUCK_AFTER {
        make_everything_due(&db.pool).await;
        let acked = outbox.drain(&down, &down, 10).await.expect("drain");
        assert_eq!(acked, 0);
        assert_eq!(attempts_of(&db.pool, key).await, expected);
        assert_eq!(outbox.pending().await.expect("pending"), 1);
    }

    // And now it is visible, which is the whole point: a queue nobody reads loses data quietly.
    let stuck = outbox.stuck().await.expect("stuck");
    assert_eq!(stuck.len(), 1);
    assert_eq!(stuck[0].1, key);
    assert!(stuck[0].2.contains("unreachable"));

    // When the control plane comes back, the row that was never dropped is delivered.
    let up = Flaky::new(false);
    make_everything_due(&db.pool).await;
    assert_eq!(outbox.drain(&up, &up, 10).await.expect("drain"), 1);
    assert_eq!(up.seen.load(Ordering::SeqCst), 1);
    assert_eq!(outbox.pending().await.expect("pending"), 0);
    assert!(outbox.stuck().await.expect("stuck").is_empty());

    db.drop_it().await;
}

#[tokio::test]
async fn a_claimed_row_that_is_never_acked_stays_pending() {
    // The at-least-once contract, from the failure that produces it: the process dies between the
    // remote write and the ack. The row must still be there.
    let db = Disposable::create("at_least_once").await;
    let outbox = Outbox::open(db.pool.clone()).await.expect("open");
    let summary = summarize(&fixtures::sealed_receipt()).expect("summarize");
    outbox
        .enqueue(
            Destination::Event,
            fixtures::ORG,
            "some-key",
            serde_json::to_value(&summary).expect("json"),
            summary.occurred_at.to_datetime(),
        )
        .await
        .expect("enqueue");

    let claimed = outbox.claim(10).await.expect("claim");
    assert_eq!(claimed.len(), 1);
    // Simulate the crash: the send happened, the ack did not.
    assert_eq!(outbox.pending().await.expect("pending"), 1);

    // A second worker cannot take it while the lease holds, so the same row is not sent twice at
    // once. It becomes claimable again when the lease lapses.
    assert!(outbox.claim(10).await.expect("claim").is_empty());

    db.drop_it().await;
}

#[tokio::test]
async fn reconciling_twice_queues_each_receipt_once_and_advances_the_cursor() {
    let db = Disposable::create("reconcile").await;
    let outbox = Arc::new(Outbox::open(db.pool.clone()).await.expect("open"));
    let chain = fixtures::chain(3);
    let last = chain[2].id.to_string();
    let reconciler = Reconciler::new(Box::new(FixedChain(chain)), Arc::clone(&outbox));

    let first = reconciler.run_for(fixtures::ORG).await.expect("first pass");
    assert_eq!(first.receipts, 3);
    assert_eq!(first.events_queued, 3);
    assert_eq!(first.readings_queued, 3);
    assert_eq!(outbox.pending().await.expect("pending"), 6);
    assert_eq!(
        outbox.cursor(fixtures::ORG).await.expect("cursor"),
        Some(last)
    );

    // The cursor now covers the whole chain, so a second pass reads nothing and queues nothing.
    let second = reconciler
        .run_for(fixtures::ORG)
        .await
        .expect("second pass");
    assert_eq!(second.receipts, 0);
    assert_eq!(second.events_queued, 0);
    assert_eq!(outbox.pending().await.expect("pending"), 6);

    db.drop_it().await;
}

#[tokio::test]
async fn a_pass_that_re_reads_after_a_crash_queues_nothing_new() {
    // The cursor is advanced last, so a crash between queueing and advancing re-reads. That is only
    // safe if re-reading is a no-op, which is what this asserts.
    let db = Disposable::create("crash_replay").await;
    let outbox = Arc::new(Outbox::open(db.pool.clone()).await.expect("open"));
    let chain = fixtures::chain(2);
    let reconciler = Reconciler::new(Box::new(FixedChain(chain)), Arc::clone(&outbox));

    reconciler.run_for(fixtures::ORG).await.expect("first pass");
    let before = outbox.pending().await.expect("pending");

    // Rewind the cursor to nothing, as a crash before `advance` would leave it.
    db.pool
        .execute("delete from handoff_managed_cursor")
        .await
        .expect("rewind");

    let replay = reconciler.run_for(fixtures::ORG).await.expect("replay");
    assert_eq!(replay.receipts, 2, "the whole chain is re-read");
    assert_eq!(replay.events_queued, 0, "and none of it is queued twice");
    assert_eq!(replay.readings_queued, 0);
    assert_eq!(outbox.pending().await.expect("pending"), before);

    db.drop_it().await;
}

#[tokio::test]
async fn two_tenants_with_the_same_receipt_position_do_not_collide() {
    // §3.2 applies to derived rows too: an unscoped key does not merely risk a collision, it lets
    // one tenant's row silently absorb another's.
    let db = Disposable::create("tenant_isolation").await;
    let outbox = Outbox::open(db.pool.clone()).await.expect("open");
    let reading_a = crate::meter::reading(
        fixtures::ORG,
        &handoff_protocol::id::RequestId::parse("req_01K3M7QW8ZC4YRXB2N6VD9FTHE").expect("id"),
        crate::meter::Metered::Intervention,
        handoff_protocol::clock::Timestamp::from_millis(1_700_000_000_000).expect("now"),
    )
    .expect("reading");
    let reading_b = crate::meter::reading(
        "org_01K3M7QW8ZC4YRXB2N6VD9FTHF",
        &handoff_protocol::id::RequestId::parse("req_01K3M7QW8ZC4YRXB2N6VD9FTHE").expect("id"),
        crate::meter::Metered::Intervention,
        handoff_protocol::clock::Timestamp::from_millis(1_700_000_000_000).expect("now"),
    )
    .expect("reading");

    for entry in [&reading_a, &reading_b] {
        assert!(outbox
            .enqueue(
                Destination::Meter,
                &entry.tenant,
                &entry.idempotency_key,
                serde_json::to_value(entry).expect("json"),
                entry.occurred_at.to_datetime(),
            )
            .await
            .expect("enqueue"));
    }
    assert_eq!(outbox.pending().await.expect("pending"), 2);

    db.drop_it().await;
}

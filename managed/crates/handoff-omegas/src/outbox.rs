//! The durable outbox that makes the derived read model honest.
//!
//! "Derived records may be delayed; they must not be silently dropped" is easy to write and hard to
//! do, and the difference is entirely in where the record sits between being produced and being
//! acked. A best-effort call at the moment of the receipt is not it: the mirror would be missing
//! exactly the events that happened during the control plane's outage, which is when an audit log
//! matters most.
//!
//! So a receipt summary and its meter readings are **written to a table in Handoff's own database**,
//! in Handoff's own transaction, and a worker drains them until the control plane acks. The shape is
//! the one the in-repo `OutboxRepo` already proved: claim under a lease with
//! `FOR UPDATE SKIP LOCKED`, count attempts, back off, and never delete an unacked row.
//!
//! # The defect this deliberately does not repeat
//!
//! B-25, from the control plane's own review: **nothing anywhere reads `outbox WHERE
//! status = 'failed'`**, so a dropped delivery is silent. An outbox whose failures nobody reads is a
//! queue that loses data quietly and reports success. [`Outbox::stuck`] exists so that the managed
//! deployment can alarm on it, and [`Outbox::pending`] so an operator can see depth. Reliability
//! that is not observable is not reliability, and this is the cheapest possible version of making it
//! so.
//!
//! # At-least-once, and honest about it
//!
//! A row is acked after the sink returns success. If the process dies between the remote write and
//! the ack, the row is sent again — which is why every payload carries a dedupe key and why the
//! ingestion contracts are specified as idempotent. Exactly-once across two services does not exist
//! and claiming it would be the same lie as claiming exactly-once delivery.

use handoff_core::seam::{AuditEvent, EventSink, MeterReading, MeterSink};
use handoff_protocol::error::{ErrorCode, ProtocolError, Result};
use sqlx::{PgPool, Row};
use std::time::Duration;

/// What a queued row is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Destination {
    /// The org-level audit index.
    Event,
    /// The usage ledger.
    Meter,
}

impl Destination {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Event => "event",
            Self::Meter => "meter",
        }
    }

    fn parse(raw: &str) -> Result<Self> {
        match raw {
            "event" => Ok(Self::Event),
            "meter" => Ok(Self::Meter),
            other => Err(ProtocolError::new(
                ErrorCode::InvalidRequest,
                format!("unknown outbox destination `{other}`"),
            )),
        }
    }
}

/// How long a claim is held before another worker may take the row.
pub const LEASE: Duration = Duration::from_secs(60);

/// The backoff schedule, in seconds, indexed by attempt. The last entry repeats.
///
/// It tops out rather than growing forever: a mirror that has been failing for an hour needs a human
/// to look at it, not a slower retry.
const BACKOFF_SECS: &[i64] = &[5, 15, 60, 300, 900];

/// After this many attempts a row is *stuck* — still queued, still retried, and now visible.
///
/// It is never dropped. "Stuck" is a reporting threshold, not a deletion policy.
pub const STUCK_AFTER: i32 = 5;

/// One queued row.
#[derive(Debug, Clone, PartialEq)]
pub struct Queued {
    /// The row's own id.
    pub id: i64,
    /// Where it is going.
    pub destination: Destination,
    /// The tenant it belongs to.
    pub tenant: String,
    /// The dedupe key, unique per destination.
    pub dedupe_key: String,
    /// The payload, in the sink's own shape.
    pub payload: serde_json::Value,
    /// How many attempts have been made.
    pub attempts: i32,
}

/// The queue.
pub struct Outbox {
    pool: PgPool,
}

impl Outbox {
    /// Wrap a pool. The table is created here rather than in the open core's migration set, because
    /// it belongs to the managed deployment and a self-hosted operator must never inherit it.
    pub async fn open(pool: PgPool) -> Result<Self> {
        let outbox = Self { pool };
        outbox.migrate().await?;
        Ok(outbox)
    }

    async fn migrate(&self) -> Result<()> {
        // Additive and idempotent, applied at boot. The control plane's own review calls editing an
        // applied migration "the highest-consequence footgun in the repo" — this table is
        // create-if-absent for exactly that reason, and any change to it must be a new statement
        // rather than an edit to this one.
        for statement in [
            "create table if not exists handoff_managed_outbox (
                 id            bigserial primary key,
                 destination   text        not null,
                 tenant        text        not null,
                 dedupe_key    text        not null,
                 payload       jsonb       not null,
                 occurred_at   timestamptz not null,
                 attempts      integer     not null default 0,
                 next_attempt_at timestamptz not null default now(),
                 leased_until  timestamptz,
                 acked_at      timestamptz,
                 last_error    text,
                 unique (destination, dedupe_key)
             )",
            "create index if not exists handoff_managed_outbox_due
                 on handoff_managed_outbox (next_attempt_at)
                 where acked_at is null",
            "create table if not exists handoff_managed_cursor (
                 tenant       text primary key,
                 last_receipt text        not null,
                 updated_at   timestamptz not null default now()
             )",
        ] {
            sqlx::query(statement)
                .execute(&self.pool)
                .await
                .map_err(db("preparing the managed outbox"))?;
        }
        Ok(())
    }

    /// Queue one row, idempotently on `(destination, dedupe_key)`.
    ///
    /// A repeat is a no-op rather than a second row, so a reconciler that re-reads a receipt it
    /// already summarized does not double-count anyone's usage.
    pub async fn enqueue(
        &self,
        destination: Destination,
        tenant: &str,
        dedupe_key: &str,
        payload: serde_json::Value,
        occurred_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool> {
        let inserted = sqlx::query(
            "insert into handoff_managed_outbox
                 (destination, tenant, dedupe_key, payload, occurred_at)
             values ($1, $2, $3, $4, $5)
             on conflict (destination, dedupe_key) do nothing
             returning id",
        )
        .bind(destination.as_str())
        .bind(tenant)
        .bind(dedupe_key)
        .bind(&payload)
        .bind(occurred_at)
        .fetch_optional(&self.pool)
        .await
        .map_err(db("queueing an outbox row"))?;
        Ok(inserted.is_some())
    }

    /// Claim up to `limit` due rows, leasing them so two workers cannot both send one.
    pub async fn claim(&self, limit: i64) -> Result<Vec<Queued>> {
        let rows = sqlx::query(
            "with due as (
                 select id from handoff_managed_outbox
                 where acked_at is null
                   and next_attempt_at <= now()
                   and (leased_until is null or leased_until <= now())
                 order by next_attempt_at
                 limit $1
                 for update skip locked
             )
             update handoff_managed_outbox o
                set leased_until = now() + ($2 || ' seconds')::interval
               from due
              where o.id = due.id
              returning o.id, o.destination, o.tenant, o.dedupe_key, o.payload, o.attempts",
        )
        .bind(limit)
        .bind(LEASE.as_secs() as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(db("claiming outbox rows"))?;

        rows.into_iter()
            .map(|row| {
                Ok(Queued {
                    id: row.try_get("id").map_err(db("reading id"))?,
                    destination: Destination::parse(
                        row.try_get::<String, _>("destination")
                            .map_err(db("reading destination"))?
                            .as_str(),
                    )?,
                    tenant: row.try_get("tenant").map_err(db("reading tenant"))?,
                    dedupe_key: row
                        .try_get("dedupe_key")
                        .map_err(db("reading dedupe_key"))?,
                    payload: row.try_get("payload").map_err(db("reading payload"))?,
                    attempts: row.try_get("attempts").map_err(db("reading attempts"))?,
                })
            })
            .collect()
    }

    /// Mark a row delivered.
    pub async fn ack(&self, id: i64) -> Result<()> {
        sqlx::query(
            "update handoff_managed_outbox
                set acked_at = now(), leased_until = null, last_error = null
              where id = $1",
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(db("acking an outbox row"))?;
        Ok(())
    }

    /// Record a failed attempt and schedule the next one. **The row is never removed.**
    pub async fn defer(&self, row: &Queued, error: &str) -> Result<()> {
        let attempts = row.attempts.saturating_add(1);
        let backoff = BACKOFF_SECS[(attempts as usize)
            .saturating_sub(1)
            .min(BACKOFF_SECS.len() - 1)];
        sqlx::query(
            "update handoff_managed_outbox
                set attempts = $2,
                    next_attempt_at = now() + ($3 || ' seconds')::interval,
                    leased_until = null,
                    last_error = $4
              where id = $1",
        )
        .bind(row.id)
        .bind(attempts)
        .bind(backoff)
        .bind(truncate(error))
        .execute(&self.pool)
        .await
        .map_err(db("deferring an outbox row"))?;
        Ok(())
    }

    /// How many rows are still waiting.
    pub async fn pending(&self) -> Result<i64> {
        let row =
            sqlx::query("select count(*) as n from handoff_managed_outbox where acked_at is null")
                .fetch_one(&self.pool)
                .await
                .map_err(db("counting pending rows"))?;
        row.try_get("n").map_err(db("reading count"))
    }

    /// Rows that have failed enough times that somebody should look.
    ///
    /// This is the reader B-25 says does not exist in the control plane's own outbox, which is why a
    /// dropped delivery there is silent. Having the query is the whole point; a deployment wires it
    /// to an alarm.
    pub async fn stuck(&self) -> Result<Vec<(i64, String, String)>> {
        let rows = sqlx::query(
            "select id, dedupe_key, coalesce(last_error, '') as last_error
               from handoff_managed_outbox
              where acked_at is null and attempts >= $1
              order by id",
        )
        .bind(STUCK_AFTER)
        .fetch_all(&self.pool)
        .await
        .map_err(db("reading stuck rows"))?;
        rows.into_iter()
            .map(|row| {
                Ok((
                    row.try_get("id").map_err(db("reading id"))?,
                    row.try_get("dedupe_key")
                        .map_err(db("reading dedupe_key"))?,
                    row.try_get("last_error")
                        .map_err(db("reading last_error"))?,
                ))
            })
            .collect()
    }

    /// Where this tenant's reconciler got to.
    pub async fn cursor(&self, tenant: &str) -> Result<Option<String>> {
        let row = sqlx::query("select last_receipt from handoff_managed_cursor where tenant = $1")
            .bind(tenant)
            .fetch_optional(&self.pool)
            .await
            .map_err(db("reading the reconciler cursor"))?;
        row.map(|r| r.try_get("last_receipt").map_err(db("reading cursor")))
            .transpose()
    }

    /// Advance the cursor. Only ever called after the rows for that receipt are queued, so a crash
    /// re-reads rather than skips.
    pub async fn advance(&self, tenant: &str, last_receipt: &str) -> Result<()> {
        sqlx::query(
            "insert into handoff_managed_cursor (tenant, last_receipt)
             values ($1, $2)
             on conflict (tenant) do update
                set last_receipt = excluded.last_receipt, updated_at = now()",
        )
        .bind(tenant)
        .bind(last_receipt)
        .execute(&self.pool)
        .await
        .map_err(db("advancing the reconciler cursor"))?;
        Ok(())
    }

    /// Drain one batch through the two sinks.
    ///
    /// Returns how many rows were acked. A failure defers the row and continues to the next: one
    /// unreachable destination must not stall the other.
    pub async fn drain(
        &self,
        events: &dyn EventSink,
        meter: &dyn MeterSink,
        limit: i64,
    ) -> Result<usize> {
        let mut acked = 0;
        for row in self.claim(limit).await? {
            let sent = match row.destination {
                Destination::Event => {
                    match serde_json::from_value::<AuditEvent>(row.payload.clone()) {
                        Ok(event) => events.append(vec![event]).await,
                        Err(e) => Err(ProtocolError::new(
                            ErrorCode::InvalidRequest,
                            format!("a queued audit row does not parse: {e}"),
                        )),
                    }
                }
                Destination::Meter => {
                    match serde_json::from_value::<MeterReading>(row.payload.clone()) {
                        Ok(reading) => meter.record(vec![reading]).await,
                        Err(e) => Err(ProtocolError::new(
                            ErrorCode::InvalidRequest,
                            format!("a queued meter row does not parse: {e}"),
                        )),
                    }
                }
            };
            match sent {
                Ok(()) => {
                    self.ack(row.id).await?;
                    acked += 1;
                }
                Err(error) => {
                    tracing::warn!(
                        row = row.id,
                        attempts = row.attempts,
                        "outbox row deferred: {error}"
                    );
                    self.defer(&row, &error.to_string()).await?;
                }
            }
        }
        Ok(acked)
    }
}

fn db(what: &'static str) -> impl Fn(sqlx::Error) -> ProtocolError {
    move |e| {
        ProtocolError::new(
            ErrorCode::DeliveryUnavailable,
            format!("the managed store failed while {what}: {e}"),
        )
    }
}

fn truncate(text: &str) -> String {
    let mut cut = text.len().min(500);
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    text[..cut].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_backoff_tops_out_rather_than_growing_without_bound() {
        // A mirror that has been failing for an hour needs a human, not a slower retry.
        let at = |attempt: usize| BACKOFF_SECS[attempt.min(BACKOFF_SECS.len() - 1)];
        assert_eq!(at(0), 5);
        assert_eq!(at(4), 900);
        assert_eq!(at(99), 900);
    }

    #[test]
    fn a_destination_round_trips_and_an_unknown_one_is_refused() {
        for destination in [Destination::Event, Destination::Meter] {
            assert_eq!(
                Destination::parse(destination.as_str()).unwrap(),
                destination
            );
        }
        assert!(Destination::parse("slack").is_err());
    }

    #[test]
    fn an_error_is_truncated_on_a_character_boundary() {
        assert!(truncate(&"é".repeat(400)).len() <= 500);
        assert_eq!(truncate("short"), "short");
    }
}

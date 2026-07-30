//! The reference store.
//!
//! Every method that changes state is one transaction, and each of those transactions writes its
//! state row **and** its event row before it commits (I12). That is not a convention here; it is
//! the only shape the code has, because the store port exposes no way to write one without the
//! other.
//!
//! Two other rules are visible in almost every query:
//!
//! - `tenant_ref` appears in the `WHERE` clause of every statement (I17). Row-level security is
//!   enabled on top of that, and every request-scoped transaction names its tenant before it reads
//!   anything, so a query that lost its predicate still cannot see another tenant's rows.
//! - Settling is a **conditional write** — `… WHERE state = 'pending'` — never a read followed by a
//!   write (§6.2 R5). When it affects no row the current state is inspected and the specific `409`
//!   for it is returned (§6.7 rule 2).

use chrono::{DateTime, Utc};
use handoff_core::auth::{AuthPolicy, Principal, PrincipalKind};
use handoff_core::capability::{BlastRadius, CapabilityRegistry};
use handoff_core::channel::{rung_targets, ChannelRegistry};
use handoff_core::events;
use handoff_core::ids;
use handoff_core::model::*;
use handoff_core::plan::{self, AnswerInput};
use handoff_core::ports::{
    BoxFuture, CallbackJob, IdempotencySlot, ResolveGrant, Store, StoredResponse,
};
use handoff_protocol::authorization::{
    Authorization, AuthorizationBinding, RedeemRequest, Redemption,
};
use handoff_protocol::clock::{IsoDuration, Timestamp};
use handoff_protocol::delivery::{DeliveryGrade, DeliveryState};
use handoff_protocol::error::{ErrorCode, ProtocolError, Result};
use handoff_protocol::id::{
    AuthorizationId, DeliveryId, GrantHandle, ReceiptId, RequestId, ResumeToken, SignalId,
};
use handoff_protocol::receipt::PresentationBinding;
use handoff_protocol::receipt::{ChainHead, ChainLink, Digest, Receipt};
use handoff_protocol::request::{
    Callback, Continuation, Liveness, Mode, OnExpiry, OnWaiterTerminal, Prompt, RequestState,
    Routing, TtlPolicy, Urgency, UrgencyState,
};
use handoff_protocol::requires::{
    AuthStrength, CapabilityScope, DeploymentProfile, Requires, Role, Target, TargetKind,
};
use handoff_protocol::waiter::{Decision, Signal, SignalType, WaiterState};
use serde_json::{json, Map, Value};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, Row, Transaction};

use crate::migrations::{MIGRATIONS, RLS_HELPER};

/// The Postgres store.
pub struct PgStore {
    pool: PgPool,
    /// What this deployment will accept in a declaration (§19, `GET /meta`).
    pub profile: DeploymentProfile,
    /// Whether `link_only` may settle a request here (§4.4).
    pub policy: AuthPolicy,
    /// Channels and the default ladder (§7.4).
    pub channels: ChannelRegistry,
    /// Capability providers (§11.1).
    pub capabilities: CapabilityRegistry,
    /// A fault-injection point used only by the conformance harness. See [`PgStore::crash_point`].
    crash_point: Option<String>,
}

/// Where a deliberate crash is injected, for C-23.
///
/// §18 calls C-23 "the case an implementation is most tempted to skip", because emitting the event
/// just after the commit passes every happy-path test and only fails under a crash. Demonstrating
/// the opposite needs the process to die *between* the two writes, which is not something a
/// black-box client can arrange. So the conformance harness sets `HANDOFF_CRASH_POINT` and the
/// server aborts at the named point, inside the open transaction, and the assertion is that the
/// rollback took both writes with it.
///
/// Unset — which is every deployment that is not running the suite — this reads once at startup
/// and changes nothing.
pub const CRASH_AFTER_ANSWER_STATE_WRITE: &str = "answer_after_state_write";

impl PgStore {
    /// Connect, run the migrations, and return a ready store.
    ///
    /// `max_connections` is explicit rather than fixed because the same store backs two very
    /// different processes: a server that holds long polls and wants a real pool, and a one-shot
    /// subcommand that runs a single query and exits. A `handoffd verify-chain` that reserves
    /// sixteen backends is how a handful of concurrent invocations exhaust a database's connection
    /// budget and produce failures that look like anything except what they are.
    pub async fn connect_with(
        database_url: &str,
        max_connections: u32,
        profile: DeploymentProfile,
        policy: AuthPolicy,
        channels: ChannelRegistry,
        capabilities: CapabilityRegistry,
    ) -> Result<Self> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(max_connections.max(1))
            .connect(database_url)
            .await
            .map_err(db)?;
        let store = Self {
            pool,
            profile,
            policy,
            channels,
            capabilities,
            crash_point: std::env::var("HANDOFF_CRASH_POINT")
                .ok()
                .filter(|s| !s.is_empty()),
        };
        store.migrate().await?;
        Ok(store)
    }

    /// The connection pool, for the maintenance subcommands.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Apply every migration that has not run yet.
    pub async fn migrate(&self) -> Result<()> {
        sqlx::query(
            "create table if not exists handoff_migrations (
                number integer primary key,
                name text not null,
                applied_at timestamptz not null default now())",
        )
        .execute(&self.pool)
        .await
        .map_err(db)?;
        sqlx::raw_sql(RLS_HELPER)
            .execute(&self.pool)
            .await
            .map_err(db)?;

        for migration in MIGRATIONS {
            let applied: Option<i32> =
                sqlx::query_scalar("select number from handoff_migrations where number = $1")
                    .bind(migration.number)
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(db)?;
            if applied.is_some() {
                continue;
            }
            let mut tx = self.pool.begin().await.map_err(db)?;
            sqlx::raw_sql(migration.sql)
                .execute(&mut *tx)
                .await
                .map_err(db)?;
            sqlx::query("insert into handoff_migrations (number, name) values ($1, $2)")
                .bind(migration.number)
                .bind(migration.name)
                .execute(&mut *tx)
                .await
                .map_err(db)?;
            tx.commit().await.map_err(db)?;
            tracing::info!(
                number = migration.number,
                name = migration.name,
                "migration applied"
            );
        }
        Ok(())
    }

    /// Begin a transaction that has named its tenant.
    ///
    /// The `set_config` is what makes row-level security bite: from here on the connection can only
    /// see this tenant's rows, whatever the statements forget to say.
    pub(crate) async fn tenant_tx(&self, tenant: &str) -> Result<Transaction<'_, Postgres>> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        sqlx::query("select set_config('handoff.tenant_ref', $1, true)")
            .bind(tenant)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        Ok(tx)
    }

    /// Serialize a tenant's chain writes, so two receipts cannot claim one height.
    async fn lock_chain(tx: &mut Transaction<'_, Postgres>, tenant: &str) -> Result<()> {
        sqlx::query("select pg_advisory_xact_lock(hashtext($1))")
            .bind(tenant)
            .execute(&mut **tx)
            .await
            .map_err(db)?;
        Ok(())
    }
}

// ------------------------------------------------------------------------------------- helpers

/// Turn a driver failure into a protocol error without leaking a query into a response body.
fn db(e: sqlx::Error) -> ProtocolError {
    tracing::error!(error = %e, "store failure");
    ProtocolError::new(
        ErrorCode::InvalidRequest,
        "the store rejected this operation",
    )
}

pub(crate) fn to_chrono(ts: Timestamp) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(ts.to_millis()).unwrap_or_else(Utc::now)
}

pub(crate) fn from_chrono(dt: DateTime<Utc>) -> Timestamp {
    Timestamp::from_millis(dt.timestamp_millis())
        .unwrap_or_else(|| Timestamp::from_millis(0).expect("epoch is representable"))
}

fn opt_chrono(ts: Option<Timestamp>) -> Option<DateTime<Utc>> {
    ts.map(to_chrono)
}

fn parse_json<T: serde::de::DeserializeOwned>(value: Value, what: &str) -> Result<T> {
    serde_json::from_value(value).map_err(|e| {
        ProtocolError::new(
            ErrorCode::InvalidRequest,
            format!("stored {what} is not readable: {e}"),
        )
    })
}

fn to_json<T: serde::Serialize>(value: &T, what: &str) -> Result<Value> {
    serde_json::to_value(value).map_err(|e| {
        ProtocolError::new(
            ErrorCode::InvalidRequest,
            format!("{what} is not serializable: {e}"),
        )
    })
}

fn not_found(what: ErrorCode) -> ProtocolError {
    ProtocolError::new(what, "no such object in this tenant")
}

/// The `409` a settled request answers a late write with (§6.7 rule 2).
fn settled_conflict(
    state: RequestState,
    receipt_id: Option<&str>,
    superseded_by: Option<&str>,
) -> ProtocolError {
    let code = state
        .settled_error_code()
        .unwrap_or(ErrorCode::AlreadyAnswered);
    let mut err = ProtocolError::new(code, format!("this request is {state:?}").to_lowercase());
    if let Some(id) = receipt_id {
        err = err.with_receipt(id);
    }
    if let Some(id) = superseded_by {
        err = err.with_superseded_by(id);
    }
    err
}

fn state_name(state: RequestState) -> &'static str {
    match state {
        RequestState::Pending => "pending",
        RequestState::Answered => "answered",
        RequestState::Expired => "expired",
        RequestState::Cancelled => "cancelled",
        RequestState::Superseded => "superseded",
    }
}

fn parse_state(text: &str) -> RequestState {
    match text {
        "answered" => RequestState::Answered,
        "expired" => RequestState::Expired,
        "cancelled" => RequestState::Cancelled,
        "superseded" => RequestState::Superseded,
        _ => RequestState::Pending,
    }
}

pub(crate) fn grade_name(grade: DeliveryGrade) -> &'static str {
    match grade {
        DeliveryGrade::Dispatched => "dispatched",
        DeliveryGrade::Delivered => "delivered",
        DeliveryGrade::Seen => "seen",
        DeliveryGrade::Acted => "acted",
    }
}

pub(crate) fn parse_grade(text: &str) -> DeliveryGrade {
    match text {
        "delivered" => DeliveryGrade::Delivered,
        "seen" => DeliveryGrade::Seen,
        "acted" => DeliveryGrade::Acted,
        _ => DeliveryGrade::Dispatched,
    }
}

pub(crate) fn delivery_state_name(state: DeliveryState) -> String {
    serde_json::to_value(state)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "queued".into())
}

pub(crate) fn parse_delivery_state(text: &str) -> DeliveryState {
    serde_json::from_value(Value::String(text.to_string())).unwrap_or(DeliveryState::Queued)
}

fn enum_name<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn parse_enum<T: serde::de::DeserializeOwned>(text: &str, fallback: T) -> T {
    serde_json::from_value(Value::String(text.to_string())).unwrap_or(fallback)
}

/// How many times one signal is pushed before its endpoint is disabled.
///
/// `signing.md` §1.5 RECOMMENDS 12, "spanning roughly 24 hours" against the `2^n` backoff below.
pub const MAX_CALLBACK_ATTEMPTS: u32 = 12;

/// Exponential backoff with jitter, `2^n` seconds capped at 300 (§7.3, `signing.md` §1.5).
///
/// Flat retries synchronize: every worker wakes at the same instant and hits an endpoint that is
/// already struggling. The jitter is what spreads them.
pub fn backoff_seconds(attempt: u32) -> i64 {
    let base = 2i64.saturating_pow(attempt.min(9)).min(300);
    let jitter = (attempt as i64 * 7) % 3;
    base + jitter
}

// -------------------------------------------------------------------------------- row mapping

pub(crate) fn row_to_delivery(row: &PgRow) -> Result<DeliveryView> {
    Ok(DeliveryView {
        id: DeliveryId::parse(row.get::<String, _>("id").as_str())?,
        request_id: RequestId::parse(row.get::<String, _>("request_id").as_str())?,
        channel: row.get("channel"),
        target: Target {
            kind: parse_target_kind(&row.get::<String, _>("target_kind")),
            value: row.get("target_value"),
        },
        rung: row.get::<i32, _>("rung") as u32,
        state: parse_delivery_state(&row.get::<String, _>("state")),
        grade_reached: row
            .get::<Option<String>, _>("grade_reached")
            .map(|g| parse_grade(&g)),
        max_grade: parse_grade(&row.get::<String, _>("max_grade")),
        can_authenticate_person: row.get("can_authenticate_person"),
        attempts: Vec::new(),
        created_at: from_chrono(row.get("created_at")),
        updated_at: from_chrono(row.get("updated_at")),
    })
}

pub(crate) fn parse_target_kind(text: &str) -> TargetKind {
    match text {
        "principal" => TargetKind::Principal,
        "role" => TargetKind::Role,
        "group" => TargetKind::Group,
        "rotation" => TargetKind::Rotation,
        _ => TargetKind::Anyone,
    }
}

pub(crate) fn target_kind_name(kind: TargetKind) -> &'static str {
    match kind {
        TargetKind::Principal => "principal",
        TargetKind::Role => "role",
        TargetKind::Group => "group",
        TargetKind::Rotation => "rotation",
        TargetKind::Anyone => "anyone",
    }
}

fn row_to_signal(row: &PgRow) -> Result<Signal> {
    Ok(Signal {
        id: SignalId::parse(row.get::<String, _>("id").as_str())?,
        request_id: RequestId::parse(row.get::<String, _>("request_id").as_str())?,
        waiter_ref: row.get("waiter_ref"),
        signal_type: parse_signal_type(&row.get::<String, _>("type")),
        sequence: row.get::<i64, _>("sequence") as u64,
        resume_token: ResumeToken::parse(row.get::<String, _>("resume_token").as_str())?,
        decision: match row.get::<Option<Value>, _>("decision") {
            Some(v) if !v.is_null() => Some(parse_json::<Decision>(v, "decision")?),
            _ => None,
        },
        resume_ref: row.get("resume_ref"),
        resume_payload: row.get("resume_payload"),
        attempts: row.get::<i32, _>("attempts") as u64,
        created_at: from_chrono(row.get("created_at")),
        acked_at: row
            .get::<Option<DateTime<Utc>>, _>("acked_at")
            .map(from_chrono),
    })
}

fn parse_signal_type(text: &str) -> SignalType {
    match text {
        "expired" => SignalType::Expired,
        "cancelled" => SignalType::Cancelled,
        "superseded" => SignalType::Superseded,
        "attempt_lapsed" => SignalType::AttemptLapsed,
        _ => SignalType::Answered,
    }
}

fn signal_type_name(t: SignalType) -> &'static str {
    match t {
        SignalType::Answered => "answered",
        SignalType::Expired => "expired",
        SignalType::Cancelled => "cancelled",
        SignalType::Superseded => "superseded",
        SignalType::AttemptLapsed => "attempt_lapsed",
    }
}

fn row_to_grant(row: &PgRow) -> Result<GrantView> {
    Ok(GrantView {
        handle: GrantHandle::parse(row.get::<String, _>("handle").as_str())?,
        request_id: RequestId::parse(row.get::<String, _>("request_id").as_str())?,
        capability_type: row.get("capability_type"),
        scope: if row.get::<String, _>("scope") == "drive" {
            CapabilityScope::Drive
        } else {
            CapabilityScope::View
        },
        provider: row.get("provider"),
        resource_ref: row.get("resource_ref"),
        label: row.get("label"),
        purpose: row.get("purpose"),
        optional: row.get("optional"),
        blast_radius: parse_json::<BlastRadius>(row.get("blast_radius"), "blast radius")?,
        blast_radius_digest: Digest::parse(row.get::<String, _>("blast_radius_digest").as_str())?,
        expires_at: from_chrono(row.get("expires_at")),
        revoked_at: row
            .get::<Option<DateTime<Utc>>, _>("revoked_at")
            .map(from_chrono),
        max_holders: row.get("max_holders"),
        bound_principal: row.get("bound_principal"),
    })
}

fn row_to_authorization(row: &PgRow) -> Result<Authorization> {
    let mut authorization = Authorization {
        id: AuthorizationId::parse(row.get::<String, _>("id").as_str())?,
        receipt_id: ReceiptId::parse(row.get::<String, _>("receipt_id").as_str())?,
        request_id: RequestId::parse(row.get::<String, _>("request_id").as_str())?,
        grants: parse_json::<Map<String, Value>>(row.get("grants"), "authorization grants")?,
        single_use: row.get("single_use"),
        expires_at: row
            .get::<Option<DateTime<Utc>>, _>("expires_at")
            .map(from_chrono),
        bound_to: AuthorizationBinding {
            waiter_ref: row.get("waiter_ref"),
            effect_digest: match row.get::<Option<String>, _>("effect_digest") {
                Some(d) => Some(Digest::parse(&d)?),
                None => None,
            },
        },
        redemptions: Vec::new(),
    };
    authorization.redemptions = Vec::new();
    Ok(authorization)
}

impl PgStore {
    async fn load_request(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        tenant: &str,
        id: RequestId,
    ) -> Result<Option<RequestView>> {
        let row = sqlx::query("select * from handoff_requests where tenant_ref = $1 and id = $2")
            .bind(tenant)
            .bind(id.to_string())
            .fetch_optional(&mut **tx)
            .await
            .map_err(db)?;
        match row {
            Some(row) => Ok(Some(self.hydrate(tx, tenant, row).await?)),
            None => Ok(None),
        }
    }

    async fn hydrate(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        tenant: &str,
        row: PgRow,
    ) -> Result<RequestView> {
        let id = RequestId::parse(row.get::<String, _>("id").as_str())?;

        let delivery_rows =
            sqlx::query("select * from handoff_deliveries where tenant_ref = $1 and request_id = $2 order by created_at, id")
                .bind(tenant)
                .bind(id.to_string())
                .fetch_all(&mut **tx)
                .await
                .map_err(db)?;
        let mut deliveries = Vec::with_capacity(delivery_rows.len());
        for row in &delivery_rows {
            let mut delivery = row_to_delivery(row)?;
            let attempts = sqlx::query(
                "select * from handoff_delivery_attempts where tenant_ref = $1 and delivery_id = $2 order by n",
            )
            .bind(tenant)
            .bind(delivery.id.to_string())
            .fetch_all(&mut **tx)
            .await
            .map_err(db)?;
            delivery.attempts = attempts
                .iter()
                .map(|a| DeliveryAttemptView {
                    n: a.get::<i32, _>("n") as u32,
                    started_at: from_chrono(a.get("started_at")),
                    ended_at: a
                        .get::<Option<DateTime<Utc>>, _>("ended_at")
                        .map(from_chrono),
                    outcome: a.get("outcome"),
                    transport_status: a.get("transport_status"),
                    error: a.get("error"),
                })
                .collect();
            deliveries.push(delivery);
        }

        let receipt_row = sqlx::query(
            "select body from handoff_receipts where tenant_ref = $1 and request_id = $2 \
             order by height limit 1",
        )
        .bind(tenant)
        .bind(id.to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(db)?;
        let receipt = match receipt_row {
            Some(row) => Some(parse_json::<Receipt>(row.get("body"), "receipt")?),
            None => None,
        };

        let authorization = match sqlx::query(
            "select * from handoff_authorizations where tenant_ref = $1 and request_id = $2 limit 1",
        )
        .bind(tenant)
        .bind(id.to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(db)?
        {
            Some(row) => {
                let mut authorization = row_to_authorization(&row)?;
                authorization.redemptions = self
                    .load_redemptions(tx, tenant, authorization.id)
                    .await?;
                Some(authorization)
            }
            None => None,
        };

        let waiter_ref: String = row.get("waiter_ref");
        let waiter_row = sqlx::query(
            "select state from handoff_waiters where tenant_ref = $1 and waiter_ref = $2",
        )
        .bind(tenant)
        .bind(&waiter_ref)
        .fetch_optional(&mut **tx)
        .await
        .map_err(db)?;

        let liveness: Liveness = parse_enum(&row.get::<String, _>("liveness"), Liveness::Durable);
        Ok(RequestView {
            id,
            tenant_ref: row.get("tenant_ref"),
            waiter_ref,
            state: parse_state(&row.get::<String, _>("state")),
            version: row.get::<i64, _>("version") as u64,
            urgency: parse_enum(&row.get::<String, _>("urgency"), Urgency::Normal),
            urgency_state: parse_enum(
                &row.get::<String, _>("urgency_state"),
                UrgencyState::Attention,
            ),
            prompt: parse_json::<Prompt>(row.get("prompt"), "prompt")?,
            requires: parse_json::<Requires>(row.get("requires"), "requires")?,
            mode: parse_enum(&row.get::<String, _>("mode"), Mode::Advisory),
            presentation_binding: parse_enum(
                &row.get::<String, _>("presentation_binding"),
                PresentationBinding::Advisory,
            ),
            liveness,
            on_waiter_terminal: parse_enum(
                &row.get::<String, _>("on_waiter_terminal"),
                OnWaiterTerminal::Keep,
            ),
            ttl_policy: match row.get::<Option<Value>, _>("ttl_policy") {
                Some(v) if !v.is_null() => Some(parse_json::<TtlPolicy>(v, "ttl policy")?),
                _ => None,
            },
            routing: parse_json::<Routing>(row.get("routing"), "routing")?,
            attempt_ttl: IsoDuration::from_secs(row.get::<i64, _>("attempt_ttl_secs") as u64),
            created_at: from_chrono(row.get("created_at")),
            expires_at: row
                .get::<Option<DateTime<Utc>>, _>("expires_at")
                .map(from_chrono),
            attempt_expires_at: row
                .get::<Option<DateTime<Utc>>, _>("attempt_expires_at")
                .map(from_chrono),
            answered_at: row
                .get::<Option<DateTime<Utc>>, _>("answered_at")
                .map(from_chrono),
            superseded_by: match row.get::<Option<String>, _>("superseded_by") {
                Some(id) => Some(RequestId::parse(&id)?),
                None => None,
            },
            cancel_reason: row.get("cancel_reason"),
            deliveries,
            receipt,
            authorization,
            waiter: WaiterView {
                state: waiter_row
                    .map(|w| parse_waiter_state(&w.get::<String, _>("state")))
                    .unwrap_or(WaiterState::Armed),
                liveness,
            },
            metadata: parse_json::<Map<String, Value>>(row.get("metadata"), "metadata")?,
            rung: row.get::<i32, _>("rung") as u32,
            request_digest: Digest::parse(row.get::<String, _>("request_digest").as_str())?,
            rendered_digest: Digest::parse(row.get::<String, _>("rendered_digest").as_str())?,
            rendered_ref: row.get("rendered_ref"),
            callback: row
                .get::<Option<String>, _>("callback_url")
                .map(|url| Callback {
                    url,
                    secret_ref: None,
                }),
            continuation: Continuation {
                resume_ref: row.get("resume_ref"),
                resume_payload: row
                    .get::<Option<String>, _>("resume_payload")
                    .and_then(|p| handoff_protocol::request::base64_decode(&p).ok()),
            },
            attempt_lapse_notified: row.get("attempt_lapse_notified"),
        })
    }

    async fn load_redemptions(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        tenant: &str,
        id: AuthorizationId,
    ) -> Result<Vec<Redemption>> {
        let rows = sqlx::query(
            "select effect_key, redeemed_at from handoff_redemptions \
             where tenant_ref = $1 and authorization_id = $2 order by id",
        )
        .bind(tenant)
        .bind(id.to_string())
        .fetch_all(&mut **tx)
        .await
        .map_err(db)?;
        Ok(rows
            .iter()
            .map(|r| Redemption {
                effect_key: r.get("effect_key"),
                redeemed_at: from_chrono(r.get("redeemed_at")),
            })
            .collect())
    }

    /// Write an event. Only ever called from inside the transaction that wrote the state (I12).
    pub(crate) async fn emit(
        tx: &mut Transaction<'_, Postgres>,
        tenant: &str,
        request_id: Option<&str>,
        event_type: &str,
        payload: Value,
        now: Timestamp,
    ) -> Result<()> {
        sqlx::query(
            "insert into handoff_events (tenant_ref, request_id, type, payload, created_at) \
             values ($1, $2, $3, $4, $5)",
        )
        .bind(tenant)
        .bind(request_id)
        .bind(event_type)
        .bind(payload)
        .bind(to_chrono(now))
        .execute(&mut **tx)
        .await
        .map_err(db)?;
        Ok(())
    }

    /// Meter one intervention. Tenant-scoped like everything else (§3.2, Appendix A).
    async fn meter(
        tx: &mut Transaction<'_, Postgres>,
        tenant: &str,
        metric: &str,
        request_id: Option<&str>,
        now: Timestamp,
    ) -> Result<()> {
        sqlx::query(
            "insert into handoff_usage (tenant_ref, metric, quantity, request_id, created_at) \
             values ($1, $2, 1, $3, $4)",
        )
        .bind(tenant)
        .bind(metric)
        .bind(request_id)
        .bind(to_chrono(now))
        .execute(&mut **tx)
        .await
        .map_err(db)?;
        Ok(())
    }

    /// Enqueue exactly one signal, in the transaction that changed the state (W2, I11, I12).
    async fn enqueue_signal(
        tx: &mut Transaction<'_, Postgres>,
        tenant: &str,
        signal: SignalToEnqueue<'_>,
        now: Timestamp,
    ) -> Result<SignalId> {
        let SignalToEnqueue {
            waiter_ref,
            request_id,
            signal_type,
            decision,
            continuation,
            callback_url,
        } = signal;
        let sequence: i64 = sqlx::query_scalar(
            "update handoff_waiters set highest_sequence = highest_sequence + 1, \
             state = case when $3 then 'signalled' else state end, updated_at = $4 \
             where tenant_ref = $1 and waiter_ref = $2 returning highest_sequence",
        )
        .bind(tenant)
        .bind(waiter_ref)
        .bind(true)
        .bind(to_chrono(now))
        .fetch_one(&mut **tx)
        .await
        .map_err(db)?;

        let id = ids::mint::<handoff_protocol::id::Signal>(now.to_millis() as u64)?;
        let token = ids::mint::<handoff_protocol::id::Resume>(now.to_millis() as u64)?;
        sqlx::query(
            "insert into handoff_signals \
             (id, tenant_ref, waiter_ref, request_id, type, sequence, resume_token, decision, \
              resume_ref, resume_payload, created_at, next_callback_at, callback_url) \
             values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)",
        )
        .bind(id.to_string())
        .bind(tenant)
        .bind(waiter_ref)
        .bind(request_id.to_string())
        .bind(signal_type_name(signal_type))
        .bind(sequence)
        .bind(token.to_string())
        .bind(match decision {
            Some(d) => Some(to_json(d, "decision")?),
            None => None,
        })
        .bind(continuation.resume_ref.as_deref())
        .bind(
            continuation
                .resume_payload
                .as_ref()
                .map(|b| handoff_protocol::request::base64_encode(b)),
        )
        .bind(to_chrono(now))
        .bind(callback_url.map(|_| to_chrono(now)))
        .bind(callback_url)
        .execute(&mut **tx)
        .await
        .map_err(db)?;
        Ok(id)
    }

    /// Cancel every open delivery and revoke every open grant, as a terminal transition must
    /// (§6.2 R5–R8, §11.4).
    async fn close_out(
        tx: &mut Transaction<'_, Postgres>,
        tenant: &str,
        request_id: RequestId,
        now: Timestamp,
    ) -> Result<()> {
        sqlx::query(
            "update handoff_deliveries set state = case when state in ('queued','sending','retrying') \
             then 'cancelled' else 'stale' end, updated_at = $3 \
             where tenant_ref = $1 and request_id = $2 \
             and state not in ('acted','cancelled','stale','failed','bounced','suppressed')",
        )
        .bind(tenant)
        .bind(request_id.to_string())
        .bind(to_chrono(now))
        .execute(&mut **tx)
        .await
        .map_err(db)?;

        sqlx::query(
            "update handoff_grants set revoked_at = $3, revoke_reason = 'the request settled' \
             where tenant_ref = $1 and request_id = $2 and revoked_at is null",
        )
        .bind(tenant)
        .bind(request_id.to_string())
        .bind(to_chrono(now))
        .execute(&mut **tx)
        .await
        .map_err(db)?;
        Ok(())
    }

    /// Seal a receipt into the tenant's chain and insert it (§9.4).
    async fn append_receipt(
        tx: &mut Transaction<'_, Postgres>,
        tenant: &str,
        receipt: Receipt,
    ) -> Result<Receipt> {
        Self::lock_chain(tx, tenant).await?;
        let previous = sqlx::query(
            "select height, prev_digest, digest from handoff_receipts \
             where tenant_ref = $1 order by height desc limit 1",
        )
        .bind(tenant)
        .fetch_optional(&mut **tx)
        .await
        .map_err(db)?;

        let previous_link = match previous {
            Some(row) => Some(ChainLink {
                height: row.get::<i64, _>("height") as u64,
                prev_digest: match row.get::<Option<String>, _>("prev_digest") {
                    Some(d) => Some(Digest::parse(&d)?),
                    None => None,
                },
                digest: Digest::parse(row.get::<String, _>("digest").as_str())?,
            }),
            None => None,
        };

        let sealed = receipt.seal(previous_link.as_ref())?;
        let chain = sealed.chain.as_ref().ok_or_else(|| {
            ProtocolError::new(ErrorCode::InvalidRequest, "sealing produced no chain link")
        })?;

        sqlx::query(
            "insert into handoff_receipts \
             (id, tenant_ref, request_id, kind, height, prev_digest, digest, decided_at, decision, body) \
             values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
        )
        .bind(sealed.id.to_string())
        .bind(tenant)
        .bind(sealed.request_id.to_string())
        .bind(enum_name(&sealed.kind))
        .bind(chain.height as i64)
        .bind(chain.prev_digest.as_ref().map(ToString::to_string))
        .bind(chain.digest.to_string())
        .bind(to_chrono(sealed.decided_at))
        .bind(to_json(&sealed.decision, "receipt decision")?)
        .bind(to_json(&sealed, "receipt")?)
        .execute(&mut **tx)
        .await
        .map_err(db)?;
        Ok(sealed)
    }

    /// Mint the deliveries one rung produces.
    async fn mint_rung(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        tenant: &str,
        request_id: RequestId,
        routing: &Routing,
        rung_index: u32,
        now: Timestamp,
    ) -> Result<usize> {
        let Some(rung) = routing.ladder.get(rung_index as usize) else {
            return Ok(0);
        };
        let targets = rung_targets(routing, rung);
        let mut minted = 0;
        for channel in &rung.channels {
            let capabilities = self.channels.capabilities(channel);
            for target in &targets {
                let id = ids::mint::<handoff_protocol::id::Delivery>(now.to_millis() as u64)?;
                sqlx::query(
                    "insert into handoff_deliveries \
                     (id, tenant_ref, request_id, channel, target_kind, target_value, rung, state, \
                      grade_reached, max_grade, can_authenticate_person, created_at, updated_at, \
                      next_attempt_at) \
                     values ($1,$2,$3,$4,$5,$6,$7,'queued',null,$8,$9,$10,$10,$10)",
                )
                .bind(id.to_string())
                .bind(tenant)
                .bind(request_id.to_string())
                .bind(channel)
                .bind(target_kind_name(target.kind))
                .bind(&target.value)
                .bind(rung_index as i32)
                .bind(grade_name(capabilities.max_grade))
                .bind(capabilities.can_authenticate_person)
                .bind(to_chrono(now))
                .execute(&mut **tx)
                .await
                .map_err(db)?;
                minted += 1;
            }
        }
        Ok(minted)
    }

    /// Append the step record that preserves what a person could have been shown (§9.2).
    async fn append_step(
        tx: &mut Transaction<'_, Postgres>,
        tenant: &str,
        step: StepToAppend<'_>,
        now: Timestamp,
    ) -> Result<()> {
        let StepToAppend {
            request_id,
            n,
            prompt,
            requires,
            rendered_digest,
            rendered_ref,
        } = step;
        sqlx::query(
            "insert into handoff_request_steps \
             (tenant_ref, request_id, n, requires_snapshot, prompt_snapshot, rendered_digest, \
              rendered_ref, created_at) values ($1,$2,$3,$4,$5,$6,$7,$8) \
             on conflict (tenant_ref, request_id, n) do nothing",
        )
        .bind(tenant)
        .bind(request_id.to_string())
        .bind(n as i64)
        .bind(to_json(requires, "requires")?)
        .bind(to_json(prompt, "prompt")?)
        .bind(rendered_digest.to_string())
        .bind(rendered_ref)
        .bind(to_chrono(now))
        .execute(&mut **tx)
        .await
        .map_err(db)?;
        Ok(())
    }
}

fn parse_waiter_state(text: &str) -> WaiterState {
    match text {
        "signalled" => WaiterState::Signalled,
        "delivering" => WaiterState::Delivering,
        "acked" => WaiterState::Acked,
        "orphaned" => WaiterState::Orphaned,
        "released" => WaiterState::Released,
        _ => WaiterState::Armed,
    }
}

/// One signal, ready to enqueue.
///
/// Grouped rather than passed loose because every field is required together: a signal without its
/// waiter, its request, and its type is not a partial signal, it is a bug.
struct SignalToEnqueue<'a> {
    /// The waiter it is enqueued for.
    waiter_ref: &'a str,
    /// The request whose state changed.
    request_id: RequestId,
    /// What happened.
    signal_type: SignalType,
    /// The typed decision. `None` only for `attempt_lapsed`, which decides nothing (§8.3).
    decision: Option<&'a Decision>,
    /// The Level 2 fields, carried verbatim (§14).
    continuation: &'a Continuation,
    /// Where to push it, if a callback was registered.
    callback_url: Option<&'a str>,
}

/// One step record, preserving what a person could have been shown at a version (§9.2).
struct StepToAppend<'a> {
    /// The request.
    request_id: RequestId,
    /// The version this step records.
    n: u64,
    /// The prompt as it stood.
    prompt: &'a Prompt,
    /// The declarations as they stood.
    requires: &'a Requires,
    /// The digest of that rendering.
    rendered_digest: &'a Digest,
    /// The retained rendering's opaque pointer.
    rendered_ref: &'a str,
}

/// The principal reference stored on an idempotency row.
fn principal_ref(principal: &Principal) -> String {
    principal
        .id
        .map(|id| id.to_string())
        .unwrap_or_else(|| format!("{}::anonymous", principal.tenant_ref))
}

// ------------------------------------------------------------------------------ the store port

impl Store for PgStore {
    fn raise(&self, command: RaiseCommand) -> BoxFuture<'_, Result<RaiseResult>> {
        Box::pin(async move {
            let tenant = command.principal.tenant_ref.clone();
            let principal = principal_ref(&command.principal);
            let now = command.now;
            let mut tx = self.tenant_tx(&tenant).await?;

            // §3.3 rule 1 and 2. A matching key with the same body returns the stored request **in
            // its current state** — a retried raise after a person has answered returns the
            // answered request and its receipt, and does not re-ask.
            if let Some(key) = command.idempotency_key.as_deref() {
                let row = sqlx::query(
                    "select body_digest, request_id from handoff_idempotency \
                     where tenant_ref = $1 and principal_ref = $2 and operation = 'raise' and key = $3",
                )
                .bind(&tenant)
                .bind(&principal)
                .bind(key)
                .fetch_optional(&mut *tx)
                .await
                .map_err(db)?;
                if let Some(row) = row {
                    if row.get::<String, _>("body_digest") != command.body_digest.to_string() {
                        return Err(ProtocolError::new(
                            ErrorCode::IdempotencyKeyReused,
                            "this Idempotency-Key was used with a different body",
                        ));
                    }
                    if let Some(id) = row.get::<Option<String>, _>("request_id") {
                        let id = RequestId::parse(&id)?;
                        if let Some(request) = self.load_request(&mut tx, &tenant, id).await? {
                            tx.commit().await.map_err(db)?;
                            return Ok(RaiseResult {
                                request,
                                status: 200,
                            });
                        }
                    }
                }
            }

            // §3.3 rule 3. Ask-once: a `pending` request with this `dedupe_key` is the same
            // request, and the newer raise merges forward so the person sees the newest
            // description rather than the first one.
            let existing = sqlx::query(
                "select id from handoff_requests \
                 where tenant_ref = $1 and dedupe_key = $2 and state = 'pending' for update",
            )
            .bind(&tenant)
            .bind(&command.dedupe_key)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?;

            if let Some(row) = existing {
                let id = RequestId::parse(row.get::<String, _>("id").as_str())?;
                let prompt = to_json(&command.raise.prompt, "prompt")?;
                let requires = to_json(&command.raise.requires, "requires")?;
                let version: i64 = sqlx::query_scalar(
                    "update handoff_requests set prompt = $3, requires = $4, version = version + 1, \
                     request_digest = $5, rendered_digest = $6 \
                     where tenant_ref = $1 and id = $2 returning version",
                )
                .bind(&tenant)
                .bind(id.to_string())
                .bind(&prompt)
                .bind(&requires)
                .bind(command.raise.digest()?.to_string())
                .bind("sha256:0")
                .fetch_one(&mut *tx)
                .await
                .map_err(db)?;

                let rendered = plan::rendered_digest(&prompt, &requires, version as u64)?;
                sqlx::query(
                    "update handoff_requests set rendered_digest = $3 where tenant_ref = $1 and id = $2",
                )
                .bind(&tenant)
                .bind(id.to_string())
                .bind(rendered.to_string())
                .execute(&mut *tx)
                .await
                .map_err(db)?;

                Self::append_step(
                    &mut tx,
                    &tenant,
                    StepToAppend {
                        request_id: id,
                        n: version as u64,
                        prompt: &command.raise.prompt,
                        requires: &command.raise.requires,
                        rendered_digest: &rendered,
                        rendered_ref: &format!("render:{id}:{version}"),
                    },
                    now,
                )
                .await?;

                // §3.3 rule 3 records the merge as an amendment, so the event is R2's.
                Self::emit(
                    &mut tx,
                    &tenant,
                    Some(&id.to_string()),
                    events::REQUEST_AMENDED,
                    json!({"reason": "dedupe_key merged forward", "version": version}),
                    now,
                )
                .await?;

                if let Some(key) = command.idempotency_key.as_deref() {
                    remember_raise(
                        &mut tx,
                        &tenant,
                        &principal,
                        key,
                        &command.body_digest,
                        &id,
                        now,
                    )
                    .await?;
                }

                let request = self
                    .load_request(&mut tx, &tenant, id)
                    .await?
                    .ok_or_else(|| not_found(ErrorCode::RequestNotFound))?;
                tx.commit().await.map_err(db)?;
                return Ok(RaiseResult {
                    request,
                    status: 200,
                });
            }

            // §3.3 rule 4. A new request.
            let id = ids::mint::<handoff_protocol::id::Request>(now.to_millis() as u64)?;
            let prompt = to_json(&command.raise.prompt, "prompt")?;
            let requires = to_json(&command.raise.requires, "requires")?;
            let rendered = plan::rendered_digest(&prompt, &requires, 1)?;
            let rendered_ref = format!("render:{id}:1");

            sqlx::query(
                "insert into handoff_requests \
                 (id, tenant_ref, waiter_ref, state, version, urgency, urgency_state, liveness, \
                  on_waiter_terminal, mode, presentation_binding, dedupe_key, prompt, requires, \
                  ttl_policy, routing, attempt_ttl_secs, metadata, callback_url, resume_ref, \
                  resume_payload, test_mode, requester_principal, request_digest, rendered_digest, \
                  rendered_ref, created_at, expires_at, attempt_expires_at, rung) \
                 values ($1,$2,$3,'pending',1,$4,'attention',$5,$6,$7,$8,$9,$10,$11,$12,$13,$14, \
                         $15,$16,$17,$18,$19,$20,$21,$22,$23,$24,$25,null,0)",
            )
            .bind(id.to_string())
            .bind(&tenant)
            .bind(&command.raise.waiter_ref)
            .bind(enum_name(&command.raise.urgency))
            .bind(enum_name(&command.raise.liveness))
            .bind(enum_name(&command.raise.on_waiter_terminal))
            .bind(enum_name(&command.raise.mode))
            .bind(enum_name(&command.raise.presentation_binding))
            .bind(&command.dedupe_key)
            .bind(&prompt)
            .bind(&requires)
            .bind(match &command.raise.ttl_policy {
                Some(p) => Some(to_json(p, "ttl policy")?),
                None => None,
            })
            .bind(to_json(&command.routing, "routing")?)
            .bind(command.raise.attempt_ttl.as_secs() as i64)
            .bind(Value::Object(command.raise.metadata.clone()))
            .bind(command.raise.callback.as_ref().map(|c| c.url.clone()))
            .bind(command.raise.continuation.resume_ref.as_deref())
            .bind(
                command
                    .raise
                    .continuation
                    .resume_payload
                    .as_ref()
                    .map(|b| handoff_protocol::request::base64_encode(b)),
            )
            .bind(command.raise.test_mode)
            .bind(&principal)
            .bind(command.raise.digest()?.to_string())
            .bind(rendered.to_string())
            .bind(&rendered_ref)
            .bind(to_chrono(now))
            .bind(opt_chrono(command.expires_at))
            .execute(&mut *tx)
            .await
            .map_err(db)?;

            // W1. The wait is a durable server-side row from the moment the request exists.
            sqlx::query(
                "insert into handoff_waiters (tenant_ref, waiter_ref, state, liveness, created_at, updated_at) \
                 values ($1,$2,'armed',$3,$4,$4) \
                 on conflict (tenant_ref, waiter_ref) do update set \
                   state = case when handoff_waiters.state in ('released','acked') then 'armed' \
                                else handoff_waiters.state end, \
                   liveness = excluded.liveness, updated_at = excluded.updated_at",
            )
            .bind(&tenant)
            .bind(&command.raise.waiter_ref)
            .bind(enum_name(&command.raise.liveness))
            .bind(to_chrono(now))
            .execute(&mut *tx)
            .await
            .map_err(db)?;

            Self::append_step(
                &mut tx,
                &tenant,
                StepToAppend {
                    request_id: id,
                    n: 1,
                    prompt: &command.raise.prompt,
                    requires: &command.raise.requires,
                    rendered_digest: &rendered,
                    rendered_ref: &rendered_ref,
                },
                now,
            )
            .await?;

            // §11.4. Grants are minted server-side, inside the raise transaction, one per declared
            // capability. The runtime receives a handle at most, and never anything resolvable.
            for grant in &command.grants {
                sqlx::query(
                    "insert into handoff_grants \
                     (handle, tenant_ref, request_id, capability_type, scope, provider, resource_ref, \
                      label, purpose, optional, blast_radius, blast_radius_digest, expires_at, \
                      max_holders, created_at) \
                     values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)",
                )
                .bind(grant.handle.to_string())
                .bind(&tenant)
                .bind(id.to_string())
                .bind(&grant.capability_type)
                .bind(enum_name(&grant.scope))
                .bind(grant.provider.as_deref())
                .bind(grant.resource_ref.as_deref())
                .bind(grant.label.as_deref())
                .bind(grant.purpose.as_deref())
                .bind(grant.optional)
                .bind(to_json(&grant.blast_radius, "blast radius")?)
                .bind(grant.blast_radius_digest.to_string())
                .bind(to_chrono(grant.expires_at))
                .bind(grant.max_holders)
                .bind(to_chrono(now))
                .execute(&mut *tx)
                .await
                .map_err(db)?;
            }

            // §12. The sink is a declaration the surface can post to; no value column exists.
            if let Some(sink) = command
                .raise
                .requires
                .answer
                .as_ref()
                .and_then(|a| a.value_sink.as_ref())
            {
                sqlx::query(
                    "insert into handoff_sinks (tenant_ref, sink_ref, request_id, created_at) \
                     values ($1,$2,$3,$4) on conflict (tenant_ref, sink_ref) \
                     do update set request_id = excluded.request_id",
                )
                .bind(&tenant)
                .bind(&sink.sink_ref)
                .bind(id.to_string())
                .bind(to_chrono(now))
                .execute(&mut *tx)
                .await
                .map_err(db)?;
            }

            // §7.3. A raise never blocks on delivery: rung 0 comes back `queued`.
            self.mint_rung(&mut tx, &tenant, id, &command.routing, 0, now)
                .await?;

            Self::emit(
                &mut tx,
                &tenant,
                Some(&id.to_string()),
                events::REQUEST_RAISED,
                json!({"waiter_ref": command.raise.waiter_ref, "version": 1}),
                now,
            )
            .await?;
            Self::meter(
                &mut tx,
                &tenant,
                "intervention.raised",
                Some(&id.to_string()),
                now,
            )
            .await?;

            if let Some(key) = command.idempotency_key.as_deref() {
                remember_raise(
                    &mut tx,
                    &tenant,
                    &principal,
                    key,
                    &command.body_digest,
                    &id,
                    now,
                )
                .await?;
            }

            let request = self
                .load_request(&mut tx, &tenant, id)
                .await?
                .ok_or_else(|| not_found(ErrorCode::RequestNotFound))?;
            tx.commit().await.map_err(db)?;
            Ok(RaiseResult {
                request,
                status: 201,
            })
        })
    }

    fn get_request(
        &self,
        tenant: String,
        id: RequestId,
    ) -> BoxFuture<'_, Result<Option<RequestView>>> {
        Box::pin(async move {
            let mut tx = self.tenant_tx(&tenant).await?;
            let view = self.load_request(&mut tx, &tenant, id).await?;
            tx.commit().await.map_err(db)?;
            Ok(view)
        })
    }

    fn list_requests(
        &self,
        tenant: String,
        filter: RequestFilter,
    ) -> BoxFuture<'_, Result<Vec<RequestView>>> {
        Box::pin(async move {
            let mut tx = self.tenant_tx(&tenant).await?;
            let states: Vec<String> = filter
                .states
                .iter()
                .map(|s| state_name(*s).to_string())
                .collect();
            let rows = sqlx::query(
                "select * from handoff_requests where tenant_ref = $1 \
                 and ($2::text is null or waiter_ref = $2) \
                 and (cardinality($3::text[]) = 0 or state = any($3)) \
                 order by id limit $4",
            )
            .bind(&tenant)
            .bind(filter.waiter_ref.as_deref())
            .bind(&states)
            .bind(filter.limit.clamp(1, 200))
            .fetch_all(&mut *tx)
            .await
            .map_err(db)?;
            let mut views = Vec::with_capacity(rows.len());
            for row in rows {
                views.push(self.hydrate(&mut tx, &tenant, row).await?);
            }
            tx.commit().await.map_err(db)?;
            Ok(views)
        })
    }

    fn amend(
        &self,
        command: RequestCommand,
        patch: AmendPatch,
    ) -> BoxFuture<'_, Result<RequestView>> {
        Box::pin(async move {
            let tenant = command.principal.tenant_ref.clone();
            let now = command.now;
            let mut tx = self.tenant_tx(&tenant).await?;
            let request = self
                .load_request(&mut tx, &tenant, command.request_id)
                .await?
                .ok_or_else(|| not_found(ErrorCode::RequestNotFound))?;
            guard_pending(&request)?;

            // §6.2 R2's guard is strict: once a person has begun answering, the text underneath
            // them does not change and the caller must supersede instead.
            if request
                .deliveries
                .iter()
                .any(|d| d.grade_reached == Some(DeliveryGrade::Acted))
            {
                return Err(ProtocolError::new(
                    ErrorCode::RequestInProgress,
                    "a delivery has reached 'acted'; amend is no longer permitted",
                ));
            }

            let prompt = patch.prompt.unwrap_or_else(|| request.prompt.clone());
            let requires = patch.requires.unwrap_or_else(|| request.requires.clone());
            let prompt_json = to_json(&prompt, "prompt")?;
            let requires_json = to_json(&requires, "requires")?;
            let version = request.version + 1;
            let rendered = plan::rendered_digest(&prompt_json, &requires_json, version)?;
            let rendered_ref = format!("render:{}:{version}", request.id);

            sqlx::query(
                "update handoff_requests set prompt = $3, requires = $4, version = $5, \
                 rendered_digest = $6, rendered_ref = $7 \
                 where tenant_ref = $1 and id = $2 and state = 'pending'",
            )
            .bind(&tenant)
            .bind(request.id.to_string())
            .bind(&prompt_json)
            .bind(&requires_json)
            .bind(version as i64)
            .bind(rendered.to_string())
            .bind(&rendered_ref)
            .execute(&mut *tx)
            .await
            .map_err(db)?;

            Self::append_step(
                &mut tx,
                &tenant,
                StepToAppend {
                    request_id: request.id,
                    n: version,
                    prompt: &prompt,
                    requires: &requires,
                    rendered_digest: &rendered,
                    rendered_ref: &rendered_ref,
                },
                now,
            )
            .await?;
            Self::emit(
                &mut tx,
                &tenant,
                Some(&request.id.to_string()),
                events::REQUEST_AMENDED,
                json!({"version": version}),
                now,
            )
            .await?;

            let view = self
                .load_request(&mut tx, &tenant, request.id)
                .await?
                .ok_or_else(|| not_found(ErrorCode::RequestNotFound))?;
            tx.commit().await.map_err(db)?;
            Ok(view)
        })
    }

    fn cancel(
        &self,
        command: RequestCommand,
        reason: String,
    ) -> BoxFuture<'_, Result<RequestView>> {
        Box::pin(async move {
            let tenant = command.principal.tenant_ref.clone();
            let now = command.now;
            let mut tx = self.tenant_tx(&tenant).await?;
            let request = self
                .load_request(&mut tx, &tenant, command.request_id)
                .await?
                .ok_or_else(|| not_found(ErrorCode::RequestNotFound))?;
            // R11. A cancel racing a landed answer loses, and leaves no trace on the request.
            guard_pending(&request)?;

            let settled: Option<String> = sqlx::query_scalar(
                "update handoff_requests set state = 'cancelled', cancel_reason = $3 \
                 where tenant_ref = $1 and id = $2 and state = 'pending' returning id",
            )
            .bind(&tenant)
            .bind(request.id.to_string())
            .bind(&reason)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?;
            if settled.is_none() {
                return Err(self.conflict_for(&mut tx, &tenant, request.id).await?);
            }

            Self::close_out(&mut tx, &tenant, request.id, now).await?;
            let decision = plan::terminal_decision(RequestState::Cancelled, None);
            Self::enqueue_signal(
                &mut tx,
                &tenant,
                SignalToEnqueue {
                    waiter_ref: &request.waiter_ref,
                    request_id: request.id,
                    signal_type: SignalType::Cancelled,
                    decision: Some(&decision),
                    continuation: &request.continuation,
                    callback_url: request.callback.as_ref().map(|c| c.url.as_str()),
                },
                now,
            )
            .await?;
            Self::emit(
                &mut tx,
                &tenant,
                Some(&request.id.to_string()),
                events::REQUEST_CANCELLED,
                json!({"reason": reason}),
                now,
            )
            .await?;

            let view = self
                .load_request(&mut tx, &tenant, request.id)
                .await?
                .ok_or_else(|| not_found(ErrorCode::RequestNotFound))?;
            tx.commit().await.map_err(db)?;
            Ok(view)
        })
    }

    fn supersede(
        &self,
        command: RequestCommand,
        by: RequestId,
    ) -> BoxFuture<'_, Result<RequestView>> {
        Box::pin(async move {
            let tenant = command.principal.tenant_ref.clone();
            let now = command.now;
            let mut tx = self.tenant_tx(&tenant).await?;
            let request = self
                .load_request(&mut tx, &tenant, command.request_id)
                .await?
                .ok_or_else(|| not_found(ErrorCode::RequestNotFound))?;
            guard_pending(&request)?;

            // R8's guard: the successor exists, is in the same tenant, and is itself pending.
            // Cross-tenant supersession is not a thing (§3.2).
            let successor = self
                .load_request(&mut tx, &tenant, by)
                .await?
                .ok_or_else(|| not_found(ErrorCode::RequestNotFound))?;
            if successor.state != RequestState::Pending {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidRequest,
                    "the successor must still be pending",
                ));
            }

            let settled: Option<String> = sqlx::query_scalar(
                "update handoff_requests set state = 'superseded', superseded_by = $3 \
                 where tenant_ref = $1 and id = $2 and state = 'pending' returning id",
            )
            .bind(&tenant)
            .bind(request.id.to_string())
            .bind(by.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?;
            if settled.is_none() {
                return Err(self.conflict_for(&mut tx, &tenant, request.id).await?);
            }

            Self::close_out(&mut tx, &tenant, request.id, now).await?;
            let decision = plan::terminal_decision(RequestState::Superseded, Some(by));
            Self::enqueue_signal(
                &mut tx,
                &tenant,
                SignalToEnqueue {
                    waiter_ref: &request.waiter_ref,
                    request_id: request.id,
                    signal_type: SignalType::Superseded,
                    decision: Some(&decision),
                    continuation: &request.continuation,
                    callback_url: request.callback.as_ref().map(|c| c.url.as_str()),
                },
                now,
            )
            .await?;
            Self::emit(
                &mut tx,
                &tenant,
                Some(&request.id.to_string()),
                events::REQUEST_SUPERSEDED,
                json!({"superseded_by": by.to_string()}),
                now,
            )
            .await?;

            let view = self
                .load_request(&mut tx, &tenant, request.id)
                .await?
                .ok_or_else(|| not_found(ErrorCode::RequestNotFound))?;
            tx.commit().await.map_err(db)?;
            Ok(view)
        })
    }

    fn escalate(
        &self,
        command: RequestCommand,
        rung: Option<u32>,
    ) -> BoxFuture<'_, Result<RequestView>> {
        Box::pin(async move {
            let tenant = command.principal.tenant_ref.clone();
            let now = command.now;
            let mut tx = self.tenant_tx(&tenant).await?;
            let request = self
                .load_request(&mut tx, &tenant, command.request_id)
                .await?
                .ok_or_else(|| not_found(ErrorCode::RequestNotFound))?;
            guard_pending(&request)?;

            // I3. A rung mints deliveries, never a request. The state does not move and no receipt
            // is minted, however many people end up being tried.
            let next = rung.unwrap_or(request.rung + 1);
            let minted = self
                .mint_rung(&mut tx, &tenant, request.id, &request.routing, next, now)
                .await?;
            if minted > 0 {
                sqlx::query(
                    "update handoff_requests set rung = greatest(rung, $3) \
                     where tenant_ref = $1 and id = $2",
                )
                .bind(&tenant)
                .bind(request.id.to_string())
                .bind(next as i32)
                .execute(&mut *tx)
                .await
                .map_err(db)?;
            }
            Self::emit(
                &mut tx,
                &tenant,
                Some(&request.id.to_string()),
                events::REQUEST_ESCALATED,
                json!({"rung": next, "deliveries_minted": minted}),
                now,
            )
            .await?;

            let view = self
                .load_request(&mut tx, &tenant, request.id)
                .await?
                .ok_or_else(|| not_found(ErrorCode::RequestNotFound))?;
            tx.commit().await.map_err(db)?;
            Ok(view)
        })
    }

    fn reassign(
        &self,
        command: RequestCommand,
        to: Target,
        reason: Option<String>,
    ) -> BoxFuture<'_, Result<RequestView>> {
        Box::pin(async move {
            let tenant = command.principal.tenant_ref.clone();
            let now = command.now;
            let mut tx = self.tenant_tx(&tenant).await?;
            let request = self
                .load_request(&mut tx, &tenant, command.request_id)
                .await?
                .ok_or_else(|| not_found(ErrorCode::RequestNotFound))?;
            guard_pending(&request)?;

            sqlx::query(
                "update handoff_deliveries set state = 'cancelled', updated_at = $3 \
                 where tenant_ref = $1 and request_id = $2 and state in ('queued','sending','retrying')",
            )
            .bind(&tenant)
            .bind(request.id.to_string())
            .bind(to_chrono(now))
            .execute(&mut *tx)
            .await
            .map_err(db)?;

            let routing = Routing {
                targets: vec![to.clone()],
                ladder: request.routing.ladder.clone(),
            };
            self.mint_rung(&mut tx, &tenant, request.id, &routing, request.rung, now)
                .await?;
            Self::emit(
                &mut tx,
                &tenant,
                Some(&request.id.to_string()),
                events::REQUEST_REASSIGNED,
                json!({"to": {"kind": target_kind_name(to.kind), "value": to.value}, "reason": reason}),
                now,
            )
            .await?;

            let view = self
                .load_request(&mut tx, &tenant, request.id)
                .await?
                .ok_or_else(|| not_found(ErrorCode::RequestNotFound))?;
            tx.commit().await.map_err(db)?;
            Ok(view)
        })
    }

    fn arm_attempt(
        &self,
        command: RequestCommand,
        ttl: Option<IsoDuration>,
    ) -> BoxFuture<'_, Result<RequestView>> {
        Box::pin(async move {
            let tenant = command.principal.tenant_ref.clone();
            let now = command.now;
            let mut tx = self.tenant_tx(&tenant).await?;
            let request = self
                .load_request(&mut tx, &tenant, command.request_id)
                .await?
                .ok_or_else(|| not_found(ErrorCode::RequestNotFound))?;
            guard_pending(&request)?;

            // §6.3. Re-arming always starts a **fresh** window; a later challenge step never
            // inherits a near-expired countdown.
            let window = ttl.unwrap_or(request.attempt_ttl);
            let deadline = now.saturating_add(window);
            sqlx::query(
                "update handoff_requests set attempt_expires_at = $3, urgency_state = 'attention', \
                 attempt_lapse_notified = false where tenant_ref = $1 and id = $2 and state = 'pending'",
            )
            .bind(&tenant)
            .bind(request.id.to_string())
            .bind(to_chrono(deadline))
            .execute(&mut *tx)
            .await
            .map_err(db)?;
            Self::emit(
                &mut tx,
                &tenant,
                Some(&request.id.to_string()),
                events::ATTEMPT_ARMED,
                json!({"attempt_expires_at": deadline.to_string()}),
                now,
            )
            .await?;

            let view = self
                .load_request(&mut tx, &tenant, request.id)
                .await?
                .ok_or_else(|| not_found(ErrorCode::RequestNotFound))?;
            tx.commit().await.map_err(db)?;
            Ok(view)
        })
    }

    fn answer(&self, command: AnswerCommand) -> BoxFuture<'_, Result<AnswerResult>> {
        Box::pin(async move {
            let tenant = command.principal.tenant_ref.clone();
            let now = command.now;

            // §4.2 / I15, before anything else is read. There is no request state, no role, and no
            // configuration under which this check can be satisfied by a machine.
            if !command.principal.may_answer() {
                return Err(ProtocolError::new(
                    ErrorCode::RequesterMayNotAnswer,
                    "a service_account principal may not answer a request it can raise",
                ));
            }

            let mut tx = self.tenant_tx(&tenant).await?;
            let request = self
                .load_request(&mut tx, &tenant, command.request_id)
                .await?
                .ok_or_else(|| not_found(ErrorCode::RequestNotFound))?;
            guard_pending(&request)?;

            let grants = load_grants(&mut tx, &tenant, request.id).await?;
            let via = pick_delivery(&request, command.via_delivery_id);

            let receipt_id = ids::mint::<handoff_protocol::id::Receipt>(now.to_millis() as u64)?;
            let authorization_id =
                ids::mint::<handoff_protocol::id::Authorization>(now.to_millis() as u64)?;
            let plan = plan::plan_answer(AnswerInput {
                request: &request,
                principal: &command.principal,
                command: &command,
                policy: &self.policy,
                profile: &self.profile,
                via: via.as_ref(),
                grants: &grants,
                receipt_id,
                authorization_id,
            })?;

            if !plan.settles {
                // §6.2 R12 and R13. Both leave the request `pending`, and **neither signals the
                // waiter**: a runtime must not be able to observe that an intermediate step or a
                // delegation happened, or one intervention becomes several. Nothing here enqueues
                // a signal and nothing here mints a receipt.
                let version = request.version + 1;
                sqlx::query(
                    "update handoff_requests set version = $3, attempt_expires_at = $4, \
                     urgency_state = 'attention' where tenant_ref = $1 and id = $2 and state = 'pending'",
                )
                .bind(&tenant)
                .bind(request.id.to_string())
                .bind(version as i64)
                // R12: re-armed **fresh**, never inheriting the remaining time of the previous
                // step (§5.5 rule 4). A later challenge step gets a whole window, not the tail of
                // the last one.
                .bind(to_chrono(now.saturating_add(request.attempt_ttl)))
                .execute(&mut *tx)
                .await
                .map_err(db)?;

                if command.partial {
                    // R12. The step record is what preserves the ladder as one intervention.
                    Self::append_step(
                        &mut tx,
                        &tenant,
                        StepToAppend {
                            request_id: request.id,
                            n: version,
                            prompt: &request.prompt,
                            requires: &request.requires,
                            rendered_digest: &request.rendered_digest,
                            rendered_ref: &request.rendered_ref,
                        },
                        now,
                    )
                    .await?;
                    Self::emit(
                        &mut tx,
                        &tenant,
                        Some(&request.id.to_string()),
                        events::REQUEST_STEP_RECORDED,
                        json!({
                            "version": version,
                            "fields_provided": command.values.keys().collect::<Vec<_>>(),
                            "signalled_waiter": false,
                        }),
                        now,
                    )
                    .await?;
                } else {
                    // R13. Record the disposition, the actor, and any `delegate_to`; for a
                    // delegation, mint deliveries to the new target. §6.6: a Server MUST NOT treat
                    // a delegation as a decision, so no receipt is minted and the request stays
                    // `pending` until an authorized principal actually decides.
                    let answerer = principal_ref(&command.principal);
                    sqlx::query(
                        "insert into handoff_request_dispositions \
                         (tenant_ref, request_id, disposition, principal_ref, delegate_kind, \
                          delegate_value, note, created_at) values ($1,$2,$3,$4,$5,$6,$7,$8)",
                    )
                    .bind(&tenant)
                    .bind(request.id.to_string())
                    .bind(enum_name(&command.disposition))
                    .bind(&answerer)
                    .bind(
                        command
                            .delegate_to
                            .as_ref()
                            .map(|t| target_kind_name(t.kind)),
                    )
                    .bind(command.delegate_to.as_ref().map(|t| t.value.clone()))
                    .bind(command.note.as_deref())
                    .bind(to_chrono(now))
                    .execute(&mut *tx)
                    .await
                    .map_err(db)?;

                    if let Some(target) = &command.delegate_to {
                        let routing = Routing {
                            targets: vec![target.clone()],
                            ladder: request.routing.ladder.clone(),
                        };
                        self.mint_rung(&mut tx, &tenant, request.id, &routing, request.rung, now)
                            .await?;
                    }

                    Self::emit(
                        &mut tx,
                        &tenant,
                        Some(&request.id.to_string()),
                        events::REQUEST_DISPOSITION_RECORDED,
                        json!({
                            "disposition": enum_name(&command.disposition),
                            "delegate_to": command.delegate_to.as_ref().map(|t| json!({
                                "kind": target_kind_name(t.kind), "value": t.value,
                            })),
                            "receipt_minted": false,
                            "signalled_waiter": false,
                        }),
                        now,
                    )
                    .await?;
                }
                let view = self
                    .load_request(&mut tx, &tenant, request.id)
                    .await?
                    .ok_or_else(|| not_found(ErrorCode::RequestNotFound))?;
                tx.commit().await.map_err(db)?;
                return Ok(AnswerResult {
                    request: view,
                    receipt: None,
                    authorization: None,
                });
            }

            // §6.2 R5. A state-conditional update, never a read-then-write. When it affects no row
            // another writer settled the request first, and this answer loses (I5, C-3).
            let settled: Option<String> = sqlx::query_scalar(
                "update handoff_requests set state = 'answered', answered_at = $3, \
                 urgency_state = 'waiting' \
                 where tenant_ref = $1 and id = $2 and state = 'pending' returning id",
            )
            .bind(&tenant)
            .bind(request.id.to_string())
            .bind(to_chrono(now))
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?;
            if settled.is_none() {
                return Err(self.conflict_for(&mut tx, &tenant, request.id).await?);
            }

            // C-23's fault injection: die here, with the state written and the event not yet
            // written, inside the open transaction. The rollback must take both.
            if self.crash_point.as_deref() == Some(CRASH_AFTER_ANSWER_STATE_WRITE) {
                tracing::error!("HANDOFF_CRASH_POINT reached: aborting between the state write and the event write");
                std::process::abort();
            }

            let receipt = Self::append_receipt(&mut tx, &tenant, plan.receipt).await?;

            if let Some(authorization) = &plan.authorization {
                sqlx::query(
                    "insert into handoff_authorizations \
                     (id, tenant_ref, receipt_id, request_id, grants, single_use, expires_at, \
                      waiter_ref, effect_digest, created_at) \
                     values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
                )
                .bind(authorization.id.to_string())
                .bind(&tenant)
                .bind(receipt.id.to_string())
                .bind(request.id.to_string())
                .bind(Value::Object(authorization.grants.clone()))
                .bind(authorization.single_use)
                .bind(opt_chrono(authorization.expires_at))
                .bind(authorization.bound_to.waiter_ref.as_deref())
                .bind(
                    authorization
                        .bound_to
                        .effect_digest
                        .as_ref()
                        .map(ToString::to_string),
                )
                .bind(to_chrono(now))
                .execute(&mut *tx)
                .await
                .map_err(db)?;
            }

            // §7.2. The delivery the person answered through is the only one that reaches `acted`.
            if let Some(delivery) = &plan.via {
                let grade = delivery.max_grade.min(DeliveryGrade::Acted);
                sqlx::query(
                    "update handoff_deliveries set state = $3, grade_reached = $4, updated_at = $5 \
                     where tenant_ref = $1 and id = $2",
                )
                .bind(&tenant)
                .bind(delivery.id.to_string())
                .bind(delivery_state_name(grade.state()))
                .bind(grade_name(grade))
                .bind(to_chrono(now))
                .execute(&mut *tx)
                .await
                .map_err(db)?;
            }

            Self::close_out(&mut tx, &tenant, request.id, now).await?;

            let mut decision = plan.decision.clone().ok_or_else(|| {
                ProtocolError::new(
                    ErrorCode::InvalidRequest,
                    "a settling answer must carry a decision",
                )
            })?;
            decision.receipt_id = Some(receipt.id);
            Self::enqueue_signal(
                &mut tx,
                &tenant,
                SignalToEnqueue {
                    waiter_ref: &request.waiter_ref,
                    request_id: request.id,
                    signal_type: SignalType::Answered,
                    decision: Some(&decision),
                    continuation: &request.continuation,
                    callback_url: request.callback.as_ref().map(|c| c.url.as_str()),
                },
                now,
            )
            .await?;

            Self::emit(
                &mut tx,
                &tenant,
                Some(&request.id.to_string()),
                events::REQUEST_ANSWERED,
                json!({"receipt_id": receipt.id.to_string(), "actor": enum_name(&receipt.actor.actor_type)}),
                now,
            )
            .await?;
            Self::meter(
                &mut tx,
                &tenant,
                "intervention.answered",
                Some(&request.id.to_string()),
                now,
            )
            .await?;

            let view = self
                .load_request(&mut tx, &tenant, request.id)
                .await?
                .ok_or_else(|| not_found(ErrorCode::RequestNotFound))?;
            tx.commit().await.map_err(db)?;
            Ok(AnswerResult {
                request: view,
                receipt: Some(receipt),
                authorization: plan.authorization,
            })
        })
    }

    fn request_receipt(
        &self,
        tenant: String,
        id: RequestId,
    ) -> BoxFuture<'_, Result<Option<Receipt>>> {
        Box::pin(async move {
            let mut tx = self.tenant_tx(&tenant).await?;
            let row = sqlx::query(
                "select body from handoff_receipts where tenant_ref = $1 and request_id = $2 \
                 order by height limit 1",
            )
            .bind(&tenant)
            .bind(id.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?;
            tx.commit().await.map_err(db)?;
            match row {
                Some(row) => Ok(Some(parse_json::<Receipt>(row.get("body"), "receipt")?)),
                None => Ok(None),
            }
        })
    }

    fn receipt(&self, tenant: String, id: ReceiptId) -> BoxFuture<'_, Result<Option<Receipt>>> {
        Box::pin(async move {
            let mut tx = self.tenant_tx(&tenant).await?;
            let row =
                sqlx::query("select body from handoff_receipts where tenant_ref = $1 and id = $2")
                    .bind(&tenant)
                    .bind(id.to_string())
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(db)?;
            tx.commit().await.map_err(db)?;
            match row {
                Some(row) => Ok(Some(parse_json::<Receipt>(row.get("body"), "receipt")?)),
                None => Ok(None),
            }
        })
    }

    fn chain(&self, tenant: String) -> BoxFuture<'_, Result<ChainExport>> {
        Box::pin(async move {
            let mut tx = self.tenant_tx(&tenant).await?;
            let rows = sqlx::query(
                "select body, height, digest from handoff_receipts where tenant_ref = $1 order by height",
            )
            .bind(&tenant)
            .fetch_all(&mut *tx)
            .await
            .map_err(db)?;
            tx.commit().await.map_err(db)?;

            let mut receipts = Vec::with_capacity(rows.len());
            for row in &rows {
                receipts.push(parse_json::<Receipt>(row.get("body"), "receipt")?);
            }
            let head = match rows.last() {
                Some(row) => Some(ChainHead {
                    org_id: plan::tenant_as_org(&tenant)?,
                    height: row.get::<i64, _>("height") as u64,
                    head_digest: Digest::parse(row.get::<String, _>("digest").as_str())?,
                    as_of: from_chrono(Utc::now()),
                }),
                None => None,
            };
            Ok(ChainExport { head, receipts })
        })
    }

    fn deliveries(
        &self,
        tenant: String,
        id: RequestId,
    ) -> BoxFuture<'_, Result<Vec<DeliveryView>>> {
        Box::pin(async move {
            let mut tx = self.tenant_tx(&tenant).await?;
            let exists: Option<String> = sqlx::query_scalar(
                "select id from handoff_requests where tenant_ref = $1 and id = $2",
            )
            .bind(&tenant)
            .bind(id.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?;
            if exists.is_none() {
                return Err(not_found(ErrorCode::RequestNotFound));
            }
            let view = self
                .load_request(&mut tx, &tenant, id)
                .await?
                .ok_or_else(|| not_found(ErrorCode::RequestNotFound))?;
            tx.commit().await.map_err(db)?;
            Ok(view.deliveries)
        })
    }

    fn signals(&self, tenant: String, waiter_ref: String) -> BoxFuture<'_, Result<Vec<Signal>>> {
        Box::pin(async move {
            let mut tx = self.tenant_tx(&tenant).await?;
            // §8.3. Reading does not consume: this is a plain select, and nothing here writes
            // `acked_at`.
            let rows = sqlx::query(
                "select * from handoff_signals where tenant_ref = $1 and waiter_ref = $2 \
                 and acked_at is null order by sequence",
            )
            .bind(&tenant)
            .bind(&waiter_ref)
            .fetch_all(&mut *tx)
            .await
            .map_err(db)?;
            tx.commit().await.map_err(db)?;
            rows.iter().map(row_to_signal).collect()
        })
    }

    fn reattach(&self, tenant: String, waiter_ref: String) -> BoxFuture<'_, Result<ReattachView>> {
        Box::pin(async move {
            let now = from_chrono(Utc::now());
            let mut tx = self.tenant_tx(&tenant).await?;
            // W7. Re-arm the lease and return everything that was waiting. A Server MUST NOT
            // discard a signal because the client that raised the request has gone away.
            sqlx::query(
                "insert into handoff_waiters (tenant_ref, waiter_ref, state, liveness, created_at, updated_at) \
                 values ($1,$2,'armed','durable',$3,$3) \
                 on conflict (tenant_ref, waiter_ref) do update set \
                   state = case when handoff_waiters.state = 'orphaned' then 'armed' \
                                else handoff_waiters.state end, \
                   lease_expires_at = null, updated_at = excluded.updated_at",
            )
            .bind(&tenant)
            .bind(&waiter_ref)
            .bind(to_chrono(now))
            .execute(&mut *tx)
            .await
            .map_err(db)?;

            let state: String = sqlx::query_scalar(
                "select state from handoff_waiters where tenant_ref = $1 and waiter_ref = $2",
            )
            .bind(&tenant)
            .bind(&waiter_ref)
            .fetch_one(&mut *tx)
            .await
            .map_err(db)?;

            let signal_rows = sqlx::query(
                "select * from handoff_signals where tenant_ref = $1 and waiter_ref = $2 \
                 and acked_at is null order by sequence",
            )
            .bind(&tenant)
            .bind(&waiter_ref)
            .fetch_all(&mut *tx)
            .await
            .map_err(db)?;

            let open: Vec<String> = sqlx::query_scalar(
                "select id from handoff_requests where tenant_ref = $1 and waiter_ref = $2 \
                 and state = 'pending' order by id",
            )
            .bind(&tenant)
            .bind(&waiter_ref)
            .fetch_all(&mut *tx)
            .await
            .map_err(db)?;
            tx.commit().await.map_err(db)?;

            Ok(ReattachView {
                waiter_ref,
                state: parse_waiter_state(&state),
                open_requests: open
                    .iter()
                    .map(|id| RequestId::parse(id))
                    .collect::<Result<Vec<_>>>()?,
                signals: signal_rows
                    .iter()
                    .map(row_to_signal)
                    .collect::<Result<Vec<_>>>()?,
            })
        })
    }

    fn ack(&self, tenant: String, command: AckCommand) -> BoxFuture<'_, Result<Option<AckResult>>> {
        Box::pin(async move {
            let mut tx = self.tenant_tx(&tenant).await?;
            let row = sqlx::query(
                "select resume_token, acked_at, waiter_ref from handoff_signals \
                 where tenant_ref = $1 and id = $2 for update",
            )
            .bind(&tenant)
            .bind(command.signal_id.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?;
            let Some(row) = row else {
                return Ok(None);
            };
            if row.get::<String, _>("resume_token") != command.resume_token {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidRequest,
                    "the resume token does not match this signal",
                ));
            }

            // §3.5. Acking twice returns 200 both times, and stops redelivery exactly once.
            if let Some(acked) = row.get::<Option<DateTime<Utc>>, _>("acked_at") {
                tx.commit().await.map_err(db)?;
                return Ok(Some(AckResult {
                    acked_at: from_chrono(acked),
                    first_ack: false,
                }));
            }

            sqlx::query(
                "update handoff_signals set acked_at = $3, applied = $4, ack_reason = $5, \
                 next_callback_at = null where tenant_ref = $1 and id = $2",
            )
            .bind(&tenant)
            .bind(command.signal_id.to_string())
            .bind(to_chrono(command.now))
            .bind(command.applied)
            .bind(command.reason.as_deref())
            .execute(&mut *tx)
            .await
            .map_err(db)?;

            let waiter_ref: String = row.get("waiter_ref");
            let outstanding: i64 = sqlx::query_scalar(
                "select count(*) from handoff_signals where tenant_ref = $1 and waiter_ref = $2 \
                 and acked_at is null",
            )
            .bind(&tenant)
            .bind(&waiter_ref)
            .fetch_one(&mut *tx)
            .await
            .map_err(db)?;
            if outstanding == 0 {
                sqlx::query(
                    "update handoff_waiters set state = 'acked', updated_at = $3 \
                     where tenant_ref = $1 and waiter_ref = $2",
                )
                .bind(&tenant)
                .bind(&waiter_ref)
                .bind(to_chrono(command.now))
                .execute(&mut *tx)
                .await
                .map_err(db)?;
            }
            tx.commit().await.map_err(db)?;
            Ok(Some(AckResult {
                acked_at: command.now,
                first_ack: true,
            }))
        })
    }

    fn signal_attempts(
        &self,
        tenant: String,
        id: SignalId,
    ) -> BoxFuture<'_, Result<Option<Vec<CallbackAttemptView>>>> {
        Box::pin(async move {
            let mut tx = self.tenant_tx(&tenant).await?;
            let exists: Option<String> = sqlx::query_scalar(
                "select id from handoff_signals where tenant_ref = $1 and id = $2",
            )
            .bind(&tenant)
            .bind(id.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?;
            if exists.is_none() {
                return Ok(None);
            }
            let rows = sqlx::query(
                "select * from handoff_callback_attempts where tenant_ref = $1 and signal_id = $2 order by n",
            )
            .bind(&tenant)
            .bind(id.to_string())
            .fetch_all(&mut *tx)
            .await
            .map_err(db)?;
            tx.commit().await.map_err(db)?;
            Ok(Some(
                rows.iter()
                    .map(|r| CallbackAttemptView {
                        n: r.get::<i32, _>("n") as u32,
                        started_at: from_chrono(r.get("started_at")),
                        ended_at: r
                            .get::<Option<DateTime<Utc>>, _>("ended_at")
                            .map(from_chrono),
                        status_code: r.get("status_code"),
                        duration_ms: r.get("duration_ms"),
                        outcome: r.get("outcome"),
                        error: r.get("error"),
                    })
                    .collect(),
            ))
        })
    }

    fn authorization(
        &self,
        tenant: String,
        id: AuthorizationId,
    ) -> BoxFuture<'_, Result<Option<Authorization>>> {
        Box::pin(async move {
            let mut tx = self.tenant_tx(&tenant).await?;
            let row = sqlx::query(
                "select * from handoff_authorizations where tenant_ref = $1 and id = $2",
            )
            .bind(&tenant)
            .bind(id.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?;
            let result = match row {
                Some(row) => {
                    let mut authorization = row_to_authorization(&row)?;
                    authorization.redemptions = self.load_redemptions(&mut tx, &tenant, id).await?;
                    Some(authorization)
                }
                None => None,
            };
            tx.commit().await.map_err(db)?;
            Ok(result)
        })
    }

    fn redeem(
        &self,
        tenant: String,
        command: RedeemCommand,
    ) -> BoxFuture<'_, Result<Option<RedeemOutcome>>> {
        Box::pin(async move {
            let mut tx = self.tenant_tx(&tenant).await?;
            let row = sqlx::query(
                "select * from handoff_authorizations where tenant_ref = $1 and id = $2 for update",
            )
            .bind(&tenant)
            .bind(command.authorization_id.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?;
            let Some(row) = row else { return Ok(None) };
            let mut authorization = row_to_authorization(&row)?;
            authorization.redemptions = self
                .load_redemptions(&mut tx, &tenant, command.authorization_id)
                .await?;

            // §10.2. The check and the persist are in one transaction: doing them separately
            // reintroduces the double-spend the mechanism exists to prevent.
            let (result, redemption) = authorization.redeem(
                &RedeemRequest {
                    effect_key: command.effect_key.clone(),
                    effect_digest: command.effect_digest.clone(),
                },
                command.now,
            )?;
            if let Some(redemption) = redemption {
                sqlx::query(
                    "insert into handoff_redemptions (tenant_ref, authorization_id, effect_key, redeemed_at) \
                     values ($1,$2,$3,$4) on conflict (tenant_ref, authorization_id, effect_key) do nothing",
                )
                .bind(&tenant)
                .bind(command.authorization_id.to_string())
                .bind(&redemption.effect_key)
                .bind(to_chrono(redemption.redeemed_at))
                .execute(&mut *tx)
                .await
                .map_err(db)?;
            }
            tx.commit().await.map_err(db)?;
            Ok(Some(RedeemOutcome {
                redeemed_at: result.redeemed_at,
                first_redemption: result.first_redemption,
            }))
        })
    }

    fn grant(
        &self,
        tenant: String,
        handle: GrantHandle,
    ) -> BoxFuture<'_, Result<Option<GrantView>>> {
        Box::pin(async move {
            let mut tx = self.tenant_tx(&tenant).await?;
            let row =
                sqlx::query("select * from handoff_grants where tenant_ref = $1 and handle = $2")
                    .bind(&tenant)
                    .bind(handle.to_string())
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(db)?;
            tx.commit().await.map_err(db)?;
            row.as_ref().map(row_to_grant).transpose()
        })
    }

    fn grants_for_request(
        &self,
        tenant: String,
        id: RequestId,
    ) -> BoxFuture<'_, Result<Vec<GrantView>>> {
        Box::pin(async move {
            let mut tx = self.tenant_tx(&tenant).await?;
            let grants = load_grants(&mut tx, &tenant, id).await?;
            tx.commit().await.map_err(db)?;
            Ok(grants)
        })
    }

    fn revoke_grant(
        &self,
        tenant: String,
        handle: GrantHandle,
        reason: Option<String>,
        now: Timestamp,
    ) -> BoxFuture<'_, Result<bool>> {
        Box::pin(async move {
            let mut tx = self.tenant_tx(&tenant).await?;
            // §11.4. A single operation on a single grant. Revoking one must not affect any other
            // grant on the same resource, and must not require rotating a shared secret.
            let existed: Option<String> = sqlx::query_scalar(
                "update handoff_grants set revoked_at = coalesce(revoked_at, $3), \
                 revoke_reason = coalesce(revoke_reason, $4) \
                 where tenant_ref = $1 and handle = $2 returning handle",
            )
            .bind(&tenant)
            .bind(handle.to_string())
            .bind(to_chrono(now))
            .bind(reason.as_deref())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?;
            if existed.is_none() {
                tx.commit().await.map_err(db)?;
                return Ok(false);
            }
            sqlx::query(
                "update handoff_grant_sessions set released_at = coalesce(released_at, $3) \
                 where tenant_ref = $1 and handle = $2",
            )
            .bind(&tenant)
            .bind(handle.to_string())
            .bind(to_chrono(now))
            .execute(&mut *tx)
            .await
            .map_err(db)?;
            Self::emit(
                &mut tx,
                &tenant,
                None,
                events::GRANT_REVOKED,
                json!({"handle": handle.to_string()}),
                now,
            )
            .await?;
            tx.commit().await.map_err(db)?;
            Ok(true)
        })
    }

    fn open_grant_session(
        &self,
        tenant: String,
        resolve: ResolveGrant,
    ) -> BoxFuture<'_, Result<GrantSessionView>> {
        Box::pin(async move {
            let now = resolve.now;
            let mut tx = self.tenant_tx(&tenant).await?;
            let row = sqlx::query(
                "select * from handoff_grants where tenant_ref = $1 and handle = $2 for update",
            )
            .bind(&tenant)
            .bind(resolve.handle.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?;
            let Some(row) = row else {
                return Err(not_found(ErrorCode::CapabilityNotFound));
            };
            let grant = row_to_grant(&row)?;

            // §11.2's checks, in order, failing on the first that does not hold.
            //
            // 1. The request exists and is still `pending`. A terminal request resolves nothing.
            let request = self
                .load_request(&mut tx, &tenant, grant.request_id)
                .await?
                .ok_or_else(|| not_found(ErrorCode::CapabilityNotFound))?;
            if request.state != RequestState::Pending {
                return Err(ProtocolError::new(
                    ErrorCode::CapabilityNotFound,
                    "a terminal request resolves nothing",
                ));
            }
            // 2. Neither expired nor revoked.
            if grant.revoked_at.is_some() {
                return Err(not_found(ErrorCode::CapabilityNotFound));
            }
            if grant.expires_at.is_at_or_before(now) {
                return Err(ProtocolError::new(
                    ErrorCode::CapabilityExpired,
                    "this grant has expired and cannot be renewed",
                ));
            }
            // 3 and 4. Tenancy, then the minimum role for **each** requested scope. `drive` is
            // strictly higher than `view`, because driving a shared authenticated surface is not
            // the same act as watching one.
            for scope in &resolve.scopes {
                if *scope > grant.scope {
                    return Err(ProtocolError::new(
                        ErrorCode::InsufficientAuthority,
                        "a session may request a subset of the grant's scope, never a superset",
                    ));
                }
                if resolve.principal.role < scope.minimum_role() {
                    return Err(ProtocolError::new(
                        ErrorCode::InsufficientAuthority,
                        "this scope requires a higher role",
                    ));
                }
            }
            // 5. Binding. Two people driving one surface is how a takeover becomes unattributable.
            let holder = principal_ref(&resolve.principal);
            match &grant.bound_principal {
                Some(bound) if *bound != holder && grant.max_holders <= 1 => {
                    return Err(ProtocolError::new(
                        ErrorCode::GrantAlreadyHeld,
                        "this grant is bound to another holder and max_holders is 1",
                    ));
                }
                _ => {}
            }
            // 6. The accepted blast radius. "I accepted" has to mean "I accepted *this*".
            if resolve.accepted_blast_radius_digest != grant.blast_radius_digest {
                return Err(ProtocolError::new(
                    ErrorCode::BlastRadiusMismatch,
                    "the blast radius changed since it was shown; re-read the grant and re-confirm",
                ));
            }

            sqlx::query(
                "update handoff_grants set bound_principal = coalesce(bound_principal, $3) \
                 where tenant_ref = $1 and handle = $2",
            )
            .bind(&tenant)
            .bind(resolve.handle.to_string())
            .bind(&holder)
            .execute(&mut *tx)
            .await
            .map_err(db)?;

            let lease_until = now.saturating_add(IsoDuration::from_secs(120));
            let scopes: Vec<String> = resolve.scopes.iter().map(enum_name).collect();
            sqlx::query(
                "insert into handoff_grant_sessions \
                 (session_ref, tenant_ref, handle, principal, scopes, lease_until, created_at) \
                 values ($1,$2,$3,$4,$5,$6,$7)",
            )
            .bind(resolve.session_ref.to_string())
            .bind(&tenant)
            .bind(resolve.handle.to_string())
            .bind(&holder)
            .bind(&scopes)
            .bind(to_chrono(lease_until))
            .bind(to_chrono(now))
            .execute(&mut *tx)
            .await
            .map_err(db)?;

            // §11.2. Every successful resolve leaves a record of who took control of what, when.
            // The address it produced is deliberately not part of that record.
            Self::emit(
                &mut tx,
                &tenant,
                Some(&grant.request_id.to_string()),
                events::GRANT_RESOLVED,
                json!({
                    "handle": resolve.handle.to_string(),
                    "session_ref": resolve.session_ref.to_string(),
                    "principal": holder,
                    "scopes": scopes,
                }),
                now,
            )
            .await?;
            tx.commit().await.map_err(db)?;

            Ok(GrantSessionView {
                session_ref: resolve.session_ref,
                scopes: resolve.scopes,
                lease_until,
                renew_after_ms: 60_000,
            })
        })
    }

    fn submit_sink_values(
        &self,
        tenant: String,
        sink_ref: String,
        values: Map<String, Value>,
    ) -> BoxFuture<'_, Result<SinkAcceptance>> {
        Box::pin(async move {
            let mut tx = self.tenant_tx(&tenant).await?;
            let request_id: Option<String> = sqlx::query_scalar(
                "select request_id from handoff_sinks where tenant_ref = $1 and sink_ref = $2",
            )
            .bind(&tenant)
            .bind(&sink_ref)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?;
            let Some(request_id) = request_id else {
                return Err(not_found(ErrorCode::RequestNotFound));
            };
            let request = self
                .load_request(&mut tx, &tenant, RequestId::parse(&request_id)?)
                .await?
                .ok_or_else(|| not_found(ErrorCode::RequestNotFound))?;
            tx.commit().await.map_err(db)?;

            // §12 rule 1. The declared-field allowlist, so a compromised surface cannot smuggle
            // arbitrary keys through to the runtime. The error names the offending key and never
            // the value.
            let declared: Vec<String> = request
                .requires
                .answer
                .as_ref()
                .map(|a| a.fields.iter().map(|f| f.name.clone()).collect())
                .unwrap_or_default();
            let undeclared: Vec<&String> =
                values.keys().filter(|k| !declared.contains(k)).collect();
            if !undeclared.is_empty() {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidRequest,
                    format!(
                        "a sink accepts only declared field names; `{}` is not one of them",
                        undeclared[0]
                    ),
                ));
            }

            // The values go no further. This implementation ships no default sink (§12 rule 5), so
            // it accepts the names, forgets the values, and stores neither.
            Ok(SinkAcceptance {
                accepted: values.keys().cloned().collect(),
                state: Some("accepted".into()),
            })
        })
    }

    fn sweep(&self, now: Timestamp) -> BoxFuture<'_, Result<SweepReport>> {
        Box::pin(async move { self.sweep_once(now).await })
    }

    fn record_channel_message(
        &self,
        tenant: String,
        id: RequestId,
        channel: String,
        text: String,
        now: Timestamp,
    ) -> BoxFuture<'_, Result<bool>> {
        Box::pin(async move {
            let mut tx = self.tenant_tx(&tenant).await?;
            let exists: Option<String> = sqlx::query_scalar(
                "select id from handoff_requests where tenant_ref = $1 and id = $2",
            )
            .bind(&tenant)
            .bind(id.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?;
            if exists.is_none() {
                return Ok(false);
            }
            // §4.7. Recorded as provisional and nothing else. There is no branch below this that
            // reads `body` and settles anything: prose matching a decision format MUST NOT settle
            // a request, however authenticated the channel.
            sqlx::query(
                "insert into handoff_channel_messages \
                 (tenant_ref, request_id, channel, body, provisional, created_at) \
                 values ($1,$2,$3,$4,true,$5)",
            )
            .bind(&tenant)
            .bind(id.to_string())
            .bind(&channel)
            .bind(&text)
            .bind(to_chrono(now))
            .execute(&mut *tx)
            .await
            .map_err(db)?;
            Self::emit(
                &mut tx,
                &tenant,
                Some(&id.to_string()),
                events::CHANNEL_MESSAGE_RECEIVED,
                json!({"channel": channel, "provisional": true, "settled": false}),
                now,
            )
            .await?;
            tx.commit().await.map_err(db)?;
            Ok(true)
        })
    }

    fn record_observation(
        &self,
        tenant: String,
        id: RequestId,
        note: String,
        now: Timestamp,
    ) -> BoxFuture<'_, Result<bool>> {
        Box::pin(async move {
            let mut tx = self.tenant_tx(&tenant).await?;
            let exists: Option<String> = sqlx::query_scalar(
                "select id from handoff_requests where tenant_ref = $1 and id = $2",
            )
            .bind(&tenant)
            .bind(id.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?;
            if exists.is_none() {
                return Ok(false);
            }
            // §9.7. An observation is recorded as an observation. There is no path from here to a
            // receipt, because a Server MUST NOT fabricate a person.
            sqlx::query(
                "insert into handoff_observations (tenant_ref, request_id, note, created_at) \
                 values ($1,$2,$3,$4)",
            )
            .bind(&tenant)
            .bind(id.to_string())
            .bind(&note)
            .bind(to_chrono(now))
            .execute(&mut *tx)
            .await
            .map_err(db)?;
            Self::emit(
                &mut tx,
                &tenant,
                Some(&id.to_string()),
                events::RUNTIME_OBSERVATION,
                json!({"note": note, "clearance": "not asserted"}),
                now,
            )
            .await?;
            tx.commit().await.map_err(db)?;
            Ok(true)
        })
    }

    fn claim_callback(&self, now: Timestamp) -> BoxFuture<'_, Result<Option<CallbackJob>>> {
        Box::pin(async move {
            let mut tx = self.pool.begin().await.map_err(db)?;
            let row = sqlx::query(
                "update handoff_signals \
                 set callback_lease_until = $1, attempts = attempts + 1, \
                     callback_delivery_id = coalesce(callback_delivery_id, $3) \
                 where id = (select id from handoff_signals \
                             where acked_at is null and callback_url is not null \
                               and callback_disabled_at is null \
                               and next_callback_at is not null and next_callback_at <= $2 \
                               and (callback_lease_until is null or callback_lease_until <= $2) \
                             order by next_callback_at limit 1 for update skip locked) \
                 returning *",
            )
            .bind(to_chrono(now.saturating_add(IsoDuration::from_secs(30))))
            .bind(to_chrono(now))
            .bind(ids::mint::<handoff_protocol::id::Delivery>(now.to_millis() as u64)?.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?;
            let Some(row) = row else {
                tx.commit().await.map_err(db)?;
                return Ok(None);
            };
            let signal = row_to_signal(&row)?;
            let tenant: String = row.get("tenant_ref");
            let url: String = row
                .get::<Option<String>, _>("callback_url")
                .unwrap_or_default();
            let attempt = row.get::<i32, _>("attempts") as u32;
            tx.commit().await.map_err(db)?;

            // One delivery identity for this push, minted on the first claim and reused by every
            // retry of it. `signing.md` §1.2 puts the id inside the signed string so a signature
            // cannot be lifted onto a *different* delivery — and §1.3 rule 7 has the receiver
            // dedupe on that same id, which only works if a retry carries the id it is retrying.
            // Minting a new one per attempt satisfies the first rule and quietly breaks the
            // second, leaving a conforming receiver applying one decision once per retry.
            let delivery_id = DeliveryId::parse(
                row.get::<Option<String>, _>("callback_delivery_id")
                    .unwrap_or_default()
                    .as_str(),
            )?;
            Ok(Some(CallbackJob {
                signal_id: signal.id,
                tenant_ref: tenant,
                url,
                delivery_id,
                sequence: signal.sequence,
                body: to_json(&signal, "signal")?,
                attempt,
            }))
        })
    }

    fn record_callback_attempt(
        &self,
        job: CallbackJob,
        attempt: CallbackAttemptView,
        next_attempt_at: Option<Timestamp>,
    ) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let mut tx = self.tenant_tx(&job.tenant_ref).await?;
            sqlx::query(
                "insert into handoff_callback_attempts \
                 (tenant_ref, signal_id, n, delivery_id, started_at, ended_at, status_code, \
                  duration_ms, outcome, error) values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) \
                 on conflict (tenant_ref, signal_id, n) do nothing",
            )
            .bind(&job.tenant_ref)
            .bind(job.signal_id.to_string())
            .bind(attempt.n as i32)
            .bind(job.delivery_id.to_string())
            .bind(to_chrono(attempt.started_at))
            .bind(opt_chrono(attempt.ended_at))
            .bind(attempt.status_code)
            .bind(attempt.duration_ms)
            .bind(&attempt.outcome)
            .bind(attempt.error.as_deref())
            .execute(&mut *tx)
            .await
            .map_err(db)?;

            // §15.5: the retry budget is bounded, and an endpoint that has spent it is disabled
            // rather than retried forever. Silent permanent retry is how queues die — so this
            // stops, records why, and leaves the signal unacked and readable, because the runtime
            // may still come and collect it by polling.
            let exhausted = attempt.n >= MAX_CALLBACK_ATTEMPTS;
            sqlx::query(
                "update handoff_signals \
                 set callback_lease_until = null, next_callback_at = $3, \
                     callback_disabled_at = case when $4 then $5 else callback_disabled_at end \
                 where tenant_ref = $1 and id = $2 and acked_at is null",
            )
            .bind(&job.tenant_ref)
            .bind(job.signal_id.to_string())
            // §15.4 and §8.3. A `2xx` marks the callback dispatched; the signal stays outstanding
            // and redelivery continues until an explicit ack arrives.
            .bind(opt_chrono(
                (!exhausted).then_some(next_attempt_at).flatten(),
            ))
            .bind(exhausted)
            .bind(to_chrono(attempt.started_at))
            .execute(&mut *tx)
            .await
            .map_err(db)?;

            if exhausted {
                // "The tenant notified" (§15.5). This deployment has one channel for telling a
                // tenant something happened, and it is the event record — so the notification is
                // an event rather than a silence somebody has to go looking for.
                Self::emit(
                    &mut tx,
                    &job.tenant_ref,
                    None,
                    events::CALLBACK_ENDPOINT_DISABLED,
                    json!({
                        "signal_id": job.signal_id.to_string(),
                        "attempts": attempt.n,
                        "last_status": attempt.status_code,
                        "still_readable": true,
                    }),
                    attempt.started_at,
                )
                .await?;
            }
            tx.commit().await.map_err(db)?;
            Ok(())
        })
    }

    fn authenticate(&self, presented_secret: String) -> BoxFuture<'_, Result<Option<Principal>>> {
        Box::pin(async move {
            use sha2::{Digest as _, Sha256};
            let hash = format!("{:x}", Sha256::digest(presented_secret.as_bytes()));
            // §4.1. Tenancy comes from stored state bound to the credential, never from a body.
            // The comparison is against a digest, so the secret is not stored recoverably.
            let row = sqlx::query("select * from handoff_principals where secret_sha256 = $1")
                .bind(&hash)
                .fetch_optional(&self.pool)
                .await
                .map_err(db)?;
            let Some(row) = row else { return Ok(None) };
            let kind = match row.get::<String, _>("kind").as_str() {
                "machine" => PrincipalKind::Machine,
                "anonymous_link" => PrincipalKind::AnonymousLink,
                _ => PrincipalKind::Human,
            };
            let id = match kind {
                PrincipalKind::AnonymousLink => None,
                _ => Some(handoff_protocol::id::PrincipalId::parse(
                    row.get::<String, _>("id").as_str(),
                )?),
            };
            Ok(Some(Principal {
                id,
                kind,
                tenant_ref: row.get("tenant_ref"),
                role: parse_role(&row.get::<String, _>("role")),
                auth_strength: parse_strength(&row.get::<String, _>("auth_strength")),
                display: row.get("display"),
                scopes: row.get("scopes"),
            }))
        })
    }

    fn idempotent_replay(
        &self,
        slot: IdempotencySlot,
    ) -> BoxFuture<'_, Result<Option<StoredResponse>>> {
        Box::pin(async move {
            let row = sqlx::query(
                "select body_digest, response_status, response_body from handoff_idempotency \
                 where tenant_ref = $1 and principal_ref = $2 and operation = $3 and key = $4",
            )
            .bind(&slot.tenant)
            .bind(&slot.principal)
            .bind(&slot.operation)
            .bind(&slot.key)
            .fetch_optional(&self.pool)
            .await
            .map_err(db)?;
            let Some(row) = row else { return Ok(None) };
            if row.get::<String, _>("body_digest") != slot.body_digest.to_string() {
                return Err(ProtocolError::new(
                    ErrorCode::IdempotencyKeyReused,
                    "this Idempotency-Key was used with a different body",
                ));
            }
            Ok(Some(StoredResponse {
                status: row.get::<i32, _>("response_status") as u16,
                body: row.get("response_body"),
            }))
        })
    }

    fn remember_idempotent(
        &self,
        slot: IdempotencySlot,
        response: StoredResponse,
        now: Timestamp,
    ) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            sqlx::query(
                "insert into handoff_idempotency \
                 (tenant_ref, principal_ref, operation, key, body_digest, response_status, \
                  response_body, created_at) values ($1,$2,$3,$4,$5,$6,$7,$8) \
                 on conflict (tenant_ref, principal_ref, operation, key) do nothing",
            )
            .bind(&slot.tenant)
            .bind(&slot.principal)
            .bind(&slot.operation)
            .bind(&slot.key)
            .bind(slot.body_digest.to_string())
            .bind(response.status as i32)
            .bind(&response.body)
            .bind(to_chrono(now))
            .execute(&self.pool)
            .await
            .map_err(db)?;
            Ok(())
        })
    }
}

// ------------------------------------------------------------------------------ private helpers

/// Refuse a write against a request that has already settled (§6.7 rule 2).
fn guard_pending(request: &RequestView) -> Result<()> {
    if request.state == RequestState::Pending {
        return Ok(());
    }
    Err(settled_conflict(
        request.state,
        request
            .receipt
            .as_ref()
            .map(|r| r.id.to_string())
            .as_deref(),
        request.superseded_by.map(|id| id.to_string()).as_deref(),
    ))
}

/// Read the current state and build the specific conflict for it.
///
/// Reached when a conditional write affected no row, which means another writer settled the
/// request between the read and the write. §6.7 rule 2 requires the code to name what actually
/// happened rather than a generic conflict.
impl PgStore {
    async fn conflict_for(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        tenant: &str,
        id: RequestId,
    ) -> Result<ProtocolError> {
        let row = sqlx::query(
            "select r.state, r.superseded_by, \
                    (select id from handoff_receipts x where x.tenant_ref = r.tenant_ref \
                     and x.request_id = r.id order by height limit 1) as receipt_id \
             from handoff_requests r where r.tenant_ref = $1 and r.id = $2",
        )
        .bind(tenant)
        .bind(id.to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(db)?;
        Ok(match row {
            Some(row) => settled_conflict(
                parse_state(&row.get::<String, _>("state")),
                row.get::<Option<String>, _>("receipt_id").as_deref(),
                row.get::<Option<String>, _>("superseded_by").as_deref(),
            ),
            None => not_found(ErrorCode::RequestNotFound),
        })
    }

    /// One pass of the three deadline-driven transitions.
    ///
    /// Each request is settled in its own transaction, so a failure part-way through a batch leaves
    /// the requests it already handled committed with their events and the rest untouched.
    async fn sweep_once(&self, now: Timestamp) -> Result<SweepReport> {
        let mut report = SweepReport::default();

        // R3. The attempt clock. Stamped **once, ever**: the guard is the `attempt_lapse_notified`
        // predicate, not a hope that the sweep runs once.
        let lapsed = sqlx::query(
            "select id, tenant_ref from handoff_requests \
             where state = 'pending' and attempt_lapse_notified = false \
               and attempt_expires_at is not null and attempt_expires_at <= $1",
        )
        .bind(to_chrono(now))
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        for row in lapsed {
            let tenant: String = row.get("tenant_ref");
            let id = RequestId::parse(row.get::<String, _>("id").as_str())?;
            let mut tx = self.tenant_tx(&tenant).await?;
            let claimed: Option<String> = sqlx::query_scalar(
                "update handoff_requests set attempt_lapse_notified = true, urgency_state = 'waiting' \
                 where tenant_ref = $1 and id = $2 and state = 'pending' \
                   and attempt_lapse_notified = false returning id",
            )
            .bind(&tenant)
            .bind(id.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?;
            if claimed.is_none() {
                tx.commit().await.map_err(db)?;
                continue;
            }
            let request = self
                .load_request(&mut tx, &tenant, id)
                .await?
                .ok_or_else(|| not_found(ErrorCode::RequestNotFound))?;
            // §8.3. `attempt_lapsed` decides nothing, so it carries no decision — and because
            // signals are a queue, it can never mask the terminal signal that follows it.
            Self::enqueue_signal(
                &mut tx,
                &tenant,
                SignalToEnqueue {
                    waiter_ref: &request.waiter_ref,
                    request_id: id,
                    signal_type: SignalType::AttemptLapsed,
                    decision: None,
                    continuation: &request.continuation,
                    callback_url: request.callback.as_ref().map(|c| c.url.as_str()),
                },
                now,
            )
            .await?;
            Self::emit(
                &mut tx,
                &tenant,
                Some(&id.to_string()),
                events::ATTEMPT_LAPSED,
                json!({"urgency_state": "waiting", "still_listed": true}),
                now,
            )
            .await?;
            tx.commit().await.map_err(db)?;
            report.attempts_lapsed += 1;
        }

        // R4. Ladder rungs whose timer has elapsed.
        let due = sqlx::query(
            "select id, tenant_ref, created_at, routing, rung from handoff_requests \
             where state = 'pending'",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        for row in due {
            let routing: Routing = parse_json(row.get("routing"), "routing")?;
            let created: DateTime<Utc> = row.get("created_at");
            let elapsed = IsoDuration::from_secs(
                (now.to_millis() - from_chrono(created).to_millis()).max(0) as u64 / 1000,
            );
            let current = row.get::<i32, _>("rung") as u32;
            let highest_due = handoff_core::channel::rungs_due(&routing, elapsed)
                .into_iter()
                .max()
                .unwrap_or(0);
            if highest_due <= current {
                continue;
            }
            let tenant: String = row.get("tenant_ref");
            let id = RequestId::parse(row.get::<String, _>("id").as_str())?;
            let mut tx = self.tenant_tx(&tenant).await?;
            let claimed: Option<String> = sqlx::query_scalar(
                "update handoff_requests set rung = $3 \
                 where tenant_ref = $1 and id = $2 and state = 'pending' and rung < $3 returning id",
            )
            .bind(&tenant)
            .bind(id.to_string())
            .bind(highest_due as i32)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?;
            if claimed.is_none() {
                tx.commit().await.map_err(db)?;
                continue;
            }
            for rung in (current + 1)..=highest_due {
                self.mint_rung(&mut tx, &tenant, id, &routing, rung, now)
                    .await?;
            }
            Self::emit(
                &mut tx,
                &tenant,
                Some(&id.to_string()),
                events::REQUEST_ESCALATED,
                json!({"rung": highest_due, "fired_by": "ladder timer"}),
                now,
            )
            .await?;
            tx.commit().await.map_err(db)?;
            report.rungs_fired += 1;
        }

        // R6. The request clock. `park` never expires; `escalate` extends while rungs remain and
        // then falls through to the deployment's terminal policy.
        let expiring = sqlx::query(
            "select id, tenant_ref from handoff_requests \
             where state = 'pending' and expires_at is not null and expires_at <= $1",
        )
        .bind(to_chrono(now))
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        for row in expiring {
            let tenant: String = row.get("tenant_ref");
            let id = RequestId::parse(row.get::<String, _>("id").as_str())?;
            let mut tx = self.tenant_tx(&tenant).await?;
            let request = match self.load_request(&mut tx, &tenant, id).await? {
                Some(request) if request.state == RequestState::Pending => request,
                _ => {
                    tx.commit().await.map_err(db)?;
                    continue;
                }
            };
            let policy = request
                .ttl_policy
                .as_ref()
                .map_or(OnExpiry::ExpireAndDeny, |p| p.on_expiry);
            if policy == OnExpiry::Park {
                tx.commit().await.map_err(db)?;
                continue;
            }
            if policy == OnExpiry::Escalate
                && (request.rung as usize + 1) < request.routing.ladder.len()
            {
                let next = request.rung + 1;
                self.mint_rung(&mut tx, &tenant, id, &request.routing, next, now)
                    .await?;
                sqlx::query(
                    "update handoff_requests set rung = $3, expires_at = $4 \
                     where tenant_ref = $1 and id = $2 and state = 'pending'",
                )
                .bind(&tenant)
                .bind(id.to_string())
                .bind(next as i32)
                .bind(to_chrono(
                    now.saturating_add(IsoDuration::from_secs(15 * 60)),
                ))
                .execute(&mut *tx)
                .await
                .map_err(db)?;
                Self::emit(
                    &mut tx,
                    &tenant,
                    Some(&id.to_string()),
                    events::REQUEST_ESCALATED,
                    json!({"rung": next, "fired_by": "ttl policy escalate"}),
                    now,
                )
                .await?;
                tx.commit().await.map_err(db)?;
                report.rungs_fired += 1;
                continue;
            }

            let settled: Option<String> = sqlx::query_scalar(
                "update handoff_requests set state = 'expired', urgency_state = 'waiting' \
                 where tenant_ref = $1 and id = $2 and state = 'pending' returning id",
            )
            .bind(&tenant)
            .bind(id.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?;
            if settled.is_none() {
                tx.commit().await.map_err(db)?;
                continue;
            }

            let receipt_id = ids::mint::<handoff_protocol::id::Receipt>(now.to_millis() as u64)?;
            let terminal = plan::plan_expiry(&request, receipt_id, now)?;
            let mut decision = terminal.decision;
            if let Some(receipt) = terminal.receipt {
                let sealed = Self::append_receipt(&mut tx, &tenant, receipt).await?;
                decision.receipt_id = Some(sealed.id);
            }
            Self::close_out(&mut tx, &tenant, id, now).await?;
            Self::enqueue_signal(
                &mut tx,
                &tenant,
                SignalToEnqueue {
                    waiter_ref: &request.waiter_ref,
                    request_id: id,
                    signal_type: SignalType::Expired,
                    decision: Some(&decision),
                    continuation: &request.continuation,
                    callback_url: request.callback.as_ref().map(|c| c.url.as_str()),
                },
                now,
            )
            .await?;
            Self::emit(
                &mut tx,
                &tenant,
                Some(&id.to_string()),
                events::REQUEST_EXPIRED,
                json!({"policy": enum_name(&policy)}),
                now,
            )
            .await?;
            tx.commit().await.map_err(db)?;
            report.requests_expired += 1;
        }

        Ok(report)
    }
}

/// Every grant on one request.
async fn load_grants(
    tx: &mut Transaction<'_, Postgres>,
    tenant: &str,
    request_id: RequestId,
) -> Result<Vec<GrantView>> {
    let rows = sqlx::query(
        "select * from handoff_grants where tenant_ref = $1 and request_id = $2 order by created_at",
    )
    .bind(tenant)
    .bind(request_id.to_string())
    .fetch_all(&mut **tx)
    .await
    .map_err(db)?;
    rows.iter().map(row_to_grant).collect()
}

/// Which delivery an answer arrived through.
///
/// The caller's `via_delivery_id` wins. Otherwise the newest delivery that is still open is the
/// honest guess, because that is the one a person was most recently pointed at.
fn pick_delivery(request: &RequestView, declared: Option<DeliveryId>) -> Option<DeliveryView> {
    // "Answered through this delivery" has to be a claim about a delivery the person could
    // actually have used. Two things follow, and the first one is easy to get wrong in the
    // direction that flatters us.
    //
    // **A withheld or failed delivery is not a candidate.** Suppressed, failed and bounced
    // deliveries never put the ask in front of anybody, so nobody answered through one. On the
    // reference deployment the default ladder's second rung is `email`, a scaffold that transmits
    // nothing and ends `suppressed` — and it is the *newest* delivery by the time anyone answers.
    // Taking the newest open delivery therefore wrote an email nobody sent onto the receipt as the
    // one the person answered through.
    //
    // **But `queued` is still a candidate**, which is the part that looks wrong and is not. For
    // the in-app surface, dispatching *is* the request being listable at its canonical URL, and I4
    // guarantees that from the moment it is raised; the queue row is our own bookkeeping catching
    // up. Excluding `queued` would make a receipt's contents depend on whether a background worker
    // had run yet, which is a race, not a fact about the intervention.
    let could_have_been_used = |d: &&DeliveryView| {
        !matches!(
            d.state,
            DeliveryState::Suppressed
                | DeliveryState::Failed
                | DeliveryState::Bounced
                | DeliveryState::Cancelled
                | DeliveryState::Stale
        )
    };

    // Among candidates, the strongest claim wins rather than the most recent. §4.7: only a channel
    // that can authenticate a person can carry an answer, so one that can outranks one that cannot
    // however lately it was minted; then the grade actually reached; then recency as a tie-break.
    let best = |deliveries: &[DeliveryView]| {
        deliveries
            .iter()
            .enumerate()
            .filter(|(_, d)| could_have_been_used(d))
            .max_by_key(|(index, d)| (d.can_authenticate_person, d.grade_reached, *index))
            .map(|(_, d)| d.clone())
    };

    if let Some(id) = declared {
        // A client naming a delivery that was never sent is mistaken about its own history, and
        // honouring it would write that mistake into an evidence artifact. The answer still stands
        // — the person did answer — so this falls through to the honest choice rather than
        // refusing a decision over a wrong annotation.
        if let Some(named) = request
            .deliveries
            .iter()
            .find(|d| d.id == id)
            .filter(could_have_been_used)
        {
            return Some(named.clone());
        }
    }
    best(&request.deliveries)
}

/// Record which request an `Idempotency-Key` produced, so a replay returns it **in its current
/// state** rather than a frozen copy of the first response (§3.3 rule 1).
async fn remember_raise(
    tx: &mut Transaction<'_, Postgres>,
    tenant: &str,
    principal: &str,
    key: &str,
    body_digest: &Digest,
    request_id: &RequestId,
    now: Timestamp,
) -> Result<()> {
    sqlx::query(
        "insert into handoff_idempotency \
         (tenant_ref, principal_ref, operation, key, body_digest, response_status, response_body, \
          request_id, created_at) values ($1,$2,'raise',$3,$4,200,'',$5,$6) \
         on conflict (tenant_ref, principal_ref, operation, key) do nothing",
    )
    .bind(tenant)
    .bind(principal)
    .bind(key)
    .bind(body_digest.to_string())
    .bind(request_id.to_string())
    .bind(to_chrono(now))
    .execute(&mut **tx)
    .await
    .map_err(db)?;
    Ok(())
}

fn parse_role(text: &str) -> Role {
    match text {
        "admin" => Role::Admin,
        "editor" => Role::Editor,
        _ => Role::Viewer,
    }
}

fn parse_strength(text: &str) -> AuthStrength {
    match text {
        "mfa" => AuthStrength::Mfa,
        "reauth" => AuthStrength::Reauth,
        "link_only" => AuthStrength::LinkOnly,
        _ => AuthStrength::Session,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_is_capped() {
        assert!(backoff_seconds(1) < backoff_seconds(4));
        assert!(backoff_seconds(20) <= 303);
    }

    #[test]
    fn a_settled_request_names_the_record_that_settled_it() {
        let err = settled_conflict(RequestState::Answered, Some("rcpt_X"), None);
        assert_eq!(err.code, ErrorCode::AlreadyAnswered);
        assert_eq!(err.context().receipt_id.as_deref(), Some("rcpt_X"));

        let err = settled_conflict(RequestState::Superseded, None, Some("req_Y"));
        assert_eq!(err.code, ErrorCode::RequestSuperseded);
        assert_eq!(err.context().superseded_by.as_deref(), Some("req_Y"));
    }

    #[test]
    fn every_settled_state_has_its_own_code() {
        for (state, code) in [
            (RequestState::Answered, ErrorCode::AlreadyAnswered),
            (RequestState::Expired, ErrorCode::RequestExpired),
            (RequestState::Cancelled, ErrorCode::RequestCancelled),
            (RequestState::Superseded, ErrorCode::RequestSuperseded),
        ] {
            assert_eq!(settled_conflict(state, None, None).code, code);
        }
    }
}

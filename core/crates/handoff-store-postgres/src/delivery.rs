//! Delivery, as a thing that actually happens.
//!
//! §7 makes delivery a first-class tracked entity "not a side effect of a notification sweep".
//! Minting a row in `queued` and never touching it again has the shape of that and none of the
//! substance: "we tried" is a claim that has to survive being questioned, and a row nobody ever
//! picked up cannot survive it. So this module is the other half — claim a due delivery, put it in
//! front of an adapter, and record what came back.
//!
//! Three decisions here are load-bearing.
//!
//! **`sending` is never persisted.** The claim, the attempt, and the outcome commit as one
//! transaction, so a delivery is only ever observed in a state it actually rests in. Writing
//! `sending` first and the outcome second would leave a delivery stuck in `sending` forever
//! whenever a worker died mid-flight, and "stuck in sending" is indistinguishable from "still
//! trying". A crash instead leaves it `queued` with an expired lease, and the next worker takes it.
//!
//! **A place is resolved into people, once.** §7.5 says a Server SHOULD address deliveries to
//! individual people rather than to a place, because read state shared across a workspace means one
//! person clearing a notification clears everyone's. A delivery minted against `role:editor` is
//! therefore resolved on first claim: this row takes the first person, and **sibling deliveries**
//! are minted for the rest. Siblings carry a `principal_ref` and so are never fanned out again.
//! Minting deliveries is exactly what I3 permits — it is minting a *request* that is forbidden.
//!
//! **Nobody is not an error.** A role with no members, a rotation with no one on call, a channel
//! this build did not compile in: each of those ends as a `suppressed` delivery carrying a reason
//! code. §7.1 calls suppression a real outcome, and the alternative — a delivery that quietly
//! stays queued — is the failure mode where an operator believes somebody was paged.

use handoff_core::events;
use handoff_core::ids;
use handoff_core::outbound::Suppression;
use handoff_core::ports::BoxFuture;
use handoff_core::seam::{ContactPoint, Recipient, RecipientDirectory};
use handoff_protocol::clock::{IsoDuration, Timestamp};
use handoff_protocol::delivery::{
    self, ChannelCapabilities, DeliveryEvent, DeliveryGrade, DeliveryState,
};
use handoff_protocol::error::{ErrorCode, ProtocolError, Result};
use handoff_protocol::id::{DeliveryId, RequestId};
use handoff_protocol::request::Prompt;
use handoff_protocol::requires::{Target, TargetKind};
use serde_json::json;
use sqlx::Row;

use crate::store::{
    backoff_seconds, delivery_state_name, from_chrono, grade_name, parse_delivery_state,
    parse_grade, parse_target_kind, to_chrono, PgStore,
};

/// How many transport attempts one delivery gets before it is `failed`.
///
/// §7.3 requires "a bounded attempt count" without fixing the number. Six, with the `2^n` backoff
/// of [`backoff_seconds`], spans a little over five minutes — long enough to ride out a provider
/// blip, short enough that a ladder's next rung is the thing that escalates rather than a retry
/// loop nobody is watching.
pub const MAX_DELIVERY_ATTEMPTS: u32 = 6;

/// How long a worker holds a claimed delivery before another may take it.
const LEASE_SECONDS: u64 = 30;

/// One delivery, claimed and ready to attempt.
#[derive(Debug, Clone)]
pub struct DeliveryJob {
    /// The delivery.
    pub id: DeliveryId,
    /// The tenant, from stored state and never from a body (I13).
    pub tenant_ref: String,
    /// The one request it belongs to (I3).
    pub request_id: RequestId,
    /// Which channel is being asked to carry it.
    pub channel: String,
    /// Which ladder rung minted it.
    pub rung: u32,
    /// Which attempt this is, from 1.
    pub attempt: u32,
    /// What this channel declared it can prove (§7.2).
    pub capabilities: ChannelCapabilities,
    /// The grade already reached, if any. A grade only ever advances.
    pub grade_reached: Option<DeliveryGrade>,
    /// Who it is addressed to, once a place has been resolved into a person.
    pub recipient: Recipient,
    /// What the person is shown.
    pub prompt: Prompt,
    /// Where they go to answer. A locator, never an authorization (§4.6).
    pub surface_url: String,
}

impl PgStore {
    /// Claim one due delivery, leasing it so two workers cannot both attempt it.
    ///
    /// Returns `None` when nothing is due. Resolves the target on first claim, minting sibling
    /// deliveries for everyone else the target named, and suppresses when it named nobody.
    pub async fn claim_delivery(
        &self,
        directory: &dyn RecipientDirectory,
        public_base: &str,
        now: Timestamp,
    ) -> Result<Option<DeliveryJob>> {
        let mut tx = self.pool().begin().await.map_err(store_error)?;
        let row = sqlx::query(
            "update handoff_deliveries set lease_until = $1, attempt_count = attempt_count + 1 \
             where id = (select id from handoff_deliveries \
                         where state in ('queued','retrying') \
                           and next_attempt_at is not null and next_attempt_at <= $2 \
                           and (lease_until is null or lease_until <= $2) \
                         order by next_attempt_at limit 1 for update skip locked) \
             returning *",
        )
        .bind(to_chrono(
            now.saturating_add(IsoDuration::from_secs(LEASE_SECONDS)),
        ))
        .bind(to_chrono(now))
        .fetch_optional(&mut *tx)
        .await
        .map_err(store_error)?;
        let Some(row) = row else {
            tx.commit().await.map_err(store_error)?;
            return Ok(None);
        };

        let tenant: String = row.get("tenant_ref");
        let id = DeliveryId::parse(row.get::<String, _>("id").as_str())?;
        let request_id = RequestId::parse(row.get::<String, _>("request_id").as_str())?;
        let channel: String = row.get("channel");
        let attempt = row.get::<i32, _>("attempt_count") as u32;
        let already: Option<DeliveryGrade> = row
            .get::<Option<String>, _>("grade_reached")
            .map(|grade| parse_grade(&grade));
        let target = Target {
            kind: parse_target_kind(&row.get::<String, _>("target_kind")),
            value: row.get("target_value"),
        };
        let bound: Option<String> = row.get("principal_ref");
        tx.commit().await.map_err(store_error)?;

        // What the person is shown. Read from the request rather than carried on the delivery, so
        // an amendment is reflected in a rung that fires after it (§6.2 R2).
        let request = sqlx::query(
            "select prompt, requires from handoff_requests where tenant_ref = $1 and id = $2",
        )
        .bind(&tenant)
        .bind(request_id.to_string())
        .fetch_optional(self.pool())
        .await
        .map_err(store_error)?;
        let Some(request) = request else {
            // The request is gone; there is nothing left to deliver and nothing to record it
            // against. Release the lease rather than leaving it held for the full window.
            self.release_delivery(&tenant, id, now).await?;
            return Ok(None);
        };
        let prompt: Prompt = serde_json::from_value(request.get("prompt")).map_err(|e| {
            ProtocolError::new(ErrorCode::InvalidRequest, format!("stored prompt: {e}"))
        })?;

        let recipient = match &bound {
            // Already addressed to one person: resolved on an earlier claim, or minted as a
            // sibling. Either way it is never fanned out a second time.
            Some(principal_ref) => self.recipient_for(&tenant, principal_ref).await?,
            None => {
                let mut people = directory
                    .resolve(tenant.clone(), target.clone())
                    .await?
                    .into_iter();
                let Some(first) = people.next() else {
                    self.settle_delivery(
                        &tenant,
                        id,
                        DeliveryEvent::Suppress,
                        Some(Suppression::NoRecipient.to_string()),
                        None,
                        now,
                    )
                    .await?;
                    return Ok(None);
                };
                // Everyone else the place named gets their own delivery. Deliveries, never a
                // second request (I3).
                for other in people {
                    self.mint_sibling(
                        Sibling {
                            tenant: &tenant,
                            request_id,
                            sibling_of: id,
                            channel: &channel,
                            target: &target,
                            recipient: &other,
                        },
                        now,
                    )
                    .await?;
                }
                self.bind_delivery(&tenant, id, &first, now).await?;
                first
            }
        };

        Ok(Some(DeliveryJob {
            id,
            tenant_ref: tenant,
            request_id,
            channel: channel.clone(),
            rung: row.get::<i32, _>("rung") as u32,
            attempt,
            capabilities: self.channels.capabilities(&channel),
            grade_reached: already,
            recipient,
            prompt,
            surface_url: format!("{public_base}/requests/{request_id}"),
        }))
    }

    /// Record what one attempt achieved, and where the delivery now rests.
    ///
    /// The attempt row and the state change commit together, for the same reason every other
    /// transition in this store does (I12): an attempt log that can disagree with the state it
    /// explains is not evidence.
    pub async fn record_delivery_outcome(
        &self,
        job: &DeliveryJob,
        report: &handoff_core::seam::DeliveryReport,
        now: Timestamp,
    ) -> Result<DeliveryState> {
        // The adapter already said why, as `code: detail`. Stored verbatim: re-deriving a reason
        // from a code would throw away the half that names what an operator has to go and fix.
        let suppression = (report.state == DeliveryState::Suppressed)
            .then(|| report.detail.clone())
            .flatten();

        // The grade is only ever what the channel actually reported, clamped to what it declared
        // it can prove. §7.2: a Server MUST NOT synthesize an intermediate grade it did not
        // observe, and MUST NOT record one above the channel's ceiling.
        let event = match (report.state, report.grade) {
            (DeliveryState::Suppressed, _) => DeliveryEvent::Suppress,
            (DeliveryState::Bounced, _) => DeliveryEvent::Bounce,
            (DeliveryState::Retrying, _) if job.attempt >= MAX_DELIVERY_ATTEMPTS => {
                DeliveryEvent::Exhausted
            }
            (DeliveryState::Retrying, _) => DeliveryEvent::ScheduleRetry,
            (DeliveryState::Failed, _) => DeliveryEvent::Exhausted,
            (_, Some(grade)) => DeliveryEvent::AdvanceGrade(grade.min(job.capabilities.max_grade)),
            // A progress state with no grade is an adapter contract violation, and the honest
            // response is to record no evidence rather than the least evidence. Defaulting to
            // `dispatched` would be synthesizing a grade nothing observed — the very thing §7.2
            // forbids, and the same defect as recording a channel's ceiling as what it reached.
            // `dispatched` is a real claim ("our transport accepted it"); spending it on an
            // adapter that failed to say so puts a send in the record that may never have happened.
            // So it lands as a retryable failure: visible in the attempt log, attributable to the
            // adapter, and not evidence of anything.
            (_, None) => DeliveryEvent::ScheduleRetry,
        };

        // The attempt log says what actually happened, not what the report claimed. An adapter
        // that reported progress without a grade did not dispatch anything, and a log line saying
        // it did is how a contract violation becomes invisible.
        let ungraded_progress = report.grade.is_none()
            && !matches!(
                report.state,
                DeliveryState::Suppressed
                    | DeliveryState::Bounced
                    | DeliveryState::Retrying
                    | DeliveryState::Failed
            );

        self.settle_delivery(
            &job.tenant_ref,
            job.id,
            event,
            suppression,
            Some(Attempt {
                n: job.attempt,
                outcome: if ungraded_progress {
                    DeliveryState::Retrying
                } else {
                    report.state
                },
                transport_status: if ungraded_progress {
                    Some(format!(
                        "the {} adapter reported `{:?}` with no grade, which proves nothing; \
                         recorded as a failed attempt rather than as a dispatch",
                        job.channel, report.state
                    ))
                } else {
                    report.detail.clone()
                },
                retryable: report.retryable || ungraded_progress,
            }),
            now,
        )
        .await
    }

    /// Record evidence that arrived **after** the send returned.
    ///
    /// A synchronous `deliver` can only ever report what the transport said at the moment it was
    /// handed the message, which for most real channels is `dispatched` and nothing more. The
    /// grades that actually matter — the provider's delivery receipt, the person opening the
    /// surface — arrive later and out of band, through a webhook the deployment owns.
    ///
    /// So this is the ingress for them, and it is deliberately not on
    /// [`DeliveryChannel`](handoff_core::seam::DeliveryChannel): an adapter that had to stay alive
    /// until a person opened an email would hold a worker for hours, and a delivery fleet that
    /// reports asynchronously is the normal case rather than the exotic one.
    ///
    /// The grade is clamped to the channel's declared ceiling and may only advance, both enforced
    /// by [`handoff_protocol::delivery::transition`] — so a provider that claims more than its
    /// channel declared is refused rather than believed (§7.2).
    pub async fn advance_delivery_grade(
        &self,
        tenant: &str,
        id: DeliveryId,
        grade: DeliveryGrade,
        now: Timestamp,
    ) -> Result<DeliveryState> {
        self.settle_delivery(
            tenant,
            id,
            DeliveryEvent::AdvanceGrade(grade),
            None,
            None,
            now,
        )
        .await
    }

    /// Apply one delivery transition, with its attempt row and its event, in one transaction.
    async fn settle_delivery(
        &self,
        tenant: &str,
        id: DeliveryId,
        event: DeliveryEvent,
        suppression: Option<String>,
        attempt: Option<Attempt>,
        now: Timestamp,
    ) -> Result<DeliveryState> {
        let mut tx = self.tenant_tx(tenant).await?;
        let row = sqlx::query(
            "select state, channel, attempt_count from handoff_deliveries \
             where tenant_ref = $1 and id = $2 for update",
        )
        .bind(tenant)
        .bind(id.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(store_error)?
        .ok_or_else(|| {
            ProtocolError::new(
                ErrorCode::RequestNotFound,
                "no such delivery in this tenant",
            )
        })?;

        let from = parse_delivery_state(&row.get::<String, _>("state"));
        let channel: String = row.get("channel");
        let capabilities = self.channels.capabilities(&channel);

        // `sending` is composed rather than persisted: a send begins and ends inside this one
        // transaction, so no delivery is ever left resting in a state it is not actually in.
        let staged = match event {
            DeliveryEvent::Suppress | DeliveryEvent::Cancel | DeliveryEvent::MarkStale => from,
            _ if from == DeliveryState::Queued || from == DeliveryState::Retrying => {
                delivery::transition(Some(from), DeliveryEvent::StartSend, &capabilities)?.to
            }
            _ => from,
        };
        let moved = delivery::transition(Some(staged), event, &capabilities)?;

        let next_attempt_at = (moved.to == DeliveryState::Retrying).then(|| {
            now.saturating_add(IsoDuration::from_secs(backoff_seconds(
                row.get::<i32, _>("attempt_count") as u32,
            ) as u64))
        });

        sqlx::query(
            "update handoff_deliveries set state = $3, grade_reached = $4, updated_at = $5, \
             lease_until = null, next_attempt_at = $6, \
             suppression_reason = coalesce($7, suppression_reason) \
             where tenant_ref = $1 and id = $2",
        )
        .bind(tenant)
        .bind(id.to_string())
        .bind(delivery_state_name(moved.to))
        .bind(moved.grade_reached.map(grade_name))
        .bind(to_chrono(now))
        .bind(next_attempt_at.map(to_chrono))
        .bind(suppression.as_deref())
        .execute(&mut *tx)
        .await
        .map_err(store_error)?;

        if let Some(attempt) = &attempt {
            sqlx::query(
                "insert into handoff_delivery_attempts \
                 (tenant_ref, delivery_id, n, started_at, ended_at, outcome, transport_status, error) \
                 values ($1,$2,$3,$4,$4,$5,$6,$7) \
                 on conflict (tenant_ref, delivery_id, n) do nothing",
            )
            .bind(tenant)
            .bind(id.to_string())
            .bind(attempt.n as i32)
            .bind(to_chrono(now))
            .bind(delivery_state_name(attempt.outcome))
            .bind(attempt.transport_status.as_deref())
            // Failure detail only. §7.3's `error` never carries message content.
            .bind(
                (!attempt.retryable && attempt.outcome != DeliveryState::Dispatched)
                    .then(|| attempt.transport_status.clone())
                    .flatten(),
            )
            .execute(&mut *tx)
            .await
            .map_err(store_error)?;
        }

        PgStore::emit(
            &mut tx,
            tenant,
            None,
            events::DELIVERY_TRANSITIONED,
            json!({
                "delivery_id": id.to_string(),
                "channel": channel,
                "from": delivery_state_name(from),
                "to": delivery_state_name(moved.to),
                "grade_reached": moved.grade_reached.map(grade_name),
                "attempt": attempt.as_ref().map(|a| a.n),
                "suppression_reason": suppression.as_deref().map(Suppression::code_of),
                "max_grade": grade_name(capabilities.max_grade),
                "can_authenticate_person": capabilities.can_authenticate_person,
            }),
            now,
        )
        .await?;
        tx.commit().await.map_err(store_error)?;
        Ok(moved.to)
    }

    /// Drop a lease without recording an attempt, so the next worker can take it.
    async fn release_delivery(&self, tenant: &str, id: DeliveryId, now: Timestamp) -> Result<()> {
        sqlx::query(
            "update handoff_deliveries set lease_until = null, next_attempt_at = $3 \
             where tenant_ref = $1 and id = $2",
        )
        .bind(tenant)
        .bind(id.to_string())
        .bind(to_chrono(
            now.saturating_add(IsoDuration::from_secs(LEASE_SECONDS)),
        ))
        .execute(self.pool())
        .await
        .map_err(store_error)?;
        Ok(())
    }

    /// Pin a delivery to the person it resolved to.
    async fn bind_delivery(
        &self,
        tenant: &str,
        id: DeliveryId,
        recipient: &Recipient,
        now: Timestamp,
    ) -> Result<()> {
        sqlx::query(
            "update handoff_deliveries set principal_ref = $3, updated_at = $4 \
             where tenant_ref = $1 and id = $2",
        )
        .bind(tenant)
        .bind(id.to_string())
        .bind(recipient.principal_id.as_ref().map(ToString::to_string))
        .bind(to_chrono(now))
        .execute(self.pool())
        .await
        .map_err(store_error)?;
        Ok(())
    }

    /// Mint one more delivery, for one more person the same rung named.
    async fn mint_sibling(&self, sibling: Sibling<'_>, now: Timestamp) -> Result<()> {
        let Sibling {
            tenant,
            request_id,
            sibling_of,
            channel,
            target,
            recipient,
        } = sibling;
        let capabilities = self.channels.capabilities(channel);
        let id = ids::mint::<handoff_protocol::id::Delivery>(now.to_millis() as u64)?;
        let rung: i32 = sqlx::query_scalar(
            "select rung from handoff_deliveries where tenant_ref = $1 and id = $2",
        )
        .bind(tenant)
        .bind(sibling_of.to_string())
        .fetch_one(self.pool())
        .await
        .map_err(store_error)?;

        sqlx::query(
            "insert into handoff_deliveries \
             (id, tenant_ref, request_id, channel, target_kind, target_value, rung, state, \
              grade_reached, max_grade, can_authenticate_person, created_at, updated_at, \
              next_attempt_at, principal_ref) \
             values ($1,$2,$3,$4,$5,$6,$7,'queued',null,$8,$9,$10,$10,$10,$11)",
        )
        .bind(id.to_string())
        .bind(tenant)
        .bind(request_id.to_string())
        .bind(channel)
        .bind(crate::store::target_kind_name(target.kind))
        .bind(&target.value)
        .bind(rung)
        .bind(grade_name(capabilities.max_grade))
        .bind(capabilities.can_authenticate_person)
        .bind(to_chrono(now))
        .bind(recipient.principal_id.as_ref().map(ToString::to_string))
        .execute(self.pool())
        .await
        .map_err(store_error)?;
        Ok(())
    }

    /// One person, as the directory holds them.
    async fn recipient_for(&self, tenant: &str, principal_ref: &str) -> Result<Recipient> {
        let row = sqlx::query(
            "select id, display from handoff_principals where tenant_ref = $1 and id = $2",
        )
        .bind(tenant)
        .bind(principal_ref)
        .fetch_optional(self.pool())
        .await
        .map_err(store_error)?;
        Ok(Recipient {
            principal_id: row
                .as_ref()
                .and_then(|row| handoff_protocol::id::PrincipalId::parse(row.get("id")).ok()),
            display: row.as_ref().and_then(|row| row.get("display")),
            timezone: None,
            contacts: Vec::new(),
            quiet_hours: None,
        })
    }

    /// Re-queue a delivery so the next sweep attempts it again (`signing.md` §1.5).
    ///
    /// Only a delivery that has not reached a terminal state can be re-queued: `acted` is a person
    /// having answered, and re-delivering that would ask them again for a decision already on a
    /// receipt. Returns `false` when there is nothing to do.
    pub async fn redeliver(&self, tenant: &str, id: DeliveryId, now: Timestamp) -> Result<bool> {
        let mut tx = self.tenant_tx(tenant).await?;
        let updated = sqlx::query(
            "update handoff_deliveries \
             set next_attempt_at = $3, lease_until = null, \
                 state = case when state = 'retrying' then 'retrying' else state end \
             where tenant_ref = $1 and id = $2 \
               and state in ('queued','retrying','sending','dispatched','delivered','seen')",
        )
        .bind(tenant)
        .bind(id.to_string())
        .bind(to_chrono(now))
        .execute(&mut *tx)
        .await
        .map_err(store_error)?;
        tx.commit().await.map_err(store_error)?;
        Ok(updated.rows_affected() > 0)
    }

    /// One delivery, within the caller's tenant and nowhere else.
    ///
    /// Both statements run in one transaction that has named the tenant, so the view is read at a
    /// single point in time and row-level security is holding underneath the predicates rather
    /// than only beside them.
    pub async fn delivery(
        &self,
        tenant: &str,
        id: DeliveryId,
    ) -> Result<Option<handoff_core::model::DeliveryView>> {
        let mut tx = self.tenant_tx(tenant).await?;
        let row = sqlx::query("select * from handoff_deliveries where tenant_ref = $1 and id = $2")
            .bind(tenant)
            .bind(id.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(store_error)?;
        let Some(row) = row else {
            tx.commit().await.map_err(store_error)?;
            return Ok(None);
        };
        let mut view = crate::store::row_to_delivery(&row)?;
        view.attempts = Self::attempts_in(&mut tx, tenant, id).await?;
        tx.commit().await.map_err(store_error)?;
        Ok(Some(view))
    }

    /// Every transport-level attempt on one delivery, oldest first (§7.3).
    pub async fn delivery_attempts(
        &self,
        tenant: &str,
        id: DeliveryId,
    ) -> Result<Vec<handoff_core::model::DeliveryAttemptView>> {
        let mut tx = self.tenant_tx(tenant).await?;
        let attempts = Self::attempts_in(&mut tx, tenant, id).await?;
        tx.commit().await.map_err(store_error)?;
        Ok(attempts)
    }

    /// The attempt query itself, so [`delivery`](Self::delivery) can read the delivery and its
    /// attempts in the transaction it has already opened rather than opening a second one.
    async fn attempts_in(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        tenant: &str,
        id: DeliveryId,
    ) -> Result<Vec<handoff_core::model::DeliveryAttemptView>> {
        let rows = sqlx::query(
            "select n, started_at, ended_at, outcome, transport_status, error \
             from handoff_delivery_attempts where tenant_ref = $1 and delivery_id = $2 order by n",
        )
        .bind(tenant)
        .bind(id.to_string())
        .fetch_all(&mut **tx)
        .await
        .map_err(store_error)?;
        Ok(rows
            .iter()
            .map(|row| handoff_core::model::DeliveryAttemptView {
                n: row.get::<i32, _>("n") as u32,
                started_at: from_chrono(row.get("started_at")),
                ended_at: row
                    .get::<Option<chrono::DateTime<chrono::Utc>>, _>("ended_at")
                    .map(from_chrono),
                outcome: row.get("outcome"),
                transport_status: row.get("transport_status"),
                error: row.get("error"),
            })
            .collect())
    }
}

/// One more delivery, for one more person the same rung named.
struct Sibling<'a> {
    tenant: &'a str,
    request_id: RequestId,
    /// The delivery whose rung and channel this one copies.
    sibling_of: DeliveryId,
    channel: &'a str,
    target: &'a Target,
    recipient: &'a Recipient,
}

/// One transport-level attempt, as this module records it.
struct Attempt {
    n: u32,
    outcome: DeliveryState,
    transport_status: Option<String>,
    retryable: bool,
}

/// The directory this deployment ships with: its own principals table.
///
/// **The one place a target kind is interpreted** (§7.5). Every other file in this crate carries a
/// target around without ever asking what kind it is, which is the property that rule exists to
/// protect.
///
/// `group` and `rotation` resolve to nobody here, and that is the honest answer rather than a gap:
/// this deployment has no group membership and no on-call schedule to read. A rung naming one ends
/// as a `suppressed` delivery saying `no_recipient`, which is visible, countable, and true — where
/// quietly resolving a rotation to "everybody" would page the whole company at 3 a.m.
impl RecipientDirectory for PgStore {
    fn resolve(&self, tenant: String, target: Target) -> BoxFuture<'_, Result<Vec<Recipient>>> {
        Box::pin(async move {
            let rows = match target.kind {
                TargetKind::Principal => sqlx::query(
                    "select id, display from handoff_principals \
                         where tenant_ref = $1 and id = $2 and kind = 'human'",
                )
                .bind(&tenant)
                .bind(&target.value),
                TargetKind::Role => sqlx::query(
                    "select id, display from handoff_principals \
                     where tenant_ref = $1 and role = $2 and kind = 'human' order by id",
                )
                .bind(&tenant)
                .bind(&target.value),
                TargetKind::Anyone => sqlx::query(
                    "select id, display from handoff_principals \
                     where tenant_ref = $1 and kind = 'human' order by id",
                )
                .bind(&tenant),
                // No group table, no on-call schedule. Saying so beats inventing an audience.
                TargetKind::Group | TargetKind::Rotation => {
                    return Ok(Vec::new());
                }
            };

            let rows = rows.fetch_all(self.pool()).await.map_err(store_error)?;
            Ok(rows
                .iter()
                .map(|row| Recipient {
                    principal_id: handoff_protocol::id::PrincipalId::parse(row.get("id")).ok(),
                    display: row.get("display"),
                    timezone: None,
                    // This deployment stores no addresses. A channel needing one therefore
                    // suppresses with `no_address` rather than sending somewhere invented.
                    contacts: Vec::<ContactPoint>::new(),
                    quiet_hours: None,
                })
                .collect())
        })
    }
}

fn store_error(e: sqlx::Error) -> ProtocolError {
    tracing::error!(error = %e, "delivery store failure");
    ProtocolError::new(
        ErrorCode::InvalidRequest,
        "the store rejected this operation",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_attempt_budget_is_bounded_and_spans_minutes_not_days() {
        // §7.3 requires a bounded count; the number is this implementation's choice, and the
        // reason it is small is that the ladder's next rung should escalate before a retry loop
        // has finished being patient.
        const { assert!(MAX_DELIVERY_ATTEMPTS >= 3, "one blip must not be fatal") };
        let total: i64 = (1..=MAX_DELIVERY_ATTEMPTS).map(backoff_seconds).sum();
        assert!(
            (60..900).contains(&total),
            "the whole budget spans {total}s, which is not the few minutes intended"
        );
    }
}

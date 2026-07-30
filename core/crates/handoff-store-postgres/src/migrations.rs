//! The migration set, embedded and applied in order at startup.
//!
//! Three structural commitments run through all nine, and each of them is a requirement rather than
//! a preference:
//!
//! 1. **Handoff owns its own database.** Every table is a plain `CREATE TABLE` with **no foreign
//!    key into any other system's schema**. A deployment that also runs other software joins to it
//!    by an opaque tenant key and nothing else. A foreign key to someone else's `organizations`
//!    table would make self-hosting impossible, which is why `tenant_ref` is `text` and the store
//!    never parses it.
//! 2. **`tenant_ref` is on every table and in the `WHERE` clause of every query** (I17). Row-level
//!    security is enabled on top of that as a second line of defence, not as a replacement for it.
//! 3. **Receipts are immutable at the storage layer** (§9.4). Statement-level triggers refuse
//!    `UPDATE`, `DELETE`, and `TRUNCATE` as the application's own database role, because §9.4 is
//!    explicit that application-level immutability is insufficient — the threat includes the
//!    application.

/// One migration: a stable number, a name for the log, and the SQL.
pub struct Migration {
    /// Ordinal. Applied in ascending order, exactly once.
    pub number: i32,
    /// What it does, for the migration log.
    pub name: &'static str,
    /// The statements.
    pub sql: &'static str,
}

/// Row-level security, applied to every tenant-scoped table.
///
/// The policy passes when the connection has not named a tenant, and restricts to that tenant when
/// it has. Every request-scoped transaction names one before it reads anything, so an API query
/// that lost its `WHERE tenant_ref = …` still cannot see another tenant's rows. The background
/// sweep names none, because settling expired requests is by definition a cross-tenant maintenance
/// job — and that asymmetry is stated here rather than left for a reader to infer.
const RLS: &str = r#"
create or replace function handoff_enable_rls(target regclass) returns void language plpgsql as $$
begin
  execute format('alter table %s enable row level security', target);
  execute format('alter table %s force row level security', target);
  execute format(
    'create policy handoff_tenant_isolation on %s using ('
    || 'coalesce(current_setting(''handoff.tenant_ref'', true), '''') = '''' '
    || 'or tenant_ref = current_setting(''handoff.tenant_ref'', true))', target);
end;
$$;
"#;

/// Every migration, in order.
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        number: 1,
        name: "principals and requests",
        sql: r#"
create table if not exists handoff_migrations (
  number      integer primary key,
  name        text        not null,
  applied_at  timestamptz not null default now()
);

-- Credentials, as verification material only. §4.1: key verification MUST be constant-time with
-- respect to the secret, and secrets MUST NOT be stored recoverably — so this holds a SHA-256 of
-- the presented token and never the token.
create table if not exists handoff_principals (
  id            text primary key,
  tenant_ref    text not null,
  kind          text not null check (kind in ('machine', 'human', 'anonymous_link')),
  secret_sha256 text not null unique,
  role          text not null check (role in ('viewer', 'editor', 'admin')),
  auth_strength text not null check (auth_strength in ('link_only', 'session', 'reauth', 'mfa')),
  display       text,
  scopes        text[] not null default array['*'],
  created_at    timestamptz not null default now()
);

create table if not exists handoff_requests (
  id                     text primary key,
  tenant_ref             text        not null,
  waiter_ref             text        not null,
  state                  text        not null check (state in ('pending','answered','expired','cancelled','superseded')),
  version                bigint      not null default 1,
  urgency                text        not null,
  urgency_state          text        not null check (urgency_state in ('attention','waiting')),
  liveness               text        not null,
  on_waiter_terminal     text        not null,
  mode                   text        not null check (mode in ('advisory','gated')),
  presentation_binding   text        not null check (presentation_binding in ('advisory','strict')),
  dedupe_key             text        not null,
  prompt                 jsonb       not null,
  requires               jsonb       not null,
  ttl_policy             jsonb,
  routing                jsonb       not null,
  attempt_ttl_secs       bigint      not null,
  metadata               jsonb       not null default '{}'::jsonb,
  callback_url           text,
  resume_ref             text,
  resume_payload         text,
  test_mode              boolean     not null default false,
  requester_principal    text        not null,
  request_digest         text        not null,
  rendered_digest        text        not null,
  rendered_ref           text        not null,
  created_at             timestamptz not null,
  expires_at             timestamptz,
  attempt_expires_at     timestamptz,
  answered_at            timestamptz,
  superseded_by          text,
  cancel_reason          text,
  attempt_lapse_notified boolean     not null default false,
  rung                   integer     not null default 0
);

-- Ask-once, enforced by the database rather than by a read-then-write (§3.1, §3.3 rule 3). Scoped
-- to the tenant, because §3.2 rule 2 makes an unscoped uniqueness constraint a correctness bug: it
-- does not merely risk a collision, it lets one tenant's key absorb another tenant's write.
create unique index if not exists handoff_requests_dedupe_pending
  on handoff_requests (tenant_ref, dedupe_key) where state = 'pending';
create index if not exists handoff_requests_by_waiter on handoff_requests (tenant_ref, waiter_ref);
create index if not exists handoff_requests_pending_deadlines
  on handoff_requests (state, expires_at, attempt_expires_at);
"#,
    },
    Migration {
        number: 2,
        name: "request steps, append-only",
        sql: r#"
-- One row per version the person could have been shown. §9.2 forbids re-deriving what they saw
-- from the request's *current* content, so each step keeps its own `requires` snapshot and its own
-- rendered digest, and amendment appends rather than overwrites.
create table if not exists handoff_request_steps (
  id              bigserial primary key,
  tenant_ref      text        not null,
  request_id      text        not null,
  n               bigint      not null,
  requires_snapshot jsonb     not null,
  prompt_snapshot   jsonb     not null,
  rendered_digest text        not null,
  rendered_ref    text        not null,
  created_at      timestamptz not null,
  unique (tenant_ref, request_id, n)
);
create index if not exists handoff_request_steps_by_request
  on handoff_request_steps (tenant_ref, request_id, n desc);
"#,
    },
    Migration {
        number: 3,
        name: "deliveries and their attempts",
        sql: r#"
create table if not exists handoff_deliveries (
  id                      text primary key,
  tenant_ref              text        not null,
  request_id              text        not null,
  channel                 text        not null,
  target_kind             text        not null,
  target_value            text        not null,
  rung                    integer     not null,
  state                   text        not null,
  grade_reached           text,
  max_grade               text        not null,
  can_authenticate_person boolean     not null,
  created_at              timestamptz not null,
  updated_at              timestamptz not null
);
create index if not exists handoff_deliveries_by_request
  on handoff_deliveries (tenant_ref, request_id, created_at);

create table if not exists handoff_delivery_attempts (
  id               bigserial primary key,
  tenant_ref       text        not null,
  delivery_id      text        not null,
  n                integer     not null,
  started_at       timestamptz not null,
  ended_at         timestamptz,
  outcome          text        not null,
  transport_status text,
  error            text,
  unique (tenant_ref, delivery_id, n)
);
"#,
    },
    Migration {
        number: 4,
        name: "waiters, signals, and callback attempts",
        sql: r#"
create table if not exists handoff_waiters (
  tenant_ref         text        not null,
  waiter_ref         text        not null,
  state              text        not null,
  liveness           text        not null,
  lease_expires_at   timestamptz,
  highest_sequence   bigint      not null default 0,
  created_at         timestamptz not null,
  updated_at         timestamptz not null,
  primary key (tenant_ref, waiter_ref)
);

-- Signals are a **queue, not a flag** (§8.2 W2). A non-terminal `attempt_lapsed` nudge must not be
-- able to overwrite, replace, or mask a subsequent terminal signal, so each one is its own row and
-- nothing updates `type` in place.
create table if not exists handoff_signals (
  id             text primary key,
  tenant_ref     text        not null,
  waiter_ref     text        not null,
  request_id     text        not null,
  type           text        not null,
  sequence       bigint      not null,
  resume_token   text        not null,
  decision       jsonb,
  resume_ref     text,
  resume_payload text,
  attempts       integer     not null default 0,
  created_at     timestamptz not null,
  acked_at       timestamptz,
  applied        boolean,
  ack_reason     text,
  next_callback_at timestamptz,
  callback_lease_until timestamptz,
  callback_url   text
);
create index if not exists handoff_signals_unacked
  on handoff_signals (tenant_ref, waiter_ref, sequence) where acked_at is null;
create index if not exists handoff_signals_callback_due
  on handoff_signals (next_callback_at) where acked_at is null and callback_url is not null;

create table if not exists handoff_callback_attempts (
  id          bigserial primary key,
  tenant_ref  text        not null,
  signal_id   text        not null,
  n           integer     not null,
  delivery_id text        not null,
  started_at  timestamptz not null,
  ended_at    timestamptz,
  status_code integer,
  duration_ms bigint,
  outcome     text        not null,
  error       text,
  unique (tenant_ref, signal_id, n)
);
"#,
    },
    Migration {
        number: 5,
        name: "receipts, immutable at the storage layer",
        sql: r#"
create table if not exists handoff_receipts (
  id           text primary key,
  tenant_ref   text        not null,
  request_id   text        not null,
  kind         text        not null check (kind in ('decision','policy','correction')),
  height       bigint      not null,
  prev_digest  text,
  digest       text        not null,
  decided_at   timestamptz not null,
  -- The decided values, as their own column. Held separately from `body` so that the storage-level
  -- immutability probe of C-15 has a real column to aim an `UPDATE` at: a probe that fails because
  -- the column does not exist proves nothing about the trigger.
  decision     jsonb       not null default '{}'::jsonb,
  body         jsonb       not null,
  created_at   timestamptz not null default now(),
  unique (tenant_ref, height)
);
create index if not exists handoff_receipts_by_request on handoff_receipts (tenant_ref, request_id);

-- §9.4 layer 2. "This MUST be asserted from the storage layer directly, not from the application —
-- application-level immutability is insufficient, because the threat includes the application."
--
-- Statement-level and BEFORE, so a zero-row `UPDATE ... WHERE true` is refused too: a rule that
-- only fires when it happens to match a row is not a rule, it is a coincidence.
create or replace function handoff_receipts_are_append_only() returns trigger language plpgsql as $$
begin
  raise exception
    'handoff_receipts is append-only: a receipt is immutable at the storage layer (Handoff v0.1 '
    '§9.4). A correction is a new receipt with kind = ''correction''.'
    using errcode = 'restrict_violation';
end;
$$;

drop trigger if exists handoff_receipts_no_update on handoff_receipts;
create trigger handoff_receipts_no_update before update on handoff_receipts
  for each statement execute function handoff_receipts_are_append_only();

drop trigger if exists handoff_receipts_no_delete on handoff_receipts;
create trigger handoff_receipts_no_delete before delete on handoff_receipts
  for each statement execute function handoff_receipts_are_append_only();

drop trigger if exists handoff_receipts_no_truncate on handoff_receipts;
create trigger handoff_receipts_no_truncate before truncate on handoff_receipts
  for each statement execute function handoff_receipts_are_append_only();
"#,
    },
    Migration {
        number: 6,
        name: "authorizations and redemptions",
        sql: r#"
create table if not exists handoff_authorizations (
  id            text primary key,
  tenant_ref    text        not null,
  receipt_id    text        not null,
  request_id    text        not null,
  grants        jsonb       not null default '{}'::jsonb,
  single_use    boolean     not null default true,
  expires_at    timestamptz,
  waiter_ref    text,
  effect_digest text,
  created_at    timestamptz not null
);
create index if not exists handoff_authorizations_by_request
  on handoff_authorizations (tenant_ref, request_id);

-- One row per effect, so redemption is idempotent per `effect_key` by construction (§10.2). A
-- retried agent turn re-inserts nothing and the customer is not refunded twice.
create table if not exists handoff_redemptions (
  id               bigserial primary key,
  tenant_ref       text        not null,
  authorization_id text        not null,
  effect_key       text        not null,
  redeemed_at      timestamptz not null,
  unique (tenant_ref, authorization_id, effect_key)
);
"#,
    },
    Migration {
        number: 7,
        name: "capability grants and sessions",
        sql: r#"
-- No resolvable address has a column here, and that is the point (§11.1, I8). A grant is a handle,
-- a description, and a lifecycle. The one address in a conforming system is minted at resolve time
-- and returned in that response only.
create table if not exists handoff_grants (
  handle              text primary key,
  tenant_ref          text        not null,
  request_id          text        not null,
  capability_type     text        not null,
  scope               text        not null check (scope in ('view','drive')),
  provider            text,
  resource_ref        text,
  label               text,
  purpose             text,
  optional            boolean     not null default false,
  blast_radius        jsonb       not null,
  blast_radius_digest text        not null,
  expires_at          timestamptz not null,
  revoked_at          timestamptz,
  revoke_reason       text,
  max_holders         integer     not null default 1,
  bound_principal     text,
  created_at          timestamptz not null
);
create index if not exists handoff_grants_by_request on handoff_grants (tenant_ref, request_id);

create table if not exists handoff_grant_sessions (
  session_ref  text primary key,
  tenant_ref   text        not null,
  handle       text        not null,
  principal    text        not null,
  scopes       text[]      not null,
  lease_until  timestamptz not null,
  released_at  timestamptz,
  created_at   timestamptz not null
);
create index if not exists handoff_grant_sessions_by_handle
  on handoff_grant_sessions (tenant_ref, handle);
"#,
    },
    Migration {
        number: 8,
        name: "idempotency and secret sinks",
        sql: r#"
-- Scoped `(tenant_ref, principal_ref, operation, key)`. §3.1 scopes the key to
-- `(org_id, principal_id)`; the operation is added so one key presented to two different endpoints
-- cannot replay one endpoint's response at the other.
create table if not exists handoff_idempotency (
  tenant_ref      text        not null,
  principal_ref   text        not null,
  operation       text        not null,
  key             text        not null,
  body_digest     text        not null,
  response_status integer     not null,
  response_body   text        not null,
  request_id      text,
  created_at      timestamptz not null,
  primary key (tenant_ref, principal_ref, operation, key)
);

-- The declaration only. §12 rule 5: this specification defines no default sink implementation, and
-- a conforming open implementation SHOULD NOT ship one — so no value column exists here, and none
-- should ever be added.
create table if not exists handoff_sinks (
  tenant_ref text        not null,
  sink_ref   text        not null,
  request_id text        not null,
  created_at timestamptz not null,
  primary key (tenant_ref, sink_ref)
);
"#,
    },
    Migration {
        number: 9,
        name: "local event and usage sinks, inbound channel records, and row-level security",
        sql: r#"
-- The local event sink. Written in the same transaction as the state change it describes (I12).
-- A deployment that mirrors events outward does so from this table, asynchronously, and the mirror
-- may lag; it may not be the primary write.
create table if not exists handoff_events (
  id         bigserial primary key,
  tenant_ref text        not null,
  request_id text,
  type       text        not null,
  payload    jsonb       not null default '{}'::jsonb,
  created_at timestamptz not null
);
create index if not exists handoff_events_by_request on handoff_events (tenant_ref, request_id, id);

create table if not exists handoff_usage (
  id         bigserial primary key,
  tenant_ref text        not null,
  metric     text        not null,
  quantity   bigint      not null default 1,
  request_id text,
  created_at timestamptz not null
);

-- Inbound channel messages, recorded and never allowed to decide anything (§4.7, C-21). A message
-- is a **provisional** answer at most: it identifies the request and pre-fills the surface. There
-- is deliberately no path from this table to a receipt.
create table if not exists handoff_channel_messages (
  id           bigserial primary key,
  tenant_ref   text        not null,
  request_id   text        not null,
  channel      text        not null,
  external_ref text,
  body         text        not null,
  provisional  boolean     not null default true,
  created_at   timestamptz not null
);

-- Runtime observations. §9.7: clearance MUST be asserted, never inferred. An observation is
-- recorded as an observation, and there is no path from here to a person.
create table if not exists handoff_observations (
  id         bigserial primary key,
  tenant_ref text        not null,
  request_id text        not null,
  note       text        not null,
  created_at timestamptz not null
);

do $rls$
declare t text;
begin
  foreach t in array array[
    'handoff_principals','handoff_requests','handoff_request_steps','handoff_deliveries',
    'handoff_delivery_attempts','handoff_waiters','handoff_signals','handoff_callback_attempts',
    'handoff_receipts','handoff_authorizations','handoff_redemptions','handoff_grants',
    'handoff_grant_sessions','handoff_idempotency','handoff_sinks','handoff_events',
    'handoff_usage','handoff_channel_messages','handoff_observations'
  ] loop
    if not exists (select 1 from pg_policies where tablename = t and policyname = 'handoff_tenant_isolation') then
      perform handoff_enable_rls(t::regclass);
    end if;
  end loop;
end
$rls$;
"#,
    },
    Migration {
        number: 10,
        name: "recorded dispositions and endorsements",
        sql: r#"
-- §6.2 R13 and R14: what a person did to a request that left it `pending`.
--
-- Append-only, because each row is a thing a person did and none of them may be rewritten by the
-- next one. §6.6 requires a delegation to be recorded and NOT treated as a decision, so there is
-- deliberately no path from this table to a receipt or to a signal — the eventual receipt reads it
-- as history, and the waiter never learns any of it happened.
create table if not exists handoff_request_dispositions (
  id            bigserial primary key,
  tenant_ref    text        not null,
  request_id    text        not null,
  disposition   text        not null check (disposition in ('delegate','unable','endorse')),
  principal_ref text        not null,
  delegate_kind text,
  delegate_value text,
  note          text,
  created_at    timestamptz not null
);
create index if not exists handoff_request_dispositions_by_request
  on handoff_request_dispositions (tenant_ref, request_id, id);

do $rls10$
begin
  if not exists (select 1 from pg_policies
                 where tablename = 'handoff_request_dispositions'
                   and policyname = 'handoff_tenant_isolation') then
    perform handoff_enable_rls('handoff_request_dispositions'::regclass);
  end if;
end
$rls10$;
"#,
    },
    Migration {
        number: 11,
        name: "delivery scheduling, so a queued delivery is actually attempted",
        sql: r#"
-- §7 makes delivery a first-class tracked entity rather than a side effect of a notification
-- sweep. Minting a row in `queued` and never touching it again satisfies the shape of that and
-- none of the substance, so these are the columns that let a worker pick one up, attempt it,
-- back off, and give up — each of them a fact §7.3 requires a delivery to own.
alter table handoff_deliveries add column if not exists next_attempt_at    timestamptz;
alter table handoff_deliveries add column if not exists attempt_count      integer     not null default 0;
alter table handoff_deliveries add column if not exists lease_until        timestamptz;
alter table handoff_deliveries add column if not exists suppression_reason text;

-- Who the target actually resolved to. §7.5: a Server SHOULD address deliveries to individual
-- people rather than to a place, because read state shared across a workspace means one person
-- clearing a notification clears everyone's. Null means the rung named a place and nobody was
-- resolved from it — which is itself the honest reason a delivery was suppressed.
alter table handoff_deliveries add column if not exists principal_ref      text;

-- Deliveries already minted by an earlier build are due now rather than never.
update handoff_deliveries set next_attempt_at = created_at
  where next_attempt_at is null and state = 'queued';

create index if not exists handoff_deliveries_due
  on handoff_deliveries (next_attempt_at)
  where next_attempt_at is not null and state in ('queued','retrying');
"#,
    },
    Migration {
        number: 12,
        name: "a callback delivery identity that survives its own retries",
        sql: r#"
-- `signing.md` §1.1 makes `Handoff-Idempotency-Key` the delivery identifier "so a receiver can
-- dedupe without parsing the body", and §1.3 rule 7 requires that dedupe to stop a repeated
-- delivery applying a decision twice. A delivery id minted per *attempt* makes both impossible:
-- every retry looks like a new delivery, so a receiver that dedupes exactly as specified still
-- applies the same decision on every redelivery. The identity therefore belongs to the push, not
-- to the attempt, and it is minted once and stored here.
alter table handoff_signals add column if not exists callback_delivery_id text;

-- §15.5: an endpoint that fails every attempt MUST eventually be disabled and the tenant
-- notified. Silent permanent retry is how queues die.
alter table handoff_signals add column if not exists callback_disabled_at timestamptz;
"#,
    },
    Migration {
        number: 13,
        name: "idempotency keys are scoped to the object they act on",
        sql: r#"
-- An `Idempotency-Key` is retry safety for **one call against one object**, and the object was
-- missing from the key's scope. §3.1 scopes the key to `(org_id, principal_id)`, which is right for
-- a raise — it creates the object — but for every per-object mutation the object has to be in the
-- scope too. Without it, answering request B with the key already used on request A replays A's
-- receipt, and B is never answered at all: the caller is handed a decision about a different thing
-- and told it succeeded.
alter table handoff_idempotency add column if not exists object text not null default '';

do $idem$
begin
  if exists (
    select 1 from pg_constraint
    where conname = 'handoff_idempotency_pkey'
      and conrelid = 'handoff_idempotency'::regclass
      and array_length(conkey, 1) = 4
  ) then
    alter table handoff_idempotency drop constraint handoff_idempotency_pkey;
    alter table handoff_idempotency
      add constraint handoff_idempotency_pkey
      primary key (tenant_ref, principal_ref, operation, object, key);
  end if;
end
$idem$;
"#,
    },
];

/// The row-level-security helper, applied before the migrations that use it.
pub const RLS_HELPER: &str = RLS;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_numbered_from_one_without_gaps() {
        for (i, m) in MIGRATIONS.iter().enumerate() {
            assert_eq!(m.number, i as i32 + 1, "{} is out of order", m.name);
        }
        // Bump this deliberately when a migration lands. The count is here so that two changes
        // adding a migration in parallel collide in this assertion rather than silently agreeing
        // on a number — which is exactly what it caught.
        assert_eq!(MIGRATIONS.len(), 13);
    }

    #[test]
    fn every_tenant_scoped_table_carries_tenant_ref() {
        // I17 is a schema property before it is a query property: a table without the column
        // cannot have the predicate.
        let sql: String = MIGRATIONS.iter().map(|m| m.sql).collect();
        for table in [
            "handoff_requests",
            "handoff_request_steps",
            "handoff_deliveries",
            "handoff_signals",
            "handoff_receipts",
            "handoff_authorizations",
            "handoff_grants",
            "handoff_idempotency",
            "handoff_events",
        ] {
            let start = sql
                .find(&format!("create table if not exists {table} ("))
                .unwrap_or_else(|| panic!("{table} is not created by any migration"));
            let body = &sql[start..start + 900.min(sql.len() - start)];
            assert!(
                body.contains("tenant_ref"),
                "{table} has no tenant_ref column"
            );
        }
    }

    #[test]
    fn no_migration_declares_a_foreign_key_into_another_systems_schema() {
        // Handoff owns its own database. A reference to an `organizations`, `users`, or `spaces`
        // table would make self-hosting impossible.
        let sql: String = MIGRATIONS.iter().map(|m| m.sql).collect::<String>();
        assert!(!sql.contains("references organizations"));
        assert!(!sql.contains("references users"));
        assert!(!sql.contains("references spaces"));
        assert!(!sql.to_lowercase().contains("foreign key"));
    }

    #[test]
    fn receipts_refuse_mutation_at_the_storage_layer() {
        let sql = MIGRATIONS[4].sql;
        assert!(sql.contains("before update on handoff_receipts"));
        assert!(sql.contains("before delete on handoff_receipts"));
        assert!(sql.contains("for each statement"));
        assert!(sql.contains("raise exception"));
    }

    #[test]
    fn the_sink_table_has_nowhere_to_put_a_secret() {
        let sql = MIGRATIONS[7].sql;
        let start = sql
            .find("create table if not exists handoff_sinks")
            .unwrap();
        let body = &sql[start..];
        assert!(
            !body.contains("value"),
            "a sink table with a value column is a credential store"
        );
    }
}

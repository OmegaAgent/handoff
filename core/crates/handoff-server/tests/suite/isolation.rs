//! Tenant isolation, asserted per table and on identity.
//!
//! §18 is unusually specific about how to assert this, and the reason is worth restating: a query
//! missing its tenant predicate returns a **superset**, and a `contains` assertion passes against a
//! superset. Every assertion below therefore compares the **exact set** of rows — length and
//! identity together — which is the only form that fails when a row that should be invisible is
//! present.
//!
//! The second half checks the same property one layer down. Every `handoff_*` table has row-level
//! security, and every request-scoped transaction names its tenant before it reads anything, so a
//! query that lost its `WHERE tenant_ref = …` still cannot see another tenant's rows. That is the
//! second line of defence, and it is only a defence if it is tested per table rather than assumed.

use super::harness::*;
use std::collections::BTreeSet;

/// Every table that carries tenant-scoped rows, with the column that identifies a row.
const TENANT_SCOPED_TABLES: &[(&str, &str)] = &[
    ("handoff_requests", "id"),
    ("handoff_request_steps", "request_id"),
    ("handoff_deliveries", "id"),
    ("handoff_delivery_attempts", "delivery_id"),
    ("handoff_waiters", "waiter_ref"),
    ("handoff_signals", "id"),
    ("handoff_callback_attempts", "signal_id"),
    ("handoff_receipts", "id"),
    ("handoff_authorizations", "id"),
    ("handoff_redemptions", "authorization_id"),
    ("handoff_grants", "handle"),
    ("handoff_grant_sessions", "session_ref"),
    ("handoff_idempotency", "key"),
    ("handoff_sinks", "sink_ref"),
    ("handoff_events", "type"),
    ("handoff_usage", "metric"),
    ("handoff_channel_messages", "channel"),
    ("handoff_observations", "request_id"),
    ("handoff_principals", "id"),
];

#[tokio::test]
async fn each_tenant_reads_exactly_its_own_rows_and_nothing_of_the_others() {
    let deployment = Deployment::start("tenants", 18102).await;

    // Two requests in tenant A, one in tenant B, all under waiter references that would collide if
    // anything here were globally scoped.
    let (status, a1) = post(
        &deployment.base,
        "/requests",
        MACHINE_A,
        "iso-a1",
        raise_body("run:shared-waiter", "Tenant A, ask one"),
    )
    .await;
    assert_eq!(status, 201, "{a1}");
    let (status, a2) = post(
        &deployment.base,
        "/requests",
        MACHINE_A,
        "iso-a2",
        raise_body("run:shared-waiter", "Tenant A, ask two"),
    )
    .await;
    assert_eq!(status, 201, "{a2}");
    let (status, b1) = post(
        &deployment.base,
        "/requests",
        MACHINE_B,
        // The identical idempotency key as tenant A's first raise. §3.2 rule 2: two tenants using
        // one key MUST both succeed, and a 200 here would be one tenant's write absorbing the
        // other's — a dropped human ask that produces no error anywhere.
        "iso-a1",
        raise_body("run:shared-waiter", "Tenant B, ask one"),
    )
    .await;
    assert_eq!(
        status, 201,
        "tenant B's identical key must not collapse onto tenant A's: {b1}"
    );

    let a1 = a1["id"].as_str().unwrap().to_string();
    let a2 = a2["id"].as_str().unwrap().to_string();
    let b1 = b1["id"].as_str().unwrap().to_string();
    assert_ne!(a1, b1);

    // Length and identity, never `contains`.
    let (_, listed) = get(
        &deployment.base,
        "/requests?waiter_ref=run%3Ashared-waiter",
        MACHINE_A,
    )
    .await;
    let seen: BTreeSet<String> = listed["data"]
        .as_array()
        .expect("a page")
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        seen,
        BTreeSet::from([a1.clone(), a2.clone()]),
        "tenant A's page is exactly tenant A's rows"
    );

    let (_, listed_b) = get(
        &deployment.base,
        "/requests?waiter_ref=run%3Ashared-waiter",
        MACHINE_B,
    )
    .await;
    let seen_b: BTreeSet<String> = listed_b["data"]
        .as_array()
        .expect("a page")
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(seen_b, BTreeSet::from([b1.clone()]));

    // Refusing to *show* another tenant's request and refusing to *write* it are separate checks,
    // and only the second is usually tested.
    let (status, _) = get(&deployment.base, &format!("/requests/{b1}"), MACHINE_A).await;
    assert_eq!(status, 404, "existence is not disclosed across tenants");
    let (status, error) = post(
        &deployment.base,
        &format!("/requests/{a1}/cancel"),
        MACHINE_B,
        "iso-cross-cancel",
        serde_json::json!({"reason": "cross-tenant write attempt"}),
    )
    .await;
    assert_eq!(status, 404, "{error}");
    assert_eq!(error["error"]["code"], "request_not_found");

    // Re-read from the owning tenant rather than trusting the 404 the other one saw.
    let (_, untouched) = get(&deployment.base, &format!("/requests/{a1}"), MACHINE_A).await;
    assert_eq!(
        untouched["state"], "pending",
        "the refused cancel left no partial effect"
    );
    assert!(untouched["cancel_reason"].is_null());
}

/// Row-level security only binds a role that cannot bypass it.
///
/// A superuser — and any role with `BYPASSRLS` — ignores every policy in the database, so running
/// `handoffd` as one leaves this whole defence inert while every test still passes. This test
/// therefore creates a least-privilege role and asserts the property as **that** role, which is
/// also the way a deployment should run: the tenant predicate in every query is the primary
/// defence, and this is the one that catches the day somebody forgets it.
#[tokio::test]
async fn row_level_security_holds_on_every_tenant_scoped_table() {
    let deployment = Deployment::start("rls", 18103).await;

    // Produce rows in as many tables as the API can reach, for both tenants.
    for (machine, human, label) in [(MACHINE_A, EDITOR_A, "A"), (MACHINE_B, EDITOR_B, "B")] {
        let (status, raised) = post(
            &deployment.base,
            "/requests",
            machine,
            &format!("rls-raise-{label}"),
            raise_body(&format!("run:rls-{label}"), &format!("Tenant {label} asks")),
        )
        .await;
        assert_eq!(status, 201, "{raised}");
        let id = raised["id"].as_str().unwrap();
        let (status, answered) = post(
            &deployment.base,
            &format!("/requests/{id}/answer"),
            human,
            &format!("rls-answer-{label}"),
            serde_json::json!({"values": {"decision": "approve"}}),
        )
        .await;
        assert_eq!(status, 200, "{answered}");
    }

    let pool = deployment.pool().await;
    let least_privilege = LeastPrivilegeRole::create(&deployment).await;
    let restricted = least_privilege.pool().await;

    for (table, id_column) in TENANT_SCOPED_TABLES {
        // The predicate a correct query would carry.
        let with_predicate: Vec<String> = sqlx::query_scalar(&format!(
            "select {id_column}::text from {table} where tenant_ref = $1 order by 1"
        ))
        .bind(ORG_A)
        .fetch_all(&pool)
        .await
        .unwrap_or_else(|e| panic!("{table}: {e}"));

        // The same query with the predicate **removed**, inside a transaction that has named the
        // tenant. Without row-level security this returns a superset, which is exactly the shape a
        // `contains` assertion cannot catch.
        let mut tx = restricted.begin().await.expect("begin");
        sqlx::query("select set_config('handoff.tenant_ref', $1, true)")
            .bind(ORG_A)
            .execute(&mut *tx)
            .await
            .expect("name the tenant");
        let without_predicate: Vec<String> =
            sqlx::query_scalar(&format!("select {id_column}::text from {table} order by 1"))
                .fetch_all(&mut *tx)
                .await
                .unwrap_or_else(|e| panic!("{table}: {e}"));
        tx.commit().await.expect("commit");

        assert_eq!(
            without_predicate,
            with_predicate,
            "{table}: a query with no tenant predicate returned {} row(s) where the tenant owns \
             {}. Row-level security is not holding on this table, so a single forgotten WHERE \
             clause leaks another tenant's rows.",
            without_predicate.len(),
            with_predicate.len()
        );
    }

    // And the guard against the test passing vacuously: tenant B really does have rows that tenant
    // A must not have seen above.
    let b_requests: i64 =
        sqlx::query_scalar("select count(*) from handoff_requests where tenant_ref = $1")
            .bind(ORG_B)
            .fetch_one(&pool)
            .await
            .expect("count tenant B's requests");
    assert!(
        b_requests > 0,
        "tenant B has no rows, so the isolation assertions above proved nothing"
    );

    // And the guard against the *role* being the wrong one: a superuser ignores every policy, so a
    // run under one would have passed the loop above without proving anything at all.
    let bypasses: bool = sqlx::query_scalar(
        "select bool_or(rolsuper or rolbypassrls) from pg_roles where rolname = current_user",
    )
    .fetch_one(&restricted)
    .await
    .expect("read the role");
    assert!(
        !bypasses,
        "the assertions above ran as a role that bypasses row-level security, so they proved nothing"
    );

    drop(restricted);
    least_privilege.drop_role().await;
}

/// A database role with no more than `handoffd` needs, created for the life of one test.
struct LeastPrivilegeRole {
    name: String,
    url: String,
    admin: String,
}

impl LeastPrivilegeRole {
    async fn create(deployment: &Deployment) -> Self {
        let name = format!("handoff_app_{}", std::process::id());
        let admin = admin_url();
        let owner = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&deployment.url)
            .await
            .expect("connect as the owner");

        for statement in [
            format!("drop role if exists \"{name}\""),
            format!("create role \"{name}\" login password 'least-privilege'"),
            format!("grant usage on schema public to \"{name}\""),
            format!(
                "grant select, insert, update, delete on all tables in schema public to \"{name}\""
            ),
            format!("grant usage, select on all sequences in schema public to \"{name}\""),
        ] {
            sqlx::query(&statement)
                .execute(&owner)
                .await
                .unwrap_or_else(|e| panic!("{statement}: {e}"));
        }

        let (base, _) = deployment.url.rsplit_once('/').expect("a database URL");
        let (scheme, _) = base.split_once("://").expect("a scheme");
        let host = base.rsplit_once('@').map(|(_, h)| h).unwrap_or(base);
        let database = deployment.url.rsplit('/').next().expect("a database name");
        Self {
            url: format!("{scheme}://{name}:least-privilege@{host}/{database}"),
            name,
            admin,
        }
    }

    async fn pool(&self) -> sqlx::PgPool {
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&self.url)
            .await
            .expect("connect as the least-privilege role")
    }

    async fn drop_role(self) {
        let admin = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&self.admin)
            .await
            .expect("connect as the administrator");
        let _ = sqlx::query(&format!("drop role if exists \"{}\"", self.name))
            .execute(&admin)
            .await;
    }
}

//! Tenant isolation, asserted per table and on identity.
//!
//! §18 is unusually specific about how to assert this, and the reason is worth restating: a query
//! missing its tenant predicate returns a **superset**, and a `contains` assertion passes against a
//! superset. Every assertion below therefore compares the **exact set** of rows — length and
//! identity together — which is the only form that fails when a row that should be invisible is
//! present.
//!
//! The second half checks the same property one layer down. Every `handoff_*` table except the
//! migration log has row-level security enabled and forced, so a query that named its tenant and
//! then lost its `WHERE tenant_ref = …` still cannot see another tenant's rows. That is the second
//! line of defence, and it is only a defence if it is tested per table rather than assumed.
//!
//! What that defence does **not** cover is a query that never named a tenant at all: the policy
//! passes when the tenant setting is unset, because `handoffd` legitimately runs queries that have
//! no tenant to name — authentication, which discovers the tenant from the credential (§4.1), and
//! the cross-tenant sweeps. So the guarantee has a condition attached, and
//! `every_request_scoped_path_names_its_tenant` is what turns that condition from a convention into
//! a check: it tightens the policy until an unnamed query sees nothing, drives the request-scoped
//! surface, and asserts every route still answers.

use super::harness::*;
use std::collections::BTreeSet;

/// Every table that carries tenant-scoped rows, with the column that identifies a row.
///
/// Twenty, which is every table `handoff_enable_rls` is applied to across the migration set —
/// `handoff_migrations` is the only `handoff_*` table without a policy, and it holds no tenant's
/// data. A review found `handoff_request_dispositions` missing from this list, which left it
/// neither proven nor named as unproven; the first assertion in
/// `row_level_security_holds_on_every_tenant_scoped_table` now compares this list against
/// `pg_policies`, so the same omission fails rather than passing quietly.
const TENANT_SCOPED_TABLES: &[(&str, &str)] = &[
    ("handoff_requests", "id"),
    ("handoff_request_steps", "request_id"),
    ("handoff_request_dispositions", "id"),
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
    let deployment = Deployment::start("tenants").await;

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

/// One tenant's half of the row-level-security fixture.
struct Fixture<'a> {
    machine: &'a str,
    human: &'a str,
    /// Distinguishes this tenant's rows and idempotency keys from the other's.
    label: &'a str,
    /// The capability handle this tenant's raise declares (§11.4).
    capability: &'a str,
    /// The value sink this tenant's second raise declares (§12).
    sink: &'a str,
}

/// Everything one tenant does — so that both tenants do exactly the same things.
///
/// The eight holes a review found in this fixture were all one shape: a long flow for tenant A and
/// a short one for tenant B, which left eight tables holding rows for a single tenant. Their
/// per-table assertions then compared two empty sets and passed, which is indistinguishable from
/// coverage. Driving both tenants through *one* function is what keeps them symmetric by
/// construction: a table that gains a row here gains it for both tenants, or for neither.
async fn populate(deployment: &Deployment, tenant: Fixture<'_>) {
    let base = &deployment.base;
    let Fixture {
        machine,
        human,
        label,
        capability,
        sink,
    } = tenant;

    // ---- A gated ask that declares a capability and asks for a callback. This one raise reaches
    // requests, request_steps, waiters, idempotency, events, usage, deliveries and grants.
    let mut body = raise_body(&format!("run:rls-{label}"), &format!("Tenant {label} asks"));
    body["mode"] = serde_json::json!("gated");
    // The callback points at this deployment's own `/meta`, which refuses a POST. An attempt is
    // recorded whatever the receiver answers, and a URL on a port nothing is listening on would
    // make the fixture depend on that port staying free for the length of the test.
    body["callback"] = serde_json::json!({"url": format!("{base}/meta")});
    body["routing"] = serde_json::json!({
        "targets": [{"kind": "role", "value": "editor"}],
        "ladder": [{"after": "PT0S", "channels": ["inapp"]}],
    });
    body["requires"]["capabilities"] = serde_json::json!([{
        "handle": capability,
        "type": "interactive_surface",
        "scope": "view",
        "provider": "test/browser",
        "resource_ref": "opaque:bs_rls",
        "label": "the browser the agent is driving",
        "purpose": "Watch what the agent is doing.",
        "optional": false,
        "ttl": "PT15M",
    }]);
    let (status, raised) = post(
        base,
        "/requests",
        machine,
        &format!("rls-raise-{label}"),
        body,
    )
    .await;
    assert_eq!(status, 201, "{raised}");
    let request = raised["id"].as_str().expect("an id").to_string();
    let grant = raised["requires"]["capabilities"][0]["handle"]
        .as_str()
        .expect("a grant handle")
        .to_string();

    // ---- Resolving the grant, which is the only thing that writes a grant session (§11.2).
    let (status, grant_view) = get(base, &format!("/grants/{grant}"), human).await;
    assert_eq!(status, 200, "{grant_view}");
    let radius = grant_view["blast_radius_digest"]
        .as_str()
        .expect("a blast radius digest")
        .to_string();
    let (status, session) = post(
        base,
        &format!("/grants/{grant}/sessions"),
        human,
        &format!("rls-session-{label}"),
        serde_json::json!({"scopes": ["view"], "accepted_blast_radius_digest": radius}),
    )
    .await;
    assert_eq!(status, 200, "{session}");

    // ---- Handing the decision on rather than taking it. §6.6 is explicit that this is not a
    // decision, so the request stays pending and can still be decided below — which is why the
    // disposition does not need a request of its own.
    let (status, delegated) = post(
        base,
        &format!("/requests/{request}/answer"),
        human,
        &format!("rls-delegate-{label}"),
        serde_json::json!({
            "values": {},
            "disposition": "delegate",
            "delegate_to": {"kind": "role", "value": "admin"},
            "note": "Above my limit.",
        }),
    )
    .await;
    assert_eq!(status, 200, "{delegated}");

    // ---- The answer: receipts, authorizations, signals, and the callback a worker then pushes.
    let (status, answered) = post(
        base,
        &format!("/requests/{request}/answer"),
        human,
        &format!("rls-answer-{label}"),
        serde_json::json!({"values": {"decision": "approve"}}),
    )
    .await;
    assert_eq!(status, 200, "{answered}");
    let authorization = answered["authorization"]["id"]
        .as_str()
        .expect("a gated answer mints an authorization")
        .to_string();

    // ---- Spending it once (§10.2), which is what writes a redemption.
    let (status, redeemed) = post(
        base,
        &format!("/authorizations/{authorization}/redeem"),
        machine,
        &format!("rls-redeem-{label}"),
        serde_json::json!({"effect_key": format!("refund:rls-{label}")}),
    )
    .await;
    assert_eq!(status, 200, "{redeemed}");

    // ---- A second ask for the one table the first cannot reach: a value sink is declared at
    // raise time (§12). It is deliberately left pending, because the two subcommands below need a
    // request that has not settled.
    let mut body = raise_body(
        &format!("run:rls-{label}-sink"),
        &format!("Tenant {label} signs in"),
    );
    body["requires"]["answer"] = serde_json::json!({
        "fields": [
            {"name": "email", "label": "Email", "type": "text", "required": true},
            {"name": "password", "label": "Password", "type": "secret", "required": true},
        ],
        "value_sink": {"provider": "test/sink", "op": "submit_credentials", "ref": sink},
    });
    let (status, sink_raise) = post(
        base,
        "/requests",
        machine,
        &format!("rls-sink-{label}"),
        body,
    )
    .await;
    assert_eq!(status, 201, "{sink_raise}");
    let pending = sink_raise["id"].as_str().expect("an id").to_string();

    // ---- The two surfaces the protocol deliberately does not put on HTTP: §4.7's inbound channel
    // adapter and §9.7's runtime observation. Both are `handoffd` subcommands and nothing else
    // writes their tables, so without these two lines those two tables have no fixture at all.
    deployment.run_handoffd(&[
        "inject-channel-message",
        "--request",
        &pending,
        "--channel",
        "email",
        "--text",
        "I'll look at this tomorrow.",
    ]);
    deployment.run_handoffd(&["observe-page-change", "--request", &pending]);
}

/// Whether every tenant-scoped table now holds a row for **both** tenants.
///
/// Only the answer, not the list: naming what is missing is the per-table guard's job, and two
/// messages for one condition would let a slow fixture and an absent one read the same.
async fn both_tenants_are_everywhere(pool: &sqlx::PgPool) -> bool {
    for (table, _) in TENANT_SCOPED_TABLES {
        for tenant in [ORG_A, ORG_B] {
            let rows: i64 = sqlx::query_scalar(&format!(
                "select count(*) from {table} where tenant_ref = $1"
            ))
            .bind(tenant)
            .fetch_one(pool)
            .await
            .unwrap_or_else(|e| panic!("{table}: {e}"));
            if rows == 0 {
                return false;
            }
        }
    }
    true
}

/// Row-level security only binds a role that cannot bypass it.
///
/// A superuser — and any role with `BYPASSRLS` — ignores every policy in the database, so running
/// `handoffd` as one leaves this whole defence inert while every test still passes. This test
/// therefore probes as a role that cannot bypass it, and then asserts that the role it used really
/// could not. What a deployment should grant its service role is in `SECURITY.md`; what matters
/// here is only that the probe is not exempt.
///
/// The tenant predicate in every query is the primary defence. This is the one that catches the day
/// somebody forgets it — on a query that named its tenant. A query that named none is not covered,
/// and that is [`every_request_scoped_path_names_its_tenant`]'s subject.
#[tokio::test]
async fn row_level_security_holds_on_every_tenant_scoped_table() {
    let deployment = Deployment::start("rls").await;

    // The list above is the population under test, so a table that gains a policy without gaining
    // an entry here would be exempt from every assertion below and nothing would say so. This is
    // how `handoff_request_dispositions` came to be neither proven nor named as unproven.
    let with_a_policy: BTreeSet<String> = deployment
        .superuser_sql(
            "select tablename from pg_policies where policyname = 'handoff_tenant_isolation'",
        )
        .lines()
        .map(str::to_string)
        .collect();
    let listed: BTreeSet<String> = TENANT_SCOPED_TABLES
        .iter()
        .map(|(table, _)| table.to_string())
        .collect();
    assert_eq!(
        listed, with_a_policy,
        "TENANT_SCOPED_TABLES and the tables carrying handoff_tenant_isolation have diverged"
    );

    // Both tenants, through one flow, so neither can be the shorter one.
    for tenant in [
        Fixture {
            machine: MACHINE_A,
            human: EDITOR_A,
            label: "a",
            capability: "hg_01K3M7QW8ZC4YRXB2N6VD9FTHA",
            sink: "snk_01K3M7QW8ZC4YRXB2N6VD9FTHA",
        },
        Fixture {
            machine: MACHINE_B,
            human: EDITOR_B,
            label: "b",
            capability: "hg_01K3M7QW8ZC4YRXB2N6VD9FTHB",
            sink: "snk_01K3M7QW8ZC4YRXB2N6VD9FTHB",
        },
    ] {
        populate(&deployment, tenant).await;
    }

    let pool = deployment.pool().await;

    // Deliveries and callbacks are pushed by background workers, so the fixture is not finished
    // when the last HTTP call returns. Wait for it, and let the per-table guard below name
    // whatever never arrived rather than reporting it here — a table that is merely slow and a
    // table that has no fixture at all must not produce the same message.
    for _ in 0..200 {
        if both_tenants_are_everywhere(&pool).await {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    let least_privilege = LeastPrivilegeRole::create(&deployment).await;
    let restricted = least_privilege.pool().await;

    for (table, id_column) in TENANT_SCOPED_TABLES {
        // Per table, before the comparison it protects, and not once for the whole loop. The
        // comparison below can only fail when the *other* tenant owns a row in this table: with
        // both sides empty it holds against a table that has no policy at all. Eight of these
        // tables were empty in an earlier fixture, so eight assertions proved nothing while
        // reading as coverage.
        let other_tenant: i64 = sqlx::query_scalar(&format!(
            "select count(*) from {table} where tenant_ref = $1"
        ))
        .bind(ORG_B)
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|e| panic!("{table}: {e}"));
        assert!(
            other_tenant > 0,
            "{table}: tenant B owns no row here, so the comparison below would compare two empty \
             sets and hold against a table with no policy at all. Grow `populate` until this \
             table has a row for both tenants — every one of them is reachable through the API or \
             a handoffd subcommand — rather than removing the table from TENANT_SCOPED_TABLES."
        );

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
}

/// The request-scoped paths that run on the pool without naming a tenant.
///
/// **Empty, and that is the claim.** Every path [`probe`] drives keeps working when the database
/// refuses any query that has not named a tenant, so row-level security is underneath all of them
/// rather than beside them.
///
/// It was not empty when this test was written. `GET /deliveries/{id}` answered `404` and a keyed
/// `POST /requests/{id}/answer` answered `400`, because `delivery`, `delivery_attempts` and
/// `remember_idempotent` issued their statements against the pool. Each carried its own
/// `WHERE tenant_ref = …`, so nothing leaked — but the primary defence was carrying the whole
/// weight on those three, and a forgotten predicate there would have had nothing under it. They
/// now open a `tenant_tx` like everything else.
///
/// This list may shrink and must not grow. Adding a pool query to a request-scoped path fails this
/// test until somebody writes the line admitting it, and the line has to say **why**, so the next
/// reader knows whether it is a structural limit like `authenticate` or an oversight.
const PATHS_THAT_DO_NOT_NAME_THEIR_TENANT: &[&str] = &[];

/// Tighten the policy on every table except the one authentication must read first.
const FAIL_CLOSED: &str = "\
do $$
declare t text;
begin
  for t in select tablename from pg_policies
           where policyname = 'handoff_tenant_isolation'
             and tablename <> 'handoff_principals'
  loop
    execute format(
      'alter policy handoff_tenant_isolation on %I using \
       (tenant_ref = current_setting(''handoff.tenant_ref'', true))', t);
  end loop;
end $$;";

/// One pass over the request-scoped surface, recording what each path answered.
///
/// Breadth is the whole value here. An empty
/// [`PATHS_THAT_DO_NOT_NAME_THEIR_TENANT`] says nothing about a path this function never calls, so
/// every route that reaches the store on behalf of a caller belongs in it. The delivery worker and
/// the deadline sweep are deliberately absent: they are cross-tenant by construction, have no
/// tenant to name, and are the reason the policy keeps its permissive branch at all.
async fn probe(deployment: &Deployment, run: &str, capability: &str) -> Vec<(&'static str, u16)> {
    let base = &deployment.base;
    let mut seen: Vec<(&'static str, u16)> = Vec::new();

    // Gated, and declaring a capability, so one raise reaches grants and authorizations as well as
    // requests and deliveries.
    let mut body = raise_body(&format!("run:guc-{run}"), "Approve the release?");
    body["mode"] = serde_json::json!("gated");
    body["requires"]["capabilities"] = serde_json::json!([{
        "handle": capability,
        "type": "interactive_surface",
        "scope": "view",
        "provider": "test/browser",
        "resource_ref": "opaque:bs_guc",
        "label": "the browser the agent is driving",
        "purpose": "Watch what the agent is doing.",
        "optional": false,
        "ttl": "PT15M",
    }]);
    let (status, raised) = post(
        base,
        "/requests",
        MACHINE_A,
        &format!("guc-raise-{run}"),
        body,
    )
    .await;
    seen.push(("POST /requests", status));
    let request = raised["id"].as_str().unwrap_or_default().to_string();
    let delivery = raised["deliveries"][0]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let grant = raised["requires"]["capabilities"][0]["handle"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (status, _) = get(base, &format!("/requests/{request}"), MACHINE_A).await;
    seen.push(("GET /requests/{id}", status));
    let (status, _) = get(
        base,
        &format!("/requests?waiter_ref=run%3Aguc-{run}"),
        MACHINE_A,
    )
    .await;
    seen.push(("GET /requests", status));
    let (status, _) = get(base, &format!("/requests/{request}/deliveries"), MACHINE_A).await;
    seen.push(("GET /requests/{id}/deliveries", status));
    let (status, _) = get(base, &format!("/deliveries/{delivery}"), MACHINE_A).await;
    seen.push(("GET /deliveries/{id}", status));
    let (status, _) = post(
        base,
        &format!("/deliveries/{delivery}/redeliver"),
        MACHINE_A,
        &format!("guc-redeliver-{run}"),
        serde_json::json!({}),
    )
    .await;
    seen.push(("POST /deliveries/{id}/redeliver", status));

    let (status, grant_view) = get(base, &format!("/grants/{grant}"), EDITOR_A).await;
    seen.push(("GET /grants/{handle}", status));
    let radius = grant_view["blast_radius_digest"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, _) = post(
        base,
        &format!("/grants/{grant}/sessions"),
        EDITOR_A,
        &format!("guc-session-{run}"),
        serde_json::json!({"scopes": ["view"], "accepted_blast_radius_digest": radius}),
    )
    .await;
    seen.push(("POST /grants/{handle}/sessions", status));

    let (status, _) = get(
        base,
        &format!("/waiters/run%3Aguc-{run}/signals"),
        MACHINE_A,
    )
    .await;
    seen.push(("GET /waiters/{ref}/signals", status));

    // Without a key, §3.1 stores no replay record, so this is the answer path alone.
    let (status, answered) = post_without_key(
        base,
        &format!("/requests/{request}/answer"),
        EDITOR_A,
        serde_json::json!({"values": {"decision": "approve"}}),
    )
    .await;
    seen.push(("POST /requests/{id}/answer", status));
    let authorization = answered["authorization"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (status, _) = get(base, &format!("/requests/{request}/receipt"), MACHINE_A).await;
    seen.push(("GET /requests/{id}/receipt", status));
    let (status, _) = get(base, "/receipts", MACHINE_A).await;
    seen.push(("GET /receipts", status));
    let (status, _) = get(base, "/receipts/chain-head", MACHINE_A).await;
    seen.push(("GET /receipts/chain-head", status));
    let (status, _) = get(base, &format!("/authorizations/{authorization}"), MACHINE_A).await;
    seen.push(("GET /authorizations/{id}", status));
    let (status, _) = post(
        base,
        &format!("/authorizations/{authorization}/redeem"),
        MACHINE_A,
        &format!("guc-redeem-{run}"),
        serde_json::json!({"effect_key": format!("refund:guc-{run}")}),
    )
    .await;
    seen.push(("POST /authorizations/{id}/redeem", status));

    // With a key, on a request of its own, because the replay record it writes is what differs.
    let (_, second) = post(
        base,
        "/requests",
        MACHINE_A,
        &format!("guc-raise-keyed-{run}"),
        raise_body(&format!("run:guc-keyed-{run}"), "Approve the second one?"),
    )
    .await;
    let keyed = second["id"].as_str().unwrap_or_default().to_string();
    let (status, _) = post(
        base,
        &format!("/requests/{keyed}/answer"),
        EDITOR_A,
        &format!("guc-answer-{run}"),
        serde_json::json!({"values": {"decision": "approve"}}),
    )
    .await;
    seen.push((
        "POST /requests/{id}/answer, with an Idempotency-Key",
        status,
    ));

    // Cancelling needs a request nobody has decided, so it gets one of its own.
    let (_, third) = post(
        base,
        "/requests",
        MACHINE_A,
        &format!("guc-raise-cancel-{run}"),
        raise_body(&format!("run:guc-cancel-{run}"), "Approve the third one?"),
    )
    .await;
    let cancelled = third["id"].as_str().unwrap_or_default().to_string();
    let (status, _) = post(
        base,
        &format!("/requests/{cancelled}/cancel"),
        MACHINE_A,
        &format!("guc-cancel-{run}"),
        serde_json::json!({"reason": "the run is over"}),
    )
    .await;
    seen.push(("POST /requests/{id}/cancel", status));

    seen
}

/// Which request-scoped paths survive a database that refuses every query with no tenant named.
///
/// `SECURITY.md` and `core/dev/README.md` both say that each request-scoped transaction names its
/// tenant before it reads, so a query that lost its `WHERE tenant_ref = …` still cannot see another
/// tenant's rows. That sentence is true of the paths it describes and nothing enforced it — the
/// policy itself cannot, because it passes when no tenant is named, and it has to: **authentication
/// resolves a credential to a tenant** (§4.1), so the query that discovers the tenant is by
/// construction unable to name it. A fail-closed policy on `handoff_principals` makes every
/// authenticated request answer `401`, which is why that one table is exempted below rather than
/// tightened.
///
/// So this asserts what is actually assertable, and with the other nineteen tables fail-closed it
/// now holds for every path [`probe`] drives: [`PATHS_THAT_DO_NOT_NAME_THEIR_TENANT`] is empty.
#[tokio::test]
async fn every_request_scoped_path_names_its_tenant() {
    let deployment = Deployment::start_as_least_privilege("guc").await;

    // Otherwise the tightening below is invisible to the server and every probe passes.
    assert!(
        !deployment.bypasses_row_level_security(),
        "handoffd is connected as a role that ignores every policy, so tightening one proves \
         nothing"
    );

    // The anti-vacuity guard, one per probe rather than one per run: a path that is already broken
    // would otherwise read as a path that failed because of the tightening.
    let permissive = probe(&deployment, "permissive", "hg_01K3M7QW8ZC4YRXB2N6VD9FTHC").await;
    for (path, status) in &permissive {
        assert!(
            (200..300).contains(status),
            "{path} answered {status} before the policy was tightened, so its behaviour after \
             tightening measures nothing"
        );
    }

    deployment.superuser_sql(FAIL_CLOSED);
    let tightened: usize = deployment
        .superuser_sql(
            "select count(*) from pg_policies where policyname = 'handoff_tenant_isolation' \
             and qual not ilike '%coalesce%'",
        )
        .parse()
        .expect("a count");
    assert_eq!(
        tightened,
        TENANT_SCOPED_TABLES.len() - 1,
        "the tightening did not reach every table it was supposed to"
    );

    // And that it bites the role `handoffd` actually connects as. Rows exist; a connection that
    // has named no tenant must now see none of them.
    let as_handoffd = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&deployment.url)
        .await
        .expect("connect as the role handoffd runs as");
    let unnamed: i64 = sqlx::query_scalar("select count(*) from handoff_requests")
        .fetch_one(&as_handoffd)
        .await
        .expect("count with no tenant named");
    let total: i64 = deployment
        .superuser_sql("select count(*) from handoff_requests")
        .parse()
        .expect("a count");
    assert!(
        total > 0,
        "no requests exist, so the probe below proves nothing"
    );
    assert_eq!(
        unnamed, 0,
        "the tightened policy is not refusing an unnamed connection, so nothing below is a test \
         of naming the tenant"
    );
    drop(as_handoffd);

    let fail_closed = probe(&deployment, "failclosed", "hg_01K3M7QW8ZC4YRXB2N6VD9FTHD").await;
    let mut surprises: Vec<String> = Vec::new();
    for ((path, status), (_, before)) in fail_closed.iter().zip(&permissive) {
        let should_work = !PATHS_THAT_DO_NOT_NAME_THEIR_TENANT.contains(path);
        let worked = (200..300).contains(status);
        if worked != should_work {
            surprises.push(format!(
                "{path}: {before} permissive, {status} fail-closed — expected it to {}",
                if should_work {
                    "keep working, because it names its tenant"
                } else {
                    "fail, because PATHS_THAT_DO_NOT_NAME_THEIR_TENANT says it does not"
                }
            ));
        }
    }
    assert!(
        surprises.is_empty(),
        "row-level security protects a path only if that path names its tenant. A path that \
         started failing has grown a query on the pool and must be moved inside `tenant_tx`; a \
         path that stopped failing has been fixed and must be removed from \
         PATHS_THAT_DO_NOT_NAME_THEIR_TENANT.\n  {}",
        surprises.join("\n  ")
    );
}

/// A database role with no more than `handoffd` needs, created for the life of one test.
struct LeastPrivilegeRole {
    name: String,
    url: String,
    admin: String,
    /// The deployment's own database, as its owner. Needed to release the role's grants before it
    /// can be dropped.
    database: String,
}

impl LeastPrivilegeRole {
    async fn create(deployment: &Deployment) -> Self {
        // Unique per test run, so two runs in parallel do not fight over one role.
        let name = format!(
            "handoff_app_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or_default()
        );
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
            database: deployment.url.clone(),
        }
    }

    async fn pool(&self) -> sqlx::PgPool {
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&self.url)
            .await
            .expect("connect as the least-privilege role")
    }
}

/// Roles are cluster-wide, so dropping the disposable database does not take them with it.
///
/// The drop therefore lives here rather than at the end of the test body: a failing assertion
/// panics, and a cleanup that only runs on the happy path leaves a role behind on every red run —
/// which is exactly when tests are run most.
impl Drop for LeastPrivilegeRole {
    fn drop(&mut self) {
        let name = self.name.clone();
        // A role cannot be dropped while it still holds privileges on objects, so the grants go
        // first — in the database that issued them, as its owner.
        let _ = std::process::Command::new("psql")
            .args([
                &self.database,
                "-q",
                "-c",
                &format!("drop owned by \"{name}\" cascade"),
            ])
            .output();
        let _ = std::process::Command::new("psql")
            .args([
                &self.admin,
                "-q",
                "-c",
                &format!("drop role if exists \"{name}\""),
            ])
            .output();
    }
}

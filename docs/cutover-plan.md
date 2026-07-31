# Cutover plan — today's Ωmegas browser-login flow becomes a Handoff client

**This is a plan. No step in it has been executed, and no internal Ωmegas cutover is to be performed
on the strength of this document.** It exists so that when someone does execute it, they are reading
ordered, reversible steps with checkable gates rather than improvising against a live approval flow
that people are currently depending on.

Every step below has three parts, and a step without all three is not ready to run:

- a **gate** — something you can check, that is either true or false, before the next step begins;
- a **rollback** — what you do when the gate is false, and how long it takes;
- a **blast radius** — who notices if it goes wrong.

---

## 0. What is actually shipped today

Grounding first, because a migration plan written against a remembered system migrates a system that
does not exist.

**The API surface.** Five routes in `services/api/src/main.rs:463-471`:

| Route | Handler | What it does |
|---|---|---|
| `GET /api/approvals` | `list_approvals` | The inbox. Filters by lifecycle status; defaults to `pending`. |
| `GET /api/approvals/{id}` | `get_approval` | One approval. |
| `POST /api/approvals/{id}/approve` | `approve_action` | Ordinary write approval. |
| `POST /api/approvals/{id}/deny` | `deny_action` | The same, refused. |
| `POST /api/approvals/{id}/fulfill` | `fulfill_credential_request` | **Browser login.** A verified human types a real third-party credential, which is injected into the Space's sprite. |

**The storage.** `pending_approvals`, across four migrations: `0017_pending_approvals.sql` creates
it, `0019_approval_intent_binding.sql` adds the intent binding and the single-use `consumed_at`,
`0052_approval_credential_spec.sql` adds `credential_spec`, and `0054_credential_attempt_ttl.sql`
adds the attempt window. It is read by `pg/approvals.rs`, `pg/engine.rs`, and `pg/notifications.rs`.
**A resolved `pending_approvals` row is the durable source of truth the engine's gate reads**
(`handlers.rs:1140`) — which is the single most important fact in this document, and the reason the
last step is the one that must not be rushed.

**The web surface.** `apps/web/src/routes/spaces/$spaceId/handoff.tsx`, labelled "Handoff" in the
sidebar (`app-sidebar.tsx:57`, a static six-entry `DESTINATIONS` array with no registry and no
gating). Its own header comment records that it is a **consolidation of three previously
uncoordinated surfaces**: the bell, the Computer page's `needs_human` state, and an approvals list
that shipped complete but was reachable only by deep link. `approvals.index.tsx` already redirects
into it. `approvals.$approvalId.tsx` is **untouched on purpose**, because `credential.request`
notifications deep-link straight to it.

**What is not there.** No `handoff.*` event namespace, no receipt, no signature, no tenant scoping
beyond the Space, and no machine caller — every one of these routes is behind a human `Principal`.

**The name collision is real.** The page called "Handoff" today is not the Handoff product. It is the
Agent's in-Space "what is waiting on me" view. Both things cannot keep the name, and the rename has
to land before a product switcher ships or the two become permanently confusable.

---

## 1. The sequencing this inherits, and what it actually blocks

Five constraints come from the shared control-plane plan. Two of them are hard stops on this cutover
and three shape it.

| Constraint | Status | Effect on this plan |
|---|---|---|
| **Base commit: M0–M2 green.** Branching earlier means building on org-wide, role-blind authorization and a live P0. | **Satisfied.** M0–M3 landed at base `f6ce53d`. | The branch base is legitimate. Steps 1–5 may begin. |
| **M4 — entitlements.** `is_entitled(org, 'handoff', now)` and the `Entitled<P>` extractor. | **Not landed.** No `products`, `plans`, `entitlements`, `seats`, or `tier` table exists in any of the 74 migrations. | Handoff ships behind the existing `ActiveSubscription` extractor, which means **Handoff is included in the suite subscription**. Do not build a Handoff-specific gate: it would be the third paywall shape in one codebase. |
| **M5 — machine auth.** `omg_` keys, scopes, `Caller`. | **Not landed.** | **Hard stop on step 7.** Handoff's public API cannot authenticate a third-party caller, and Operator's integration cannot ship, because there is no in-process shortcut available once Handoff is out of repo. |
| **Rename `handoff.tsx` before any product switcher ships**, keeping both existing redirects. | Not started. | Step 2. It is early on purpose — it is the cheapest step and it stops the collision compounding. |
| **Do not ship per-tenant paging on the global `RETELL_TO_NUMBER`.** | Live defect. | Voice is out of scope for this cutover entirely. `managed/crates/handoff-omegas/src/delivery.rs` refuses to construct such a channel, so this constraint is enforced by the code rather than by this document. |

There is a sixth, and it is the one most likely to be discovered late: **`omg_` machine auth as
specified cannot serve an out-of-repo Handoff at all.** It is specified as an Axum extractor doing an
indexed fetch against `api_keys` inside the omega repo, and a service on `handoff.omegas.dev` has
neither the extractor nor the database. The recommended resolution is a client-credentials token
exchange reusing the ES256 JWKS that already works. **Until an owner rules on it, step 7 has no
mechanism, not merely no schedule.** See `managed/crates/handoff-omegas/src/auth.rs`.

---

## 2. The principle the whole plan turns on

**`pending_approvals` is the durable source of truth the engine's gate reads.** So the ordering is
forced:

> Move the **reading** last. Move the **writing** first. Never move both at once.

Every step below adds a Handoff-side record and leaves the Ωmegas-side record authoritative, until
one late, single, reversible step flips which one the engine believes. That step is 8, it is the only
irreversible-feeling one, and it has a rollback measured in minutes because the old rows never
stopped being written.

---

## 3. The steps

### Step 1 — Stand up managed Handoff, serving nobody

Deploy `handoff-omegas-server` to `handoff.omegas.dev` with its own database. No Ωmegas traffic
touches it. `handoff-omegas-server preflight` prints every absent dependency on boot.

- **Gate:** `GET https://handoff.omegas.dev/v1/meta` returns a conformance level **derived from the
  build rather than declared**, and `handoff-conformance` run against that base URL is **26/26,
  exit 0**. `/v1/meta` also carries the `handoff-core` version, which must be no more than one minor
  release behind the latest published tag — that is the anti-drift check, and it is `/v1/meta`
  because there is no `/v1/version` route and adding one would duplicate an endpoint that already
  answers the question.
- **Rollback:** delete the deployment. Nothing depends on it. Minutes.
- **Blast radius:** **that hostname already serves the night-hack demo.** This step replaces a
  running service, so it is not a greenfield deploy: decide first whether the demo is retired, moved,
  or kept on another name, and note that the existing deployment has no authentication at all. No
  Ωmegas code changes, which is a different question from nothing being affected.

> The conformance gate belongs here, on the first deploy, and not at v1.0. A gate added later is a
> gate that never gets added, and this is the only mechanism that mechanically prevents the managed
> service drifting from the open core.

### Step 2 — Rename the page, keep every redirect

`/spaces/$spaceId/handoff` → `/spaces/$spaceId/waiting`, labelled **"Waiting on you"**. Update the
one `DESTINATIONS` entry in `app-sidebar.tsx:57`. Add `/spaces/$spaceId/handoff` → `/waiting` as a
redirect; `/approvals` keeps its existing redirect *through* it, so the old deep link still resolves
in two hops.

**Rename the route; do not discard the page.** Its header comment records that it already
consolidated three uncoordinated surfaces. That consolidation is correct and rebuilding it under a
new name would re-fragment what someone already merged.

`approvals.$approvalId.tsx` is **not touched**. `credential.request` notifications deep-link straight
to it, and those notifications are already sitting in people's inboxes.

- **Gate:** all three of `/spaces/{id}/handoff`, `/spaces/{id}/approvals`, and
  `/spaces/{id}/approvals/{approvalId}` resolve to a working page; the sidebar reads "Waiting on
  you"; no route file was deleted.
- **Rollback:** revert one commit. The redirects are additive, so a revert cannot strand a link.
- **Blast radius:** every dashboard user sees a renamed sidebar entry. Nothing breaks.

### Step 3 — Land the five control-plane endpoints, unused

Net-new surface in `services/api`, and **control-plane surface rather than Handoff code**:
`POST /api/token`, `POST /api/usage/ingest`, `POST /api/events/ingest`,
`GET /api/orgs/{id}/members`, and entitlements (recommended as a JWT claim rather than a second hop).

Two properties the existing `/api` surface does not have, and both are easy to get wrong:

1. They are machine-to-machine with **no human `Principal`**, so `409
   organization_selection_required` must never fire on them. The calling key is org-scoped, so the
   org is unambiguous by construction — if that 409 can fire, the key binding is wrong.
2. The tenant comes from **the key's own binding, never from the request body**. The nearest
   precedent is the signed-webhook rule at `adapters/trigger_ingest.rs:8-11`: a verified credential
   proves *who is calling*, never *which tenant the payload is about*.

Prerequisite inside this step: `usage_events.idempotency_key` is **globally unique, not per-org**
(B-10), so the ingest endpoint must either scope it or accept keys that already carry their org.
The adapter mints `handoff:{org_id}:{request_id}:{kind}` and refuses any reading whose key does not
contain its tenant, but a server that assumes global uniqueness will still collide across products.

- **Gate:** each endpoint returns a documented shape to a synthetic caller; a request carrying a
  *different* org in its body than in its credential is refused; a replayed `idempotency_key` from
  one org does not suppress another org's row. That last one is a two-org test, and it is the one
  worth writing first.
- **Rollback:** revert. Nothing calls them yet.
- **Blast radius:** none behaviourally; new routes on a live service, so ordinary deploy risk only.

### Step 4 — Make `events` append-only, and restore `payload`

Two prerequisites that are on Handoff's critical path rather than on someone else's backlog: `events`
is **not append-only at the database** and carries two live `UPDATE events SET instance_id`
statements (`pg/engine.rs:292`, `:401`) that must be removed first; and the API drops `payload`
entirely from `EventDto`, which is exactly the field a receipt summary lives in.

- **Gate:** no `UPDATE events` statement exists anywhere in the repo; a database-level constraint or
  trigger refuses an update; `GET /api/org/audit` returns `payload`.
- **Rollback:** this one is genuinely awkward — dropping a constraint is easy, but restoring the two
  `UPDATE` statements means restoring the behaviour that needed them. Do this step on its own deploy,
  with `pg_dump` taken first, and do not bundle it.
- **Blast radius:** anything that relied on mutating an event. The two known statements are the
  audit; a third would surface as a runtime 500, because 461 queries are runtime `sqlx::query` with
  **zero** `sqlx::query!`.

### Step 5 — Dual-write: every approval also raises a Handoff request

The first step that touches the live flow, and it is deliberately write-only.

When `pending_approvals` gets a row, the API also raises a Handoff request over HTTP: same prompt,
same target, a `dedupe_key` derived from the approval id. **Ωmegas remains authoritative for
everything.** The Handoff request is written and then ignored — nothing reads it, nothing waits on
it, and the dashboard does not render it.

Failure to raise it is logged and **never** fails the approval. Handoff being down must not stop a
person approving something in Ωmegas, and this is the step where that could accidentally become
true.

- **Gate:** for a 24-hour window, `count(pending_approvals created)` equals
  `count(handoff requests raised)` per org, and every Handoff request carries the originating
  approval id. Deliberately kill Handoff for ten minutes and confirm approvals still complete and the
  only symptom is a gap in the Handoff side.
- **Rollback:** one feature flag off, or revert one commit. The Handoff rows are orphaned and
  harmless.
- **Blast radius:** one added HTTP call on the approval-creation path. Latency, and nothing else, if
  the fire-and-forget is genuinely fire-and-forget.

### Step 6 — Read-shadow: compare, render nothing

The dashboard reads **both** — `GET /api/approvals` as it does today, and the Handoff request list —
and renders **only the Ωmegas answer**. Disagreements are recorded.

This is where you find out that the two systems disagree about state, and it is much better to find
out here than in step 8. Expect real divergence: Handoff has an attempt window and a TTL sweep that
`pending_approvals` does not, and `0054_credential_attempt_ttl.sql` is not the same clock.

- **Gate:** divergence below an agreed threshold for a week, **with every divergence class named and
  explained**. "0.3% disagree" is not a passing gate; "0.3% disagree, all of them are attempt-window
  lapses on browser logins, and here is why that is the correct new behaviour" is.
- **Rollback:** stop reading the shadow. Nothing was rendered from it.
- **Blast radius:** one extra read per page load.

### Step 7 — Machine auth, and Operator over the public API — **BLOCKED ON M5**

Operator calls Handoff through the **public** API using the open SDK. There is no in-process
shortcut, because Operator lives in `omega` and Handoff does not. This is the dogfooding mechanism:
every integration bug an external developer would hit, we hit first.

**This step cannot start.** M5 has not landed, and as noted in §1 the specified mechanism does not
work for an out-of-repo service. It needs an owner decision on the token exchange before it needs a
schedule.

- **Gate:** an `omg_` key exchanges for a short-lived ES256 JWT; Handoff verifies it **offline**
  against the JWKS with no call to the control plane on the hot path; a revoked key stops working
  within the token TTL; a raw `omg_` key presented directly to Handoff is **refused** (it already is
  — `auth.rs` refuses one on sight, with the remedy in the message).
- **Rollback:** revoke the key, disable the Operator integration flag. Operator returns to its
  in-process path, which still exists because step 8 has not run.
- **Blast radius:** Operator only, and only behind a flag.

### Step 8 — Flip the read. The engine believes Handoff

The one step that matters. The engine's gate stops reading `pending_approvals` and starts reading the
Handoff receipt. Dual-write from step 5 **continues** — `pending_approvals` keeps being written,
which is exactly what makes this reversible.

Do it one org at a time. Do not do it for all orgs on one deploy.

- **Gate:** for the flipped org, every resumed workflow resumed from a Handoff receipt; no workflow
  stalled; the single-use property held (one receipt authorized exactly one effect, `consumed_at`'s
  successor); a double-resolve returned a **conflict** rather than last-write-wins.
- **Rollback:** flip the org back. Minutes, and lossless, because `pending_approvals` never stopped
  being written. **This is the entire reason step 5 is separate from step 8.**
- **Blast radius:** one org's blocked workflows. Which is why it is one org.

### Step 9 — Retire the old surface, last and slowly

Only after every org has been on step 8 for long enough to be boring.

`GET /api/approvals` and friends stay, marked deprecated, reading through to Handoff. The routes are
not deleted: `approvals.$approvalId` is deep-linked from notifications that are already sitting in
people's inboxes and will keep arriving for as long as those notifications are unread.

`pending_approvals` is **not dropped**. It stops being written, and it stays as history. Dropping it
is a separate decision on a separate day, and there is no pre-PMF reason to make it.

- **Gate:** zero reads of `pending_approvals` outside the compatibility shim for 30 days.
- **Rollback:** turn the writes back on. The table is still there, which is the point.
- **Blast radius:** any integration nobody remembered. The 30-day read-count is what finds them.

---

## 4. The order, and what blocks what

```
1 stand up managed  ─────────────┐
2 rename the page   ─────────────┤   (independent; do 2 early, it is cheap and stops the collision compounding)
                                 │
3 control-plane endpoints ───────┤
4 events append-only ────────────┤   (4 gates 3's event ingest; do 4 first if you can afford the deploy)
                                 │
                                 ▼
                        5 dual-write  ──▶  6 read-shadow  ──▶  8 flip the read  ──▶  9 retire
                                 ▲
                                 │
        7 machine auth + Operator ┘   ◀── BLOCKED ON M5 *and* on the token-exchange decision
```

Step 7 does not block step 8. The internal cutover uses the internal path; 7 is what lets a
**third party** — including Operator, which is now a third party by architecture — use the same API.
Keeping them independent is what stops an unmade owner decision holding the internal work hostage.

---

## 5. What this plan deliberately does not do

- **It does not migrate history.** Resolved `pending_approvals` rows stay where they are. Backfilling
  them into Handoff receipts would mint receipts for decisions no Handoff instance witnessed, which
  is a signed record of something that did not happen the way the record says.
- **It does not touch voice.** The only pager available uses one global destination number.
- **It does not build a Handoff-specific entitlement gate.** Handoff rides `ActiveSubscription` until
  M4, and then consumes `is_entitled(org, 'handoff', now)` and invents nothing.
- **It does not charge per intervention.** Meter everything, charge nothing, at v1. `usage_events`
  gives the volume data to price correctly later at no schema cost; the pricing table is
  immutable-append, so a guess made now is a permanent row.
- **It does not promise Space-scoped isolation.** `space_grant()` returns `None` unconditionally, so
  every org admin is an admin on every Space. "Finance's handoffs are invisible to Marketing" is not
  deliverable and must not be gated on, marketed, or designed around until that changes. The managed
  tenant is the **org**, and `managed/crates/handoff-omegas/src/tenancy.rs` refuses a Space id
  outright rather than accepting a boundary it cannot enforce.

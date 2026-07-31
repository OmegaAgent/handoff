# Hostile review — Handoff v0.1

**Reviewer:** independent adversarial pass, 2026-07-30/31. Worktree `/Users/noureddinbakir/handoff-v1`,
branch `v1/protocol` at `be9f0d1`. Read-only on all source; the only file written is this one.

**Method.** I reproduced the conformance baseline independently (24/24), then stood up my own
`handoffd` on a disposable database with my own bootstrap — including a second human principal in
tenant B, which `core/dev/bootstrap.json` does not have — and attacked the HTTP API directly. Where
a defect is claimed below, there is a reproduction. Where I could not reproduce, I say so and mark it
suspicion. §11 lists what I attacked and failed to break, which is the part that should calibrate how
much the rest is worth.

**One correction against myself, up front.** My first receipt-deletion probe reported that excising a
middle receipt went undetected. That was wrong: my script dropped only `handoff_receipts_no_update`,
so a separate `handoff_receipts_no_delete` trigger refused the delete and nothing changed. The chain
verified because the data was intact. Re-run with all three triggers dropped and a
did-the-row-count-actually-change guard, middle-excision **is** detected. This is the same failure
mode the project already found twice in its own suite, and it is worth noting that their
`chain-tamper-check.sh` has exactly the guard my first attempt lacked.

---

## Findings, by severity

### F-1 (High) — An idempotency key is not scoped to the object it acts on, so one answer can authorize a different request's effect

**Claimed.** README.md:12 — *"One human answer, delivered to your agent exactly once, as typed data,
**authorizing exactly one effect**."* README.md:26 — *"The answer is bound to the specific thing it
was shown against. It does not generalize to the next call, the next run, or a similar-looking
request."* Spec §6.7 rule 3 scopes the replay to *"the same `Idempotency-Key` as the answer that
landed"* — that sentence sits inside a section about duplicate answers to **one** request.

**True.** `core/crates/handoff-server/src/http.rs:119-135` builds the replay slot from
`(tenant, principal, operation, key, body_digest)`. The **request id from the path is not in it.**
`routes.rs:585-589` then checks that slot and returns the stored response *before* the request id is
even parsed. So the same principal answering a *different* request with the same key and identical
`values` receives the first request's receipt and authorization, and the second request is never
answered.

**Reproduction** (against a fresh `handoffd`, tenant A, human editor `tok_ha`):

```
R1=req_01KYTEPV6D1K45V7A05J6W3KZY   R2=req_01KYTEPV8FHFPN080YBEECBJS8   (distinct dedupe_keys)

POST /requests/$R1/answer  Idempotency-Key: SHARED2  {"values":{"decision":"approve"}}
 -> 200 {"authorization":{"id":"auth_01KYTEPVA30RW9RH4QKE82ZJHZ",...},
         "receipt":{"id":"rcpt_01KYTEPVA31K27B78SM0C44HCZ",...},
         "request":{"id":"req_01KYTEPV6D1K45V7A05J6W3KZY","state":"answered"}}

POST /requests/$R2/answer  Idempotency-Key: SHARED2  {"values":{"decision":"approve"}}
 -> 200  ... byte-identical body, still naming R1 ...

GET /requests/$R2  -> state: pending | answered_at: None
GET /requests/$R2/receipt -> 404 request_not_found
```

The agent now holds an authorization it believes belongs to R2, and it spends:

```
POST /authorizations/auth_01KYTEPVA30RW9RH4QKE82ZJHZ/redeem {"effect_key":"refund-for-request-R2"}
 -> 200 {"first_redemption":true}

GET /authorizations/auth_01KYTEPVA30RW9RH4QKE82ZJHZ
 -> {"request_id":"req_01KYTEPV6D1K45V7A05J6W3KZY",       <-- R1
     "bound_to":{"waiter_ref":"run:atk-X"},                <-- R1's waiter
     "redemptions":[{"effect_key":"refund-for-request-R2"}], "state":"spent"}
```

R1's single human "approve" authorized an effect named for R2. That is precisely the generalization
README.md:26 says cannot happen.

**Same defect, every per-object mutation.** `cancel`, `amend`, `supersede`, `escalate`, `reassign`,
`arm_attempt` all build their slot the same way. Reproduced for `cancel`: cancelling C1 with key `CX`,
then cancelling C2 with the same key and body, returns C1's full request document with HTTP 200 while
C2 stays `pending`. An operator who believes they called a request off has not: the human is still
asked and can still authorize the effect.

**Blast radius.** Requires a caller to reuse a key across two answers with identical `values` — which
is the common case (`{"decision":"approve"}`) for any responder UI or SDK that derives its key from a
workflow step, a session, or a constant rather than from the request id. The failure is silent in both
directions: no error, and the returned document names the *other* request, so only a client that
compares `request.id` against what it asked for will notice. Nothing in the suite covers key reuse
across objects; C-1 and C-19 both use one key per object.

**Aggravating detail.** For `AnonymousLink` principals `Principal::id` is `None`, and http.rs:127-129
falls back to the literal `"{tenant}::anonymous"`. Every link-token holder in a tenant therefore shares
one idempotency namespace, so the collision above becomes cross-*person* rather than same-person. I did
not reproduce this end-to-end (see F-6: the dev bootstrap cannot express two link credentials), so
treat the cross-person variant as **suspicion**; the same-principal variant above is demonstrated.

---

### F-2 (High) — CI never measures conformance of the reference server, and the level it reports is a hardcoded literal

**Claimed.** README.md:142-145 — *"The mechanism that keeps this from becoming a slogan is the
conformance suite. A hosted service that cannot pass the open suite has a red build, which turns 'we
did not quietly fork the core' from an intention into a check anyone can rerun."*
`conformance/README.md` and `CONTRIBUTING.md` make the suite the governance instrument.

**True.** `.github/workflows/ci.yml:183-222` is the only conformance job, and it runs the suite
**exclusively against `crates/handoff-conformance/dev/stub_501.py`**, asserting that it *fails*
(`"The suite correctly reported non-conformance (exit ${rc})."`). `handoffd` is never pointed at the
suite anywhere in CI — the `rust` job at ci.yml:55-56 only checks that the binary exists
(`test -x target/debug/handoffd`). The 24/24 result exists only when a human runs
`core/dev/run-conformance.sh` locally. The meta-test is real and valuable — it proves the suite can go
red — but it is the *inverse* of the claim, which is that a build goes red when the server stops
conforming. No build can currently do that.

Compounding: `core/crates/handoff-server/src/routes.rs:1392` returns `"conformance_level": 1` as a
literal. The level a Server advertises — which §1.2 makes normative and which C-17 reads — is
self-declared and unmeasured.

There is also no managed job at all: ci.yml:26-28 pins the Rust job to `working-directory: core`, so
none of the managed tier's tests run in CI either.

**Blast radius.** The single mechanism the README offers in place of trust is not wired to a build.
This is a fixable gap, not a false statement about the code — but as written the README asserts a
check that does not exist.

---

### F-3 (High) — `resume_payload` is stored in plaintext against a spec MUST, and a doc comment asserts the opposite

**Claimed.** `spec/handoff-protocol-v0.1.md:1300` — *"`resume_payload` | Opaque bytes, ≤ 64 KiB,
base64-encoded. The Server **MUST encrypt it at rest**, MUST store it verbatim, and MUST NOT interpret
it"*. `core/crates/handoff-protocol/src/request.rs:285` — *"Base64 on the wire; **encrypted at rest by
the Server.**"*

**True.** There is no encryption anywhere in the store. `migrations.rs:96` and `:210` declare
`resume_payload text`; `store.rs:767` and `:1181` bind the base64 string directly. `grep -rn "encrypt"`
across `core/crates/handoff-store-postgres/src/` returns **zero** hits. The doc comment states as fact
a property the code does not have, which is the one category of overclaim this project explicitly
polices.

**And it is unexercised.** `wire.rs:247-248` *does* return `resume_ref` and `resume_payload` in
signals, so the §14 path is live in the server. But `routes.rs:1392-1393` advertises
`conformance_level: 1` with `extensions: []`, and C-17's first step asserts `conformance_level == 2`
and `extensions` containing `continuation`. The runner selects cases with `level <= requested`
(`lib.rs:118-121`) and runs Level 1. So C-17 never runs, and the continuation feature is shipped,
reachable, unencrypted, and covered by no conformance case.

---

### F-4 (Medium-High) — Spec §18's C-24 row contradicts spec §1.4, and the shipped case quietly sides against the spec

**Claimed.** §18, C-24 row: *"Answer with `0`, `1e-6`, and `1e20` → **accepted**."* §18 is normative:
*"A Server is Handoff v0.1 Level 1 compliant when it passes all 24 Level 1 cases."*

**True.** §1.4 rule 2 requires every number with no fractional part to be within ±(2^53 − 1). `1e20`
is integral and vastly exceeds 2^53, so §1.4 requires it **rejected** while §18 requires it
**accepted**. The implementation follows §1.4:

```
POST /requests/{id}/answer  {"values":{"amount":1e20}}
 -> 422 answer_validation_failed, field "amount"
```

The case authors noticed and documented it in the case's own rationale
(`conformance/cases/c24_canonical_numbers_are_deterministic.yaml:37-39`): *"the accepted upper boundary
tested here is 9007199254740991, not 1e20."* So the case tests the correct behaviour — but §18, the
normative table an independent implementer reads, was never corrected.

**Blast radius.** An independent implementer working from §18 implements acceptance of `1e20`, then
fails C-24 with no explanation from the spec. `CONTRIBUTING.md` calls an ambiguity in a normative
document a real defect; this is stronger than an ambiguity — it is a contradiction. The audience most
affected is exactly the one the project is for.

---

### F-5 (Medium) — Row-level security is verified in a test and inert in every shipped configuration

**Claimed.** `core/crates/handoff-server/tests/suite/isolation.rs:9-12` — *"Every `handoff_*` table has
row-level security, and every request-scoped transaction names its tenant before it reads anything, so
a query that lost its `WHERE tenant_ref = …` still cannot see another tenant's rows. **That is the
second line of defence**, and it is only a defence if it is tested per table rather than assumed."*

**True, and the test is now honest** — it creates a least-privilege role and asserts
`not (rolsuper or rolbypassrls)`, so it can no longer pass vacuously under a superuser. That fix is
real and I confirmed it by reading it.

**But the property does not hold in any configuration this repo ships.** `run-conformance.sh:52`
defaults `PGUSER=omega`, and `omega` is a superuser; superusers bypass RLS regardless of
`FORCE ROW LEVEL SECURITY`. Demonstrated against my running deployment:

```
select relrowsecurity, relforcerowsecurity from pg_class where relname='handoff_requests';  -> t | t
select rolsuper, rolbypassrls from pg_roles where rolname=current_user;                     -> t | t

begin;
  select set_config('handoff.tenant_ref','org_…FTHA',true);
  select count(*) from handoff_requests;                  -> 8
  select count(distinct tenant_ref) from handoff_requests; -> 2      <-- both tenants
commit;
```

Tenant A named, no predicate, both tenants' rows returned. The second line of defence is off.

**Blast radius.** Application-level predicates are present and correct — I could not find a single
cross-tenant leak through the API (§11) — so this is defence-in-depth that is absent rather than an
active hole. What makes it worth reporting is that nothing tells an operator to run `handoffd` as a
non-superuser, non-owner role. The property is tested under conditions the deployment documentation
never asks anyone to create.

---

### F-6 (Medium) — Repository status documents are stale in the direction of *under*-claiming, which breaks the honesty discipline just as badly

The project's stated standard (README.md:58-59) is: *"This table is the honest state as of the last
commit. If it disagrees with something else in this repository, this table is what was checked."* That
meta-claim is now false, and it is load-bearing — it instructs readers to trust the stale document over
the accurate ones.

| Location | Says | Actually |
|---|---|---|
| `README.md:41-56` | "Milestone H0… **Nothing here serves a request yet**"; core "Every crate is a stub"; conformance "Not started"; `sdk/ts` "Not started"; *"`handoffd` … listens on nothing, and pointing a client at it will not work"* | H1–H5 landed; `handoffd` serves and passes 24/24; both SDKs exist |
| `README.md:91`, `:150`, `CONTRIBUTING.md:55` | `ui/responder/` in the layout and licence table | Does not exist on disk |
| `CONTRIBUTING.md:99-100` | "`sdk/ts/` … **(Not present yet.)**", "`conformance/` … **(Not present yet — lands with H1.)**" | Both exist |
| `conformance/README.md:11-13` | "**23 Level 1 cases and 1 Level 2 case, all red**… confirm the report reads `0/23 passing`" | 24 L1 + C-17 = 25 files; 24/24 green |
| `conformance/GATE.md:32,38-40,64,71` | `cases: 23`, `0/23 passing`, enumerates 23 ids without C-24, "**23** Level 1 cases", "currently holds **22** files" | 24 L1 cases, 25 files |

`GATE.md` is the worst of these: it is the artifact whose whole purpose is to record that the gate was
measured, and it misstates the population it measured.

**`sdk/python/pyproject.toml:4` is the one that is outward-facing and over-claims:**
`description = "await human() for AI agents: page a real person, let them take the wheel, **resume the
run**"`. That is PyPI metadata making the single claim spec §1.3 forbids in capitals, contradicting
`sdk/python/README.md:36` ("It does **not** resume your execution") two directories away. Keywords are
still hackathon-era (`captcha`, `browser-automation`) and `Homepage` points at the hack deployment.

---

### F-7 (Medium) — Documents claim mechanical verification that no code performs

- `spec/fixtures/README.md:28,36` — *"These are asserted mechanically… `02-request-created.json`, …
  `19-tenant-policy.json` **validate against** `../schemas/*.schema.json`."* Nothing in the repo reads
  `spec/schemas/`. No JSON-Schema validator appears in any manifest. The byte-identity and
  parse-level fixture assertions in `sdk/ts/test/fixtures.test.ts` and
  `sdk/python/tests/test_fixtures.py` are real; *schema* validation is asserted by hand.
- `sdk/types/README.md` — *"treat the drift check — **not good intentions** — as the mechanism."* The
  check is genuinely sound (it exits non-zero on unreadable inputs, missing markers, or an empty
  region — it cannot pass vacuously), but no CI job runs it: `grep -rn "sdk/types\|check-drift"
  .github/workflows/` returns nothing.
- `managed/README.md:31` and `docs/cutover-plan.md:100-101` make `GET /v1/version` a checkable
  anti-drift gate. That route does not exist; `routes.rs:40-79` has no `version` route and `/v1/meta`
  returns `protocol_version` only, never the `handoff-core` crate version.

---

### F-8 (Low-Medium) — The receipt chain does not detect truncation of the most recent receipt, and nothing says so

**Claimed.** §9.4 — *"Altering any historical receipt MUST invalidate the chain head. This gives
tamper-evidence with no key management at all."* README.md:109 — self-hosting guarantees *"a complete
local audit trail."*

**True and correct as far as it goes.** With all three storage triggers dropped in a disposable copy —
i.e. the DB-compromise threat model that §9.4 layer 3 exists to survive — I confirmed the chain catches
what the spec says it catches, and one thing more:

| Tamper | Detected |
|---|---|
| Flip `decision.values.decision` approve→reject | **yes** — "the recorded digest does not match the receipt's content" |
| Rewrite `actor.principal_id` (impersonation) | **yes** |
| Delete a receipt from the middle of history | **yes** — "expected height 2, found 3" |
| **Delete the head (most recent receipt)** | **no** — chain verifies silently at a lower height |

`verify_chain` (`receipt.rs:715-773`) checks height contiguity from position 1, so excision anywhere
except the tail is caught. Tail truncation is undetectable without an external anchor — which is
inherent to hash chains, not a coding error, and the spec provides the mechanism ("The chain head MUST
be exportable"). The gap is that §9.4 never states the limitation, so a reader who takes
"tamper-evidence with no key management at all" at face value will over-trust an unanchored chain.
C-15 tests only altering.

---

### F-9 (Low) — Preserved hack docs carry the retired vocabulary the spec calls not-defensible

`spec/handoff-protocol-v0.1.md:1527` lists *"Your agent resumes exactly where it stopped"* as not
defensible. `examples/night-hack/RUNBOOK.md:84` instructs: *"Close on: 'It did not restart. **It
carried on from exactly where it stopped.**'"* Also `SUBMISSION.md:18` ("the agent picks right back
up") and `:19` ("a blocked agent **resumes** about 3 seconds after").

The directory is explicitly preserved as historical evidence and `examples/night-hack/README.md:44-61`
lists what it is not — but that list covers wire shape, durability, tenancy, and maintenance, **not
claim language**. A reader has no signal these lines are retired. One added sentence fixes it.

`DISCLOSURE.md:64` claims *"passing ten of ten assertions against production"*; the only assertion set
in the tree is `examples/night-hack/demo/agent.py:493-501`, which is **eight** asserts in an offline
`selftest()` over a hardcoded sample. I could not find the production artifact, so I mark the
discrepancy **suspicion** rather than fact — the run may have happened and simply not been preserved,
which is itself the thing DISCLOSURE.md exists to prevent.

---

### F-10 (Low) — Operational and hygiene items

- **`BACKLOG.md:187-190`** names which files hold working live credentials, which token is expired, and
  a working Cloudflare token by name. No literal values, so the repo's own `secret-hygiene` job
  (ci.yml:262-263, literal-value regexes) cannot catch it. In a repository intended to be published
  this is a roadmap for an attacker. Adjacent: `BACKLOG.md:196-198` records that live
  `handoff.omegas.dev` has no auth and "anyone holding the API URL can currently ring a phone," and
  `examples/night-hack/RUNBOOK.md:103-124` publishes the exact curl.
- **`SECURITY.md:51`** says the service "is **not deployed**"; `RUNBOOK.md:3`, `BACKLOG.md:136` and
  `DISCLOSURE.md:69` all say "Live: https://handoff.omegas.dev". Direct contradiction.
- **`docs/cutover-plan.md:96-103`** rates deploying to `handoff.omegas.dev` as *"Blast radius: none"*;
  that hostname already serves the night-hack demo, so step 1 replaces a running service.
- **Dev bootstrap cannot express two anonymous-link credentials.** `config.rs:195` assigns every
  link principal the synthetic id `"{tenant}::link"`, which is the table's primary key, so a second
  link token in one tenant aborts startup: `cannot seed principal org_…::link: duplicate key value
  violates unique constraint "handoff_principals_pkey"`. Failing closed at boot is the right
  behaviour; the limitation is that no C-8/C-6b variant involving two distinct link holders can be
  written against the reference deployment. Also, the `on conflict (secret_sha256) do update` clause
  means two entries sharing a token silently overwrite each other's tenant, kind, and role.
- **Test-suite portability (not reproduced — suspicion).** `core/crates/handoff-server/tests/` hardcodes
  ports 18101–18111 and defaults to `postgres://omega:omega@localhost:5432/postgres`, an
  omega-repo-specific role, with `.expect("psql is available")`. `ci.yml` declares no `services:`
  block and installs no Postgres, so `cargo test --workspace --all-features` (ci.yml:47-48) looks
  unable to pass as written. I did not run CI, and I did not run the full test suite locally (disk),
  so this is inference from reading, not a demonstrated failure.
- **Minor.** `managed/README.md:64` says 75 managed tests; `BACKLOG.md:52` says 76; the actual count is
  76. `BACKLOG.md:42` still lists "`spec/` v0.1 … The release gate for H0 is not met until it exists"
  as open work; it exists.

---

## 11. What I attacked and could **not** break

This section is the calibration for everything above. All of it was attempted against a live
`handoffd` on a disposable Postgres, with a second human principal added to tenant B so that
cross-tenant *answering* could be tested at all.

**Tenant isolation — I found nothing.** Every endpoint, tenant B's machine key *and* tenant B's human
key, against tenant A's identifiers:

```
PROBE                    as B_m as B_h  RESULT
GET  request             404    404     ok          GET  grant             403  404   ok
GET  request receipt     404    404     ok          POST resolve grant     403  400   ok
GET  request deliveries  404    404     ok          DEL  revoke grant      403  404   ok
GET  receipt             404    404     ok          GET  waiter signals    200  200   empty page
GET  receipts list       200    200     empty page  POST waiter reattach   200  200   empty
GET  chain-head          404    404     ok          POST ack signal        400  400   ok
GET  authorization       404    404     ok          GET  signal attempts   404  404   ok
POST redeem              404    404     ok          GET  delivery          404  404   ok
POST answer as B human   403    404     ok          POST redeliver         404  404   ok
```

No response leaked any of tenant A's identifiers or prompt text. The 403s on grants are
`requester_may_not_answer` applied by principal *type before* any lookup, so they are **not** an
existence oracle — I checked, and a real handle and a nonexistent one both return 403 for a machine and
404 for a human.

**`waiter_ref` collision across tenants.** `waiter_ref` is a client-chosen opaque string and therefore
guessable. Tenant B raised and answered a request onto the *identical* `waiter_ref` string as tenant A.
A saw only A's signal, B saw only B's. Correctly tenant-scoped. B's `reattach` on A's ref did not
disturb A's waiter — A's signal was still present and still unacked afterwards.

**I10, exactly-one-effect, under contention.** 24 threads released off a barrier redeeming one
single-use authorization with 24 *distinct* effect keys: **1×200 with `first_redemption: true`,
23×409**, one redemption recorded, state `spent`. C-13 tests this sequentially; it holds concurrently.

**I5/I1, first-writer-wins under contention.** 24 concurrent answers on one request: **1×200, 23×409,
exactly one receipt minted.**

**I7/I18, secrets — including at rest, which C-7 does not scan.** I ran the C-7 scenario with my own
canary, then grepped **every table in the database**, not just the API surface: zero hits. Zero in the
server log. The 422 does not echo the value; the sink refuses undeclared keys; the receipt carries
`{"provided": true}`. (Note that the reference sink accepts names and *discards* values by design —
`store.rs:2680-2683`, per §12 rule 5 — so the custody half of §12 is untestable by construction. That
is honest, but it means C-7's step titled "the value goes to the declared sink" proves only that the
server forgot it.)

**I8/I19, capability handles.** Server mints the handle from a CSPRNG and discards the client's
(confirmed: the returned handle differs from the one posted). Resolve with no credential → 401. Stale
`accepted_blast_radius_digest` → 409 `blast_radius_mismatch`. Machine principal → 403. Two resolves →
two different session refs and two different URLs. After the request went terminal → 404 "a terminal
request resolves nothing". After revocation → refused. And the resolved `wss://…?t=…` transport token
appears in **no** database table and **no** log line — I grepped every table for it. §11.2's "the only
place a resolvable address exists" holds.

**Receipt immutability at the storage layer**, as the application's own role: `UPDATE` refused,
`DELETE` refused, and **`TRUNCATE` refused** by a dedicated trigger. C-15 tests only update and delete;
truncate is the obvious bypass and it is already closed.

**I21, fail-closed.** `requires.v` of 2 and of 0 → `unsupported_requires_version`; unknown field type →
`unsupported_field_type`; unknown capability type → `unsupported_capability_type`; unknown `min_role`,
unknown `auth_strength` → `invalid_request`; calendar durations `P1M` and `P1Y` as a TTL → rejected per
§1.4, while `PT15M` is accepted. Nothing was created in any refused case.

**I15, requester ≠ decider.** Enforced by principal *type* in three independent places —
`routes.rs:124-132`, `plan.rs:94`, and `store.rs:1760` — with no role, scope, or config that can
override it. `Principal::may_answer` is total. I could not find a route that settles a request from a
machine principal.

**C-24 canonicalization.** `1e21`, `1e-7`, and `2^53` each → 422 naming the field; `0` accepted;
`2^53 − 1` accepted. Matches §1.4 (and not §18 — see F-4).

**Managed tier honesty.** I found **no fake**. `signer.rs:47-56` `attest()` returns
`Err(MissingDependency::ATTESTATION_KEY)` — no key, no stub, no fallback, consistent with README.md's
claim that attestation is structurally unavailable. `takeover.rs:36-51` errors on both mint and revoke
and explicitly refuses to fall back to a broadcast URL. Delivery refuses at construction rather than
pretending; a missing credential yields `Suppression::NotConfigured`, not a fake success. `fixtures.rs`
and `integration.rs` are `#[cfg(test)]` (lib.rs:70-73) and are not in the binary; `FakeControlPlane`
and `StaticJwks` are compiled but `main.rs:103-122` constructs only the HTTP implementations and there
is no env flag that swaps a fake in. JWT auth is real: `alg` pinned before key lookup, signature
verified before claims are read, DER rejected, bounded JWKS refetch, empty issuer refused. Managed
Postgres tests `.expect(...)` rather than skipping green.

**`docs/cutover-plan.md` is a plan, and nothing was cut over.** It says so at :3-6; `git log -- managed
docs` shows two commits; there is no deploy config for managed, no migration runner, no DNS script,
and no execution artifacts. (Its step-1 blast-radius rating is wrong — F-10 — but nothing ran.)

**Repo hygiene.** No tracked build artifacts, zero `/Users/` paths in tracked files, no AWS keys,
private keys, or bearer tokens in tracked content.

**Conformance suite integrity.** `case.rs:48-49` makes a missing requirement a failure, never a skip;
all ten action variants are dispatched; the dev hooks are real `handoffd` subcommands rather than no-op
stubs; the matcher engine explicitly refuses to let `all_equal` pass on an empty match set
(`expect.rs:249-252`) — the exact vacuity class this review was sent to hunt. I re-derived the
spec-§18 ↔ `conformance-map.json` ↔ case-file agreement and found **zero** mismatches on ids, levels, or
invariants, and every one of I1–I21 has at least one case. `check-drift.mjs` cannot pass vacuously.

**One residual vacuity risk I could not turn into a finding.** `Op::NoneEqual` (`expect.rs:264-274`)
passes when the path matches nothing, unlike its sibling `AllEqual` which explicitly refuses to. I
grepped every case file: `none_equal` is not used by any of the 25, so it is latent, not live. Same
shape as the `readSchemaNames` regex blind spot in `check-drift.mjs` — a trap set for a future case
author, not a current false pass.

---

## 12. Verdict

**Not ready to publish as-is. Close, and closer than the finding count suggests — but F-1 is a
correctness defect in the headline claim, and publishing a protocol whose one-sentence promise is
demonstrably breakable through its own reference implementation would spend the credibility this
project is otherwise unusually careful with.**

What I want to be clear about, because it should weigh at least as heavily as the list above: the
engineering here is better than the documentation around it. The parts that are genuinely hard — the
per-tenant hash chain, storage-level immutability including the truncate case, exactly-once redemption
under real contention, capability handles that never persist a resolvable address, secrets that are
absent from the database and not merely absent from the API, fail-closed version and type handling,
principal-type enforcement in three layers — all held under direct attack. I spent most of this review
trying to break those and did not. The conformance format is the best artifact in the repository: it is
readable without Rust, it refuses to skip, its matchers are designed against the specific vacuity that
makes tenant tests lie, and the one case I examined most closely (C-15's tamper probe) already carries
the guard my own first attempt at the same test forgot.

The pattern in the defects is consistent and worth naming, because it points at one fix rather than
ten: **the code is ahead of the documents, and the mechanisms that were supposed to keep them honest
are the ones not wired up.** The README describes a milestone five milestones behind. GATE.md
misstates the count it exists to record. The suite that is "the mechanism" runs only against a stub.
The drift check that is "not good intentions" runs in no job. `/v1/version`, offered twice as the
anti-drift gate, does not exist. Fixture schema validation is claimed as mechanical and is manual.
Each is individually small; together they mean a reader cannot currently use this repository's own
documents to check this repository's own claims, which is precisely the standard it sets for itself in
DISCLOSURE.md.

Before publication I would want, in order: **F-1** (scope the idempotency slot to the object in the
path — this is a small change and it wants a conformance case that fails before it, since none of the
25 covers key reuse across objects); **F-3** (either implement encryption at rest, or downgrade the
spec MUST to a SHOULD and delete the false doc comment — the doc comment must go either way); **F-4**
(correct §18's C-24 row to `2^53 − 1`); **F-2** (point one CI job at `handoffd`); and **F-6** (the
status documents, especially `pyproject.toml`'s "resume the run", which is the only outward-facing
overclaim I found and contradicts the one thing the project most insists it does not do).

**F-5** and **F-8** I would treat as documentation rather than blockers: say that `handoffd` must not
run as a superuser or the RLS layer is inert, and say that an unanchored chain does not detect tail
truncation. Both are true limitations honestly reachable from the existing design; neither is currently
stated, and this project's own standard is that the limitation gets stated rather than discovered.

None of these is architectural. I found no design decision I would argue with, and the open/closed line
is drawn where the README says it is — I checked the managed tier specifically for a fake that would
make it look further along than it is, and there isn't one.

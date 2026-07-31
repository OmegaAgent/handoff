# Hostile review 2 — Handoff v0.1

**Reviewer:** a second, independent adversarial pass, 2026-07-31. Worktree `/Users/noureddinbakir/handoff-v1`,
branch `v1/protocol` at `bd5e55c` (23 commits). Read-only on all source; the only file written is this one.
I wrote none of this code and I did not take the first review's findings, its verdict, or the fixes
made against it on trust.

**Method.** I reproduced the conformance baseline independently (`sh core/dev/run-conformance.sh` →
**25/25 passing, exit 0**), then stood up my own `handoffd` on a disposable Postgres with my own
bootstrap — including a second human principal in tenant B, which `core/dev/bootstrap.json` still
does not have — and attacked the HTTP API and the storage layer directly. Every finding marked
**demonstrated** has a reproduction I ran. Where I could not reproduce, I say **suspicion**. §D lists
what I attacked and could not break; it is the part that should calibrate everything else.

**The single most important result.** The reference server does not implement the receipt chain
digest that `spec/signing.md` §2.2 normatively specifies. Neither published SDK can verify a single
receipt `handoffd` mints. That is finding **R-1**, and it is release-blocking on its own.

---

## Summary of the previous round's findings

| | First round's claim | Status at `bd5e55c` |
|---|---|---|
| **F-1** | Idempotency slot omitted the object id | **FIXED — confirmed by direct attack** (§A) |
| **F-2** | CI never runs conformance against the real server | **FIXED THEN RE-BROKEN** — the job exists and is red-by-construction (**R-2**) |
| **F-3** | `resume_payload` stored in plaintext under a false doc comment | **FIXED — confirmed by direct attack** (§A) |
| **F-4** | §18's C-24 row contradicts §1.4 | **HALF FIXED** — §18 corrected, `spec/conformance-map.json:241` still says `1e20 → accepted` (**R-6**) |
| **F-5** | RLS inert under a superuser | **FIXED as documentation** (`SECURITY.md:62-77`, `core/dev/README.md:47-59`). Still weak as a *test* (**R-8**) |
| **F-6** | Status documents stale | **PARTLY FIXED, RECURRED IN THE SAME FILE** (**R-5**) |
| **F-7** | Documents claim mechanical verification nothing performs | **MOSTLY FIXED** — `/v1/version` removed, drift check wired, fixture validation wired for 5 of the 19; two of the five `spec/fixtures/README.md` claims still unwired (**R-9**) |
| **F-8** | Chain does not detect tail truncation, and nothing says so | **FIXED as documentation** (`spec/handoff-protocol-v0.1.md:1005-1015`) — verified honest and complete |
| **F-9** | Retired claim language in the hack docs | **FIXED**, with residue (**R-11**) |
| **F-10** | Credential map and unauthenticated curl in a repo about to be published | **FIXED IN `BACKLOG.md` ONLY — OPEN in `examples/night-hack/`** (**R-4**) |

---

## Findings, by severity

### R-1 (RELEASE-BLOCKING) — The reference server computes a different receipt chain digest than the normative spec, so no conforming verifier can verify any receipt it mints

**Claimed.** `spec/signing.md:9-19` — the receipt scheme exists so a receipt is *"verifiable by a
party who was never given a secret and must not be able to forge one — an auditor, a regulator, a
customer after they have left."* §2.2 is normative and gives the construction in two steps:

```
core_hash    = lowercase_hex( SHA-256( JCS( receipt EXCLUDING its `chain` member ) ) )
chain_input  = height ‖ LF ‖ prev_digest ‖ LF ‖ core_hash
chain.digest = "sha256:" ‖ lowercase_hex( SHA-256( chain_input ) )
```
with *"For the first receipt in a tenant, 64 ASCII zeros prefixed `sha256:`"*.
`spec/handoff-protocol-v0.1.md:1000-1002` makes the chain a MUST; `README.md:109` sells self-hosting
as *"a complete local audit trail."*

**True.** `core/crates/handoff-protocol/src/receipt.rs:665-688` implements a **one-step** construction
that is not §2.2:

```rust
pub fn canonical_form(&self) -> Result<Value> {
    let mut value = serde_json::to_value(self)?;
    if let Some(chain) = value.get_mut("chain").and_then(Value::as_object_mut) {
        chain.remove("digest");          // removes only `digest` — height and prev_digest STAY IN
    }
    Ok(value)
}
fn digest_with(&self, height: u64, prev_digest: Option<&Digest>) -> Result<Digest> {
    …
    digest_of(&probe.canonical_form()?)  // sha256(JCS(whole receipt incl. chain.height/prev_digest))
}
```

There is no `core_hash`, no `height ‖ LF ‖ prev_digest ‖ LF ‖ core_hash`, and for a genesis receipt
`prev_digest` is **absent from the object entirely** rather than 64 zeros. Both published SDKs
implement §2.2 exactly and correctly — `sdk/python/handoff/signing.py:230-261`,
`sdk/ts/src/signing.ts:234-262`.

**Reproduction.** Against a fresh `handoffd` (12 receipts minted through the ordinary answer path):

```
$ python3 -c "…verify_chain(rows)…"          # sdk/python, unmodified
real receipts: 11
python verify_receipt_chain per receipt: [False, False, False, False, False,
                                          False, False, False, False, False, False]
python verify_chain (whole tenant): False
```

Eleven out of eleven. Now recompute one receipt both ways:

```
receipt id: rcpt_01KYV8HX0PWVP63Q0JRGAZD6KJ  height: 2
stored digest                          : sha256:aee6e7b0bcb2fe3e5bbdb71d84fbb20af8a056842ea2a0373c92adf8810ef8d0
spec signing.md §2.2 two-step          : sha256:3edf48f06ac57e6f4029ad8b18bc36022459e26ca7fcaa5ac2adbd0da3f371f2   NO MATCH
rust canonical_form(chain minus digest): sha256:aee6e7b0bcb2fe3e5bbdb71d84fbb20af8a056842ea2a0373c92adf8810ef8d0   MATCH
```

And the genesis case:

```
genesis receipt height 1, `prev_digest` key present in the stored chain object? False
  stored     : sha256:b3e8e9e0fede11d6472d3d56d29036e11212549ea8fb93a4a9edab8ea320e5ec
  rust recomp: sha256:b3e8e9e0fede11d6472d3d56d29036e11212549ea8fb93a4a9edab8ea320e5ec
  spec requires prev_digest = sha256:0000…0000 for the first receipt
```

**The published fixtures do not resolve the disagreement — they demonstrate it.**

| fixture | stored digest matches §2.2? | matches the server's construction? |
|---|---|---|
| `spec/fixtures/08-receipt-decision.json` | **yes** | no |
| `spec/fixtures/09-receipt-policy.json` | **no** | **no** |

So `08` was authored to the SDK/spec construction and could not have come from the reference server,
and `09` matches neither construction — its stored digest is simply wrong. Both SDK test suites
already work around `09`: `sdk/python/tests/test_signing.py:212-226` and
`sdk/ts/test/signing.test.ts:276-302` **recompute** `policy.chain.digest` before asserting
`verify_chain([decision, policy])`, which is documented in-place but means no test anywhere asserts
that a published receipt fixture verifies as published.

**Why the suite cannot catch this.** C-15's two chain hooks are
`core/dev/conformance-profile.yaml:42-43` → `verify-receipt-chain.sh` → `handoffd verify-chain` and
`chain-tamper-check.sh`, which also calls `handoffd verify-chain`. The Rust implementation verifies
its own construction; that is a closed loop. The SDK suites check the SDK construction against
hand-authored fixtures — a second closed loop. Nothing in the repository ever hands a
**server-minted** receipt to a **spec-derived** verifier. Two internally consistent halves that
contradict each other, with no test spanning the seam.

**Blast radius.** This is the property the receipt scheme exists for. An auditor, regulator, or
departed customer implementing `signing.md` §2.2 — the documented, correct thing to do — concludes
that **every** receipt in a Handoff deployment is forged. A user of the shipped Python or TypeScript
SDK calling the documented `verify_chain()` on receipts pulled from their own `handoffd` gets
`False` for all of them. `sdk/python/README.md:123` presents `verify_receipt_chain()` as the way to
check integrity. Publishing a protocol whose reference implementation contradicts its own normative
signing document, in the one mechanism the protocol says needs no trust, is worse than publishing no
signing document at all.

One of the two must move. If the spec is right, `receipt.rs` must implement §2.2 and every existing
receipt's digest changes. If the implementation is right, §2.2, both SDKs, and fixture `08` must
change. Either way `09` is wrong today under both. A conformance case that hands a server-minted
receipt to an independent verifier is the thing that would have caught it, and there is no such case.

---

### R-2 (Release-blocking) — The one CI job that points the suite at the real server is red on every possible run, and its assertion string proves it has never executed

**Claimed.** `README.md:144-146` — *"A hosted service that cannot pass the open suite has a red
build, which turns 'we did not quietly fork the core' from an intention into a check anyone can
rerun."* `conformance/README.md:39-41` and `GOVERNANCE.md:80-83` rest governance on the same job.
This was F-2's fix.

**True.** `.github/workflows/ci.yml:347`:

```yaml
grep -q '24/24 passing' /tmp/conformance.out || {
  echo "::error::The suite exited 0 without reporting a full pass"; exit 1; }
```

There are **25** Level-1 cases at HEAD (26 case files; C-17 is the only Level 2). `report.rs:33`
prints `"{passed}/{total} passing"` and `main.rs:144` selects `c.level <= level` with level
defaulting to 1, so a perfect run prints `25/25 passing` and a `--level 2` run prints `26/26`. There
is no invocation in which `24/24` can appear.

**Reproduction.** My own baseline run, unmodified:

```
$ sh core/dev/run-conformance.sh
…
25/25 passing
conformance exit code: 0
```

`grep -q '24/24 passing'` against that output fails, so the job errors out. The job is red by
construction, forever, on a fully conforming server.

Provenance confirms it was written and never run: `git log -S"24/24 passing" -- .github/workflows/ci.yml`
gives `451ff78`, which predates `e2b5603` (the commit that added C-25) by twenty minutes, and three
subsequent `ci.yml` edits did not revisit it. Meanwhile `README.md:51` and `docs/cutover-plan.md:100`
were updated to `25/25`. The workflow also has never run anywhere: `on:` is `push: [main]` +
`pull_request`, `origin/main` is an unrelated landing-page history with no `.github/` directory at
all, `v1/protocol` is unpushed, and no PR exists.

**Secondary, same job.** The other half (ci.yml:320-329) asserts only `rc != 0` after running the
suite against `stub_501.py`. A runner that crashes on startup, is pointed at a dead port, or cannot
find the profile satisfies "The suite correctly reported non-conformance" identically. Demonstrated:
running the suite against a port with nothing listening gives `0/25 passing, exit 1` — indistinguishable
from the intended result. The stub is backgrounded at ci.yml:312 with un-redirected stdout and is
never asserted to be answering.

**Blast radius.** The single mechanism the README offers in place of trust still does not work, one
round after being reported. Same defect class as F-6: a hardcoded count that goes stale.

---

### R-3 (High) — Eight of the suite's hardest assertions are satisfied by a hook that does nothing, and two of its waiter assertions resolve to nothing and pass vacuously

The first review cleared two things that are no longer true. `Op::NoneEqual` was "latent, not live" —
it is now used six times and **two of the six resolve to an empty match set**. And "the dev hooks are
real `handoffd` subcommands rather than no-op stubs" is true of the shipped profile, but the *cases*
never require a hook to have done anything, so `true` and `false` satisfy them.

**Vacuous by empty match set** (`core/crates/handoff-conformance/src/expect.rs:264-274` returns
`Ok(())` on zero hits, unlike `AllEqual` at `:248-252` which explicitly refuses):

- `conformance/cases/c21_channel_content_never_decides.yaml:126-136` — *"and the waiter was never
  told anything was decided"*, `data[].type / none_equal: answered`. Nothing ever settles in C-21, so
  `GET /waiters/…/signals` returns `{"data":[]}` and the path yields zero hits. **Reproduction:**
  rewriting only that operator to `all_equal` reports `` `data[].type` matched nothing, so `all_equal`
  proves nothing``; and repointing the step at `/waiters/run%3AWAITER-THAT-NEVER-EXISTED/signals`
  still gives `PASS C-21`.
- `conformance/cases/c12_ack_is_idempotent.yaml:117-126` — same shape, `data[].id / none_equal`.
  Repointing that one step at a nonexistent waiter still passes. Lower severity: the earlier poll at
  `:66` is a positive control for the endpoint.

**Vacuous by exit-code-only hook expectations** — each of these asserts `exit_code: 0` (or
`exit_code_not: 0`) and nothing else, though `HookExpect` supports `output_matches` (`case.rs:426`):

| case | hook | stubbed with | result |
|---|---|---|---|
| C-15 `:108,:121,:161,:170` | `storage_update_receipt`, `storage_delete_receipt`, `receipt_chain_verify`, `chain_tamper_is_detected` | `false`, `false`, `true`, `true` | **PASS C-15** |
| C-21 `:80-89,:138-146` | `channel_inbound` | `true` (injects nothing) | **PASS C-21** |
| C-22 `:139-176` | `observe_page_state_change` | `true` (observes nothing) | **PASS C-22** |
| C-23 `:408-418` | `crash_between_state_and_event` | `true` (crashes nothing) | **PASS C-23** |

All four runs were performed against fresh disposable databases with mutated **copies** of the
profile. C-15 is the case §18 says *"must be asserted from the storage layer, not through the
application"*, and a claimant can pass it with four one-word shell commands. C-23's own text calls
its hook *"the only assertion that distinguishes a real transaction from an emit-after-commit that
happens not to have crashed yet."*

**And the real crash script cannot tell a crash from a normal answer.**
`core/dev/scripts/crash-between-writes.sh:72-96` guards only on `kill -0 "$INSTANCE"` and then
asserts `STATE_PRESENT == EVENT_PRESENT` — which is equally true when the answer commits normally and
both writes land. It never checks that the crash point was reached, although `handoffd` logs
`HANDOFF_CRASH_POINT reached: aborting between the state write and the event write`
(`store.rs:1933-1935`). **Reproduction:** a `$HANDOFFD` wrapper that `unset`s `HANDOFF_CRASH_POINT`
(fault injection simply not implemented), serves normally, and kills the child once the answer
commits, produces `crashed between the two writes; … they agree` and `HOOK EXIT=0`.

**And C-7's `logs` source is not load-bearing.** `runner.rs:512-516` discards the hook's exit code
and pushes its (empty) output; the `corpus.is_empty()` guard at `:520` never fires because seven
other sources contributed. C-7's own rationale says *"a deployment that cannot show its logs to the
suite has not demonstrated the property, and the case fails."* **Reproduction:** with `logs: "exit 1"`,
`PASS C-7 … 1/1 passing`. (The scan machinery itself is real: with `logs: "echo CANARY"` and `CANARY`
in the needle list the case correctly fails naming *"the deployment's logs"*.)

**Blast radius.** These are the assertions that cover the properties an implementer cannot verify
from the HTTP surface — storage-level immutability, transactional atomicity, channel-content
non-authority, secret absence from logs. A third-party claiming conformance can satisfy all of them
without implementing any of them. Since `conformance/GATE.md` is the artifact that turns "we ran the
suite" into evidence, this is the difference between a governance instrument and a checkbox.

---

### R-4 (High, and blocking for *publication* specifically) — F-10's operational hygiene was fixed in `BACKLOG.md` and left intact everywhere else

`BACKLOG.md:206-210` is properly redacted. In the same tracked tree, about to become public:

- `examples/night-hack/RUNBOOK.md:33` — `export SPRITES_API_TOKEN=<from ~/hipocampus/.env>`, naming
  the private file that holds working credentials. `PAGING-UX.md:52` does the same for a Slack bot token.
- `examples/night-hack/RUNBOOK.md:103-120` — the complete `curl -X POST
  https://handoff.omegas.dev/v1/requests … "page":true` that rings a real phone, plus a healthz probe
  at `:11`, a request-listing probe at `:94`, and `:137-138` stating there is *"no auth yet beyond
  unguessable request ids."* `BACKLOG.md:216-218` still records that *"anyone holding the API URL can
  currently ring a phone."*
- `examples/night-hack/SUBMISSION.md:7` a personal email; `:24` *"**Demo Link (no auth):**
  https://handoff-human.fly.dev/try"*.

The repo's own `secret-hygiene` job (ci.yml:396) matches literal values, so none of this is caught.
Publishing an unauthenticated live endpoint together with the exact request that pages a human is a
denial-of-service handed to the first reader, and naming the credential file is a target list.

---

### R-5 (High) — `conformance/GATE.md` went stale again, by the identical mechanism it documents about itself, one commit after being fixed

Ground truth measured at HEAD: **26** case files, **25** Level 1 (`grep -h "^level:" conformance/cases/*.yaml | sort | uniq -c` → `25 level: 1`, `1 level: 2`).

| file:line | says | true |
|---|---|---|
| `conformance/GATE.md:32` | `cases: 24 from conformance/cases` | 26 files, 25 L1 |
| `:38` | `0/24 passing` | the run prints `N/25` |
| `:39-40` | enumerates 24 ids, no C-25 | 25 |
| `:64` | "§18 and the map both enumerate **24** Level 1 cases" | both enumerate 25 |
| `:71` | "All 24 Level 1 cases now exist on disk and all 24 run." | 25 |
| `core/dev/README.md:4` | "run all **24** cases" | 25 |
| `spec/CHANGELOG.md:73` | "tested by the **24** Level 1 conformance cases … C-18–C-24" | 25, C-18–C-25 |
| `spec/handoff-protocol-v0.1.md:43` (§1.2) | "all **24** Level 1 conformance cases of §18: … C-18 through C-24" | **contradicts §1424 (§18) of the same document**, which says 25 and C-18–C-25 |

`GATE.md:74-76` says of itself: *"This file was itself found stale by the hostile review — it
recorded 23 cases after C-24 landed… A document whose one job is to hold a number, holding the wrong
number, is the same failure it was written to guard against."* It now holds the wrong number after
C-25 landed.

The §1.2-vs-§18 disagreement is the serious half: two normative clauses of the same specification
define Level 1 differently, and an implementer reading §1.2 ships without C-25 — the case that exists
because the reference implementation had that exact defect. `spec/CHANGELOG.md` contains no `C-25`
entry at all, though `GOVERNANCE.md` requires spec text and case to land together, and the §18 C-24
row was also materially corrected without a changelog entry.

---

### R-6 (High) — F-4 is only half fixed: the machine-readable artifact still carries the contradiction

`spec/handoff-protocol-v0.1.md:1463` (§18, C-24 row) was corrected to `9007199254740991`.
`spec/conformance-map.json:241` still reads *"Answer with `0`, `1e-6`, and **`1e20` → accepted**"*,
which §1.4 rule 2 (`:93`) requires **rejected** and which the implementation does reject (I confirmed:
`1e20`, `1e21`, `1e-7` and `2^53` each → `422 answer_validation_failed`; `0` and `2^53 − 1` accepted).
§18 declares the map normative — *"duplicated in machine-readable form at `conformance-map.json` so
that an implementation can iterate it without parsing this table"* — and the map's `generated_from`
names §§17-18. The exact contradiction F-4 identified survives, in the artifact machines read.

The same file's inverse index is also out of date with its own forward half: deriving from `cases[]`
gives `I1 → [C-14, C-2, C-25]`, `I10 → [C-13, C-25]`, `I20 → [C-1, C-19, C-2, C-25]`; the stored
`invariants[]` entries omit C-25 in all three.

---

### R-7 (Medium-High) — `conformance/README.md` claims the reference server passes C-17; it structurally cannot

`conformance/README.md:11` — *"**Status: 25 Level 1 cases and 1 Level 2 case. The reference server
passes all of them.**"*, and `:15` *"Against the reference implementation they are green."*

`c17_continuation_returned_verbatim.yaml:22-34` requires `GET /meta` to report
`conformance_level == 2` with `extensions` containing `continuation`. `routes.rs:1469` derives that
from `state.config.continuation_supported`, which is `false` at `config.rs:94` and — correctly, and
by design — *"Deliberately not an environment switch."* `routes.rs:168` refuses any raise carrying
`resume_payload`. C-17 is unpassable by this build, which is the honest consequence of F-3's fix.
`README.md:51` gets this right (`handoffd` passes 25/25); `conformance/README.md` and `README.md:43`
("passes the conformance suite in full") do not. This is an overclaim about a security-relevant
capability the project deliberately does not have.

---

### R-8 (Medium) — The RLS test asserts per-table isolation on eight tables that are empty in its own fixture

`core/crates/handoff-server/tests/suite/isolation.rs:180-215` asserts, per table, that a query
*without* a tenant predicate returns the same rows as one *with* it. That comparison can only fail
when the **other** tenant owns a row in that table. The fixture (`:154-174`) performs one raise and
one answer per tenant, and the anti-vacuity guard at `:219-228` checks only `handoff_requests`.

Reproducing the fixture state and counting rows per tenant per table:

```
handoff_delivery_attempts  0 0   handoff_grants          0 0   handoff_channel_messages 0 0
handoff_callback_attempts  0 0   handoff_grant_sessions  0 0   handoff_observations     0 0
handoff_redemptions        0 0   handoff_sinks           0 0
```

Eight of nineteen carry no evidence. Demonstrated directly: with a genuine least-privilege role
(`rolsuper or rolbypassrls` = `f`, exactly as the test constructs), disabling RLS on
`handoff_grants` leaves `assert_eq!(without_predicate, with_predicate)` holding, because both sides
are `[]`. Only once tenant B actually owns a row — which the fixture never creates — does the
assertion fail.

The role check itself (`:230-241`) is real and I could not defeat it; the weakness is coverage, not
the guard. The correct fix is a per-table assertion that the other tenant owns at least one row.

---

### R-9 (Medium) — Residual F-7: two of the five "asserted mechanically" fixture claims are still asserted by hand, and the published example profile cannot pass C-24

`spec/fixtures/README.md:26-28` says the five claims that follow *"are asserted mechanically, and a
change that breaks one is a bug in the fixture set."* Claim 3 is now genuinely wired
(`ci.yml:246-252`, and I confirmed independently that it is falsifiable — see §D). Claims 4 and 5 are
not: nothing validates `use-cases/*.json` against `request.schema.json`
(`grep -rn "request.schema" --include=*.py --include=*.ts --include=*.rs --include=*.mjs` returns only
`ci.yml`), and only the `kind` half of claim 5 is asserted
(`sdk/python/tests/test_fixtures.py:_is_permitted_kind`) — nothing checks for a secret value or a
resolvable capability address.

Separately, `core/crates/handoff-conformance/profile.example.yaml` — the file
`conformance/README.md:21-22` sends every independent implementer to — defines no `canonicalize`
hook, which `c24_canonical_numbers_are_deterministic.yaml:75` requires. Per
`conformance/README.md:25` (*"A case whose requirements are missing **fails**; it is never skipped"*),
anyone starting from the published example fails C-24 for a missing hook rather than a protocol
defect. The same file says *"three requirements are below the HTTP API"* at `:75` while defining ten,
and `:131` says `spec/signing.md` *"is not yet published"* — it is 440 lines in this tree and §15
cites it as normative. `core/dev/README.md:21` says "Five requirements cannot be asserted over HTTP"
above a six-row table that omits `logs`.

---

### R-10 (Medium) — `cargo test --workspace` is flaky by construction; `managed/` and the Python SDK tests run in no CI job at all

`bd5e55c` genuinely fixed the missing Postgres service (ci.yml:33-48, 58-59), and the tests cannot
skip-green — every dependency failure is a hard panic (`harness/mod.rs:189` `.expect("psql is
available")`, `:131`, `:140`). Both are real improvements.

But three ports are duplicated across tests compiled into one parallel binary: 18108
(`transitions.rs:337` / `callbacks.rs:106`), 18109 (`transitions.rs:418` / `deliveries.rs:37`), 18110
(`transitions.rs:472` / `deliveries.rs:208`). Forcing the 18109 pair to overlap failed **2 of 3**
runs with `ConnectError(… 127.0.0.1:18109, ConnectionRefused)` at `harness/mod.rs:223`. Worse than
flakiness: `harness/mod.rs:134-140` polls `GET /meta` and never calls `try_wait()` on the child, so a
second server that fails to bind can have its readiness probe answered by the *first* one — and the
test then asserts against the wrong database and passes. `run-conformance.sh:101-112` documents and
guards this exact hazard; the Rust harness does not.

And two whole test suites are outside CI. `grep -n "managed" .github/workflows/ci.yml` returns
nothing — `managed/Cargo.toml:11` is a separate `[workspace]` and the Rust job is pinned to
`working-directory: core`, so `managed/README.md:64`'s "76 tests" (the count is exact) is verified by
nobody. The `python-sdk` job (ci.yml:98-137) runs `compileall`, `ruff`, an import check and
`python -m build`, and never invokes `sdk/python/tests/` — which is where the receipt-chain
assertions of R-1 live. `sdk/ts`'s `npm test` does run (80/80 locally).

Minor, same area: the `no-fork-markers` job is named *"no `[patch]`, no path dependency on a
published crate"* (ci.yml:82) and its body (`:90-96`) greps only for `[patch]`. `CONTRIBUTING.md:85`
says "CI greps for this" about the whole rule. `managed/Cargo.toml:34-37` is exactly the path-dependency
shape the job's own comment calls "the moment a fork begins".

---

### R-11 (Low-Medium) — Remaining document contradictions and one uncorrected number

- `DISCLOSURE.md:64` still asserts *"passing ten of ten assertions against production"*.
  `examples/night-hack/README.md:67-71` explicitly retracts it (*"eight asserts … Treat the number as
  unsupported"*), and `grep -c "assert " examples/night-hack/demo/agent.py` → **8**. Two tracked
  documents state contradictory facts about one number, and the uncorrected one is the file
  `README.md:196-199` holds up as the project's honesty standard.
- `examples/night-hack/README.md:63` cites *"Spec §19"* for the not-defensible claim-language list;
  §19 is "Versioning and extension policy" and the list is Appendix B (`:1540-1552`). The sentence
  added to fix F-9 points at the wrong section.
- `README.md:41` "there is no hosted service" vs `DISCLOSURE.md:69` "**Live:**
  https://handoff.omegas.dev" vs `SECURITY.md:51-54`'s hedge ("has existed… do not assume it is gone
  until someone has checked") vs `docs/cutover-plan.md:106` "that hostname already serves the
  night-hack demo".
- `README.md:57` "Nothing has been published to crates.io, npm, or PyPI" vs `README.md:83` "`core/`…
  **Published to crates.io**" vs `NOTICE:21` "(sdk/python, **published to PyPI as `handoff-human`**)"
  vs `GOVERNANCE.md:51` and `sdk/python/README.md:7`. `README.md:59-61` instructs readers that the
  status table wins — which points them at the wrong answer, since the MIT grant `NOTICE` relies on
  presupposes publication.
- `sdk/python/pyproject.toml:4` — the "resume the run" overclaim **is gone**, which was F-6's one
  outward-facing item. `:9` keywords still `captcha`, `browser-automation`; `:20` `Homepage` still
  points at the unauthenticated hackathon deployment `SECURITY.md:51-53` describes.
- `managed/README.md:24` "~2k LOC of adapters" against ~2,850 non-test lines (`find managed/crates
  -name "*.rs" | xargs wc -l` → 4,486 total). Its "76 tests" and "five need a local Postgres" are exact.

---

### R-12 (Low) — `handoffd verify-chain` prints `OK` on the same tenant it has just reported `BROKEN`

Exit code is correct (`1`), but the output is not:

```
$ handoffd verify-chain
org_01K3M7…FTHA: BROKEN — a stored receipt no longer parses: … is not a valid identifier
org_01K3M7…FTHA: OK — 11 receipt(s), head sha256:b814e77d… at height 11
EXIT CODE = 1
```

There are 12 rows; the `OK` line summarizes the parseable subset and names a head that is not the
head. An operator (or a script) grepping for `OK` in the output of the tamper-evidence tool sees
`OK`. `core/dev/scripts/verify-receipt-chain.sh` is `exec handoffd verify-chain`, so C-15 reads only
the exit code and never sees this.

---

### R-13 (Low) — `handoff_receipts.decision` is outside the hash chain, contradicting §9.4's wording, with near-zero blast radius

§9.4 — *"Altering any historical receipt MUST invalidate the chain head."* With all three storage
triggers dropped (the layer-3 DB-compromise model):

```
update handoff_receipts set decision = jsonb_set(decision,'{values,decision}','"reject"')
  where height = 2 …                                          -> UPDATE 1
handoffd verify-chain -> OK — 12 receipt(s), head sha256:7cd4a62… at height 12    (exit 0)
```

The chain digest is taken over `body`; the separate `decision` column is not covered. Tampering
`body` **is** detected (`BROKEN — the recorded digest does not match the receipt's content; it has
been altered`), and the API reads only `body` (`store.rs:530, 2048, 2068, 2086`) — I confirmed the
API still serves `"approve"` while the column says `"reject"` — so nothing reads the tampered value.
`migrations.rs:254-257` says the column exists *"so that the storage-level immutability probe of C-15
has a real column to aim an `UPDATE` at"*. It is a write-only column that no code reads, so this is a
wording defect in §9.4 rather than an exposure — but a row in `handoff_receipts` can be altered
without invalidating the head, which is precisely what §9.4 says cannot happen.

---

### R-14 (Suspicion, not reproduced) — two latent traps in the matcher and one shallow check

- `signing.rs:294-310` `CarriesNoResolvableUrl` tests only `contains("http://") || contains("https://")`.
  The one resolvable address a conforming Handoff system mints is `wss://` — I confirmed live:
  `wss://127.0.0.1:8443/surfaces/hs_0V57TJ3HWG6VCPWND4TQPA2D76?t=KMDNFYH8Q89R1YWYN4SH` — and C-8's own
  matcher accepts `^(https|wss?)://`. A callback leaking that URL, bearer token and all, passes the
  check named "carries no resolvable URL". Not live today (no case exercises it against a `wss` leak).
- `signing.rs:312-327` `TenancyNotDerivableFromBody` uses `doc.get(key)` — top level only; a nested
  `decision.org_id` passes.
- `expect.rs:135-147` `Op::Exists(false)` passes on any mistyped path. Every current use has a
  sibling matcher proving the parent resolves, so latent rather than live — the same trap class as
  `NoneEqual`, which was latent in the first review and is live now.

---

## §D. What I attacked and could **not** break

This is the calibration for everything above. All of it was run against a live `handoffd` on a
disposable Postgres with a second human principal in tenant B.

**F-1 is genuinely fixed, including the anonymous-link variant the first review could only suspect.**
The replay slot now carries the object (`http.rs:118-149`), the id is parsed before the replay check
at all eight call sites, the object is in the primary key (`migrations.rs:547-568`) and bound in both
the replay and remember queries (`store.rs:2965-3015`). Key reuse across two objects, same body, same
principal:

```
op          1st call          2nd call (same key, different object)     verdict
answer      names R1          names R2, distinct receipt + auth         correct
cancel      names C1          names C2                                  correct
amend       names A1          names A2                                  correct
supersede   names S1          names S2                                  correct
escalate    names E1          names E2                                  correct
reassign    names N1          names N2                                  correct
attempt     names T1          names T2                                  correct
grade       names D1          names D2                                  correct
answer as an anonymous-link principal (link_only): names L1, then L2, two distinct receipts
```

And a *genuine* retry still replays: re-answering R1 with the same key returned the identical
receipt id, so the fix did not trade the defect for a worse one. I enumerated every route in
`routes.rs:39-80` and every `slot(` call site; the eight per-object mutations all pass their id, and
`raise` deliberately passes an empty object (`store.rs:3398-3421`) because it is the call that
creates one. I found no mutation whose slot still omits its object.

**F-3 is genuinely fixed, and I could not get continuation state persisted by any route.**
`resume_payload` at the top level of a raise → `400 invalid_request` with an explanation that names
the missing encryption. Inside a `continuation` object → `400` ("`continuation` is not a field of a
raise"). Via `amend` → refused. Via `reattach` → ignored, `200` with no payload stored. `resume_ref`
is accepted and stored, which is the documented intent (it is a pointer the runtime owns). The
advertised level is derived from the same config the refusal reads (`routes.rs:1466-1475`,
`config.rs:94`), so the level and the behaviour cannot drift — and `continuation_supported` is
deliberately not an environment variable.

**Tenant isolation — I found nothing.** Every endpoint, as tenant B's machine key *and* tenant B's
human key, against tenant A's identifiers: request `404/404`, request receipt `404/404`, deliveries
`404/404`, receipt `404/404`, chain-head `404/404`, authorization `404/404`, redeem `404/404`, cancel
`404/404`, answer `404` (human) / `403` (machine, by principal type before lookup), grant `404/404`,
waiter signals `200` with `{"data":[],"has_more":false}`. No response leaked any of tenant A's
identifiers or prompt text.

**I15 — I could not settle a request from a machine principal.** `answer` → `403
requester_may_not_answer`; a disposition → refused; `POST /sinks/{ref}/values` → `403 "a
service_account principal may not perform this operation"`; resolving a capability grant → `403`.
Enforced by principal *type* in three independent places with no role, scope, or setting that reaches it.

**I10, exactly-one-effect, under real contention.** 24 threads off a barrier redeeming one single-use
authorization with 24 distinct effect keys: **1×200 with `first_redemption: true`, 23×409**, one
redemption recorded, state `spent`. Sequentially: re-redeeming the *same* effect key returns
`200 first_redemption:false` (correct retry semantics); a *different* effect key returns
`409 authorization_spent`.

**I5/I1, first-writer-wins under contention.** 24 concurrent answers on one request: **1×200, 23×409,
exactly one receipt minted.**

**I7/I18, secrets — including at rest.** A raw value for a `secret` field → `422
secret_value_not_permitted` naming the field and not echoing the value. `{"provided": true}` accepted;
the sink accepts only declared names (`nope` → `400`) and discards values. I then grepped **every
text/varchar/json/jsonb column of every table** for the canary: zero hits, and zero in the server log.
The only place my canary appeared was `handoff_requests.metadata`, where I had put it myself — that
is caller-supplied free-form data, not a declared secret, and I do not count it.

**I8/I19, capability handles.** The client-declared handle is discarded and replaced by a
server-minted one (`hg_1H1G6R01GMS9PJNS849FGK2QVW` ≠ the one I posted). No `url` key in the
declaration. Resolve with no credential → `401`; as a machine → `403`; with a stale
`accepted_blast_radius_digest` → `409 blast_radius_mismatch`; twice → two different `wss://` addresses;
re-reading the grant afterwards carries no URL; after the request went terminal → `404 "a terminal
request resolves nothing"`; after revocation → refused. And the resolved transport token
`KMDNFYH8Q89R1YWYN4SH` appears in **no** database column and **no** log line — I scanned every column
of every table. §11.2's "the only place a resolvable address exists" holds.

**I13/§15, callbacks.** Signatures verify against **both** active secrets and against neither of a
wrong one, exactly as `signing.md` §1.4's rotation-overlap rule requires; the canonical string
`1 ‖ LF ‖ t ‖ LF ‖ delivery_id ‖ LF ‖ sha256(body)` reproduced byte-for-byte; every required header
present; no secret in the body. (The callback half of `signing.md` is correct and interoperable —
which makes R-1, the receipt half, more surprising, not less.)

**Receipt tamper-evidence, within its stated limits.** With all three storage triggers dropped:
altering `body` is detected and named (`the recorded digest does not match the receipt's content; it
has been altered`); deleting the head is not, which §9.4:1005-1015 now states plainly and correctly,
and which I re-confirmed. A forged appended receipt was detected — though only because I computed its
digest with the *spec's* construction, which is R-1. As the application's own role, `UPDATE`,
`DELETE` and `TRUNCATE` on `handoff_receipts` are all refused by statement-level `BEFORE` triggers,
so even a zero-row `UPDATE … WHERE true` is refused.

**A receipt does not name a delivery that was never sent.** On a three-rung ladder producing one
dispatched in-app delivery and eight suppressed email/Slack deliveries, the receipt's `via` named the
in-app delivery with `grade_reached: acted`. Grading a suppressed email `acted` → `400` ("`acted` is
not a grade a channel reports"); `seen` or `delivered` on a terminal suppressed delivery → `400`. The
new grade route refuses what its own doc comment says it refuses, and its residual risk is stated
rather than glossed.

**I21, fail-closed.** `1e20`, `1e21`, `1e-7`, `2^53` → `422` naming the field; `0` and `2^53 − 1`
accepted. Unknown `on_expiry`, unknown field key, over-long capability handle, malformed `routing`,
malformed `Target` — each refused with a typed code and nothing created.

**Conformance suite integrity, beyond R-3.** All operators except `NoneEqual` and `Exists(false)`
route through `single()` (`expect.rs:127-132`), which errors *"`{path}` is absent"* on zero hits;
`AllEqual` explicitly refuses an empty set; `set_equals` compares length *and* identity and refuses a
superset. `Scan` refuses an empty needle and an empty corpus and treats an unreadable artifact as a
failure. Requirements are a failure, never a skip (`case.rs:48-49`). `audit_coverage`
(`lib.rs:151-206`) reads the case **files from disk** — the vacuity the first review found there is
genuinely closed — and I re-derived 26 files ↔ 26 map entries ↔ 26 §18 rows with zero mismatches on
id or level. `chain-tamper-check.sh` remains the best-guarded script in the tree: it pre-verifies the
untouched copy, picks `min(height)` **within one tenant**, mutates a member every receipt must carry
so `jsonb_set` cannot no-op, guards on `returning id` being exactly one row, and re-reads to confirm
the bytes changed. I could not construct a false pass. C-24's `canonicalize` hook cannot be stubbed:
it pins published byte counts *and* digests plus an exact-match regex on member ordering.

**C-25 is genuinely load-bearing.** Simulating the pre-fix behaviour by forging the idempotency row
for the second object with the first object's stored response — the exact observable of the old bug —
makes `c25:103-104,121-127` (`request.id / same_as: r2`) go red, backed by `receipt.id not_equals`
and `authorization.id not_equals`. Its final step is a genuine positive control that replay on the
*same* object still works, so a "mint a new receipt for every retry" fix would also go red.

**Two CI checks that were written from scratch rather than transcribed are real and falsifiable.**
The fixture schema validator (ci.yml:212-271) fails on eight distinct corruptions of a copied
fixture — missing `required`, wrong type, `additionalProperties`, a nested `$defs` violation, a
cross-file `$defs` enum violation, a pattern violation, and a missing file. The schemas genuinely
constrain (`type`, `required`, `additionalProperties: false`, cross-file `$id` resolution).
`check-drift.mjs` reports `87 schemas` (an independent YAML parse agrees) and fails five ways
including `exit 2` when the region markers are missing and `exit 2` when the regex reads nothing — the
"empty set is trivially equal" vacuity is explicitly guarded at `:166-174`. Its documented limitation
(name sets only; a renamed field or changed type passes) is stated honestly in
`sdk/types/README.md:108`, though the CI step name "Declared types still cover the spec" oversells it.

**Managed tier honesty.** I re-checked the first review's conclusion and agree: no fake. `signer.rs`
`attest()` returns `Err(MissingDependency::ATTESTATION_KEY)`; `takeover.rs` errors on mint and revoke
and refuses to fall back to a broadcast URL; delivery yields `Suppression::NotConfigured` rather than
a fake success; `FakeControlPlane` and `StaticJwks` are compiled but never constructed by `main.rs`,
and no env flag swaps one in.

**Repo hygiene, machine-checkable half.** No tracked build artifacts, no AWS keys, no private keys,
no bearer tokens in tracked content. The only `/Users/` paths in tracked files are the two inside
`docs/hostile-review.md` itself — which is how that file falsifies its own `:412` claim of "zero
`/Users/` paths in tracked files" by being committed.

---

## Verdict

**No. This is not ready to publish as an open protocol.**

**R-1 blocks it by itself, and would block it even if every other finding here were false.** The
reference implementation computes a receipt chain digest that `spec/signing.md` §2.2 does not
specify, and that neither published SDK can reproduce. Eleven of eleven receipts minted through the
ordinary answer path fail `verify_chain()` in the project's own Python SDK. A protocol whose central
tamper-evidence mechanism — the one the spec says needs no keys and no trust, the one sold to
"an auditor, a regulator, a customer after they have left" — cannot be verified by an implementation
written from its own normative document is not a protocol yet. It is two implementations that
disagree, each with a test suite that only ever talks to itself. Publishing it means every
independent verifier's first correct action produces a false alarm on every receipt, and the
project's credibility is spent on the exact claim it is most careful about everywhere else.

**R-2 blocks it too**, for the reason the first review already gave and which has not changed: the
README offers the conformance suite in place of trust, and the only job that points that suite at the
real server cannot go green. That it is broken on a *hardcoded count* — the same failure mode as F-6,
in a file edited three times after the count changed — is evidence that the fix was written and never
executed, which is the pattern this project's own standards exist to prevent.

**R-3 blocks it as a governance instrument.** Eight of the assertions covering the properties an
implementer cannot check over HTTP — storage immutability, transactional atomicity, channel-content
non-authority, secrets absent from logs — are satisfied by `true` and `false`. `GATE.md` exists to
turn "we ran the suite" into evidence; today a claimant can produce that evidence without
implementing the properties.

**R-4 blocks *publication* specifically**, independent of correctness: a tracked, unauthenticated
live endpoint with the exact phone-ringing curl, plus two files naming the private credential store.

In order, what must close before publication:

1. **R-1** — reconcile `receipt.rs` and `signing.md` §2.2, fix `09-receipt-policy.json`, and add a
   conformance case that hands a **server-minted** receipt to a **spec-derived** verifier. Without
   that last part the seam is unguarded again the moment either side moves.
2. **R-2** — make the gate compute the count rather than hardcode it, and assert the stub is
   answering before concluding anything from a non-zero exit. Also push the branch, because a
   workflow that has never run is not a check.
3. **R-3** — require evidence, not exit codes, from every hook step; make `NoneEqual` refuse an empty
   match set as `AllEqual` does; make `ScanSource::Logs` fail on a non-zero hook; make
   `crash-between-writes.sh` assert the crash point was reached.
4. **R-4** — redact `examples/night-hack/`, or do not ship it.
5. **R-5, R-6, R-7** — one count, derived once. `GATE.md` and §1.2 should read the number rather than
   hold it, since holding it has now failed three times.

**R-8** through **R-13** I would not block on, but R-8 and R-13 are the same category the project
itself names: a limitation that should be stated rather than discovered.

What I want on the record, because it should weigh as heavily as the list above: the parts that are
genuinely hard held under sustained direct attack, and several of them are better than the first
review found them. The F-1 fix is complete across all eight mutations *and* the anonymous-link case
that the first reviewer could only mark as suspicion. The F-3 fix chose refusal over inventing a
key-management scheme, derived the advertised level from the same value, and refused to make it an
environment switch — that is the right call and it is implemented the right way. Exactly-once
redemption and first-writer-wins both hold under 24-way contention. Secrets are absent from every
column of the database, not merely from the API. The one resolvable address in the system exists in
no row and no log line. The callback signing scheme is correct and interoperable. Tenant isolation
did not leak anywhere I could reach. `chain-tamper-check.sh` and `audit_coverage` are both stronger
than the tests around them, and C-25 is a real regression case that would have caught the bug it was
written for.

The pattern in the defects is the same one the first review named, and naming it again is the useful
thing: **the parts that were built are sound, and the parts that were meant to keep them honest are
the ones that do not run.** The chain verifier verifies itself. The SDK verifies fixtures it was
written against. The conformance job greps for a number that can no longer occur. The hooks assert
exit codes rather than effects. `GATE.md` holds a count instead of reading one. Each is small; the
sum is that this repository cannot currently be used to check this repository's own claims — and R-1
is what happens when nothing spans the seam between two things that were each verified against
themselves.

# Hostile review 4 — re-verification against the acceptance criteria

**Reviewer:** the third hostile reviewer, returning to re-verify. 2026-07-31, against
`638c72a85321095d434f07c07482b27da27a6838` on `v1/protocol`, 62 commits from base `5336bb3`, clean
tree, nothing moving underneath. Read-only on all source; the only file written is this one.

**Method.** Measured against `docs/review-3-acceptance.md`, which I wrote blind before any fix
landed. For each finding I ran the reproduction from `review-3.md` verbatim, plus at least one
sibling shape from the same class that no report had named, and I checked the **anti-criterion** —
the plausible fix that would close my reproduction while leaving the property untrue. I rebuilt the
two binaries at this HEAD first, because the shipped `handoff-conformance` binary was older than the
sources and would have measured the wrong tree.

**Result in one line.** Thirteen of the fourteen findings are closed, several beyond what I asked
for. One is not, and it is the one criterion I wrote in the most explicit terms I had.

---

## The one thing that blocks publication

### N-5 — the workflow has still never run

My criterion, written blind:

> **Closed when.** There is **an actual workflow run**, on a pushed branch, that is green. Not a
> local reproduction, not a claim that the jobs would pass — a run.
>
> **Not closed by.** Fixing the two red jobs and asserting they now pass. The whole finding is that
> this repository has never executed its own CI; three rounds of "written and never run" is the
> pattern.

Measured at this HEAD:

```
$ gh api repos/OmegaAgent/handoff/actions/runs --jq .total_count      0
$ git ls-remote --heads origin                                        5336bb3  refs/heads/main
```

Zero runs. The branch is unpushed and `origin/main` is still the unrelated night-hack history. Every
one of the fourteen jobs remains a prediction the repository has not yet contradicted.

**Everything else in N-5 is genuinely fixed, and I verified each locally:**

| N-5 sub-finding | State |
|---|---|
| `python-sdk` red on a clean tree | **fixed** — `ruff==0.6.1` pinned, `[tool.ruff]` config present, `ruff check --no-cache .` → *All checks passed*; `pytest tests/ -q` → 104 passed |
| `fixtures-validate` red on a clean tree | **fixed** — extracted the step's heredoc verbatim and ran it against a clean copy of `spec/`: exit 0 |
| the address check was a no-op | **fixed and falsifiable** — it now uses `finditer`/`group(0)`, and carries its own must-fire self-test (*"fires on 5 known-bad, silent on 7 known-good"*). I planted `https://relay.attacker.example.net/live?token=…LEAKED` into a fixture copy: **exit 1**, naming the file and the token |
| `managed` and `sdk/python/tests` in no job | **fixed** — both now have jobs (N-11) |

So the *contents* of N-5 are closed and the *criterion* is not. I am not going to soften that,
because the class it guards against is the one that has recurred in all three prior rounds, and
because a local pass is not the same measurement: the `conformance` job needs `psql` on the runner
and no step installs it (the `rust` job installs it explicitly, the `conformance` job does not), the
`managed`, `published-artifacts` and `cross-implementation-receipts` jobs have never executed in any
environment, and the DCO job has never validated against a real pull request. Environment difference
is precisely where "red on a clean tree" lived twice.

**This is cheap to clear and I want to be plain that it is the only thing.** Push the branch, let the
workflow run once, fix whatever it says. If it comes back green, my verdict flips.

---

## Everything else: closed

Each verified against its own anti-criterion, not merely its reproduction.

### N-1 — closed, root cause and all

The fix went past the SDK to the root cause I identified while reviewing the verification record:
`spec/signing.md` itself specified **code-point** ordering while naming RFC 8785, and shipped a
reference verifier implementing that error. All three sites are corrected — `signing.md:451` and
`:454` now say **UTF-16 code unit** with a worked example showing the two orders diverging at
U+D7FF, the reference verifier at `:419` sorts by `encode("utf-16-be")`, and the Python SDK does the
same at `_document.py:113`. `sort_keys=True` is gone from the canonicalizer, which was my hard test.

**Anti-criterion checked:** the fix is *not* the narrowing I predicted. `document` values still
accept arbitrary keys; nothing was restricted to ASCII to make my reproduction impossible.

**Reproduction plus siblings, live.** I minted receipts through the ordinary answer path carrying
eight key shapes — the original U+FF01/U+1F600 pair, the U+FFFF/U+10000 surrogate boundary, keys
straddling U+D7FF, an empty-string key, prefix keys, a pair nested inside an array inside a document,
escapes and control characters in keys, and non-ASCII in values only. All eight accepted, and with
the six number forms below that is 14 receipts on one tenant:

```
PYTHON SDK  : all 14 True     PYTHON chain: True
TS SDK      : all 14 True     TS chain    : true
MY VERIFIER : all 14 True          <- my own JCS, written from signing.md §2.2, sharing no code
```

Three implementations, one of them mine, agreeing on every shape that broke it.

**Residual, not blocking.** My gap-A criterion asked for a **generated** differential test across the
three canonicalizers; what exists is a *fixed* adversarial corpus, shared between C-26 and
`scripts/verify-minted-receipts.sh`. That covers every named shape and is a real cross-implementation
gate — but it tests the shapes someone thought of. I said I would treat this as the finding closed
and the *class* residual, and that is where I land: **N-1 closed; I still recommend property-based
generation**, because it is the instrument that finds shape #3 without anyone predicting it, and this
project has now been surprised twice in the same place.

### N-2 — closed, exactly to the amended criterion

The rule became normalization rather than the withdrawn lexical rule. Live, at both paths:

```
answer -0.0 -> 200    stored: {"amount": 0}
answer  0.0 -> 200    stored: {"amount": 0}
answer  1.0 -> 200    stored: {"amount": 1}
answer  1e2 -> 200    stored: {"amount": 100}
answer  1.5 -> 422 answer_validation_failed, request still pending
answer 2^53 -> 422 answer_validation_failed, request still pending
raise min=1.0 -> 201     raise min=0.5 -> 400 invalid_request
```

Every accepted form is an **integer at rest**. The round-trip assertion I asked for holds: the Python
canonicalizer refuses none of the 14 stored receipts, and both SDKs verify all of them.

**Anti-criterion checked:** not a special case for `-0.0`. `0.0`, `1.0` and `1e2` all normalize, so
the class is closed rather than the instance. And the client-side half I flagged is done — the
Python guard is no longer a blanket `isinstance(value, float)` rejection.

### N-3 — closed as the honest downgrade, with the control, and not silently

This is the one I most expected to defeat again, and the fix is structural in exactly the way I
argued for: **the hook count went down.** `receipt_chain_verify` and `chain_tamper_is_detected` are
**deleted**; the suite walks the chain itself over receipts it reads from the deployment's HTTP
surface, in its own §2.2 implementation. Four hooks became one parameterized `storage_mutate`.

**My old attack no longer works.** I rebuilt the liar profile — a hook that touches no storage and
narrates a refusal:

```
$ handoff-conformance --profile profile-total.yaml
FAIL  C-15 ...
25/26 passing        EXIT=1
```

The positive control is what kills it: the same command aimed at a row the engine *permits* writing
must land and become visible over HTTP, and a hook that touches nothing cannot make a nonce appear
in a request the suite then reads back.

**The declared gap is real, and correctly described.** A hook honest for `target: request` and
stubbed for `target: receipt` passes 26/26. That is exactly what the case says it cannot catch, in
its own rationale, and it is named in `conformance/GATE.md:154`, `conformance/README.md:98`, and
§18's rewritten C-15 row (*"the refusal observed rather than reported"*). My criterion was that I
would accept the honest downgrade but not silently. It is not silent, and §18 no longer claims more
than the instrument can see.

### N-4 — closed

`08` and `09` both verify **as published** under the Python SDK, and `verify_chain([08, 09])` is
`True`. The recompute workarounds are gone from both SDK suites, replaced by assertions whose
comments say why (*"Verbatim, with nothing recomputed first"*). Their deletion was part of my
criterion and it was done.

### N-6 and N-12 — closed

27 case files, 26 Level 1, 1 Level 2. Every live claim in the tree reads 26 — `README.md`,
`CONTRIBUTING.md`, `spec/CHANGELOG.md`, spec §1.2 at `:43`, spec §18 at `:1498`, `GATE.md`,
`conformance/README.md`. §1.2 and §18 agree, which was the serious half. The only stale numbers left
are inside `docs/hostile-review.md` and `docs/review-2.md`, which are historical records of what was
true when written.

All four N-12 contradictions are corrected: `height` is **1-based** in `signing.md:272`,
`receipt.schema.json` (`minimum: 1`) and `openapi.yaml` (`minimum: 1`), with a "Why 1-based" note;
`signing.md` states the integral-in-value rule and names `-0.0`, `1.0`, `1e2` as accepted-and-
normalized; `request.schema.json` `min`/`max` are `"type": "integer"` with safe-integer bounds and a
description explaining that JSON Schema's `integer` matches on value; and no `"type": "number"`
remains anywhere in `spec/schemas/`.

**Anti-criterion checked:** the count check discovers occurrences by pattern rather than enumerating
files, and derives ground truth from `conformance/cases/*.yaml` rather than from prose.

### N-7 — closed

62 of 62 commits carry `Signed-off-by`. The rebase covered the whole history, not only new commits.

### N-8 — closed, precisely to criterion

`machine_a2` exists in `core/dev/bootstrap.json` and in the profile, and C-5 now requires
`[machine_a, machine_a2, human_editor]`. The discriminating step has machine_a2 answer a request it
did **not** raise, expecting `403 requester_may_not_answer`, with a `because` that names the exact
distinction — *"a Server checking `principal.id == request.created_by` returns 200 here"* — and there
is a positive control in which `human_editor` answers successfully.

**Anti-criterion checked:** the second machine does not answer its own request, which would have been
the current state with extra steps.

Live against the shipped bootstrap: machine_a2 → `403 requester_may_not_answer`; machine_a →
`403 requester_may_not_answer`. Enforcement is by type.

### N-9, N-10, N-11, N-13, N-14 — closed

- **N-9.** The guard now walks **every** `.rs` file under the crates directory recursively, and
  flattens line continuations and strips `//` comments before matching — both holes I named, closed
  together. My anti-criterion was adding `cli.rs` while keeping same-line matching; that is not what
  happened.
- **N-10.** 20 tenant-scoped tables, `handoff_request_dispositions` included with a comment naming
  the review that found it missing. This went **past** my criterion: there is a new test that
  tightens the policy to fail-closed (removing the permissive `coalesce` clause I demonstrated) and
  asserts every request-scoped path still works, with a per-probe anti-vacuity guard requiring each
  path to be 2xx *before* the tightening, plus an assertion that the role does not bypass RLS. That
  addresses the fail-open property I reported rather than only the coverage count.
- **N-11.** `pytest tests/ -q` runs in CI (104 tests, verified locally); a `managed` job exists with
  its own Postgres service (76 tests).
- **N-13.** `demo/agent.py` defaults `HANDOFF_URL` to `https://handoff.example.invalid` and
  `PAGE_PHONE = False`. The hosted-service contradiction is resolved honestly rather than papered
  over: `README.md:41-49` now distinguishes the preserved hackathon deployment from this
  implementation, dates the check, and says plainly that the previous flat denial was the wrong
  sentence.
- **N-14.** `set_equals` refuses an empty match set, with a comment identifying it as the third
  operator to need the guard and the one missed when the other two got theirs.

### No regressions in what I had previously verified

Against the live server at this HEAD: idempotency still scoped to the object (two objects, one key,
two receipts, both settled, genuine retry still replays); exactly-one-effect still `200/200/409`;
tenant isolation still `404` on every probe with no identifier leaked.

---

## The four questions

### 1. The absent I15 mutation — your reasoning is sound, and it understates your own result

It is not a convenient dodge. But the conclusion is weaker than what you actually established, and
the reason matters.

You asked the question "can I remove the property", found you could not, and concluded that a green
suite under such a mutation says nothing about C-5. The question C-5 has to answer is different:
**would it notice if enforcement were by requester identity rather than by principal type?** That is
now answered by construction, and your own experiment answers it too.

C-5's machine_a2 step asserts `status: 403` **and** `error.code == requester_may_not_answer`. Under
an identity-based server, machine_a2 is not the requester, so it passes the first four expressions
and reaches the layer you found — which refuses with `400 — an actor of type user must be a person,
not a machine`. C-5 expects 403 with a named code and would get 400. **C-5 goes red.** So the
mutation you could not complete would have been caught by the case anyway, and your finding that the
property survives four disabled expressions is evidence that it is defended in depth, not evidence
that the case is uninformative.

My N-8 said C-5 was weak. With `machine_a2` and that `because`, it is not weak any more, and that is
established independently of whether the mutation is runnable. I would state it that way in
`verification-pass.md` rather than leaving the bullet reading as an unresolved gap.

### 2. `docs/verification-pass.md` — it now holds, with three gaps in the "did not reach" section

All four of my conditions were applied, the rewrite is honest, and the section recording the green
result that was wrong at the moment it was produced is the best thing in the document. My third rule
was adopted verbatim.

Three things still missing from *what this pass did not reach*, offered in the spirit of "such
sections are always incomplete":

- **The managed tier was never probed adversarially.** 76 passing tests is not the same claim as an
  attack, and the document's battery table invites reading it as one.
- **Nothing was tested under contention.** Exactly-once and first-writer-wins are the two properties
  where a passing sequential probe is least informative, and no concurrency appears anywhere in the
  pass.
- **The pass did not look for an eighth vacuous test.** Seven have been found in this project. Not
  looking is fine; implying the sweep was complete is not, and "what it did not reach" is where that
  belongs.

One small overclaim: *"Every check named here is a script in this repository, so a reader can re-run
it"* is true of the four scripts in the table beneath it, but the battery table above also lists test
counts, clippy and fmt, which are cargo invocations rather than scripts. Trivial to reword.

One hygiene defect, found by accident: `core/dev/mutation-pass.sh` leaks its databases. A 9 MB
`handoff_mut_42437_1785498473` was still on the server when I finished. Every other harness in this
repository drops what it creates.

### 3. The mis-attributed commits — acceptable, and the record is what makes them acceptable

Two commits carry files their messages do not describe: `1218504`, a §18/C-15 wording change that
also carried `sdk/python/handoff/_document.py` and reverted the N-1 fix, and `67eb18d` ("Install what
the published-artifact checks need, and count them"), which carried the restoration.

**No surgery.** Rewriting history under other agents' work is a larger risk than the defect, the
misattribution is documented in `verification-pass.md` including the mechanism (`git add <file>`
followed by a bare `git commit` taking another lane's staged file), and a reader who cares can run
`git show`. The commit messages in this repository are unusually informative and two exceptions
recorded as exceptions is a better artifact than a rewritten history that hides them.

One correction to your framing: you said fixing attribution would mean rewriting history, but the DCO
rebase already rewrote all 62 commits. So the constraint was never "history cannot be rewritten" —
it was "it is not worth rewriting again for this", which is the right answer for a different reason.

### 4. What regressed — one, it was the release-blocking one, and the guard I asked for caught it

`1218504` at 14:31 reverted N-1: it restored `json.dumps(value, sort_keys=True, …)` together with the
stale comment claiming that matches JCS, deleting the `utf-16-be` sort. The release-blocking defect
was live in HEAD again.

It was restored at 14:36, five minutes later, by `d950d33` — whose message names it: *"Mint with the
server, verify with the SDKs, **and restore the fix a commit had reverted**"*. So the regression was
caught by `scripts/verify-minted-receipts.sh` as that script was being built, which is the
cross-implementation gate I argued was load-bearing for spec-conformance rather than merely SDK
health. It has now justified itself once before ever running in CI.

The conformance suite could not have caught it: it links `handoff-protocol` and canonicalizes with
the same code the server does. `verification-pass.md` says exactly this, and calls it an accident
rather than a demonstration, which is the honest reading.

I re-verified the current state directly rather than trusting the restoration: 14 receipts, 8
adversarial key shapes, three implementations, all verifying. **No regression survives at this HEAD**
in anything I had previously measured.

---

## Per-finding status

| | Criterion met | Notes |
|---|---|---|
| **N-1** | **yes** | root cause fixed in the spec too; 8 shapes × 3 implementations. Class residual: no generated differential test |
| **N-2** | **yes** | normalization + round-trip; not a `-0.0` special case |
| **N-3** | **yes** | hook count went down; control kills the total liar; declared gap documented in three places |
| **N-4** | **yes** | verifies as published; recompute workarounds deleted |
| **N-5** | **NO** | contents fixed and locally verified; **the workflow has never run** |
| **N-6** | **yes** | §1.2 and §18 agree at 26; check discovers rather than enumerates |
| **N-7** | **yes** | 62/62 signed |
| **N-8** | **yes** | machine_a2 answers a request it did not raise; positive control present |
| **N-9** | **yes** | all `.rs` files, continuations flattened |
| **N-10** | **yes, exceeded** | 20 tables + a fail-closed policy test with per-probe guards |
| **N-11** | **yes** | pytest and managed both wired |
| **N-12** | **yes** | all four contradictions corrected |
| **N-13** | **yes** | placeholder host, paging off, hosted-service question settled honestly |
| **N-14** | **yes** | `set_equals` guarded |

---

## Verdict

**Not yet — blocked on exactly one thing, and it is one push away.**

`.github/workflows/ci.yml` has never executed. That was my most explicit acceptance criterion, I
wrote it blind, and I am holding to it for the reason I wrote it: three consecutive rounds found
checks that were written and never run, and two of those were red on a clean tree in ways nobody
noticed because nothing ever ran them. Fourteen jobs at this HEAD are predictions. I verified locally
that the two previously-red ones now pass and that the previously-vacuous address check is
falsifiable — but a local pass on a Mac is not the measurement, and at least one job (`conformance`)
depends on `psql` that no step installs on the runner.

**Push the branch, let CI run once, fix what it reports. If it is green, this is ready to publish.**
I am not asking for new work, a further round, or anything I have not already written down.

Everything else is closed. Several fixes are better than what I asked for: the harness deleted two
hooks rather than tightening them, and now walks the chain itself; the RLS work went past coverage
counting to demonstrating the policy fail-closed with per-probe anti-vacuity guards; the fixture-leak
validator carries its own must-fire self-test; and the N-1 fix reached the specification and its
reference verifier rather than stopping at the SDK that was reported. The C-15 rewrite is the single
most improved artifact in the repository — it states the rule that produced it (*"a claim the suite
can compute, the suite computes"*), names the attack that defeated its predecessor, and declares its
own residual limit in the case, in `GATE.md`, in `conformance/README.md` and in §18 rather than
leaving a reviewer to find it.

The pattern I named in `review-3.md` — *the parts that were built are sound, and the parts meant to
keep them honest are the ones that do not run* — is now true of exactly one thing instead of six.
That one thing is CI, and it is the last instance of the pattern this project has been fighting for
four rounds. Running it once is how the pattern ends.

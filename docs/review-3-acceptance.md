# Acceptance criteria for review-3's findings

**Written blind, 2026-07-31, before any lane's fix landed.** Branch at `e3faeb2` when this was
written. The point of writing it now is that my re-verdict should be measured against criteria I set
without seeing the fixes, rather than against whatever the lanes happened to build.

For each finding: **Closed when** — the evidence that would convince me. **Not closed by** — the
plausible fix I expect an author to produce that leaves the defect reachable. **How I will test** —
what I will actually run.

Two general rules I will apply to everything below, because all three rounds have now been partly
undone by the same two moves:

1. **A guard nobody has watched fail is not a guard.** For every new check, I need to see it go red
   against a deliberately broken input. If the commit does not demonstrate that, I will construct
   the broken input myself, and if the check stays green the finding is not closed.
2. **A fix that makes my exact reproduction pass is not a fix.** I wrote reproductions to exhibit a
   class. I will re-run each one *and* at least one sibling shape from the same class that no report
   has named. If the sibling still reproduces, the finding is not closed.

---

## First: the two decisions you asked me to rule on now

### The lexical number rule — **half right, and it will create a new N-1 if shipped as stated**

Your reasoning for it is sound: `-0.0` got through because `f.fract() != 0.0` is false for it, so a
value-based test cannot see it. A lexical rule kills that whole class at the door.

**But a lexical rule is not enforceable in JavaScript**, and one of your two published SDKs is
TypeScript. `JSON.parse` discards the lexeme irrecoverably:

```
$ node -e 'console.log(JSON.stringify(JSON.parse("{\"a\":1.0,\"b\":1,\"c\":-0.0,\"d\":1e2}")))'
{"a":1,"b":1,"c":0,"d":100}
```

All four inputs land on integers, indistinguishable from each other post-parse. Python *can* see it
(`type` is `float` vs `int`) and Rust *can* (`is_f64()` vs `is_i64()`), so a lexical rule is
enforceable in two of your three implementations and structurally impossible in the third without
raw-byte scanning before the parser runs. Ship it as written and a TS server accepts `{"amount":1.0}`
while the Rust server rejects it — two conforming implementations disagreeing about what is legal,
which is N-1's shape with the roles swapped.

**More importantly, a lexical input rule does not close N-2 on its own**, because N-2's actual root
cause is downstream of validation. Two things went wrong, and the input rule only touches the first:

- The server **stored the number as it arrived** rather than as its own canonicalizer renders it.
  `write_number` correctly emits `0` for `-0.0` (`receipt.rs:200-204`), but the *stored* jsonb kept
  `0.0`. The canonical form and the form at rest disagreed.
- The Python SDK rejects by **Python type**, not by value — `_reject_floats` refuses any `float`, so
  it would refuse `0.0` even though `0.0` is integral and canonicalizes to `0`.

**What I would do instead, and what I will accept either way.** The invariant worth specifying is
*normalization*, not lexis: **a digest-covered number MUST be stored and served in the exact form the
canonicalizer emits for it.** That is checkable in all three languages, it closes `-0.0` and `1.0`
and `1e2` together, and it is the property a verifier actually depends on. If you want the lexical
rule as well, keep it — it is a good ingest guard — but specify it as a **wire-level** rule with an
explicit note that an implementation whose parser discards the lexeme MUST scan the raw bytes, and
add a conformance step that sends `{"amount": 1.0}` and requires the same status from every
implementation. Without that note the rule is unenforceable in a third of your ecosystem and the
disagreement will surface as an interop bug rather than a validation error.

I will accept the lexical rule as *sufficient* for N-2 only if the normalization property is also
asserted — see N-2 below.

> ### Amendment, same day: the lexical rule was withdrawn
>
> The maintainer withdrew it and adopted normalization instead: a digest-covered number MUST be
> stored and served in the exact form the canonicalizer emits, values still integral and within
> ±(2^53 − 1), so `-0.0`, `1.0` and `1e2` are **accepted and normalized** to `0`, `1` and `100`
> rather than refused. That is the right rule and it supersedes part of my N-2 criterion below —
> where I wrote "`-0.0` is refused at both the raise and the answer path", the correct criterion is
> now "**accepted at both, and stored as `0`**". I am recording that here rather than silently
> re-reading my own criterion at re-verification.
>
> **One consequence the lanes should not miss.** Normalization is now a rule about *documents*, not
> just about the server's ingest path, so the SDKs have to implement it too. The Python
> canonicalizer's float guard is an `isinstance(value, float)` rejection — it refuses **any** float,
> integral or not — so a user who builds a document locally containing `0.0` or `1.0` cannot
> canonicalize it at all. That is the client-side half of N-2, untouched by anything the server does.
> It needs to become "normalize integral floats, reject non-integral ones". (The file is mid-edit as I
> write this and I have deliberately not read the lane's diff, so treat this as a criterion to hit
> rather than a claim about what is there now.) TypeScript gets this for free because `JSON.parse`
> already collapses them, which
> is the same asymmetry that killed the lexical rule, pointing the other way this time. If only the
> server normalizes, I will find a document the server accepts and the Python SDK cannot hash, and
> that is N-2 again with a different entry point.

### Level 1 becomes 26 cases with C-26 — **right, and underspecified in three ways that would make it the eighth vacuous test**

Making it Level 1 is correct: an implementation that cannot have its receipts verified by an
independent party has not implemented the receipt chain, so it belongs at the baseline bar rather
than as an extension.

Three things C-26 must do, or it will pass on the happy path exactly as every existing signing test
does:

1. **The verifier must share no code with either side.** If C-26 calls
   `handoff_protocol::receipt::verify_chain`, it is the closed loop R-1 described, with a new case id.
   The verifier must be written from `signing.md` §2.2 — its own JCS, its own SHA-256 call — and live
   in the conformance crate as an independent implementation, with a comment saying that is why it
   does not reuse the protocol crate.
2. **It must verify with both published SDKs, not one.** N-1 is precisely a case where the Rust and
   the TypeScript implementations agree and the Python one does not. A C-26 that verifies in Rust
   only would have passed at `e3faeb2` while every Python user saw `False`.
3. **It must carry the adversarial shapes, not a plain receipt.** At minimum: a `document` field
   value containing a non-BMP key and a BMP-high key together (the N-1 pair, e.g. `U+1F600` and
   `U+FF01`), a number at each end of ±(2^53 − 1), a string with JSON escapes and a raw non-ASCII
   character, and an empty-string key. A C-26 that answers `{"decision":"approve"}` and verifies the
   result proves only what the existing six receipts already proved.

If C-26 lands without (1), (3) is what I will attack first and I expect to defeat it.

> ### Amendment, same day, after the maintainer's ruling on (2)
>
> **Requirement (2) is withdrawn. The ruling against me is correct and I was wrong.** A conformance
> suite defines what a *Server* must do, and requiring our Python and TypeScript packages inside a
> case would impose our client libraries — and a Python runtime — on every third party running the
> suite against their own server. A Go implementer should not need our Python to claim Level 1. That
> is a portability defect I proposed, and I withdraw it. C-26 stays Rust-only against the harness's
> own in-crate §2.2 verifier; the cross-implementation check moves to
> `scripts/verify-minted-receipts.sh`, sharing C-26's adversarial payload.
>
> **The split covers N-1**, and I checked that specifically: N-1 was the *Python SDK* diverging, which
> a Rust-only C-26 would not have caught and the script does. Four caveats, offered now rather than at
> re-verification:
>
> **(A) A shared static payload only ever tests the shapes someone thought of.** Both instruments will
> be exactly as good as that fixture. N-1 was found by reasoning about UTF-16 versus code point; the
> next divergence will be a shape nobody predicted, and neither instrument generates input. What
> closes the *class* rather than the two known instances is a **differential property test**: generate
> random documents — keys drawn from an alphabet spanning the surrogate boundary, values across the
> integer range, nesting, empty keys, escapes — and assert all three canonicalizers emit identical
> bytes. That is the one instrument that finds shape #3 without anyone naming it first, and it is
> cheap. I will treat its absence as the finding staying open in class even if both named shapes pass.
>
> **(B) C-26's verifier is independent of the protocol crate, but not of its author or its
> language.** R-1's original defect was a *misreading* of §2.2 — one-step instead of two — and two
> Rust implementations written by the same people from the same reading can share a misreading. What
> exposed R-1 was that the Python and TypeScript SDKs had been written separately. So
> `verify-minted-receipts.sh` is load-bearing for **spec-conformance**, not merely for SDK health, and
> should be a release gate rather than a convenience. If it is ever skipped or disabled, C-26 alone
> does not establish that the server matches §2.2 as an independent reader would implement it.
>
> **(C) The script must be shown failing before it counts.** N-5 is precisely "a job was written,
> never ran, and was red on a clean tree". A new script wired as a CI job inherits that risk exactly.
> I will want it demonstrated red against a deliberately mis-sorted canonicalizer — reverting
> `_document.py` to `sort_keys=True` is the obvious mutation and should turn it red.
>
> **(D) State what C-26 does not establish.** A third party who passes the suite learns that their
> server agrees with the harness's Rust verifier. They do **not** learn that their receipts verify
> under any other implementation — which is the property `signing.md` sells to "an auditor, a
> regulator, a customer after they have left". `conformance/README.md` should say so and point
> implementers at the published vectors so they can run their own cross-check. This is the project's
> own standard — state the limitation rather than let it be discovered — and it costs two sentences.

One more, on the count: extending `gate-count` to all four places the number lives fixes the four
places the number lives. The number has now gone stale in a *new* file three times. The check should
**discover** occurrences rather than enumerate them — grep the tree for the pattern (`\b\d+ Level 1\b`
and `C-\d+ through C-\d+`) and assert every hit agrees with the derived count — or the fifth
recurrence lands in the fifth file.

---

## N-1 — Python SDK sorts by code point, not UTF-16 code unit

**Closed when.** `sdk/python/handoff/_document.py` sorts object members by UTF-16 code units, and a
receipt minted by `handoffd` containing a non-BMP key verifies under **both** SDKs and under an
independent verifier. A unit test exists that pins the two orderings apart for a known pair, so the
test would fail if someone reverted to `sort_keys=True`.

**Not closed by.**
- **Constraining `document` values to ASCII keys.** This is the fix I most expect, because it makes
  my reproduction impossible without touching the canonicalizer. It is the wrong fix: it narrows the
  protocol to avoid a bug, leaves `canonical_bytes` wrong for every other route into a digest-covered
  object, and would have to be enforced identically in three implementations — a new N-1.
- Fixing the sort but testing it only with ASCII keys. The test must contain a pair whose two
  orderings differ; an ASCII-only test passes under both sorts and proves nothing.
- Putting the test only in `sdk/python/tests/`, which runs in no CI job (N-11). It must execute
  somewhere that executes.
- Any fix that leaves `sort_keys=True` in the canonicalizer. Python's `sort_keys` is code-point
  ordering by definition; if that call survives, the defect survives whatever else changed.

**How I will test.** Re-run my reproduction verbatim (`{"！":1,"😀":2,"a":0}` into a `document`
field, verify with both SDKs). Then the siblings no report has named: a key pair straddling the
surrogate boundary exactly (`U+FFFF` vs `U+10000`), a key that is the empty string, two keys where
one is a prefix of the other, and a nested object inside an array inside a document value. And I
will read `_document.py` for the literal string `sort_keys`.

---

## N-2 — `-0.0` reaches a digest-covered position

**Closed when.** Every digest-covered number is stored and served in the form the canonicalizer
emits, and a test asserts that by **round-trip**: mint a receipt, read the stored bytes back, and
assert they equal `canonical_json` of the same document. Plus `-0.0` is refused at both the raise and
the answer path.

**Not closed by.**
- **Special-casing `-0.0`.** The obvious minimal fix, and it leaves `1.0` and `1e2` to be stored as
  `1.0` and `100.0` the moment anything else reaches that column.
- Fixing the answer path and not the raise path, or the reverse. Both are digest-covered; I tested
  both and will again.
- The lexical rule alone, for the reason argued above: it guards ingest and says nothing about what
  is at rest. If a number ever enters storage by a route that skips validation — a migration, an
  admin path, a future field — the round-trip assertion catches it and the input rule does not.

**How I will test.** `-0.0`, `0.0`, `1.0`, `1e2`, `1.0e0`, and `2^53 − 1` written as `9007199254740991.0`,
on both a `number` field and nested inside a `document` value, at raise and at answer. Then read
every digest-covered column back out of Postgres and diff it against the canonicalizer's output.

---

## N-3 — The harness accepts the hook's narration as evidence

You asked what would make me believe the harness verifies rather than narrates. Here is the general
principle, and then the specific tests.

**The principle: any requirement a hook can satisfy as a pure function of its own inputs is
defeatable, because the claimant writes the hook.** The bar has moved twice — exit code, then printed
words — and both times the new requirement was computable from what the hook was handed. A nonce
will be echoed. A timestamp will be echoed. A hash of the arguments will be computed. Requiring
*more* tokens is the third iteration of the same mistake and I will defeat it the same way, in about
fifteen lines of shell.

**So the criterion is structural: the number of below-HTTP hooks should go DOWN.** Specifically:

- **`receipt_chain_verify` and `chain_tamper_is_detected` should be deleted, not strengthened.**
  Neither property needs a hook. "The chain verifies over the full history" is checkable by the
  harness: fetch the receipts over HTTP and verify them with the harness's own independent
  implementation — which C-26 is building anyway. "Altering a receipt invalidates the head" is a
  property of the digest construction, not of the deployment: the harness takes the receipts it
  already fetched, mutates one *in memory*, and asserts its own verifier rejects it. No hook, nothing
  to lie about, and it tests the same claim more strongly than the current version does.
- **`storage_update_receipt` / `storage_delete_receipt` should become a credential, not a command.**
  The profile should supply a store connection the harness itself uses, and the harness performs the
  UPDATE and the DELETE. Then the refusal is observed by the harness rather than reported to it.
  This one needs a **positive control**, because "storage refused my write" and "I never wrote" are
  observationally identical from outside: the same credential must be shown to successfully write to
  *some* other table, so a claimant cannot pass by handing over a read-only connection. Without that
  control the credential form is no better than the command form.

**Closed when.** For C-15: the two verification hooks are gone and the harness verifies the chain
itself; the two storage hooks are performed by the harness against a supplied credential, with a
positive control proving that credential can write somewhere. **And** I cannot construct a profile
that passes C-15 without implementing storage-level immutability.

**Not closed by.**
- More `output_matches` tokens. Any token derived from `HANDOFF_ARG_*`, from the database, or from a
  fixed string is supplied, not proved.
- Requiring the hook to echo a harness-generated nonce. Supplied.
- Requiring the hook to print a digest it "computed". A lying hook reads the digest from the
  `chain` column; I already did exactly this.
- Tightening `output_matches` to reject multi-line brute force without removing the underlying
  weakness. That closes one of my two defeats; the other — `select max(height)` — needs no
  brute force at all.
- Keeping the hooks and adding a sentence to `GATE.md` saying they are trusted. That is honest, but
  it downgrades C-15 from measured to attested, and if that is the choice then §18 must stop calling
  C-15 an assertion "from the storage layer" and `GATE.md` must list which assertions are attested
  rather than measured. I would accept that as an honest resolution; I would not accept it silently.

**On `crash_between_state_and_event` (C-23):** I do not think the harness can own a crash without
becoming deployment-specific, so I expect this one to stay a hook, and I will not block on it. What
I will check is that the project *says so* — if one assertion in the suite is claimant-attested, the
document that turns the suite into evidence has to name it. Silence is what made the last three
findings invisible.

**How I will test.** Rebuild my liar profile against the new harness and re-run. Then, if the
credential form landed, hand it a connection to a database where `handoff_receipts` has no triggers
at all and confirm C-15 goes red; and hand it a read-only connection and confirm the positive
control goes red.

---

## N-4 — `09-receipt-policy.json` does not verify as published

**Closed when.** `verify_receipt_chain(json.load(open('spec/fixtures/09-receipt-policy.json')))` is
`True` in both SDKs, **and** the recompute workarounds are deleted from
`sdk/python/tests/test_signing.py` and `sdk/ts/test/signing.test.ts`, replaced by an assertion that
the file as it sits on disk verifies.

**Not closed by.** Correcting the digest while leaving the recompute lines in place. Those lines are
why a wrong fixture survived two rounds; if they stay, the next drift is silent again. Their deletion
is part of the criterion, not a nicety.

**How I will test.** Load both receipt fixtures from disk, verify as published in both SDKs, and
`grep` both test files for any recomputation of `chain.digest` before an assertion.

---

## N-5 — CI is red on a clean tree, one check is a no-op, and the workflow has never run

**Closed when.** There is **an actual workflow run**, on a pushed branch, that is green. Not a local
reproduction, not a claim that the jobs would pass — a run.

**Not closed by.**
- Fixing the two red jobs and asserting they now pass. The whole finding is that this repository has
  never executed its own CI; three rounds of "written and never run" is the pattern.
- Fixing the address regex without a test. The capture-group-in-a-lookahead bug produced a check that
  printed its own reassurance while a token-bearing URL sat in the tree. The fix needs a test that
  plants such a URL and shows the job red.
- Suppressing the two fixture false positives by adding an allowlist for the two offending files.
  That re-breaks the check for those files, which are exactly the ones carrying a
  `password`-named member. Either the heuristic learns the difference between a placeholder and a
  secret, or it is replaced by something that can.

**How I will test.** Ask for the run URL and read the log. Then, locally: plant a token-bearing URL
in a fixture copy and run the validator; run `ruff check .` in `sdk/python`; and diff every literal
number in `ci.yml` against ground truth derived from the tree.

---

## N-6 and N-12 — normative documents contradict the implementation and each other

**Closed when.**
- **Count:** every occurrence of the Level 1 count in the tree agrees with the derived count, and the
  CI check **discovers** occurrences by pattern rather than enumerating known files.
- **Height:** `signing.md:272`, `receipt.schema.json:646` and `openapi.yaml:3907-3909` all say 1-based
  with `minimum: 1`, and C-15 (or C-26) asserts that a tenant's first receipt has `height: 1` and
  `prev_digest` of 64 zeros.
- **Number rule:** `signing.md:431` states the integers-only rule, and the paragraph after it no
  longer reasons from the retired band to the conclusion that two canonicalizers "cannot diverge".
- **Schema:** `min`/`max` are `"type": "integer"`, and a sweep confirms no other digest-covered
  `"type": "number"` remains.
- **Changelog:** one statement of the number rule, not two.

**Not closed by.**
- Fixing the four places the count lives by hand. The count has gone stale in a *new* file three
  times; a check that names files will miss the fourth.
- Changing `signing.md`'s height sentence and leaving `minimum: 0` in the schema and the OpenAPI
  document. Those are the machine-readable artifacts — the ones a generated client is built from.
- Fixing the height text without pinning it in a case. The reason this survived is that nothing
  tests it; both SDKs read `height` from the receipt, so no existing test can notice.

**How I will test.** `grep -rn` for `0-based`, for `minimum: 0` near `height`, for the `1e-6` band,
and for every `\b\d+ Level 1\b` in the tree. Then mint a receipt on a virgin tenant and read its
height. Then raise with `min: 0.5` and confirm the schema and the server now agree about it — either
both accept or both reject.

---

## N-7 — no commit carries a sign-off

**Closed when.** `git log --format='%(trailers:key=Signed-off-by,valueonly)' | grep -c .` equals the
commit count.

**Not closed by.** Signing off new commits only. `SECURITY.md` calls the DCO trail load-bearing for
the licence grant; a trail with a hole is not a trail.

**How I will test.** The command above, and a spot check that the sign-off identity matches the
author on a sample.

---

## N-8 — C-5 cannot distinguish enforcement by principal type from enforcement by requester identity

**Closed when.** The shipped profile has a **second machine principal in tenant A**, and C-5 has that
principal attempt to answer a request it did **not** raise, expecting `403
requester_may_not_answer` — plus a positive control in which a human answers the same request
successfully.

**Not closed by.**
- Adding only the positive control. That proves `/answer` works, not that the refusal is by type.
- Adding a second machine principal that answers its **own** request. That is the current state with
  extra steps: "is a machine" and "is the requester" remain indistinguishable.
- Adding the second principal to `core/dev/bootstrap.json` but not to
  `core/dev/conformance-profile.yaml`, or vice versa — the case needs both.

**How I will test.** Read C-5's `requires.principals` and its steps. Then, against the shipped
profile, run the three-way probe I ran this round: requester answers, non-requester machine answers,
human answers.

---

## N-9 — the `decision` write-only guard is evadable

**Closed when.** The guard scans every file that queries `handoff_receipts` (today: `store.rs`,
`migrations.rs`, `cli.rs`) and is insensitive to line breaks — the codebase's own SQL is written with
backslash continuations, so a same-line match is defeated by the house style.

**Not closed by.** Adding `cli.rs` to the scan while keeping the same-line requirement. Both holes
are independent; closing one leaves the query
`"select decision \` / `from handoff_receipts where …"` invisible.

**How I will test.** Write that exact two-line query into a scratch copy of each scanned file and
confirm the guard panics for each.

---

## N-10 — RLS coverage and the fail-open policy

**Closed when.** All **20** tenant-scoped tables (including `handoff_request_dispositions`) carry a
tenant-B row in the fixture, and the per-table assertion is shown to **fail** when RLS is dropped on
a table — demonstrated, not asserted. The permissive-when-unset clause is either changed or its
rationale is written down where an operator will read it.

**Not closed by.**
- Adding the 20th table to `TENANT_SCOPED_TABLES` without creating rows for it. That is the `[] == []`
  class again, and it would make the count read 12 of 20 while proving 11.
- Keeping `assert!(covered > 0)` as the guard. One covered table satisfies it.
- Reporting the uncovered tables through `eprintln!`. `cargo test` captures stderr on a passing test,
  so the "keeps the gap in view" mechanism is invisible in exactly the case where it matters. If the
  gap is meant to stay visible it has to be an assertion with an allowlist that shrinks, or a printed
  line on a *failing* test.
- Fixing `SECURITY.md`'s "every `handoff_*` table" without noting `handoff_migrations`.

**How I will test.** Create a least-privilege role, populate both tenants, drop RLS on one table at a
time, and check the test goes red for each of the 20. Then re-run the no-tenant-named query and see
whether it still returns every tenant.

---

## N-11 — Python SDK tests and the managed tier run in no CI job

**Closed when.** A job runs `pytest sdk/python/tests` and a job runs `cargo test` in `managed/`, and
both have been **shown failing** against a deliberately broken test.

**Not closed by.** Adding the jobs without demonstrating they can go red — a `pytest` step that
silently collects zero tests exits 5 or 0 depending on invocation, and a `cargo test` in the wrong
working directory passes vacuously. Given N-5, "the job exists" and "the job runs" are different
claims here.

**How I will test.** Read the job definitions, then break one test in each suite in a scratch copy
and run the job's exact command.

---

## N-13 — `demo/agent.py` defaults to the live host with paging on

**Closed when.** `HANDOFF_URL` defaults to a placeholder host and `PAGE_PHONE` defaults to `False`,
matching `agent_sprite.py`. And the hosted-service question is settled in one direction across
`README.md:41`, `SECURITY.md:52` and `docs/cutover-plan.md:106`.

**Not closed by.** Redacting more documentation. `RUNBOOK.md` is already redacted; the finding is
that a committed script re-creates the redacted command with fewer keystrokes.

**How I will test.** Read the four defaults, and grep the tree for the live hostname.

---

## N-14 — residual matcher traps

**Closed when.** `set_equals` refuses an empty match set as `all_equal` and `none_equal` do, with a
unit test; and C-17's three uncontrolled negatives have positive controls.

**Not closed by.** Fixing `set_equals` alone. The finding is that three of four operators were
guarded and the fourth was missed — which means the audit, not the operator, is what failed. I want
the table: every operator, its empty-match behaviour, and whether it is guarded.

**How I will test.** Read `expect.rs` operator by operator and write a case that exercises each
against a path that resolves to nothing.

---

## What I will do on re-verification

Against the **stable pushed HEAD**, not a moving branch:

1. Re-run every reproduction in `review-3.md` verbatim, plus the siblings listed above.
2. Rebuild the liar profile against the new harness (N-3) and try to defeat C-15 again.
3. Re-derive every count in the tree independently rather than reading the check's output.
4. Read the CI run log, not the CI file.
5. Try to find the eighth vacuous test. Seven have been found; the base rate says there is one.

If a finding's fix is a narrowing of the protocol rather than a correction of the defect — the ASCII
restriction for N-1, an allowlist for N-5 — I will say so and treat it as open, because both would
close my reproduction while leaving the property untrue.

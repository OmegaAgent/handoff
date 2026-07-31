# Hostile review 3 — Handoff v0.1

**Reviewer:** a third, independent adversarial pass, 2026-07-31. Worktree
`/Users/noureddinbakir/handoff-v1`, branch `v1/protocol`. Read-only on all source; the only file
written is this one. I wrote none of this code, and I took neither prior review's findings, nor its
verdict, nor any fix made against it on trust.

**A note on the commit under test.** I was briefed to review `cb1604f` with 33 commits. The branch
is at **`e3faeb2`, 50 commits** — it advanced twice while I worked. `e3faeb2` ("Write down the three
ways a check passes without measuring anything") touches documentation only, so every finding below
is measured at `e3faeb2` and holds at `cb1604f` unless stated. The count discrepancy is worth noting
in its own right: a review commissioned against a moving branch cannot be a gate.

**Method.** I reproduced the conformance baseline independently on a disposable Postgres
(`25/25 passing, exit 0`), then stood up my own `handoffd` with my own bootstrap — including a
**second machine principal in tenant A**, which `core/dev/bootstrap.json` still does not have and
which turns out to matter (§N-8) — and attacked the HTTP API, the storage layer, the receipt chain
and the conformance harness directly. Every finding marked **demonstrated** carries a reproduction I
ran. Where I could not reproduce, I say **suspicion**. §D lists what I attacked and could not break;
it is the part that should calibrate the rest.

**The two results that matter most.** First: R-1 was fixed for the shape it was reported in, and I
confirmed that — but the seam it lives on is still unguarded, and I got a receipt through the
ordinary answer path that the project's own Python SDK reports as forged. Twice, by two unrelated
routes. Second: R-3's hook fix replaced "an exit code is a claim" with "print the evidence", and the
evidence is either handed to the hook as an argument or readable from a column, so **C-15 passes
against a deployment with no storage-level immutability and no chain verifier at all** — I ran it,
25/25, exit 0.

---

## Findings, by severity

### N-1 (RELEASE-BLOCKING, demonstrated) — The Python SDK reports a valid, server-minted receipt as forged whenever a `document` field carries a non-ASCII object key

**Claimed.** `spec/signing.md:9-19` — a receipt must be *"verifiable by a party who was never given a
secret and must not be able to forge one — an auditor, a regulator, a customer after they have
left."* `sdk/python/README.md:123` presents `verify_receipt_chain()` as the way to check integrity.
R-1's fix commit (`7b8b886`, "a receipt anyone can verify") closed the reported defect.

**True — for the reported shape.** I verified the fix genuinely: `receipt.rs:717-756` now implements
`signing.md` §2.2 as two steps, removes the **whole** `chain` member for the core hash, and stores a
genesis `prev_digest` of 64 ASCII zeros. Six receipts minted through the ordinary answer path verify
under the unmodified Python SDK, under the TypeScript SDK, and under a verifier I wrote from the
spec text with my own JCS implementation. That part is real and I want it on the record.

**But the three canonicalizers do not agree, and one of them is wrong.**

| implementation | member ordering | correct per RFC 8785? |
|---|---|---|
| Rust `receipt.rs:163-169` | `a.encode_utf16().cmp(b.encode_utf16())` | **yes** |
| TypeScript `sdk/ts/src/document.ts` | JS string comparison = UTF-16 code units | **yes** |
| **Python `sdk/python/handoff/_document.py:95`** | `json.dumps(sort_keys=True)` = **code point** | **no** |

The Python file says so itself at `:93-94`: *"sort_keys sorts recursively by code point, which
matches JCS for the ASCII member names this protocol uses."* That assumption is false.
`FieldType::Document` (`core/crates/handoff-protocol/src/requires.rs:361-368`) accepts **any**
non-null JSON value — *"we only assert that something was carried"* — so an answer to a `document`
field puts caller-chosen object keys straight into `decision.values`, which is inside the receipt
core. Field *names* are ASCII-constrained (`^[a-z][a-z0-9_]{0,63}$`); the keys **inside a document
value** are not constrained at all.

UTF-16 and code-point order diverge for any non-BMP character against any BMP character above
U+D7FF, because a non-BMP character encodes as a surrogate pair starting at 0xD800:

```
U+FF01 (65281)  vs  U+1F600 (128512)
  code-point order (Python sort_keys) : ['！', '😀']
  UTF-16 unit order (JCS, Rust, TS)   : ['😀', '！']
```

**Reproduction** (fresh `handoffd`, disposable Postgres, ordinary answer path, no privileges beyond
a normal human answerer):

```
POST /requests            field: {"name":"payload","type":"document","required":true}
POST /requests/{id}/answer  {"values":{"payload":{"！":1,"😀":2,"a":0}}}   -> 200

stored: handoff_receipts height 7, decision.values = {"a": 0, "！": 1, "😀": 2}

PYTHON SDK per-receipt verify_receipt_chain: [True,True,True,True,True,True,False]
PYTHON SDK whole-tenant verify_chain       : False
TS SDK     per-receipt verifyReceiptChain  : [true × 7]
TS SDK     whole-tenant verifyChain        : true
```

**Blast radius.** One answer containing one non-ASCII key permanently poisons that tenant's chain
for every Python-SDK holder: `verify_chain()` walks the tenant in order and returns `False` for the
whole tenant, not just the offending receipt. This is the exact failure R-1 described — *"an auditor
implementing the documented, correct thing concludes that every receipt is forged"* — reintroduced
by a different route, and now **data-dependent**, so it will surface in production rather than in
review. The two published SDKs disagree with each other about the same bytes, which is the one thing
a protocol cannot ship with.

Note the asymmetry: the reference server and the TypeScript SDK are **correct** and the Python SDK
is wrong. The fix is one line in `_document.py` (sort by `s.encode('utf-16-be')`), plus a
conformance case that mints a receipt containing a non-ASCII key and hands it to an independent
verifier. R-1's own closing recommendation asked for exactly that case — *"a conformance case that
hands a server-minted receipt to a spec-derived verifier… without that last part the seam is
unguarded again the moment either side moves."* The case was not added, and the seam moved.

---

### N-2 (RELEASE-BLOCKING, demonstrated) — `-0.0` puts a non-integer into a digest-covered position, and the Python SDK then refuses to verify the receipt

**Claimed.** `spec/handoff-protocol-v0.1.md:91-105` and `receipt.rs:186-235` — digest-covered content
carries **integers only**, bounded to ±(2^53 − 1). `receipt.rs` calls this *"one rule: no fractional
part, and inside the safe-integer range."* `_document.py:105-110` rejects any float outright.

**True.** `FieldType::Number` validation (`requires.rs:333-343`) checks only `min`/`max` via
`as_f64()`. `-0.0` has no fractional part, so `write_number` short-circuits it to `0` at
`receipt.rs:200-204` (*"Covers negative zero, which ECMAScript renders as `0` and Rust as `-0`"*) —
the canonicalizer is right. But the **stored** value keeps its float type:

```
POST /requests/{id}/answer  {"values":{"amount":-0.0}}   -> 200 answered

psql> select body->'decision'->'values' from handoff_receipts where height=10;
      {"amount": 0.0}                       <-- a JSON float, in a digest-covered position

PYTHON SDK verify_receipt_chain -> False   (ValueError: cannot canonicalize a non-integer number)
TS SDK     verifyReceiptChain   -> true
```

**Blast radius.** Same class as N-1 and the same consequence — a receipt the reference server minted
and stored that the published Python SDK reports as unverifiable — but a different root cause, and
here it is the **server** that is out of line with its own rule rather than the SDK. Every other
non-integer route I tried is correctly refused (§D), which makes this the one hole in an otherwise
airtight rule: `1.5`, `0.1`, `1e-6`, `1e20`, `2^53`, and floats nested arbitrarily deep inside a
`document` value are all `422`. Only `-0.0` gets through, because `f.fract() != 0.0` is false for it.

---

### N-3 (RELEASE-BLOCKING as a governance instrument, demonstrated) — C-15 passes against a deployment that implements neither storage-level immutability nor chain verification

**Claimed.** `conformance/cases/c15_receipt_immutable_at_storage.yaml:38-46` — *"Every one of these
four exits the way the case wants when it is replaced by a one-word shell command… An exit code is a
claim. So each step also requires evidence in the hook's output that it did the thing, and two of
them require that evidence to agree with what the case independently read over HTTP — the receipt
id, and the chain head. **A deployment that has not implemented storage-level immutability cannot
produce the engine's refusal, and one that has not implemented chain verification cannot produce the
head digest.**"* §18 calls C-15 the case that *"must be asserted from the storage layer, not through
the application."*

**The exit-code half of R-3 is genuinely fixed.** I confirmed it: with every hook stubbed `true`
(and the two refusal hooks `false`), **seven cases now fail** — C-7, C-8, C-15, C-21, C-22, C-23,
C-24 (`18/25 passing`). Under review-2 all of those passed. That is a real improvement and the
`runner.rs` failure text (*"an exit code alone is a claim, not evidence"*) is well aimed.

**But the evidence is self-asserted, and every required value is handed to the hook or sits in a
column.** `runner.rs:914-918` exports each `args` entry as `HANDOFF_ARG_<NAME>`. C-15 passes
`receipt_id` to both refusal hooks and `org_head` to `receipt_chain_verify` — the hooks are given
the values they must echo back. `output_matches` is a regex **search** over concatenated
stdout+stderr, so anything a hook does not know it can brute-force.

**Reproduction.** Four scripts, no `psql` in three of them, nothing implemented:

```sh
# storage_update_receipt / storage_delete_receipt — touch no storage whatsoever
echo "attempted=update receipt=$HANDOFF_ARG_RECEIPT_ID"
echo "ERROR: handoff_receipts is append-only; UPDATE refused"; exit 1

# receipt_chain_verify — verify nothing; echo the head you were handed, read the height
H=$(psql "$HANDOFF_DATABASE_URL" -At -c "select max(height) from handoff_receipts ...")
echo "chain_verified head=$HANDOFF_ARG_ORG_HEAD height=$H"; exit 0

# chain_tamper_is_detected — tamper with nothing; read the head out of a column
HEAD=$(psql "$HANDOFF_DATABASE_URL" -At -c "select body->'chain'->>'digest' from handoff_receipts order by height desc limit 1")
echo "tamper_detected altered=$HANDOFF_ARG_RECEIPT_ID head_before=$HEAD head_after=did-not-verify"; exit 0
```

```
$ handoff-conformance --profile profile-liar.yaml
PASS  C-15   Storage itself refuses to update or delete a receipt, and the hash chain proves history
25/25 passing
EXIT=0
```

The one value not handed over is `height`. I defeated it two ways: a `select max(height)` (above),
and — to show no knowledge at all is needed — a loop emitting `height=1` through `height=400`, one
line of which matches. A fixed *wrong* height correctly fails (real height was 13), which confirms
the step is load-bearing and that brute-forcing it is what defeats it.

**Blast radius.** `conformance/GATE.md` exists to turn "we ran the suite" into evidence, and
`README.md:147` offers the suite in place of trust. A third party can produce a complete 25/25 green
run with about fifteen lines of shell while their receipts are freely mutable and their chain is
never walked. This is the third round in which the assertions covering the properties an implementer
cannot check over HTTP are satisfiable without implementing them; the bar moved from "exit zero" to
"print these words", and the words are supplied. The structural fix is for the harness to verify the
claim itself rather than accept the hook's narration — recompute the chain from the receipts the
case already read over HTTP, and require the refusal to be observed as a *failed mutation* (re-read
the row and prove it is unchanged **after** the hook claims refusal), rather than as a sentence.

---

### N-4 (Release-blocking for publication, demonstrated) — `spec/fixtures/09-receipt-policy.json` still does not verify as published

R-1's closing list required: *"reconcile `receipt.rs` and `signing.md` §2.2, **fix
`09-receipt-policy.json`**, and add a conformance case…"*. The first was done; the second was not.

```
$ python3 -c "…sdk/python verify_receipt_chain(json.load(open('spec/fixtures/09-receipt-policy.json')))"
08-receipt-decision.json  verify_receipt_chain AS PUBLISHED: True
09-receipt-policy.json    verify_receipt_chain AS PUBLISHED: False
verify_chain([08, 09])    AS PUBLISHED: False

09 stored : sha256:c1a4f0bb7d2e6935481acdf20e7b3c56d9084e1fa27bc3d5608e94af1236b7d0
09 recomp : sha256:1c4738c06a55a1ecc2217b55ac20fa6ba65319e81fc3b7ac49a726536afeb669
```

Both SDK suites still **recompute** the digest before asserting (`sdk/python/tests/test_signing.py`,
`sdk/ts/test/signing.test.ts:276-284`), so no test anywhere asserts that a published receipt fixture
verifies as published — which is precisely how this survived a round after being named. A published
normative fixture that fails the project's own verifier is the first thing an independent
implementer will run.

---

### N-5 (High, demonstrated) — Two CI jobs are red on the pristine tree, one security check is completely vacuous, and neither workflow has ever run

`.github/workflows/ci.yml` and `dco.yml` have **never executed**: `gh api` reports 0 workflows, 0
runs, 0 PRs; `origin/main` is the unrelated night-hack history with no `.github/` directory;
`v1/protocol` is unpushed. Every claim below is therefore a prediction the repository has never
contradicted — the same condition that produced R-2.

- **`fixtures-validate` is RED on a clean checkout.** `ci.yml:325` flags any member named
  `secret|password|token|value` longer than 12 characters. Two published fixtures trip it:
  `use-cases/03-login-assistance.json:69` `"password": "<never leaves TLS>"` — a deliberate
  anti-secret placeholder — and `07-reassign-escalate.json:12` a principal id. Running the job's own
  heredoc verbatim gives `2 fixture consistency failure(s), exit=1`. Same class as R-2: an assertion
  whose subject exists in the tree in a form it cannot accept.
- **The "no resolvable capability address" check can never fire.** `ci.yml:313`:
  `re.compile(r"https?://(?!example\.(com|org|invalid)\b)[^\s\"']+|wss?://[^\s\"']+")`. The one
  capture group sits inside a negative lookahead, so `re.findall` returns `''` for every match and
  the `"token=" in hit` test at `:334` is always false. Demonstrated end-to-end: planting
  `"live_capability_url": "https://relay.attacker.example.net/live?token=…LEAKED"` into a fixture
  copy yields `no fixture carries a secret, an address, or a kind; exit=0`. `spec/fixtures/README.md:39-40`
  asserts this mechanically. It asserts nothing.
- **`python-sdk` is RED**: `ruff check .` reports 7 errors on the pinned version and 188 on current
  ruff, with no `[tool.ruff]` config anywhere — the rule set is whatever ships that morning.
- **`no-fork-markers`** is named *"no `[patch]`, **no path dependency on a published crate**"* and
  its body (`:90-96`) greps only for `[patch]`. `managed/Cargo.toml:34-37` is exactly the path
  dependency the job's own comment calls *"the moment a fork begins"*, from a separate workspace,
  unflagged. `CONTRIBUTING.md:89-92` says *"CI greps for this"* about the whole rule.
- **`spec-needs-conformance` is satisfied by deleting a conformance case** — it counts changed paths
  only. Demonstrated: `spec/…md` + deleting `c01_idempotent_raise.yaml` → GREEN.
- **`secret-hygiene` catches 6 of 26 real credential shapes** and misses `whsec_…`, the format
  `signing.md` §1.2 itself defines.

**Genuinely fixed and confirmed:** the conformance gate now **derives** `passed`/`total` from the
report instead of hardcoding a count (R-2's specific defect), the stub meta-test now has a real
readiness probe and asserts `passed == 0`, `gate-count` is sensitive in all five directions tried,
and the bound-fixture schema validator rejects all nine corruptions thrown at it. Those are real.

---

### N-6 (High) — Spec §1.2 and §18 still disagree about what Level 1 *is*; fourth recurrence of the hand-maintained count

| location | says | true |
|---|---|---|
| `spec/handoff-protocol-v0.1.md:43` (§1.2) | *"all **24** Level 1 conformance cases of §18: C-1 through C-16, plus C-6b and C-18 through **C-24**"* | 25, C-18 through C-25 |
| `spec/handoff-protocol-v0.1.md:1469` (§18) | *"passes all **25** Level 1 cases … C-18 through **C-25**"* | correct |
| `spec/CHANGELOG.md:73-74` | *"the **24** Level 1 conformance cases … **C-18**–**C-24**"* | 25, C-18–C-25 |

Ground truth: `grep -h "^level:" conformance/cases/*.yaml | sort | uniq -c` → 25 `level: 1`, 1
`level: 2`; `conformance-map.json` `levels."1"` has 25 entries. Two normative clauses of the same
specification define Level 1 differently, and an implementer reading §1.2 ships without C-25 — the
case that exists because the reference implementation had that exact defect (F-1).

`gate-count` (`ci.yml:463-488`) reads **only** `conformance/GATE.md`. Nothing parses the spec prose
or the changelog, which is why the number went stale in the two places the new check does not look.
`GATE.md` itself is now correct. Also stale: `spec/CHANGELOG.md:58` says 32 error codes; the
`ErrorCode` enum has 33.

---

### N-7 (High) — The DCO is documented as an enforced gate and as an existing provenance trail; **0 of 50 commits carry a sign-off**

`CONTRIBUTING.md:9` — *"**Every commit must carry a `Signed-off-by:` trailer.**"*; `:40` — *"CI
enforces this. A pull request with an unsigned commit will not merge."* `SECURITY.md:101-102` — *"the
MIT grant on already-published code and **the DCO trail** both depend on it staying intact."*

```
$ git rev-list --count HEAD                                            50
$ git log --format='%(trailers:key=Signed-off-by,valueonly)' | grep -c .    0
```

There is no DCO trail. The `dco.yml` logic is correct and would be red on all 34 commits of the
first PR — so the pull request that publishes this repository cannot merge under the repository's
own rule. Fixable with `git rebase --signoff`, but it must happen before the branch is pushed, or
CI's first-ever run is red on three axes at once (this, N-5's two red jobs).

---

### N-8 (Medium-High, demonstrated) — C-5 observes I15 at one call site, by the one principal for which "is a machine" and "is the requester" are indistinguishable

`c05_requester_may_not_answer.yaml:17` requires exactly one principal, `machine_a`, and answers only
the request `machine_a` itself raised. The case asserts a refusal and then only absences — it never
answers successfully, so nothing in it shows `/answer` discriminates on anything. Across all 26 case
files, `POST /requests/{id}/answer` is called by a machine principal in **C-5 alone**, and
`machine_b` never calls `/answer` anywhere. `grep -rn 'requester_may_not_answer\|require_person'
core/crates/handoff-server/tests/ managed/` returns nothing.

A server that replaced the principal-**type** check at `routes.rs:125-133` with a
requester-**identity** check (`principal.id == request.created_by`) would pass all 25 cases while
letting **any machine key that did not raise the request answer it** — one agent approving another
agent's refund, which is what §4.2 and I15 exist to forbid. C-5's own rationale calls this the rule
without which *"every other guarantee in the specification is decoration."*

**The reference server is correct** — I added a second machine principal to tenant A and checked
directly, which the shipped `bootstrap.json` cannot express:

```
raised by tok_ma:
  answer as tok_ma2 (different machine, same tenant, NOT the requester) -> 403 requester_may_not_answer
  answer as tok_ma  (the requester itself)                              -> 403 requester_may_not_answer
  state after both attempts: pending
```

So this is a **coverage gap, not a vulnerability**. It is the same class the project fixed twice
(C-12 and C-21 gained positive controls in `6fd4bd3`; C-6 and C-6b already had them) and missed on
the highest-stakes case. Closing it needs a second machine principal in tenant A and a step that
answers successfully after the refusal.

---

### N-9 (Medium) — The R-13 "write-only" guard is evaded by the codebase's own SQL formatting, and does not scan one of the three files that query the table

`migrations.rs:633` scans `include_str!("store.rs")` line by line and panics if a line contains both
`from handoff_receipts` and `decision`. Two holes:

1. **Same-line requirement.** The prevailing style in `store.rs` splits SQL across lines with a
   backslash continuation — `store.rs:844`: `"select height, prev_digest, digest from
   handoff_receipts \` / `where tenant_ref = $1 …"`. A query written the same way with `decision` on
   the first line and `from handoff_receipts` on the second never matches the guard.
2. **One file.** `grep -rln "handoff_receipts" core/crates/*/src/` returns `store.rs`,
   `migrations.rs` and **`cli.rs`** — the last is not scanned at all.

The underlying exposure remains nil (nothing reads the column today, and I confirmed the API serves
from `body`), so this is a weak guard rather than a live defect. It is worth naming because the
commit message says *"my first attempt to inject it silently did not match, and the test passed,
which is precisely why a guard nobody has seen fail is not a guard"* — the author found the
fragility and shipped the fragile version.

---

### N-10 (Medium, demonstrated) — Row-level security is permissive when the tenant GUC is unset, so the second line of defence fails open on the mistake it exists to catch

The policy on all 20 tenant-scoped tables is:

```sql
COALESCE(current_setting('handoff.tenant_ref', true), '') = ''
  OR tenant_ref = current_setting('handoff.tenant_ref', true)
```

Against a genuine least-privilege role (`rolsuper` and `rolbypassrls` both `f`) with both tenants
holding rows:

```
no tenant named, no predicate  -> 22 rows, 2 tenants     <-- every tenant's rows
tenant A named, no predicate   -> 21 rows, 1 tenant      <-- correct
```

`SECURITY.md:68-70` and `core/dev/README.md:57-59` are carefully worded — *"each request-scoped
transaction names its tenant before reading, so a query that lost its `WHERE tenant_ref = …` still
cannot see another tenant's rows"* — and that sentence is true as written. The gap is that the
conditional half is an unenforced convention: a query that is outside `tenant_tx` **and** missing its
predicate returns every tenant, and no test asserts that request-scoped paths go through `tenant_tx`.
A developer who forgets the `WHERE` clause is likely the same one who wrote a raw query on the pool.

Related, from R-8's fix (`cb1604f`): the fix is **documentation, not coverage**, which the commit
message says plainly and honestly. Two residues: the hard assertion is still `assert!(covered > 0)`
— satisfied by 1 of 19 — and the sentence naming the eight uncovered tables is an `eprintln!`, which
`cargo test` captures and never prints on a passing run, so "keeps the gap in view" is invisible in
the green case. And the population is **20**, not 19: `handoff_request_dispositions` is tenant-scoped
and RLS-enabled (`migrations.rs:480-502`) but absent from `TENANT_SCOPED_TABLES`, so it is neither
proven nor named as unproven. `SECURITY.md:68`'s *"Every `handoff_*` table has RLS enabled and
forced"* is also an absolute with one exception, `handoff_migrations` (21 tables, 20 covered).

---

### N-11 (Medium) — The Python SDK's tests and the entire managed tier run in no CI job

`ci.yml`'s `python-sdk` job runs `compileall`, `ruff`, an import check and `python -m build`. There
is no `pytest` step anywhere: `grep -n pytest .github/workflows/*.yml` returns nothing.
`sdk/python/tests/` holds **65 test functions**, and `sdk/python/README.md:140` documents
`python3 -m pytest tests/ -q`. That directory is where the receipt-chain assertions of R-1 live — so
the SDK half of the seam that produced N-1 is verified by nobody. `managed/` is a separate workspace
and the Rust job is pinned to `working-directory: core`, so its **76 tests** (`managed/README.md:64`)
run nowhere. R-10 reported both; neither moved.

---

### N-12 (High, demonstrated) — The normative documents contradict the implementation, and each other, in four separate places

Four independent instances, each verified by me at HEAD. Individually each is small; together they
are the finding, because the audience they mislead is exactly the one this repository is for — an
implementer who reads the normative documents and writes code from them. `CONTRIBUTING.md` calls an
ambiguity in a normative document a real defect; every one of these is stronger than an ambiguity.

**(a) `height` is specified 0-based and implemented 1-based.** Three normative sites say 0-based:
`spec/signing.md:272` (*"the receipt's **0-based** position in the tenant's chain, ASCII decimal"*),
`spec/schemas/receipt.schema.json:646` (same words, `minimum: 0`), and `spec/openapi.yaml:3907-3909`
(`ChainLink.height`, `minimum: 0`). The implementation is 1-based: `receipt.rs:747`
`previous.map_or(1, |p| p.height + 1)`, and `verify_chain` (`receipt.rs:794`) computes
`position = index + 1` and **rejects** any receipt whose height disagrees. Verified live on a virgin
tenant:

```
FIRST receipt: height=1  prev_digest=sha256:0000…0000
spec/signing.md:272 says "0-based position" -> would be 0
```

`height` is the **first field of `chain_input`**, so this is not cosmetic: a server built from the
spec seals its first receipt over `"0\n<zeros>\n<core_hash>"` and the reference over
`"1\n<zeros>\n<core_hash>"` — different digests for identical content, and the reference verifier
rejects the spec-conforming chain outright with *"expected height 1, found 0"*. Nothing catches it,
because both SDKs take `height` **from the receipt** rather than from position, which is precisely
why it survived. This is the same family as N-1: the specification, followed exactly, produces
something the reference implementation refuses. 1-based is what is built, stored and enforced, and it
reads correctly — the zero digest stands in for a predecessor at height 0 that does not exist — so
the three spec sites are what should move. C-15 currently asserts only that the reported head agrees
with the chain; it should also pin that the first receipt in a tenant has height 1 and the genesis
`prev_digest`, or the next drift is silent too.

**(b) `spec/signing.md:431` states the superseded number rule as normative.** It says
`handoff-protocol-v0.1.md` §1.4 requires *"numbers in digest-covered objects MUST be `0` or within
`1e-6 ≤ |x| < 1e21`, integers MUST be within ±(2^53 − 1)"*. §1.4 (`:91-95`) now requires **integers
only**, and explicitly retires the band at `:97-105` as *"an earlier revision… correct about the
notation and still leaves the value unsafe."* Two normative documents in one release disagree about
what a Server must reject, and the one that is wrong is the signing document — the one an independent
verifier reads. The paragraph that follows compounds it by reasoning from the retired band to the
conclusion that *"a complete JCS canonicalizer and one that refuses to emit numbers outside the band
produce identical bytes… They cannot diverge."*

**(c) `spec/schemas/request.schema.json:196-197` admits a document the reference server rejects.**
A number field's `min` and `max` are declared `{"type": "number"}`. `requires` is digest-covered
through `request_digest`, so the integers-only rule applies. Verified live:

```
raise with min=0    -> 201        raise with min=0.5  -> 400 invalid_request
raise with min=1    -> 201        raise with min=2.5  -> 400 invalid_request
                                  raise with min=-0.5 -> 400 invalid_request
   "0.5 is not an integer, and digest-covered content carries integers only…"
```

A client generated from the published schema hits a runtime 400 for a document its own validator
called legal. These are the only two `"type": "number"` declarations in `spec/schemas/`
(`grep -n '"type": "number"' spec/schemas/*.json`), so the fix is two words.

**(d) `spec/CHANGELOG.md` makes two contradictory "now" statements about one unreleased version.**
`:127` — *"§1.4 **now** constrains digest-covered numbers to `0` or `1e-6 ≤ |x| < 1e21`"* — and
`:213` in the same 0.1.0 section — *"**Digest-covered numbers are now integers only.**"* Lowest
stakes of the four, since a changelog is not normative, but it is the same defect: a document
describing a state the tree no longer has, in the present tense.

Related and already counted separately: **N-6**, where §1.2 and §18 of the *same* normative document
define Level 1 differently. That makes five normative contradictions across the spec set.

---

### N-13 (Low-Medium) — R-4's redaction is undone by a committed script in the same directory

`examples/night-hack/RUNBOOK.md:110` is properly redacted and says so: *"The original text carried a
working request against a live, unauthenticated deployment that rang a real phone."* In the same
tracked directory, `demo/agent.py:53` defaults `HANDOFF_URL` to `https://handoff.omegas.dev` — the
real host per `SECURITY.md:52` — `:128` sets `PAGE_PHONE = True`, and `:520-521` makes paging
opt-**out**. `python3 demo/agent.py --scripted` with no arguments is the redacted curl, one command
shorter. `agent_sprite.py:337` correctly defaults `page=False`; `agent.py` does not.

Related contradiction, unresolved across three rounds: `README.md:41` *"there is no hosted service"*
vs `SECURITY.md:52` *"a deployment of it has existed"* vs `docs/cutover-plan.md:106` *"that hostname
**already serves** the night-hack demo… this step **replaces a running service**."* These cannot all
be true, and which one is right determines whether N-13 is a live hazard or a dead link.

---

### N-14 (Low) — Residual latent traps in the matcher, and one live one

- **`set_equals` is the one empty-match-set operator without a guard.** `expect.rs:309-325`:
  `set_equals: []` against a path resolving to nothing is `[] == []` → PASS. `AllEqual`, `NoneEqual`
  and `Exists(false)` each carry an explicit guard *with a comment explaining the trap*; this one was
  missed. All 8 live uses have a non-empty `want`, so it is latent — armed for the next author who
  writes "assert the page is empty" the obvious way.
- **`single()` silently takes `hits[0]`** (`expect.rs:142-147`). Eleven operators route through it, so
  a wildcard path with N hits is checked only at index 0. No live use, nothing warns.
- **C-17 asserts three negatives with no positive control** (`c17:69-81`, `:85-94`, `:129-139`):
  `not_contains_text` on a listing never shown to be non-empty, and `exists: false` on a document
  never shown to be the request. `http.rs:48-53` turns an unparseable body into JSON `null`, so
  `path: ""` + `not_contains_text` passes on any garbage. C-17 is Level 2 and unrunnable against this
  build, so this is live but low-stakes.
- **R-14 is genuinely closed**: `signing.rs:303` is now `(?i)\b(https?|wss?)://` and
  `tenancy_not_derivable_from_body` walks the whole document rather than the top level.

---

## §D. What I attacked and could **not** break

This is the calibration for everything above. All of it ran against a live `handoffd` on a
disposable Postgres with a second machine principal and a second human principal added.

**The R-1 fix is real for the shape it was reported in.** `receipt.rs` now implements §2.2 as two
steps, removes the whole `chain` member for the core hash, and *stores* the 64-zero genesis
`prev_digest` rather than substituting it — so a party holding one receipt can verify it without
being handed the chain. Six receipts minted through the ordinary answer path verified under the
unmodified Python SDK, under the TypeScript SDK, and under a verifier I wrote from the spec text
with my own from-scratch JCS. Fixture `08-receipt-decision.json` verifies as published. The callback
half of `signing.md` remains correct.

**F-1 is genuinely closed, re-confirmed by direct attack.** One key reused across two different
objects: `answer` names R1 then R2 with two distinct receipts, both settle, and a genuine retry on
R1 still replays the identical receipt id. Same for `cancel` on two objects. Migration 13 —
*"idempotency keys are scoped to the object they act on"* — applies at boot.

**The integers-only rule holds on every route but one.** `1.5`, `0.1`, `1e-6`, `1e20`, `2^53` → `422
answer_validation_failed`, request still `pending`, nothing created. Floats smuggled inside a
`document` value — as an object member, as an array element, bare, and nested four levels deep —
are all refused. `42` and `2^53 − 1` accepted. The **raise** path fails closed too, which is the
half I had not tested until N-12(c) sent me back to it: a non-integer `min` on a number field is
refused at raise with `400 invalid_request` and a message that explains the rule, rather than
surfacing later as an unverifiable receipt. Only `-0.0` slips through (N-2).

**I10, exactly-one-effect.** Redeem `eff-1` → `200 first_redemption: true`; redeem `eff-1` again →
`200 first_redemption: false` (correct retry semantics); redeem `eff-2` → `409 authorization_spent`.

**I15, requester ≠ decider, enforced by principal type.** With a second machine principal in tenant
A — which the shipped bootstrap cannot express and which is the only way to test the property at all
— a machine that did **not** raise the request is still refused `403 requester_may_not_answer`. The
enforcement is by type, exactly as §4.2 requires. The suite cannot see this (N-8); the server does it
correctly.

**Tenant isolation — I found nothing.** Every endpoint, as tenant B's machine key *and* tenant B's
human key, against tenant A's identifiers: request, receipt, deliveries, authorization, redeem,
cancel all `404/404`; answer `403` (machine, by type before lookup) / `404` (human). No response
leaked a tenant A identifier or any prompt text. With a genuine least-privilege role and both
tenants holding rows, naming tenant A returned exactly tenant A's 21 of 22 rows.

**No cross-tenant existence oracle — re-tested specifically, because a candidate one was reported to
me.** The 403-vs-404 split on `/answer` looks like an oracle until the negative control is
well-formed. Re-run by me, with a 26-character ULID that was never issued:

```
as a MACHINE principal in tenant B, POST /requests/{id}/answer
  exists, foreign tenant            -> 403 requester_may_not_answer
  never issued, WELL-FORMED 26 char -> 403 requester_may_not_answer     <-- identical
  malformed 25 char                 -> 404 request_not_found            <-- ID parsing, before lookup
as the same principal, GET /requests/{id}
  exists, foreign tenant            -> 404        never issued -> 404   <-- identical
```

The refusal is by principal type and runs before the object is touched, so a machine learns nothing
about existence, and a `GET` is uniform. The reported oracle was an artifact of a 25-character
negative control being rejected by the ID parser for a different reason than the one under test.
Recording it because the method is the point: a negative control has to be well-formed, or it answers
a different question than the one asked.

**Receipt tamper-evidence, within its stated limits.** With all three storage triggers dropped — the
DB-compromise model §9.4 layer 3 exists for — altering a receipt `body` is detected and named
(*"the recorded digest does not match the receipt's content; it has been altered"*, exit 1). Tail
truncation is **not** detected (deleting heights 2–11 leaves `OK — 1 receipt(s)`, exit 0), which
§9.4:1005-1015 now states plainly and correctly and which I re-confirmed rather than assumed.

**R-12 is genuinely closed.** An unparseable receipt now produces two `BROKEN` lines and **no `OK`
line at all** for that tenant — `verify_chains` (`cli.rs:60-75`) `continue`s rather than verifying
the survivors, with a comment explaining exactly why a head over a subset would be worse than
useless. One cosmetic note for anyone building on it: the literal substring `OK` still occurs in the
output, inside the word `BROKEN`, so an operator's `grep OK` matches. `grep -w OK` or `grep ': OK —'`
is the correct probe, and the tool's exit code is authoritative and correct.

**R-3's exit-code half is genuinely closed.** With every hook stubbed by a one-word command, seven
cases fail where all of them previously passed. 20 of 21 hook expectations now demand output
evidence. The regression is that the evidence is self-asserted (N-3), not that the fix was absent.

**The conformance harness itself is sound.** I reproduced `25/25 passing, exit 0` independently. A
missing hook, principal or fixture directory is a failure and never a skip. `audit_coverage` reads
the case files from disk. `profile.flag()` errors on a missing flag rather than defaulting false.
Running the suite twice against one database correctly produced failures — the residue hazard
`run-conformance.sh` documents at length — and the script's per-run database, kernel-allocated
ports, database-name blocklist, `lsof` pre-flight refusal and child-liveness check are all real
defences against measuring the wrong thing.

**Managed tier honesty.** I re-checked both prior reviews' conclusion and agree: no fake.
`signer.rs` `attest()` returns `Err(MissingDependency::ATTESTATION_KEY)`; `takeover.rs` errors on
mint and revoke and refuses to fall back to a broadcast URL; delivery yields
`Suppression::NotConfigured` rather than a fake success; `FakeControlPlane` and `StaticJwks` are
compiled but never constructed by `main.rs`.

**Repo hygiene, machine-checkable half.** No tracked build artifacts, no AWS keys, no private keys,
no bearer tokens. `RUNBOOK.md`, `SUBMISSION.md` and `PAGING-UX.md` are genuinely redacted and
`~/hipocampus/.env` is gone from the tracked tree — R-4's documentation half is closed. The residue
is `demo/agent.py`'s live default (N-13).

---

## Status of every prior finding

**Round 1.** F-2 and F-7 are folded into R-2/R-9 and assessed there.

| | Round 1 claim | Status at `e3faeb2` |
|---|---|---|
| **F-1** | Idempotency slot omitted the object id | **CLOSED — re-confirmed by direct attack** (§D) |
| **F-2** | CI never runs conformance against the real server | **CLOSED in substance, but see N-5**: the job exists and derives its count; it has still never run |
| **F-3** | `resume_payload` plaintext under a false doc comment | **CLOSED** (level derived from the same config; refusal verified by round 2) |
| **F-4** | §18's C-24 row contradicts §1.4 | **CLOSED** in §18 and in `conformance-map.json` |
| **F-5** | RLS inert under a superuser | **CLOSED as documentation**; the property is weaker than the words imply (**N-10**) |
| **F-6** | Status documents stale | **PARTLY CLOSED — recurred in the spec itself** (**N-6**) |
| **F-7** | Documents claim verification nothing performs | **PARTLY CLOSED — one claim is still vacuous** (**N-5**, the address check) |
| **F-8** | Chain misses tail truncation, unstated | **CLOSED** — stated plainly, and I re-confirmed both halves |
| **F-9** | Retired claim language in hack docs | **PARTLY CLOSED** — the retirement note enumerates two files; four more live instances remain (`PAGING-UX.md:30`, `app/page.py:283,325`, `demo/agent_sprite.py:310`), plus `BACKLOG.md:153` unmarked |
| **F-10** | Credential map, unauthenticated curl | **PARTLY CLOSED** — docs redacted, `demo/agent.py` still points at the live host with paging on (**N-13**) |

**Round 2.**

| | Round 2 claim | Status at `e3faeb2` |
|---|---|---|
| **R-1** | Server's chain digest ≠ `signing.md` §2.2 | **CLOSED FOR THE REPORTED SHAPE, REOPENED BY TWO NEW ROUTES** (**N-1**, **N-2**), and the fixture it required fixing is still wrong (**N-4**) |
| **R-2** | Conformance gate hardcoded `24/24` | **CLOSED** — the count is derived and the stub probe is real. The workflow has still never run (**N-5**) |
| **R-3** | Hooks satisfiable by `true`/`false` | **HALF CLOSED** — exit codes fixed and verified; evidence is self-asserted, C-15 defeated (**N-3**) |
| **R-4** | Live endpoint + phone-ringing curl tracked | **PARTLY CLOSED** — see F-10 / **N-13** |
| **R-5** | `GATE.md` stale again | **CLOSED** for `GATE.md`, with a CI check; **recurred in the spec and changelog** (**N-6**) |
| **R-6** | `conformance-map.json` still said `1e20 → accepted` | **CLOSED — verified** |
| **R-7** | `conformance/README.md` claimed C-17 passes | **CLOSED — verified** |
| **R-8** | Per-table RLS asserted on empty tables | **DOCUMENTED, NOT CLOSED** — honestly so; residues in **N-10** |
| **R-9** | Fixture claims asserted by hand | **PARTLY CLOSED** — schema binding is real and sharp; the address half is vacuous (**N-5**) |
| **R-10** | Flaky ports; managed + Python SDK outside CI | **PARTLY CLOSED** — ports fixed; both suites still run nowhere (**N-11**) |
| **R-11** | Document contradictions, "ten of ten" | **MOSTLY CLOSED** — the assertion count is corrected and consistent; the hosted-service contradiction survives (**N-13**) |
| **R-12** | `verify-chain` printed `OK` for a broken tenant | **CLOSED — verified by direct attack** (§D) |
| **R-13** | `handoff_receipts.decision` outside the chain | **CLOSED as wording**; the new guard is weak (**N-9**) |
| **R-14** | `wss://` missing from the resolvable matcher | **CLOSED — verified** (`signing.rs:303`) |

---

## Verdict

**No. This is not ready to publish as an open protocol.**

Five things block it. The first three are the same defect wearing different clothes — **a mechanism
is verified against itself, and nothing spans the seam** — and the fifth is a specification that
does not agree with the thing it specifies.

1. **N-1 and N-2 — the two published SDKs disagree with each other about receipts the reference
   server mints.** The Python SDK reports a valid receipt as forged when a `document` field carries a
   non-ASCII key, and again when a `number` field carried `-0.0`. Both are reachable through the
   ordinary answer path by an ordinary answerer, and the first poisons a whole tenant's chain for
   every Python verifier. R-1's closing recommendation asked for a conformance case that hands a
   **server-minted** receipt to a **spec-derived** verifier, precisely so the seam would not reopen.
   The case was not written, and the seam reopened within one round. This is release-blocking for the
   same reason R-1 was: the receipt chain is the one mechanism the protocol says needs no trust, and
   a protocol whose two reference clients cannot agree on the same bytes is not a protocol yet.
2. **N-3 — C-15 passes against a deployment that implements nothing it asserts.** Fifteen lines of
   shell that touch no storage and walk no chain produce a 25/25 green run. `GATE.md` is the artifact
   that converts "we ran the suite" into evidence, and today it converts nothing. The hooks are handed
   the values they must echo; the fix needs the harness to check the claim rather than accept the
   narration.
3. **N-4 — a published normative fixture still fails the project's own verifier**, one round after
   being named in the fix list, because both SDK suites recompute the digest before asserting it.
4. **N-5 and N-7 — the first CI run will be red on three axes**: two jobs fail on a clean checkout,
   the address-leak check is a no-op, and no commit carries the sign-off `CONTRIBUTING.md` says
   cannot merge. A workflow that has never executed is not a check, and this one has never executed.

5. **N-12 and N-6 — the normative documents contradict the implementation, and each other, in five
   places.** `height` is specified 0-based and implemented 1-based, and since `height` is the first
   field of `chain_input`, a server built from the spec produces a different digest for identical
   content and has its chain rejected outright by the reference verifier. `signing.md:431` states
   the retired number band as the current rule while §1.4 says integers only. The published request
   schema admits a `min: 0.5` the server refuses with a 400. §1.2 and §18 of one document define
   Level 1 differently. The changelog makes two contradictory "now" statements about one unreleased
   version. This is a **specification** being published, and its normative statements have to be
   true; each of these is the F-4 failure — an implementer working from the document and failing for
   a reason the document does not explain. None is hard to fix, and none should ship.

**N-8** through **N-11**, **N-13** and **N-14** I would not block on, though N-8 is the one I would fix next: the suite
cannot currently distinguish the rule it calls load-bearing from a weaker rule that would let one
agent approve another's refund. The server implements it correctly, which is the only reason this is
a coverage finding rather than a vulnerability.

What I want on the record, because it should weigh as heavily as the list above. I spent most of
this review trying to break the parts that are genuinely hard, and they held: exactly-once redemption,
first-writer-wins, tenant isolation across every endpoint with two kinds of credential, requester ≠
decider enforced by principal type against a second machine principal the shipped fixtures cannot
even express, the integers-only rule against every smuggling route but one, storage-level
immutability under real triggers, tamper detection on an altered receipt, and a `verify-chain` that
now refuses to say `OK` about a tenant it has just called broken. Several round-2 fixes are not
merely present but *well made* — R-12's `continue` carries a comment explaining why verifying the
survivors would be worse than useless; R-8's fix is honest about being documentation rather than
coverage and names the eight tables it does not prove; the R-1 fix stores the genesis `prev_digest`
rather than substituting it, so one receipt can be verified without the chain. That is the work of
people who understand what they are building.

The pattern in the defects is the one both prior reviews named, and it has not changed, which is why
naming it a third time is the useful thing: **the parts that were built are sound, and the parts
meant to keep them honest are the ones that do not run.** The chain verifier now agrees with the
spec, but nothing tests it against a receipt with an unusual key. The hooks now demand evidence, but
the evidence is supplied to them. The count is now checked, in one of the four files that hold it.
The CI that would catch all of this has never executed once.

One process observation, offered as a finding about the review rather than the code: this branch
moved twice while I was reviewing it, and I was briefed against a commit and a commit count that
were both already stale. Three hostile reviews have now each found the previous round's fixes
partially undone or newly evaded. That is not a code problem — it is what happens when fixes are
written and the checks that would confirm them are not run. **Push the branch, let the workflow
execute once, and fix what it says** before commissioning a fourth review; a green first CI run
would have caught N-5 and N-7 without a reviewer, and N-1 would have been caught by the one
conformance case R-1 already asked for.

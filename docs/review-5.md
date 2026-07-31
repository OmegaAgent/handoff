# Hostile review 5 — final verdict

**Reviewer:** the third hostile reviewer, closing out. 2026-07-31, against
`39275378ff4af46768bf2b0101808104529bd62e` on `v1/protocol`, pushed, tree clean. Read-only on all
source; the only file written is this one.

This re-verifies the single blocker left open by `docs/review-4.md` — that `.github/workflows/ci.yml`
had never executed — and attacks the three fixes the first real CI run forced.

---

## The blocker is cleared, and I verified it rather than accepted it

`review-4.md` held publication on one criterion, written blind in `review-3-acceptance.md`:

> There is **an actual workflow run**, on a pushed branch, that is green. Not a local reproduction,
> not a claim that the jobs would pass — a run.

Queried directly:

```
$ gh api repos/OmegaAgent/handoff/actions/runs --jq .total_count          2      (was 0)
30633185643  3927537  completed  success
30632624179  cd9b486  completed  failure
```

The success run's jobs, from the API rather than from the summary I was handed: **twelve success,
one skipped**, and the skipped one is `a spec change must carry a conformance case`. The failed run
failed exactly the three jobs described — `sdk/python — lint, import, build`, `sdk/ts — lint, test`,
and `no credentials or capability tokens in the tree`. The account I was given matches the API in
every particular.

### The skip: correct, but not for the stated reason

I was asked to confirm the skip is a path filter doing its job. **It is not a path filter.** The job
carries `if: github.event_name == 'pull_request'` and both runs were `push` events, so it would have
skipped even if the push *had* touched `spec/`. The skip is correct — the job diffs against
`origin/${{ github.base_ref }}`, which only exists on a pull request — but the reason given ("since
that push touched no spec file") is wrong, and the distinction matters: this job is still the one
piece of CI that has never executed, and it will not execute until the first pull request.

That is inherent to a PR-gated check rather than something a push could have fixed, so it does not
reopen the blocker. But my own rule says a job nobody has watched run is not yet a check, so I
exercised its logic myself against synthetic diffs:

| changed paths | result |
|---|---|
| spec change + **added** case | PASS |
| spec change + **modified** case | PASS |
| spec change, no case | **REFUSED** — "spec changed, no case arrived" |
| spec change + **deleted** case | **REFUSED**, twice over |
| case deleted, no spec change | **REFUSED** |
| no spec change | PASS |

Sound in all six directions, including the exact bypass I reported in `review-3.md` N-5 — a spec edit
plus the deletion of `c01_idempotent_raise.yaml` used to satisfy the old check and go green. It now
refuses deletion outright and separately, which is the right shape given §18 requires a withdrawn
case to keep its identifier.

### The finding that paid for itself

`review-4.md` reported that the `conformance` job needs `psql` and no step installed it, while the
`rust` job installed it explicitly — found by reading the workflow, not by running it. That was fixed
before the first run (`af98cf7`), and the job passed. Had it not been, the first run would have
failed four jobs rather than three. I note it because it is the one place where reading beat running,
and the rest of this document is the opposite lesson.

---

## The three fixes, attacked

### 1. TypeScript — the `ArrayBuffer` copy is genuinely version-proof, and I proved it rather than agreed with it

This was the one I was asked to attack hardest, on the correct instinct that a compile error appearing
on one toolchain will come back. It will not, and the reason is structural rather than lucky.

The fix replaces `bytes as unknown as ArrayBufferView` with `asBuffer(bytes)`, which allocates a fresh
`new ArrayBuffer(n)` and copies into it. `BufferSource` is `ArrayBufferView | ArrayBuffer` in every
version of the ambient types; whatever narrowing happens to the `ArrayBufferView` arm — and the
narrowing that broke CI was `ArrayBufferView<ArrayBuffer>` against a `Uint8Array` typed over
`ArrayBufferLike` — the `ArrayBuffer` arm is the concrete, non-generic type. `new ArrayBuffer(n)`
produces exactly that. **The fix does not depend on the generic parameter at all**, which is why it
cannot go stale when the parameter moves again.

I proved it at the type level, declaring both ambient shapes myself and compiling under TypeScript
5.9.3 with `--strict`:

```ts
type BufferSourceOld = ArrayBufferView | ArrayBuffer;
interface ArrayBufferViewOf<T extends ArrayBufferLike> { buffer: T; byteLength: number; byteOffset: number }
type BufferSourceNew = ArrayBufferViewOf<ArrayBuffer> | ArrayBuffer;

const a: BufferSourceOld = asBuffer(view);   // legal
const b: BufferSourceNew = asBuffer(view);   // legal
// @ts-expect-error
const c: BufferSourceNew = view;             // the old code
```

Exit 0 — both assignments legal *and* the `@ts-expect-error` satisfied, meaning the old code really
is rejected. Removing the suppression reproduces CI's error verbatim, down to the wording:

```
error TS2322: Type 'Uint8Array<ArrayBufferLike>' is not assignable to type 'BufferSourceNew'.
    Types of property 'buffer' are incompatible.
      Type 'ArrayBufferLike' is not assignable to type 'ArrayBuffer'.
```

The only way this breaks is if `BufferSource` stopped admitting a plain `ArrayBuffer`, which would
break every WebCrypto consumer in the ecosystem. "A cast asserts, a copy establishes" is the right
principle and this is a correct application of it.

**Two things I checked and did not find fault with.** The copy is behaviourally safe for a subarray
view: `bytes.byteLength` is the view's length and `.set(bytes)` copies the view's logical contents,
so a `Uint8Array` with a non-zero `byteOffset` hashes the same bytes it did before. And CI genuinely
typechecks — `npm run typecheck` is `tsc --noEmit -p tsconfig.json`, a real compile, not
`node --experimental-strip-types`, which erases types and would never have caught this.

**One residual, benign.** `sdk/ts/src/client.ts:98` still carries `(globalThis as any).process?.env`.
That is a different cast — runtime feature detection in a cross-runtime SDK, where `any` is the
idiomatic spelling — and not the class that failed. Every cast of the failing class is gone.

At this HEAD: `tsc --noEmit` exit 0, `npm test` **83 pass, 0 fail, 0 skipped**.

### 2. Python — `cryptography` as a test-only dependency, and the tests fail rather than skip

`dependencies = []` in `pyproject.toml`, and the `sdk/python — standard library only` job still
asserts it, so the property that made the SDK dependency-free is intact. `cryptography==43.0.1` is
pinned and installed only in the test step and the published-artifacts step.

The attack worth making is whether those two tests now **skip** when the module is absent, which
would reproduce the original defect in a quieter form. They do not. There is no `importorskip`, no
`skipif`, no `try/except ImportError` anywhere in `sdk/python/tests/`. A missing module is a collection
error — which is exactly what the first CI run reported, `ModuleNotFoundError` rather than a failure
or a skip.

Better than that, and worth recording because it is the strongest anti-vacuity construction in the
repository: the test **extracts the reference verifier from `spec/signing.md` at run time** and
executes it, locating the marker and the fence structurally, with a docstring saying why — *"a copy
is a second implementation that drifts… a failure to find either is an assertion failure rather than
a skip."* So the snippet a third party would copy out of the specification is the snippet under test.
That is the seam that produced N-1, and it is now the only seam in the project guarded by executing
the document itself. That those two tests had never been asked to run is the single most valuable
thing the first CI run revealed.

### 3. secret-hygiene — the mutation still fails for the reason under test

This is the fix I was most suspicious of, because it changes the **inputs to a test** in order to
satisfy a scanner, and `core/dev/mutation-pass.sh` runs in **no CI job** — so nothing would have
caught it if the rewrite had quietly changed what the mutation measures. Their own rule applies: a
negative control must fail for the reason under test.

The C-18 mutation's secrets went from runs of `f` and `e` to all-zeros of two different lengths. A
malformed or rejected secret would make the server refuse to start, and C-18 would then fail because
nothing worked rather than because signatures mismatched — a green mutation pass proving nothing. I
ran it:

```
SERVER STARTED AND SERVING: yes
25/26 passing — exactly one case failing, C-18

FAIL C-18   step: the signature binds body, timestamp, and delivery
  callback 0 does not verify against the configured secret:
  no active secret produces any of the offered v1 values
```

The server starts, exactly one case fails, it is the case that owns the property, and the failure is
a signature mismatch — the property under test, not a side effect. The rewrite is sound.

Choosing the scanner's existing all-zero stand-in convention over an exemption was the right call for
the reason given: a check that has to special-case its own repository is weaker than one that does
not. **The residual is that `mutation-pass.sh` is unguarded** — it is a dev script no job runs, so
the next edit to it has nothing watching. Not blocking, and worth a line in `BACKLOG.md`.

---

## Nothing regressed in the final push

The push touched `sdk/ts/src/document.ts`, `sdk/ts/src/signing.ts`, `core/dev/mutation-pass.sh` and
`.github/workflows/ci.yml` — two of which are the files my N-1 verification depended on. Re-measured
at this HEAD rather than assumed:

- **Conformance: 26/26, exit 0.**
- **`scripts/verify-minted-receipts.sh`: exit 0** — both payloads (`ordering`, the N-1 key shapes;
  `normalization`, the N-2 number shapes) minted by `handoffd` and verified under both SDKs.
- **TypeScript SDK: 83 pass, 0 skipped**, including *"every published receipt fixture verifies as
  published"* — N-4's assertion, still holding after its file was edited.
- **`tsc --noEmit`: exit 0.**

The class residual I recorded in `review-4.md` is unchanged and remains a recommendation rather than
a finding: the cross-implementation gate runs a **fixed** two-payload corpus, not generated input, so
it covers every named shape and would not surprise itself with a third. Property-based generation
across the three canonicalizers is still the instrument I would add next.

---

## Verdict

**Publish-ready. Yes.**

All fourteen findings from `review-3.md` are closed, verified against acceptance criteria I wrote
blind before any fix existed, and the last one — CI had never run — is closed in the only way it
could be: by running, and by failing first.

That it failed first is the point, and I want it on the record rather than smoothed over. Three jobs
went red on the first execution and **not one of them was visible locally**: a TypeScript compile
error that depended on which `@types/node` the machine happened to have, two tests that had never
been *asked* to run because a dependency was never installed — and those two were the tests covering
the seam that produced the release-blocking N-1 — and a scanner catching this repository's own
mutation script. That is precisely the argument `review-4.md` made for holding publication on a real
run, made better by events than by me. A local pass was not the measurement, and now there is one.

The three fixes hold under attack. The TypeScript one is version-proof for a structural reason I
proved rather than accepted: it removes the dependence on the moving type parameter instead of
asserting around it. The Python one fails closed rather than skipping, and executes the
specification's own published verifier as the thing under test. The secret-hygiene one changes the
mutation's inputs without changing what the mutation measures, which I confirmed by running it and
watching C-18 fail for a signature mismatch with the server healthy.

What I would still do, none of it blocking, all of it already written down: property-based
generation across the three canonicalizers (`review-4.md`, the N-1 class residual); a CI job for
`mutation-pass.sh`, which is now the one instrument in the repository that nothing guards; and the
three additions to `verification-pass.md`'s "what it did not reach" — the managed tier was never
probed adversarially, nothing was tested under contention, and nobody has gone looking for an eighth
vacuous test.

Seven vacuous checks were found in this project across five rounds, and the pattern behind them was
always the same: *the parts that were built are sound, and the parts meant to keep them honest are
the ones that do not run.* Every one of them now runs. The conformance suite deletes hooks rather
than trusting them and walks the chain itself; the case that was defeated by fifteen lines of shell
now names that attack in its own rationale and declares what it still takes on trust; the count is
derived rather than held; the fixture-leak scanner tests that it fires; the receipt seam is guarded
by two independent implementations and by executing the specification's own verifier; and the
workflow that would have caught the rest has now caught three things nobody could see from a laptop.

This is ready to publish as an open protocol.

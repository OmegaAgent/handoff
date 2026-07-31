# Review of `docs/independent-verification.md`

**Reviewer:** the third hostile reviewer, 2026-07-31, at the maintainer's request, against the draft
held outside the tree. This is a review of an *evidence record* rather than of code: the question is
not whether the measurements were taken but whether the sentences describing them are worth what a
reader will spend on them.

**Short answer to the four questions.** Two sections overclaim materially and one of them is
falsified by a defect that was live in the tree at the moment the pass ran. The "what this does not
cover" section is missing five things, one of which is the largest. The false positive reads as
honest but is currently positioned so that an unkind reader could call it performance, and the fix is
not to remove it. And yes, it should be committed — but retitled, pinned to a commit, shipped with
the code that produced it, and with its most valuable section promoted out of prose and into CI.

**Before any of that: attacking §1's independence claim produced a finding that is not in
`review-3.md` and that enlarges N-1.** It is the most important thing in this review, so it goes
first.

---

## 0. New finding — the specification itself specifies the wrong sort order, and its own reference verifier implements the bug

`review-3.md` N-1 says the Python SDK sorts object members by code point where JCS requires UTF-16
code units, and concludes *"the reference server and the TypeScript SDK are correct and the Python
SDK is wrong."* **That conclusion was wrong, and checking this document is what exposed it.**

`spec/signing.md:425`, normative:

> Every digest in this document is taken over **RFC 8785 (JSON Canonicalization Scheme)** output,
> UTF-8 encoded: object members **sorted by code point**, no insignificant whitespace, and the RFC's
> number serialization.

RFC 8785 sorts by **UTF-16 code units**, not code points. The two orders differ for any non-BMP
character against any BMP character above U+D7FF. So the document names RFC 8785 and then defines a
different ordering in the same sentence.

And `spec/signing.md:398-402` ships a **"Reference verifier (Python, `cryptography`)"** that
implements the same error, with a comment asserting it is safe:

```python
# canonical_json must be RFC 8785 (JCS); json.dumps with sorted keys and no
# whitespace matches it for the value types this protocol uses.
core_bytes = json.dumps(core, sort_keys=True, separators=(",", ":"),
                        ensure_ascii=False).encode("utf-8")
```

`sort_keys=True` is code-point ordering. The hedge — *"for the value types this protocol uses"* — is
about value **types** and says nothing about key **charset**, which is where the divergence lives.
Nothing in the repository executes this snippet, so it has never been tested.

This re-frames N-1 in a way that matters for who has to change:

| | orders by | agrees with RFC 8785 | agrees with `signing.md` |
|---|---|---|---|
| `spec/signing.md:425` + its reference verifier | code point | **no** | — |
| Python SDK | code point | no | **yes** |
| Rust `receipt.rs`, TypeScript SDK | UTF-16 code unit | **yes** | **no** |

The Python SDK is not a rogue implementation. **It is the only one that faithfully implements the
published specification**, and the specification is wrong about the standard it names. A third party
implementing from `signing.md` — or copying the reference verifier, which is the blessed thing to do
— produces the Python behaviour and disagrees with the reference server. That is a larger blast
radius than N-1 as filed, and it means the fix is not "correct the Python SDK" but "correct
`signing.md:425`, correct the reference verifier, then correct the Python SDK to match."

I am recording it here rather than editing `review-3.md`, which is committed and which the lanes are
working against; fold it into N-1 however you prefer.

*(Noticed in passing, offered as a heads-up rather than a review, since I have deliberately not read
the lanes' diffs: `signing.md` §3 in the working tree currently states the **lexical** number rule —
"MUST be written as an integer literal — no decimal point, no exponent" — which you told me was
withdrawn in favour of normalization. Either a lane has not received the correction or the working
tree is mid-edit. Worth a look before the spec lane commits.)*

---

## 1. Which claims are weaker than they sound

### §1 — "A verifier written from the prose alone reproduces all of it"

**Weaker than it sounds, for two independent reasons.**

First, *"from the prose alone"* is a claim about your own discipline that nothing in the artifact
lets a reader check, and the verifier is not committed. A record asserting "I ran something you
cannot see and it passed" is testimony, not evidence — which is the exact structure this repository
has been burned by three times.

Second, and worse: **the prose contains a working Python implementation.** `signing.md:396-416` is
labelled "Reference verifier" and gives complete code for the core hash, the chain input, the digest
comparison and the Ed25519 check. A verifier written while reading that document is at best a
transcription with different variable names. The strongest available evidence that it *was*
independent is that it would have had to reproduce the reference verifier's bug — and since the
payload contained no non-ASCII keys, we cannot tell whether it did.

**Rewrite:**

> A verifier written against `signing.md` §2.2's construction reproduces every published vector:
> the 1125-byte receipt core and its `sha256`, the chain digest at height 4211 over the genesis
> predecessor, the Ed25519 seed derived rather than trusted, the public key, and the signature. All
> four negative vectors fail as required. This checks the *arithmetic* of the published vectors. It
> is not an independence check: `signing.md` §2.5 ships a reference verifier in Python, so a verifier
> written from that document shares its author's reading of it — including, as it turns out, an
> error in the ordering rule that no vector in this set would expose.

Also: the row **"both fixtures are JCS of their own parse — holds"** is true and misleading. "Is JCS
of its own parse" and "its stored digest verifies" are different properties, and
`spec/fixtures/09-receipt-policy.json` satisfies the first while failing the second (`review-3.md`
N-4). A reader scanning this table takes the row as reassurance about the fixture set. Name which
fixtures, and say the digest check is elsewhere.

### §2 — "6 of 6 verified … content chosen to stress canonicalization"

**This is the claim I would strike hardest, because it was falsified at the moment it was made.**

Two canonicalization defects were reachable in the tree at that commit through the ordinary answer
path — a non-ASCII **object key** in a `document` value, and `-0.0` on a `number` field. This pass
minted six receipts with content described as *"chosen to stress canonicalization"* and found
neither.

The reason is precise and worth writing down, because it is the generalizable lesson: the payload
stressed **values** and not **keys**. `café — naïve ✓ 日本語` is unicode in a *string value*, and JCS
ordering is a property of *member names*. Worse, every character in that string is BMP, so even
promoted to a key it would not have exposed the UTF-16-versus-code-point split. Likewise
`±(2^53 − 1)` probes the integer *bound* and not the integer *predicate*, which is where `-0.0`
walked through. Six receipts passed, and both live defects were outside the sampled dimensions.

And "an implementation sharing no code with the server" carries the §1 problem plus a new one: it
verified with **one** implementation. N-1 is precisely a case where two implementations agree and a
third does not, so "verified under a third implementation" cannot establish the property the section
is selling.

**Rewrite:**

> Six requests were raised and answered on a throwaway database, with content varying unicode in
> string values, JSON escapes, integers at ±(2^53 − 1), and nested objects and arrays. All six
> verified under a separately written implementation, and altering one receipt's core was detected.
>
> This is a smoke test of the runtime path, not a canonicalization stress test, and it should not be
> read as one. It varied *values*; JCS member ordering is a property of *keys*, and the review that
> followed found a receipt the reference server mints that the published Python SDK reports as forged
> — a non-ASCII key inside a `document` value — plus a second shape, `-0.0`, that this payload's
> integer bounds did not reach. Both were live at this commit and this pass did not find them. What
> would have found them is a differential test across all three canonicalizers over generated input,
> which is not done here.

### §3 — "The security invariants, black-box"

**The table is accurate; the heading generalizes from eight probes to "the security invariants."**

One row is weaker than it reads. *"the requesting machine answers its own handoff → 403"* is the
only I15 probe, and it uses the **requesting** machine — so it cannot distinguish enforcement by
principal *type* from enforcement by requester *identity*. That is exactly `review-3.md` N-8, and it
is the ambiguity that lets a server which checks `principal.id == request.created_by` pass the whole
conformance suite while letting one agent approve another agent's refund. I ran the stronger probe
with a second machine principal in tenant A and the server is correct — but this pass did not
establish that, and the row reads as though it did.

*"another tenant reads the request → 404"* is one endpoint with one credential kind. Tenant isolation
is a property of the whole surface.

**Rewrite:** retitle to **"A black-box probe of eight invariants"**, and amend two rows:

> | the requesting machine answers its own handoff | `403 requester_may_not_answer` — note this does not distinguish enforcement by principal type from enforcement by requester identity; a second machine principal in the same tenant is required for that, and the dev bootstrap cannot express one |
> | another tenant reads the request | `404 request_not_found` — one endpoint, one credential kind; not a sweep of the surface |

### §4 — "The suite goes red against a server that implements nothing"

**Literally true, and it invites an inference that is false.**

`0/25` against a 501 stub proves the suite is not unconditionally green. It does not prove that any
individual case detects non-implementation, and a reader will take the heading to mean it does.
`review-3.md` N-3 is the counterexample: C-15 — the case §18 says must be asserted from the storage
layer — passes 25/25 against a deployment with no storage-level immutability and no chain verifier,
because the hooks are handed the values they must echo. A 501 stub is the weakest possible negative
control: it fails at the first byte of every response.

**Rewrite:**

> Against the 501 stub the suite reports 0/25, names all 25 cases, and exits 1, re-measured rather
> than re-read from `GATE.md`. This establishes only that the suite is not unconditionally green. It
> is the weakest available negative control — a stub that answers nothing fails at the first
> assertion of every case — and it says nothing about whether any individual case detects a
> deployment that implements *most* of the protocol and not the property that case owns. The review
> that followed showed C-15 does not.

### §5 — "The suite catches a deployment whose behaviour contradicts its profile"

**The best section in the document, and still generalizing from n=2 in a biased sample.**

The technique is genuinely good and the result — exactly one case failed, and it was the owning case
— is the right thing to measure. Two caveats, the second of which is structural:

You already know a mutation caught by one case says nothing about the other twenty-four. The sharper
point is that **both mutations were chosen from the half of the suite that cannot hide a defect.**
`link_only_permitted` and callback signing are directly observable over HTTP, so the suite sees them
without a hook. The properties that are *not* HTTP-observable — storage immutability, chain
verification, channel-content non-authority, secrets in logs, transactional atomicity — are exactly
the ones a configuration mutation cannot reach, and exactly where the vacuity was found. The
technique systematically sampled the tractable half and reported the result as a property of the
suite.

**Rewrite:**

> The reference server was mutated by configuration alone, twice, with the profile left claiming the
> unmutated behaviour. In both runs exactly one case failed and it was the case that owns the
> property, which is the result a suite that failed everything under any perturbation would not
> produce.
>
> Two mutations are two data points, and they are not a random sample: both properties are directly
> observable over HTTP. The assertions that need a hook — storage immutability, chain verification,
> channel-content non-authority, secrets in logs, the crash — cannot be reached by a configuration
> mutation at all, and are where the review that followed found a case passing against a deployment
> implementing none of it. A source-level mutation pass is the instrument that reaches them.

---

## 2. What the pass did not cover but implies it did

The existing three bullets are true. Five more belong there, in roughly descending order of how
badly their absence misleads:

1. **The published SDKs were never run.** Nothing in this pass exercised `sdk/python` or `sdk/ts`
   against server-minted receipts. That is precisely where N-1 lived, and §2's phrasing —
   "receipts a live server minted verify under a third implementation" — reads as though the client
   story was checked. It was not.
2. **All hook-backed assertions.** The draft names only "the crash half of C-23", which understates
   it by a lot. Every below-HTTP hook is outside this pass: both C-15 storage probes, chain
   verification, tamper detection, `channel_inbound`, `observe_page_state_change`, and C-7's `logs`
   source. That is the whole set of properties an implementer cannot check over HTTP — the set most
   worth verifying and the only set this pass structurally cannot see.
3. **Canonicalization of object keys**, as distinct from values. See §2 above.
4. **CI.** No job was executed, and at this commit two were red on a clean tree, one security check
   was a no-op, and neither workflow had ever run. A verification record that does not mention CI
   implies the automated checks are a separate, working layer. They were not.
5. **The documents.** No count, cross-reference, or normative claim was checked. Five normative
   contradictions were live at this commit.

One structural item that is not a coverage gap but belongs in the same section: **this pass predates
the fixes and describes a tree that no longer exists.** Which brings me to the defect in the record's
own provenance.

**The document promises a commit hash and never gives one.** Line 3 says *"against the tree at the
commit named below"*. No SHA appears anywhere in the file:

```
$ grep -nE "[0-9a-f]{7,40}" independent-verification.md
(no matches)
```

For a repository that has twice been bitten by a document whose one job was to hold a measured number
and which held the wrong one, an evidence record whose one job is to say *what it measured* and which
does not say *what it measured against* is the same failure with the same shape. This is the single
cheapest fix in the document and the one I would not let ship.

---

## 3. The false positive: honest, but currently positioned so that it reads as performance

**It is honest, it is the most useful section in the document, and it should stay.** Recording a
finding you talked yourself out of is rare and it is the behaviour the project should want.

The unkind reading, which you asked for: it is the *only* self-criticism in a document that is
otherwise uniformly green, it sits last, and it is a self-correction that makes the system look
**better** rather than worse. That is the flattering kind of error to admit — it demonstrates rigour
while conceding no defect. A reader inclined to be skeptical will notice that the one thing the
author caught himself getting wrong happened to be an over-accusation, and will ask what happened to
the over-reassurances.

The answer, from §§1–5 above, is that there were several and none was caught. So the fix is not to
cut the section — it is to **put honest limitations next to it**. Once §2 says "this payload missed
the two defects that were live" and §4 says "a 501 stub is the weakest negative control", the false
positive stops being the token piece of humility and starts reading as one sample of a consistent
discipline. Self-criticism reads as performance when it is *isolated*, not when it is *specific*.

**On the two rules.** Both are right and both generalize:

1. *A negative control has to be well-formed.* Correct, and it has a sibling worth stating: a control
   must fail for the reason under test. That is the same defect as C-24's would-be stub passing for a
   missing hook rather than a protocol violation.
2. *The moment a probe produces the finding you were hoping for is the moment to attack it.* Correct.

**A third rule is missing, and this document is the evidence for it.** Rule 2 is asymmetric: it
attacks results you *hoped were findings*. Nothing here attacks results you *hoped were
reassurances*. §2's "6 of 6 verified" was exactly such a result — a green outcome, accepted without
the scrutiny the 403/404 anomaly received, at a commit where two defects in the property being
verified were reachable. The rule:

> **A passing probe deserves the same attack as a failing one.** A green result is a claim that the
> defect is absent, and it is the claim easiest to accept without asking what it would have taken to
> see the defect. Before recording a pass, name the shapes the probe could not have detected.

That rule, applied to §2 at the time, would have found N-1 a day earlier. I would put it beside the
other two.

---

## 4. Should it be committed at all?

**Yes — but not under this title, not without its code, and not with its best section left as prose.**

The case against is real and you stated it: a hand-maintained artifact asserting measurements is the
exact shape that has failed twice here, it goes stale the instant the tree moves, and the tree is
moving under five lanes right now. Titled "Independent verification" it will be read as a clearance,
and it is not one — it is one person's pass over one commit, and the review that ran beside it found
four release-blockers the pass did not reach.

The case for is also real: the negative results and the mutation technique are recorded nowhere else,
and deleting them loses the only evidence that anyone tried to make the suite go red on purpose.

Four conditions, in descending order of value:

1. **Promote §5 out of the document and into the harness.** A file saying "I mutated the server and
   the right case failed" is worth far less than a job that does it on every run. Two config
   mutations, each asserting that exactly one named case fails, is perhaps forty lines of shell and
   it converts the document's best contribution into a check. Extend it with the source-level
   mutations you already plan — break `require_person`, break the idempotency slot — and it becomes
   the instrument that would have caught N-8. This is the whole point of the project's own stated
   pathology: *the parts meant to keep them honest are the ones that do not run.* Do not let the
   mutation pass become another one.
2. **Commit the verifier and the mutation scripts alongside the record.** This converts §§1, 2 and 5
   from testimony into evidence, and it is what makes "reproduces from scratch" checkable rather than
   asserted. Without it, the document's central claims cannot be re-run by the reader they are
   written for.
3. **Pin the commit and freeze it.** First line: the SHA, the date, and a sentence saying this is a
   historical record of one pass that will not be updated as the tree moves. A dated record that is
   explicitly stale is useful; an undated one that looks current is a liability.
4. **Retitle.** "Independent verification" claims both independence (§1 shows it is qualified) and
   verification (§§2–5 show it is partial). Something like **"A verification pass, and what it did
   not reach"** describes the artifact and sets the reader's expectation to what it can bear.

If you would rather not do (1) and (2), then my answer flips: delete it. A record whose claims cannot
be re-run, whose best technique stays manual, and whose title overstates its reach is a fourth
hand-maintained artifact, and this repository does not need one. The document is worth keeping
exactly to the extent that it stops being only a document.

---

## What I checked and found sound

Credit where it is due, because the list above is long and the work underneath it is not bad.

The **mutation-by-configuration technique (§5) is the best idea in the document** and I have not seen
it in either prior round. "The stub gate proves the suite can fail; it does not prove the suite can
fail for the right reason" is the correct distinction and it is stated better here than I stated it.

The **Ed25519 seed being derived rather than trusted** (§1) is a real independence measure and
exactly right — trusting a published seed would make the whole vector set circular.

**Re-measuring the stub gate rather than reading `GATE.md`** (§4) is the correct instinct, applied
for the correct stated reason.

Every factual row I spot-checked is **accurate**. The overclaiming is in the headings and the
framing sentences, not in the measurements — which is worth saying plainly, because it means the fix
is editorial rather than a retraction. Nothing here is fabricated and nothing is wrong about what was
run; the sentences just describe a larger thing than what was run.

And keeping the false positive at all, unprompted, is the reason I believe the rest of the
measurements happened as described.

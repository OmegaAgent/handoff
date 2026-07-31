# A verification pass, and what it did not reach

**Measured at `e023c8dbe70c`, 2026-07-31. This is a historical record of one pass. It is not
updated as the tree moves, and it is not a clearance.**

Read the title literally. Three independent hostile reviews sit in `docs/`, and each found defects
the one before it missed; the most recent found seven that block publication. This file records what
one verification pass measured, what it could not see, and — the part worth reading — a green result
it produced that was wrong at the moment it was produced.

Every check named here is a script in this repository, so a reader can re-run it rather than trust
this file. That was not true of the first draft, and a reviewer's judgement on that draft was that it
should be deleted unless its claims became executable. They did:

| What | Where |
|---|---|
| Published vectors, the spec's own reference verifier, and the ordering rule | `scripts/verify-published-artifacts.py` |
| Server-minted receipts against both published SDKs | `scripts/verify-minted-receipts.sh` |
| Break a property, check the owning case notices | `core/dev/mutation-pass.sh` |
| The suite against hooks that implement nothing | `core/crates/handoff-conformance/dev/run-lying-hooks.sh` |

## What was measured

Full battery at the commit above, from a clean build:

| Gate | Result |
|---|---|
| `core` workspace | 310 tests, 0 failures |
| `managed` workspace | 76 tests, 0 failures |
| clippy, both workspaces, `-D warnings` | clean, and no `allow(…)` suppressions |
| `cargo fmt --check`, both workspaces | clean |
| Python SDK | 104 tests |
| TypeScript SDK | 83 tests |
| Type drift against `spec/openapi.yaml` | 87 schemas, all exported |
| Conformance, real Postgres | **26/26, exit 0** |
| Conformance against a 501 stub | **0/26, exit 1** |
| Conformance against hooks that implement nothing | **23/26**, C-15 red as required; gate passed |
| Published artifacts | 19 checks, exit 0 |
| Server-minted receipts, both SDKs | agree, exit 0 |
| Mutation pass | 3 mutations, each caught by exactly the case that owns it |

## The published vectors, and the reference verifier

`spec/signing.md` §2.5 publishes worked vectors so anyone can check our arithmetic. They reproduce:
the 1125-byte receipt core and its `sha256`, the chain digest at height 4211 over the genesis
predecessor, the Ed25519 seed **derived** from its published sentence rather than trusted, the public
key, the signature, and all four negative vectors. The callback body, its digest, and both
rotation-overlap HMACs reproduce too.

**This checks the arithmetic of the vectors. It is not an independence check.** The document ships a
working Python reference verifier, so anything written while reading it is at best a transcription
with different variable names — and the strongest evidence of independence would have been
reproducing that verifier's bug, which no vector in the published set would expose.

What is a real check is that the script **extracts the reference verifier from the fenced block in
`spec/signing.md` at run time and executes it**. Nothing in this repository used to. That is how it
came to ship an ordering rule that contradicted the standard it named, under a comment asserting it
was safe. Both published receipt fixtures now verify **as published** under it, without recomputing
the digest first — recomputing before asserting is how a broken fixture survived a full review round.

The last two checks are the ones with teeth: that UTF-16 and code-point ordering actually disagree on
a non-BMP key, and that the emoji sorts where RFC 8785 says. Without them a fix that changed nothing
would pass everything else, because every published fixture is all-ASCII and canonicalizes
identically under both orders.

## The claim this pass got wrong

An earlier draft said six receipts were minted "with content chosen to stress canonicalization" and
that 6 of 6 verified. That was recorded as evidence. **Two canonicalization defects were reachable at
that commit through the ordinary answer path, and this pass found neither.**

The reason generalizes, which is why it is here rather than quietly fixed. The payload stressed
**values** — unicode inside strings, integers at ±(2^53 − 1) — and JCS member ordering is a property
of **keys**. Every character in that unicode string was BMP, so even promoted to a key it could not
have exposed the UTF-16-versus-code-point split. And an integer *bound* is not the integer
*predicate*, which is where `-0.0` walked through. The probe sat outside the defect's dimensions in
two independent ways and came back clean.

A pass is a claim that a defect is absent. It is the claim easiest to accept without asking what it
would have taken to see one.

## What this pass did not reach

- **Every hook-backed assertion.** All of C-15's storage probes, C-23's crash injection, C-7's log
  scan, `channel_inbound`, `observe_page_state_change`. That is the whole set of properties an
  implementer cannot check over HTTP — the set most worth verifying, and the one a black-box pass
  structurally cannot see. `conformance/GATE.md` names which of them a lying deployment survives.
- **Canonicalization of object keys**, as opposed to values, until the checks above were written.
- **CI.** No workflow has ever executed. Every job is a prediction the repository has not yet
  contradicted.
- **The documents.** No count, cross-reference or normative claim was checked by this pass; a review
  found five normative contradictions live at the time.
- **Anything beyond three mutations.** The pass removes properties one at a time in the places it
  knows to look, and says nothing about a property nobody thought to break.
- **I15 specifically, and by extension the invariants defended the same way.** The pass carries no
  mutation for "a requester can never answer its own request", because the property could not be
  removed by any edit small enough to resemble a plausible defect. Five independent expressions
  enforce it — a direct kind comparison in `routes.rs`, `may_answer()` in `plan.rs` and `store.rs`,
  `is_machine()` in `requires.rs::evaluate`, and the receipt's refusal to record a machine as a
  `user` actor. With the first four disabled and the server rebuilt, a machine answering its own
  request still got `400 — an actor of type user must be a person, not a machine`. Authority is
  defended the same way, from ten sites across five files. So a green suite under such a mutation
  means the property survived the edit; it is not evidence about the case that covers it, in either
  direction.
- **Truncation of a chain's tail.** Inherent to an unanchored hash chain, and `SECURITY.md` says so.

## Two errors, kept because the method is the point

**A false positive.** A foreign-tenant machine appeared to leak request existence — `403` for a
request that exists, `404` for one that does not. That is a cross-tenant existence oracle and it was
most of the way into a finding before the identifier was checked: it was 25 characters, so the `404`
came from parsing, before any lookup. With a well-formed ULID a machine principal gets `403`
uniformly. No oracle.

**A regression I introduced and shipped.** A commit described as a one-file specification wording
change also carried a stale copy of `sdk/python/handoff/_document.py`, reverting the UTF-16 ordering
fix: `git add <file>` followed by a bare `git commit` takes the whole index, and another lane had
that file staged. The release-blocking defect was live again in `HEAD` until `verify-minted-receipts.sh`
caught it. The conformance suite could not have: it links `handoff-protocol` and therefore
canonicalizes with the same code the server does. That is the single best argument for the
cross-implementation check in this repository, and it is an accident rather than a demonstration.

Four rules came out of this, and they are in `conformance/README.md` next to the shapes of a check
that measures nothing: a negative control must be well-formed and must fail for the reason under
test; the moment a probe produces the finding you hoped for is the moment to attack it; a guard must
be scoped to the same unit as the assertion it protects; and **a passing probe deserves the same
attack as a failing one.**

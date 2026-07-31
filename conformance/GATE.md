# The H1 gate: the suite fails loudly before any server exists

This file records the milestone gate for H1, run and verified by the session lead rather than by the
agent that wrote the suite.

## Why this gate exists

The conformance suite was written **before** the reference server. That ordering is deliberate. A
suite written after an implementation gets written to match it, and then it measures nothing. The
only way to show that a suite measures something is to point it at a server that does nothing and
watch every case fail with a stated reason.

Three of these cases — C-7, C-8 and C-16 — would fail against the Ωmegas Agent as it stands today.
That is the reason the reference server is greenfield rather than an extraction.

## The run

A stub server that answers `501 Not Implemented` to every route, with a well-formed error envelope:

```
python3 core/crates/handoff-conformance/dev/stub_501.py --port 8080
cargo run -p handoff-conformance -- \
    --base-url http://127.0.0.1:8080/v1 \
    --profile core/crates/handoff-conformance/profile.example.yaml
```

Result:

```
handoff-conformance 0.1.0 — protocol 0.1, Level 1
target: http://127.0.0.1:8080/v1
cases:  26 from conformance/cases

FAIL  C-1    One Idempotency-Key raises one request — 201, then 200  [I20]
      step: raise a request with an idempotency key
      POST /requests returned 501 but the specification requires 201
…
0/26 passing
not conformant: 26 Level 1 case(s) failing — C-1, C-2, C-3, C-4, C-5, C-6, C-6b, C-7,
C-8, C-9, C-10, C-11, C-12, C-13, C-14, C-15, C-16, C-18, C-19, C-20, C-21, C-22, C-23, C-24, C-25,
C-26
```

Exit code: **1**.

Run without `--profile`, every case still fails, but for a different and equally correct reason: the
deployment has supplied no credentials, so it has demonstrated nothing. Both are red; neither is
silent.

## What the gate asserts

1. Every Level 1 case runs and is individually named in the report.
2. Every failure states the step and what the specification required, not merely "failed".
3. The summary line is machine-readable (`N/M passing`).
4. The exit code is non-zero. Verified directly, not through a pipeline — `| tail` masks the exit
   status of the command before it, which is how a broken gate goes unnoticed.

## Coverage

Verified independently against `spec/conformance-map.json` **and** against the case files, because
those are two different questions and only the second one is executable:

- 21 invariants declared in spec §17. Against the **map**, every one has at least one case.
- No Level 1 case maps to zero invariants.
- §18 and the map both enumerate **26** Level 1 cases (C-1…C-16, C-6b, C-18…C-26) plus C-17 at
  Level 2, and `conformance/cases/` holds a file for every one of them.

**Closed:** `C-23` initially had no case file. It is the sole case for **I12** — every state
transition emits its event in the same transaction as the state change, probed by killing the Server
between the two writes. The spec itself observes that C-23 "is the case an implementation is most
tempted to skip", which is exactly why it was not quietly dropped: the case was written rather than
the map edited down. Every case §18 defines now exists on disk, and every one of them runs.

**This file has now been found stale twice**, by two independent reviews, for the same reason both
times: a case landed and the hand-typed count did not follow. The second time was one commit after
the first was fixed.

A document whose one job is to record a measured count, holding the wrong count, is the failure it
was written to guard against — and after twice, the honest conclusion is that the number should not
be maintained by hand at all. CI now asserts that the count stated here equals the number of Level 1
case files on disk (`.github/workflows/ci.yml`, job `gate-count`), so the third occurrence fails a
build instead of surviving into a review.

### A note on how that gap was found, because it generalizes

The first coverage check run here passed. It compared the invariant list against the **map**, and
the map is complete. The gap only appeared when the same question was asked of the **files on
disk**. A conformance suite has two populations — what the specification claims is tested, and what
is actually executable — and a check that only reads the first will report full coverage over a
directory that is missing cases.

The invariant meta-test in `handoff-protocol` must therefore derive coverage from
`conformance/cases/`, or from both sources while reporting the difference. Deriving it from the map
alone reproduces the bug it exists to prevent.

---

# The second gate: the suite fails loudly against hooks that lie

The gate above points the suite at a server that does nothing. It says nothing about the part of the
suite a **deployment** writes, and that is where the last two reviews got in.

Twice, a reviewer produced a complete green run against a deployment that had implemented none of
what C-15 asserts. The first time by stubbing four hooks with `true` and `false`. The second time —
after the case had been changed to require evidence in each hook's output — with about fifteen lines
of shell that echoed the receipt id and the chain head the suite had handed them as arguments, and
brute-forced the one value it had not by printing four hundred candidate heights, because
`output_matches` is a search. Both runs reported a fully passing suite and exit 0, against mutable
receipts and no chain verifier at all.

Nothing in this repository would have caught either. So the attack is a gate now.

## The run

```
core/crates/handoff-conformance/dev/run-lying-hooks.sh
```

It stands up the ordinary reference deployment — same server, same database, same credentials — and
points the suite at `dev/lying-hooks/profile.yaml`, in which every hook is a script that performs no
work and prints exactly what its case matches on. Result, 2026-07-31:

```
FAIL  C-15   Storage itself refuses to update or delete a receipt, and the hash chain proves history
FAIL  C-23   Every transition emits its event in the same transaction as the state change
FAIL  C-24   Numbers outside the deterministic band are rejected, and canonicalization is reproducible
23/26 passing
```

Exit code: **1**, and the gate additionally asserts that **C-15** by name is among the failures.

C-15 is red for the reason the whole redesign turns on: the chain is no longer something a hook can
claim — the suite walks it itself, in its own implementation of `signing.md` §2.2 — and a storage
refusal is decided by re-reading the receipt over HTTP, with a control step that requires the same
command, aimed at a row the engine permits, to actually write something the suite can then see.
A hook that touches no storage fails that control.

## What the liar survives, published rather than implied

```
C-7   A raw secret value is refused, and the value appears in no artifact anywhere
C-21  Channel content matching a decision format does not settle a request
C-22  All eight interaction patterns work with no request-kind anywhere, and clearance is asserted
```

These three rest on a hook reporting something the suite cannot observe: a log that was shown to it,
a channel message that was injected, a page change that was observed. A deployment that fabricates
those has fabricated evidence rather than omitted work, and this suite cannot tell the difference.
The same is true of one narrow part of C-15: `storage_mutate` must be one parameterized command, so
a deployment cannot leave the receipt branch unimplemented — but one that *branches* on the target to
fake the refusal would pass. `BACKLOG.md` carries the fix, which is for the suite to perform the
mutation itself against a supplied store credential, and which needs a driver per storage engine.

A published conformance run therefore attests those, and measures everything else. Saying so here is
the point: the previous two rounds were not defeated by dishonesty, they were defeated by a document
that implied more than it measured.

## Both gates run against the same tree

- 501 stub: `0/26 passing`, exit 1, every case individually named — re-run 2026-07-31 after C-26
  landed.
- Lying hooks: `23/26 passing`, exit 1, C-15 red — 2026-07-31.
- Reference deployment: see the run recorded by the maintainers alongside each release.

Each of the three is a different question, and a suite is evidence only when all three are asked.

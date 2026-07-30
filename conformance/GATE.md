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
cases:  24 from conformance/cases

FAIL  C-1    One Idempotency-Key raises one request — 201, then 200  [I20]
      step: raise a request with an idempotency key
      POST /requests returned 501 but the specification requires 201
…
0/24 passing
not conformant: 24 Level 1 case(s) failing — C-1, C-2, C-3, C-4, C-5, C-6, C-6b, C-7,
C-8, C-9, C-10, C-11, C-12, C-13, C-14, C-15, C-16, C-18, C-19, C-20, C-21, C-22, C-23, C-24
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
- §18 and the map both enumerate **24** Level 1 cases (C-1…C-16, C-6b, C-18…C-24) plus C-17 at
  Level 2, and `conformance/cases/` holds a file for every one of them.

**Closed:** `C-23` initially had no case file. It is the sole case for **I12** — every state
transition emits its event in the same transaction as the state change, probed by killing the Server
between the two writes. The spec itself observes that C-23 "is the case an implementation is most
tempted to skip", which is exactly why it was not quietly dropped: the case was written rather than
the map edited down. All 24 Level 1 cases now exist on disk and all 24 run.

**This file was itself found stale by the hostile review** — it recorded 23 cases after C-24 landed,
which is a small thing except that the file exists precisely to record that count. A document whose
one job is to hold a number, holding the wrong number, is the same failure it was written to guard
against.

### A note on how that gap was found, because it generalizes

The first coverage check run here passed. It compared the invariant list against the **map**, and
the map is complete. The gap only appeared when the same question was asked of the **files on
disk**. A conformance suite has two populations — what the specification claims is tested, and what
is actually executable — and a check that only reads the first will report full coverage over a
directory that is missing cases.

The invariant meta-test in `handoff-protocol` must therefore derive coverage from
`conformance/cases/`, or from both sources while reporting the difference. Deriving it from the map
alone reproduces the bug it exists to prevent.

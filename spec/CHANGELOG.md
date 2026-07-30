# Changelog — Handoff Protocol specification

All notable changes to the specification are recorded here. The specification follows the versioning
and deprecation policy in `handoff-protocol-v0.1.md` §19.

A version is **released** when the conformance suite passes against a reference implementation. A
change that cannot be expressed as a conformance test is not a protocol change; it is an
implementation detail and does not appear here.

Categories used below: **Added**, **Changed**, **Deprecated**, **Removed**, **Fixed**, **Security**.
Every entry that alters observable behaviour names the invariant (`I…`) or conformance test (`C-…`)
it affects.

---

## [0.1.0] — 2026-07-30

Initial specification. Draft status: the wire contract, the state machines, and the record formats
are complete and testable, and nothing in this version has yet been ratified by a shipped
implementation passing the suite.

### Added — the object model

- Three state machines — REQUEST, DELIVERY, WAITER — plus two immutable records, RECEIPT and
  AUTHORIZATION (§2). A request has many deliveries and at most one receipt (**I1**).
- Full state and transition tables for all three machines, with guards and atomic effects:
  R1–R14 (§6.2), the delivery states and grade ladder (§7.1), W1–W9 (§8.2).
- Request identity as **three distinct keys** — server-minted `request_id`, client `Idempotency-Key`,
  and `dedupe_key` — each with its own scope and lifetime (§3.1).
- The **declared `requires` model**: `answer.fields`, `capabilities`, `authority`. There is no request
  `kind` enum anywhere in the protocol (**I14**), and all eight required interaction patterns are
  expressed through the declaration alone (§5.6, `fixtures/use-cases/`, **C-22**).
- Eight closed answer field types, including `secret`, `attestation`, and `document` (§5.3).
- Progressive disclosure: a multi-step challenge is one request amended in place, with the attempt
  clock re-armed fresh at each step and the waiter never signalled mid-ladder (§5.5).
- The **request/attempt clock split** (§6.3) with four TTL policies — `escalate`, `expire_and_deny`,
  `default`, `park` (§6.4).
- Escalation ladders that mint deliveries and never requests (**I3**, **C-14**), with four delivery
  evidence grades that keep "our transport accepted it" distinct from "a person received it" (§7.2).
- Waiter reattachment (§8.5) so that a client's own process death is survivable (**C-11**).
- Level 2 `continuation` extension: `resume_ref` and `resume_payload` stored opaquely, returned
  byte-identical, never interpreted (§14, **C-17**).

### Added — records

- RECEIPT: immutable, minted in the outcome transaction, recording what was decided, by whom, when,
  **what they saw** (`request_digest` and `rendered.digest`), through what, and under what authority
  (§9.2).
- Three-layer receipt immutability — application, storage engine, and a per-tenant hash chain with an
  exportable head (§9.4, **C-15**). Corrections are new receipts, never edits (§9.5).
- Policy receipts with `actor.type = "policy"` for outcomes reached without a person (§9.6).
- Clearance provenance with `source ∈ {human_assertion, runtime_inference, timeout}` (§9.7).
- AUTHORIZATION: single-use, redeemable idempotently per `effect_key`, optionally bound to an
  `effect_digest` so a decision about one effect cannot be spent on another (§10, **C-13**).

### Added — artifacts

- `openapi.yaml` — OpenAPI 3.1, 30 operations across 28 paths, 87 schemas, three security schemes,
  and the complete 32-code error taxonomy as a single `ErrorCode` enum.
- `schemas/request.schema.json`, `receipt.schema.json`, `policy.schema.json`,
  `delivery-attempt.schema.json` — JSON Schema 2020-12.
- `signing.md` — the callback (HMAC-SHA-256) and receipt (Ed25519) schemes with exact canonical
  string construction, a 300-second replay window, key identification and overlap rotation, and
  worked positive and negative test vectors verified against the fixtures on disk.
- `fixtures/` — canonical request and response fixtures, the eight use-case scenarios, and the
  byte-exact signing inputs.
- `conformance-map.json` — the invariant-to-case mapping of §18 in machine-readable form, so an
  implementation can iterate I1–I21 and assert coverage without parsing the specification's prose.

### Security

The twelve protocol-level security requirements of §16 are normative, realized by invariants
**I1–I21** (§17) and tested by the 24 Level 1 conformance cases of §18 (**C-1**–**C-16**, **C-6b**,
**C-18**–**C-24**), with **C-17** at Level 2. Every invariant maps to at least one case and every
Level 1 case maps to at least one invariant; the mapping is duplicated machine-readably in
`conformance-map.json`. The requirements that constrain implementations most
sharply:

- **Requester ≠ decider, enforced by principal type** — not by role, permission, or configuration.
  There is no deployment mode in which a machine principal can satisfy a human-intervention request
  (**I15**, **C-5**).
- **Capabilities are opaque handles.** The protocol never carries a resolvable address by value, in
  any object, event, delivery, or message. Handles are randomly generated so they can be revoked
  individually and rotated without a shared-secret rotation (**I8**, **C-8**).
- **Secret values travel out of band** and MUST NOT appear in a URL, query string, path, argv,
  environment variable, header, redirect, log line, metric label, trace attribute, or crash report
  (**I18**, **C-7**). The specification defines the sink seam and deliberately ships no default sink
  implementation (§12).
- **Identity is tenant-scoped and never globally unique.** Uniqueness constraints, lookups, and
  idempotency keys are all tenant-scoped, because an unscoped key does not collide loudly — it
  silently absorbs another tenant's write (**I17**, **C-19**, **C-20**).
- **A signature proves the sender, never the tenant.** Tenancy is resolved from stored state on every
  signed path (**I13**, `signing.md` §0).
- **Clearance is asserted, never inferred**, and a runtime observation is recorded as such rather
  than credited to a person (**I16**, **C-22**).
- **Blast radius is declared, shown before acceptance, and digest-bound at resolve** (**I19**).
- **Input content is never recorded.** Capability use records presence and effect — held duration,
  input event counts, navigation origins — and never keystrokes or payloads (§11.6).

### Fixed — pre-release corrections

Found by the first implementer building against the draft, and corrected before any release. None
renumbers an existing invariant, state, or transition; the three new transition numbers and the one
new error code are additive.

- **Two invariants had no conformance case.** I12 (transactional event emission) and I19 (blast
  radius) were stated normatively and tested nowhere. Added **C-23** for I12; extended C-8 to assert
  `409 blast_radius_mismatch` and pre-resolve blast-radius disclosure for I19; extended C-18 and C-20
  to cover I13. §18 now carries an explicit `Invariants` column and a stable-id rule.
- **Three `pending` → `pending` transitions had normative prose but no number**, so implementers had
  no shared vocabulary for them: **R12** (progressive-disclosure partial answer), **R13** (a
  `delegate` or `unable` disposition), **R14** (a quorum endorsement below quorum). All three are
  things a person does that leave the request `pending`, and none of them may signal the waiter.
- **`value_sink.ref` contradicted its own normative fixture.** It was typed as a `snk_` identifier
  while §5.6.1 and `fixtures/use-cases/03-login-assistance.json` carried an opaque provider
  coordinate. Since §12 rule 4 makes the sink runtime-owned, the opaque reading is correct:
  `ValueSink.ref` is now a bounded opaque string and `Field.sink_ref` remains the typed handle.
- **No error code existed for redeeming an expired authorization**, though `Authorization.state`
  already had an `expired` member. Added `authorization_expired` (409). The three codes it would
  otherwise collapse into each state something false: spent, non-existent, or malformed.
- **The duration type admitted years and months**, so a `ttl` of `P1M` meant a different length in
  February than in March. `Duration` now permits only exact units (weeks, days, hours, minutes,
  seconds); retention windows, where calendar semantics are what an operator means, use the new
  `CalendarDuration`.
- **Canonical JSON did not pin number serialization**, leaving three implementations free to diverge
  at exactly the boundary where RFC 8785 inherits ECMAScript's switch to exponential notation. §1.4
  now constrains digest-covered numbers to `0` or `1e-6 ≤ |x| < 1e21`, requires integers within
  ±(2^53 − 1), and rejects the rest with `422 answer_validation_failed`. The emitted output is a
  strict subset of legal JCS, so a full canonicalizer and one that refuses to emit outside the band
  produce identical bytes and neither can silently diverge. Added **C-24**, which also pins the two
  signing fixtures' byte lengths and digests as the cross-implementation check.
- **Two diagrams were ambiguous.** Delivery grades are now stated as an ordered ladder with monotone
  advancement that MAY skip a rung, bounded by the channel's `max_grade`, and a Server MUST NOT
  synthesize a grade it did not observe. In the waiter machine, W2 and W8 now partition the terminal
  transitions rather than both claiming R7/R8, and the previously undrawn `signalled → armed` return
  edge is numbered **W9** — without it, a Server retires the waiter on the first `attempt_lapsed` ack
  and silently drops the answer that arrives afterwards.

### Explicitly not claimed

- **Exact execution resumption.** This specification guarantees typed answer delivery, effectively-once
  application via at-least-once delivery plus an idempotent ack, one-effect authorization, and a typed
  terminal signal on every outcome — and nothing beyond that (§1.3). Continuation is a property of the
  runtime; Level 2 stores a runtime-owned pointer or blob without interpreting it. Appendix B lists the
  claim language this distinction permits and forbids.
- **Exactly-once network delivery.** The protocol states at-least-once plus an idempotent ack, and
  says so rather than implying otherwise.

### Known open items, deferred deliberately

- **Quorum above 1** is modelled (§4.5) and only `quorum: 1` is required for conformance. Modelling it
  now costs one integer; retrofitting it later would be a migration across receipts.
- **A principal type for a person who is not a member of the tenant** — the external-approver case — is
  named but not specified. `auth_strength` is already a graded enum and `actor.type` is already a
  union, so adding it is additive under §19 and breaks no receipt.
- **Voice as an answer channel** is expressible (`max_grade: "delivered"`,
  `can_authenticate_person: false`) but extracting a decision from a call is an `acted` claim that
  needs its own evidence before any implementation makes it.
- **Cross-request batching** — one answer settling several waiting runtimes — needs a rule for how one
  answer produces N receipts without violating **I1**, and is not in this version.
- **Receipt retention** is a policy field (`policy.schema.json`) with no protocol-mandated minimum.

### Fixed — the two spec artifacts disagreed about `via.grade_reached`

*(Corrected. An earlier revision of this entry said the field was a required four-value enum that
forced a Server to invent a value. That was wrong: `grade_reached` has never appeared in any
`required` array, so omitting it was always valid. The reviewer who found the underlying bug caught
the error in my description of it.)*

The real defect was a disagreement between two normative artifacts. `openapi.yaml` typed
`grade_reached` as `anyOf: [enum, "null"]`; `schemas/receipt.schema.json` typed it as a bare
four-value enum with no null. A receipt carrying `null` — which the reference server produces for
every policy receipt from an expiry, and which C-10 accepts — validated against one artifact and was
rejected by the other. Two implementations reading different files would have disagreed about
whether a legal receipt was legal.

`receipt.schema.json` now matches `openapi.yaml`. A Server MAY omit the key or send `null`; both
mean the same thing, and §7.2 now says plainly that neither may be rounded up to `dispatched`.
`dispatched` asserts that a transport accepted the message, so writing it for a delivery nothing
ever graded puts a send on the receipt that may never have happened.

That normative sentence earned itself immediately. The reference server was naming a **suppressed**
delivery — our own email scaffold, which transmits nothing — as the delivery the person answered
through, and stamping `dispatched` on it. The cause was a selection that preferred the *most recent*
candidate over the one that could actually have carried the answer; the invented grade was the
second half of it. Both are fixed in the implementation.

This was the fifth appearance of one shape in a single milestone: a channel's *ceiling* written as
the grade *reached*; `failed()` and `suppressed()` reporting `dispatched`; an `unwrap_or` filling a
`None` arm; a schema that disagreed with its sibling about how to say "nothing"; and a selection
that reached for a plausible candidate when the honest answer was "none of these". Four were
implementations and one was the contract.

**Zero evidence and the weakest evidence are different claims**, and a receipt may not blur them.

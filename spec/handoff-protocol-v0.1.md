# Handoff Protocol, version 0.1

**Status:** Draft specification. Normative.
**Protocol version:** `0.1` · **HTTP path prefix:** `/v1`
**Companions in this directory:** `openapi.yaml` (the wire contract, normative),
`schemas/*.schema.json` (JSON Schema, normative), `signing.md` (signature scheme, normative),
`fixtures/` (canonical examples, normative for conformance), `CHANGELOG.md`.

Handoff is an open protocol for **human intervention in automated work**: a program that cannot
proceed asks a person, a person answers, and the answer comes back as typed data with a durable,
tamper-evident record of who decided what, on what basis, and through which channel.

This document defines the object model, the three state machines, the record formats, the security
requirements, the conformance levels, and the versioning policy. `openapi.yaml` defines the wire
contract. Where a shape appears in both, `openapi.yaml` is authoritative for syntax and this document
is authoritative for meaning.

An implementation of this specification has **no dependency on any particular vendor, runtime, agent
framework, or control plane**. Appendix A describes one managed profile; nothing in it is normative.

---

## 1. Conformance and terminology

### 1.1 Requirement keywords

The key words **MUST**, **MUST NOT**, **REQUIRED**, **SHALL**, **SHALL NOT**, **SHOULD**,
**SHOULD NOT**, **RECOMMENDED**, **MAY**, and **OPTIONAL** in this document are to be interpreted as
described in BCP 14 (RFC 2119, RFC 8174) when, and only when, they appear in all capitals.

Requirements are placed on three kinds of party, and each requirement names the party it binds:

| Party | Definition |
|---|---|
| **Server** | An implementation that serves the `/v1` API defined by `openapi.yaml`. |
| **Client** | Any caller of that API: a runtime that raises requests, a surface that presents them, a tool that reads receipts. |
| **Runtime** | The specific kind of client that raises a request and then waits for the answer. |

### 1.2 Conformance levels

| Level | Requirement |
|---|---|
| **Level 1** | REQUIRED of every conforming Server. All of this document except §14 (`continuation`), and all 24 Level 1 conformance cases of §18: C-1 through C-16, plus C-6b and C-18 through C-24. |
| **Level 2** | OPTIONAL. Level 1 plus the `continuation` extension of §14, and conformance case C-17. |

A Server MUST declare its level at `GET /v1/meta`. A Server MUST NOT advertise Level 2 unless it
passes C-17. A Server that fails any Level 1 test MUST NOT describe itself as conforming to this
specification.

### 1.3 What this protocol guarantees, and what it does not

This specification makes exactly four guarantees about the delivery of a human's answer. They are
stated here so that no implementer, and no document derived from this one, overstates them.

1. The answer reaches the waiting runtime **as typed data**, structured according to the request's own
   declaration — never as prose that a model or a regular expression must interpret.
2. Delivery is **at-least-once with an idempotent acknowledgement**. Combined, these give
   effectively-once application. This specification does **not** claim exactly-once network delivery.
3. One answer mints **exactly one authorization**, and that authorization can be spent on exactly one
   effect.
4. Every terminal outcome — including nobody answering — produces a **typed terminal signal**. A
   request never goes quiet.

**This specification does NOT guarantee, and a conforming implementation MUST NOT claim, that a
runtime's execution state is preserved or that execution resumes exactly where it stopped.**
Continuation is a property of the runtime, not of this protocol. Some runtimes can snapshot their
state and some cannot; §14 defines an optional extension in which the Server stores a runtime-owned
pointer or blob **without interpreting it**, so that runtimes able to continue may do so. The Server
provides storage and verbatim return. It does not provide continuation.

### 1.4 Notation and common types

| Type | Rule |
|---|---|
| Timestamps | RFC 3339, UTC, e.g. `2026-07-30T14:02:11Z`. Servers MUST use their own clock for all recorded times and MUST NOT accept a client-supplied `decided_at`. |
| Durations | ISO 8601 of **exact** length, e.g. `PT15M`, `PT4H`, `P1D`, `P2W`. Years and months MUST NOT be used for any TTL, attempt window, grant expiry, session lease, or ladder delay: `P1M` is 28 to 31 days depending on when the clock starts, so it is not a fixed length. Weeks are exactly seven days and are permitted. Retention windows are the sole exception and use a calendar-resolved duration, because "keep this for one year" is what an operator means. |
| Identifiers | `<prefix>_<26-character Crockford base32 ULID>`, lowercase prefix. |
| Digests | `sha256:` followed by lowercase hex, e.g. `sha256:9f2c…`. |
| Canonical JSON | RFC 8785 (JCS). Every digest defined in this document is taken over the RFC 8785 canonicalization of the named object, UTF-8 encoded. |

#### Numbers in digest-covered values

RFC 8785 serializes numbers with the ECMAScript `Number::toString` algorithm, which switches to
exponential notation outside a bounded range. That switch is the single most common source of
disagreement between independent canonicalizer implementations, and a disagreement there produces
receipts that one implementation can verify and another cannot.

This protocol therefore emits a **strict subset** of legal JCS output. In any object over which a
digest defined by this specification is computed:

1. Every JSON number MUST be finite and MUST be an **integer**. A number with a fractional part
   MUST be rejected.
2. Every JSON number MUST be within ±(2^53 − 1), so that it round-trips through an IEEE-754 double
   without loss.
3. A Server MUST reject an answer carrying a number outside either rule with
   `422 answer_validation_failed`, naming the offending field.

> **Why integers only, when JCS permits more.** An earlier revision of this section admitted any
> number in `1e-6 ≤ |x| < 1e21`, reasoning that `Number::toString` produces plain decimal there. That
> reasoning is correct about the *notation* and still leaves the *value* unsafe: RFC 8785 inherits
> ECMAScript number formatting, which is precisely what independent implementations do not reproduce
> reliably. It was not theoretical — a reference server minted receipts carrying `1.5` that neither
> published SDK could canonicalize at all, so a decision a person really made produced a receipt
> nobody could verify. Integers are exact in IEEE-754 to 2^53 − 1 and render identically everywhere,
> which collapses two bounds into one rule with no ambiguity left to disagree about.
>
> A Client carrying an exact decimal quantity — money, most obviously — sends it as `text`, which
> sidesteps binary floating point rather than negotiating with it.
>
> Two consequences worth stating rather than leaving to be discovered. `metadata` is digest-covered
> through `request_digest`, so a non-integer there fails a raise closed rather than surfacing later
> at the receipt. And a complete JCS canonicalizer and one that refuses to emit outside this subset
> produce identical bytes for every object this protocol digests — **both are conforming**, which is
> the property that makes the narrowing safe rather than merely strict.

Output meeting these constraints is valid JCS, so an implementation with a complete JCS
canonicalizer is conforming and needs no change. An implementation that refuses to *emit* numbers
outside the band is equally conforming, and cannot silently diverge from one that does. A Client
SHOULD carry monetary amounts and other exact decimal quantities as `text` rather than `number`,
which sidesteps binary floating point entirely.

Identifier prefixes, all REQUIRED:

`req_` request · `rcpt_` receipt · `auth_` authorization · `dlv_` delivery · `sig_` waiter signal ·
`hg_` capability grant handle · `hs_` grant session · `snk_` secret sink · `rt_` resume token ·
`usr_` human principal · `sa_` service-account principal · `org_` tenant.

Identifiers are ordinary data. They are time-sortable and safe to log. **An identifier is never an
authorization** (§4.6).

### 1.5 Definitions

**Tenant** — the isolation boundary. Every object in this protocol belongs to exactly one tenant.
This document writes the tenant identifier as `org_id`; a Server MAY name the concept differently in
its own documentation but MUST NOT weaken the isolation rule of §3.2.

**Request** — a durable ask directed at a person. §6.

**Delivery** — one attempt to put a request in front of a person through one channel. §7.

**Waiter** — the durable server-side record of a runtime waiting for a request's outcome. §8.

**Receipt** — the immutable record of an outcome. §9.

**Authorization** — the single-use spend right minted with a receipt. §10.

**Capability** — something a person must be handed in order to be able to answer. §11.

**Principal** — an authenticated subject: a person, a service account, or the policy engine acting in
the absence of either.

---

## 2. The object model

Handoff is **three state machines that reference one another, plus two immutable records**.

| Machine | Owned by | Lifetime | Question it answers |
|---|---|---|---|
| REQUEST | Server | Durable; outlives every client process | *Does a person still owe us an answer?* |
| DELIVERY | Server; one per (request, target, rung) | One delivery ladder | *Did the ask reach a person, and through what?* |
| WAITER | Server; one per waiting runtime | The runtime's wait | *Has the outcome been handed to the runtime, exactly once?* |

Plus RECEIPT (immutable, minted at the outcome) and AUTHORIZATION (single-use, minted with the
receipt).

The load-bearing structural rule, from which much of this document follows:

> **A request has many deliveries and at most one receipt.**

Escalation, reminders, re-delivery, and channel fallback all mint **deliveries**. Nothing but an
outcome mints a receipt. A Server MUST NOT mint a second receipt for a request except as a
`correction` record under §9.5.

---

## 3. Request identity

### 3.1 Three keys, deliberately distinct

Implementations frequently collapse "this is a retry", "don't ask twice", and "this is the same
action" into one key. This protocol separates them. A Server MUST implement all three.

| Key | Minted by | Scope | Purpose | Lifetime |
|---|---|---|---|---|
| `request_id` | Server | `(org_id)` | The identity that appears in receipts, events, and URLs | Forever |
| `Idempotency-Key` | Client, OPTIONAL | `(org_id, principal_id)` | **Retry safety.** A repeat returns the identical request *and its stored response*, whatever its state | 24 h, configurable |
| `dedupe_key` | Client, or Server-derived | `(org_id, waiter_ref)` | **Ask-once.** Collapses a second raise while one is still `pending` | Until the request settles |

`request_id` MUST be minted by the Server. A Server MUST NOT accept a client-supplied `request_id`.

A Client MUST NOT rely on the Server treating a supplied `Idempotency-Key` as scoped more broadly
than `(org_id, principal_id)`.

### 3.2 Identity is tenant-scoped

**Every identifier defined by this protocol is scoped to a tenant, and MUST NOT be treated as
globally unique.**

Specifically:

1. A Server MUST resolve every object lookup within the authenticated caller's tenant. An
   implementation MUST NOT serve an object by identifier alone.
2. A Server MUST scope every uniqueness constraint on a client-supplied key by tenant. Two tenants
   using the identical `Idempotency-Key` or `dedupe_key` MUST both succeed and MUST see only their
   own object.
3. When an object exists in another tenant, a Server MUST respond `404 request_not_found` (or the
   corresponding `*_not_found` code) rather than `403`, so that existence is not disclosed. A Server
   MAY respond `403 tenant_mismatch` only where the caller is already known to hold a grant spanning
   both tenants.

This rule exists because an unscoped uniqueness constraint does not merely risk a collision: it
causes one tenant's key to silently absorb another tenant's write. For a request store that is a
dropped human ask, and it produces no error anywhere. Conformance: C-19, C-20.

Identifiers MAY be globally unique as an implementation detail (a ULID usually will be). Nothing in
this protocol, and no Server behaviour, may **depend** on that.

### 3.3 What makes two raises "the same request"

On `POST /v1/requests` a Server MUST evaluate exactly these rules, in this order:

1. **Same `Idempotency-Key` within the window, same request body digest** → the same request. The
   Server MUST respond `200` with the stored representation in its *current* state. A retried raise
   after a person has already answered MUST return the answered request and its receipt, and MUST NOT
   re-ask.
2. **Same `Idempotency-Key` within the window, different body digest** → `409 idempotency_key_reused`.
   The Server MUST NOT modify the stored request.
3. **No `Idempotency-Key`, and a `pending` request exists with the same `dedupe_key`** → the same
   request. The Server MUST respond `200`, MUST merge `prompt` and `requires` forward from the newer
   raise, MUST increment `version`, and MUST record the merge as an amendment (§6.2, R2).
4. **Otherwise** → a new request; `201`.

When a Client supplies no `dedupe_key`, a Server MUST derive one as

```
dedupe_key = sha256( waiter_ref ‖ canonical_json(requires) ‖ canonical_json(prompt.title) )
```

so that collapse-on-retry holds for callers that never consider idempotency. `‖` is concatenation of
the UTF-8 byte sequences with no separator.

The `201` / `200` distinction is contract: it is how a Client tells "I asked" from "I already asked".

### 3.4 `waiter_ref`

```
waiter_ref : string, ≤ 512 bytes, opaque to the Server
```

A `waiter_ref` names the unit of runtime work that is waiting. A Server MUST treat it as an opaque
grouping key and MUST NOT parse, interpret, or attach meaning to its structure. It is used for
`dedupe_key` scoping, for listing everything blocking one unit of work, for reattachment (§8.5), and
for cancel-on-waiter-death (§8.4).

### 3.5 Idempotency of mutating operations

Every mutating operation in this protocol MUST be idempotent under a caller-supplied key:

| Operation | Idempotency key | Repeat behaviour |
|---|---|---|
| raise | `Idempotency-Key` header | §3.3 |
| amend, cancel, supersede, escalate, reassign, arm attempt | `Idempotency-Key` header | Same stored representation; no second state change |
| answer | `Idempotency-Key` header | §6.7 |
| ack a signal | `signal_id` | `200`; `first_ack: false` on the repeat |
| redeem an authorization | `effect_key` in the body | `200`; `first_redemption: false` on the repeat |
| submit values to a sink | `Idempotency-Key` header | `202`; values are not re-applied |

A Server MUST accept `Idempotency-Key` as an HTTP header. A Server MAY additionally accept a body
field of the same name; where both are present and disagree, the Server MUST respond
`400 invalid_request`.

---

## 4. Principals, authentication, and authority

### 4.1 Three kinds of principal

| Principal | Who | Authenticated by | MAY |
|---|---|---|---|
| **Requester** | A machine: an agent runtime | An API key bound to a `service_account` subject | raise, amend, cancel, supersede, escalate, reassign, read, poll signals, ack, redeem — **never answer** |
| **Answerer** | A person | An interactive session, or a channel identity verifiably bound to a person (§4.7) | answer, view, delegate, escalate, resolve capabilities |
| **Operator** | A tenant administrator | An interactive session with an administrative role | configure routing policy, revoke keys and grants, read the audit record |

An API key MUST authenticate to an **org-scoped principal of type `service_account`**, carrying no
user identity of its own. A Server MUST resolve the tenant from stored state bound to the key and
MUST NOT read the tenant from any request body (I13).

A requester principal MAY carry an `on_behalf_of` reference to the person who initiated the work.
When present, a Server MUST record it on the receipt. When absent, a Server MUST record its absence
explicitly rather than leaving the field blank in a way that reads as unattributed.

This protocol does not specify how keys are minted, formatted, or stored, beyond one requirement:
key verification MUST be constant-time with respect to the secret, and secrets MUST NOT be stored
recoverably.

### 4.2 The requester ≠ decider rule

> **A requester principal MUST NOT answer a request.**

A Server MUST enforce this by **principal type**, not by role, permission, or configuration. There
MUST NOT be any role, scope, setting, or deployment mode under which a machine principal can satisfy
a human-intervention request. A Server MUST respond `403 requester_may_not_answer`.

Without this rule an agent holding an API key can approve itself and every other guarantee in this
document is decoration. Conformance: C-5.

A Server MUST record the answering principal's type on the receipt, and a query for
"human-intervention decisions" MUST NOT be satisfiable by a non-human principal.

### 4.3 Authority is declared on the request

Required authority is **data on the request**, evaluated at answer time. It MUST NOT be a per-endpoint
or per-interaction-type branch in the Server.

```json
"authority": {
  "min_role": "editor",
  "auth_strength": "session",
  "assignees": [{"kind": "role", "value": "editor"}],
  "quorum": 1,
  "forbid_requester": true,
  "reason": "the resulting session cookie outlives the run"
}
```

A Server MUST evaluate `authority` against the **authenticated identity of the answerer at the moment
of the answer**, not at raise time, and MUST record both what was required and what was satisfied on
the receipt (§9.2).

`forbid_requester` is always `true`. It appears in the wire format for clarity and a Server MUST
reject any request that sets it to `false` with `400 invalid_request`.

**Derived floor (normative).** If any declared answer field has `"type": "secret"`, then regardless
of what the Client declared, the Server MUST raise the effective `min_role` to its administrative
role and the effective `auth_strength` to at least `session`. A Client MAY raise the floor further
and MUST NOT be able to lower it. This makes the stricter authority a consequence of the request's
*shape* rather than a hand-written branch per interaction type.

### 4.4 `auth_strength`

| Grade | Meaning | Recorded on the receipt as |
|---|---|---|
| `link_only` | Possession of a single-use delivery token only. **No person is identified.** | `actor.type = "anonymous_link"` |
| `session` | An authenticated principal in the tenant | `actor.type = "user"`, principal id and role snapshot |
| `reauth` | Re-entered a primary credential within the last 5 minutes | additionally `actor.reauth_at` |
| `mfa` | Presented a second factor within the last 5 minutes | additionally `actor.mfa_at` |

The grades are ordered `link_only < session < reauth < mfa`. A Server MUST reject an answer whose
achieved grade is below the request's declared grade with `403 insufficient_authority`.

**`link_only` is defined but constrained.** A Server MUST NOT accept `link_only` unless the
deployment has explicitly enabled it. A Server that accepts it MUST record `actor.type =
"anonymous_link"` on the receipt and MUST NOT record a principal identity it does not have. A
deployment profile MAY forbid `link_only` entirely, in which case the Server MUST respond
`403 auth_strength_not_permitted`. Conformance: C-6b.

A receipt that cannot say who decided is not a receipt. `link_only` exists in the model so that
implementations which need it are honest about it, not so that it becomes a convenient default.

### 4.5 Quorum

`quorum: n` requires `n` distinct principals to answer before the request settles. Partial answers
MUST be recorded as endorsements on the request, each with its own actor and timestamp, and the
request MUST remain `pending` until quorum is met. The final answer mints the single receipt (I1),
which MUST list every endorsing principal.

At this version, `quorum: 1` is the only value REQUIRED for conformance. A Server MUST accept the
field and MUST respond `400 invalid_request` to a value it cannot honour, rather than silently
treating `n > 1` as `1`.

### 4.6 An identifier is not an authorization

A Server MUST NOT treat possession of a `request_id`, a `surface_url`, a `waiter_ref`, a `receipt_id`,
or any other identifier defined here as evidence of authority to read or act on the referenced
object. Every read and every mutation MUST be authorized against the caller's authenticated principal.

`surface_url` is a **locator, not a capability**: opening it MUST prompt for authentication.

A Server MUST NOT expose an endpoint that lists requests, receipts, or grants to an unauthenticated
caller.

### 4.7 A delivery channel never confers authority

> **A verified channel signature proves the *caller*. It never proves the *tenant*, and it never
> proves the *person*.**

A Server MUST resolve tenancy from stored per-account state, never from anything in an inbound
request body (I13).

Every channel adapter MUST declare `can_authenticate_person`. A channel that declares `false`:

- MAY deliver a request and MAY collect an intent;
- MUST produce a **provisional** answer only, which identifies the request and pre-fills the surface
  but MUST NOT settle the request;
- becomes answer-capable for a given person only after a verified binding
  `(channel, external_user_id) → principal_id` exists.

A Server MUST NOT derive a decision from message content, however authenticated the channel. Prose
matching a decision format MUST NOT settle a request. Conformance: C-21.

The binding flow is not specified here beyond its required property: the binding MUST be established
by an already-authenticated principal claiming a short-lived, single-use code, so that the binding is
an act by an identified person rather than an assertion by a channel.

---

## 5. What a request declares

### 5.1 No `kind` enum

> **The core MUST NOT switch on a request "kind", "type", or "interaction type".**

A request declares *what it needs*, and the renderer, the router, and the validator are driven
entirely by that declaration. New interaction types arrive as new **field types** or new **capability
types** behind the declaration, never as a new branch. This is invariant I14 and it is the reason the
eight use cases of §5.6 need no special cases.

A request MAY carry `metadata.hint` as a **non-normative** analytics label. A Server MUST NOT branch
on it, MUST NOT validate it against a closed set, and MUST NOT let its absence change behaviour.

### 5.2 The three declarations

```json
{
  "prompt": {
    "title": "Refund $2,400 to Acme Corp?",
    "body": "Invoice **INV-8821** was double-charged on 2026-07-28.",
    "evidence": [{"kind": "link", "label": "Invoice INV-8821", "url": "https://billing.internal/inv/8821"}]
  },
  "requires": {
    "v": 1,
    "answer": {
      "fields": [
        {"name": "decision", "label": "Decision", "type": "choice", "required": true,
         "options": [{"id": "approve", "label": "Refund it"}, {"id": "reject", "label": "Don't refund"}]},
        {"name": "note", "label": "Add a note", "type": "text", "required": false, "max_len": 500}
      ]
    },
    "capabilities": [],
    "authority": {"min_role": "editor", "auth_strength": "session"}
  }
}
```

`requires.v` is REQUIRED. A Server that does not implement the declared version MUST respond
`400 unsupported_requires_version` and MUST NOT create a request. Partial acceptance — creating the
request with unrecognized fields dropped — is forbidden. Conformance: C-16.

### 5.3 `answer.fields` — the shape of the answer

The field list is **metadata only**. It declares the shape of an answer; it never carries the answer.

| `type` | The answer carries | Typically rendered as |
|---|---|---|
| `choice` | the selected option `id`, or an array when `multi` | buttons or a select |
| `text` | a string | an input or textarea |
| `number` | a number | a numeric input |
| `boolean` | a boolean | a toggle |
| `secret` | **`{"provided": true}` and nothing else** | a masked input routed to a sink (§12) |
| `attestation` | `true` and nothing else | an "I've done it" control |
| `document` | a value validated against `schema_ref` | an editor seeded with `initial` |
| `file_ref` | an opaque handle | an upload control |

Rules:

1. `fields` MAY be empty. An empty field list means the entire request is an attestation: there is
   nothing to type and the person acts out of band. A Server MUST accept it.
2. A Server MUST reject an unknown `type` with `400 unsupported_field_type`. A Server MUST NOT
   degrade an unrecognized field to a text input. A field the surface cannot draw is a field the
   person cannot answer, and rendering it anyway produces a receipt that misstates what was asked.
3. A Server MUST validate a submitted answer against the declared fields and MUST respond
   `422 answer_validation_failed` with per-field detail on a mismatch.
4. A Server MUST reject a submitted answer that carries a raw value for a `secret` field with
   `422 answer_validation_failed`. Conformance: C-7.
5. A Server MUST reject an answer carrying a key that is not a declared field name.

### 5.4 `capabilities` — what the person must be handed

A capability declares something the answerer needs *in order to be able to answer* — a live view of a
browser, a document to edit, a screen to drive. It is not what the request "is". See §11.

### 5.5 Progressive disclosure: multi-step asks

A challenge chain — a password, then a one-time code — is **one request**, amended in place. It MUST
NOT become a second request.

On `POST /v1/requests/{id}/answer` with `partial: true`, a Server MUST:

1. validate the submitted values against the current field set, routing `secret` values to the sink;
2. amend `requires.answer.fields` to the next field set and increment `version`;
3. leave the request `pending`;
4. re-arm the attempt clock **fresh**, never inheriting the remaining time of the previous step;
5. **not** signal the waiter — the runtime MUST NOT learn that an intermediate step occurred;
6. append a step record to the eventual receipt.

The receipt therefore records the whole ladder as one intervention with one decision.

### 5.6 The eight required interaction patterns, on one model

Every pattern below is expressible through the declarations above. **None of them adds a branch to
the core.** The `hint` column is non-normative.

| Pattern | `answer.fields` | `capabilities` | `authority` | Expiry policy | `hint` |
|---|---|---|---|---|---|
| Approve / reject | one `choice`, optional `text` note | — | editor / session | `expire_and_deny` | `approval` |
| Answer a question | one `text`, or `choice` when options are known | optional `document` evidence | viewer / session | `default` with a declared fallback | `question` |
| Login assistance | N × `secret` + M × `text`, read live off the target | optional `interactive_surface{scope:"drive"}` | **admin / session** (derived, §4.3) | `park` with reminders | `credential` |
| Challenge or takeover | zero fields, or one `attestation` | `interactive_surface{scope:"drive"}`, REQUIRED | admin / session | `escalate` → `expire_and_deny` | `takeover` |
| Review and correction | one `document` with `initial` and `schema_ref` | `document` evidence | editor / session | `default` (accept as-is) or `expire_and_deny` | `review` |
| Confirm an external side effect | one `choice{confirm,cancel}` | evidence of the exact effect | editor / reauth | `expire_and_deny` | `confirm` |
| Reassign / escalate | **not a request shape** — an operation (§6.6) and an answer disposition | — | — | — | — |
| Expiry / unanswered | **not a request shape** — the TTL policy of §6.4 and transitions R3/R6 | — | — | — | — |

Two of the eight deliberately resolve to "not a request shape". That is the point: modelling them as
kinds would grow a renderer branch and a state per case. They are an **operation** and a **policy**
respectively.

Worked example payloads for all eight are in `fixtures/use-cases/`. Each is a complete
`POST /v1/requests` body, and each is asserted by conformance test C-22.

#### 5.6.1 Login assistance without exposing credential values

```json
{
  "waiter_ref": "run:0198f2a1",
  "prompt": {"title": "Sign in to app.example.com for this workspace's browser",
             "body": "Your credentials go straight to the browser and never pass through the agent."},
  "requires": {
    "v": 1,
    "answer": {
      "fields": [
        {"name": "email",    "label": "Email or username", "type": "text",   "required": true},
        {"name": "password", "label": "Password",          "type": "secret", "required": true,
         "sink_ref": "snk_01K3M7QW8ZC4YRXB2N6VD9FTHF"}
      ],
      "value_sink": {"provider": "example/browser", "op": "submit_credentials", "ref": "opaque:bs_4KpQ"}
    },
    "capabilities": [
      {"handle": "hg_01K3M7QW8ZC4YRXB2N6VD9FTHG", "type": "interactive_surface", "scope": "drive",
       "optional": true, "ttl": "PT15M", "label": "the browser the agent is driving",
       "purpose": "Finish sign-in yourself if the site uses single sign-on or a challenge."}
    ],
    "authority": {"min_role": "admin", "auth_strength": "session",
                  "reason": "the resulting session outlives the run"}
  },
  "attempt_ttl": "PT15M",
  "ttl_policy": {"on_expiry": "park", "reminder_every": "PT1H"},
  "metadata": {"hint": "credential"}
}
```

The declared fields are derived by the runtime from the live target, so no per-site code exists
anywhere. Zero fields means the surface offers only the live view; fields present means a typed form
with the live view as an escape hatch; both is both. All three behaviours fall out of the
declaration.

---

## 6. The REQUEST state machine

### 6.1 States

```
                    ┌──────────── amend / attempt lapse / escalate ────────────┐
                    │                    (self-transitions)                    │
                    ▼                                                          │
  ∅ ──raise──▶  pending ───────────────────────────────────────────────────────┘
                    │
                    ├── answer ─────────▶ answered    (mints RECEIPT + AUTHORIZATION)
                    ├── TTL elapsed ────▶ expired     (mints a policy RECEIPT)
                    ├── cancel ─────────▶ cancelled
                    └── supersede ──────▶ superseded
```

`pending` is the only non-terminal state. `answered`, `expired`, `cancelled`, and `superseded` are
terminal. A Server MUST NOT transition out of a terminal state.

Every terminal transition MUST produce a typed terminal signal to the waiter (I11).

### 6.2 Transition table

| # | From | To | Trigger | Guard (all MUST hold) | Effects, in one atomic transaction |
|---|---|---|---|---|---|
| **R1** | ∅ | `pending` | raise | No live `Idempotency-Key` match; no `pending` request with the same `dedupe_key`; caller holds the write scope; tenant is entitled | Insert the request; register the WAITER as `armed`; resolve routing policy and snapshot it onto the request; enqueue rung-0 DELIVERIES; mint declared capability grants; emit `request.raised` |
| **R2** | `pending` | `pending` | amend | Zero deliveries have reached `acted`; caller is the original requester or shares the `waiter_ref` | Merge `prompt` and `requires` forward; increment `version`; re-render open deliveries; emit `request.amended` |
| **R3** | `pending` | `pending` | attempt lapse | `attempt_expires_at ≤ now`; the lapse has not already been notified; waiter is `armed` | Stamp the lapse **once, ever**; signal the waiter `{type: "attempt_lapsed"}`; set `urgency_state` to `waiting`; **re-list the request in every inbox**; emit `attempt.lapsed` |
| **R4** | `pending` | `pending` | escalate | A further ladder rung exists; request still `pending` | Mint DELIVERIES for the rung; extend `expires_at` by the rung's grant; emit `request.escalated` |
| **R5** | `pending` | `answered` | answer | The conditional write on `state = 'pending'` affects a row; answerer satisfies the declared authority; the answer validates against `requires.answer`; quorum met | Mint RECEIPT and AUTHORIZATION; signal the waiter `{type: "answered"}`; cancel open deliveries; revoke open capability grants; emit `request.answered` |
| **R6** | `pending` | `expired` | TTL sweep | `expires_at ≤ now`; no further rung; policy is `expire_and_deny` or `default` | Mint a **policy** RECEIPT with `actor.type = "policy"`; signal the waiter `{type: "expired", effective: …}`; cancel open deliveries; revoke grants; emit `request.expired` |
| **R7** | `pending` | `cancelled` | cancel | Caller is the requester principal or shares the `waiter_ref`; or the waiter went terminal with `on_waiter_terminal: "cancel"` | Signal the waiter `{type: "cancelled"}`; cancel open deliveries; revoke grants; emit `request.cancelled` |
| **R8** | `pending` | `superseded` | supersede | The successor request exists, is in the same tenant, and is `pending` | Link `superseded_by` and the inverse; cancel open deliveries; revoke grants; signal the waiter `{type: "superseded", by: …}`; emit `request.superseded` |
| **R9** | `answered` | `answered` | duplicate answer, same `Idempotency-Key` | — | **No state change.** `200` with the original receipt |
| **R10** | `answered` | `answered` | conflicting answer | — | **No state change.** `409 already_answered`, with the existing `receipt_id` attached |
| **R11** | terminal | terminal | cancel or expiry racing a landed answer | — | **The person's answer wins.** The cancel returns `409 already_answered` |
| **R12** | `pending` | `pending` | answer with `partial: true` (§5.5) | Answerer satisfies the declared authority; submitted values validate against the **current** field set | Route `secret` values to the sink; amend `requires.answer.fields` to the next set; increment `version`; re-arm the attempt clock **fresh**; append a step to the eventual receipt; **do not signal the waiter**; emit `request.step_recorded` |
| **R13** | `pending` | `pending` | answer with `disposition` of `delegate` or `unable` (§6.6) | Answerer satisfies the declared authority | Record the disposition, the actor, and any `delegate_to` on the request; for `delegate`, mint deliveries to the new target; for `unable`, MAY advance the ladder per policy; **mint no receipt**; **do not signal the waiter**; emit `request.disposition_recorded` |
| **R14** | `pending` | `pending` | answer that endorses but does not meet quorum (§4.5) | `quorum > 1`; answerer satisfies the declared authority; recorded endorsements remain below `quorum` | Record the endorsement with its principal and timestamp; **mint no receipt**; **do not signal the waiter**; emit `request.endorsed`. When the endorsement brings the count **to** `quorum`, R5 fires instead and mints the single receipt |

**R5 is a conditional write.** A Server MUST implement the answer as a state-conditional update
(`… WHERE state = 'pending'`) or an equivalent atomic compare-and-set, and MUST NOT implement it as a
read-then-write. Conformance: C-3.

**R11 is a product rule, not an edge case.** A machine changing its mind a millisecond after a person
acted MUST NOT discard that person's work.

**R12, R13, and R14 share one property, and it is the reason they are numbered.** All three are
things a *person* does to a `pending` request that leave it `pending`, and none of them may signal the
waiter. A runtime MUST NOT be able to observe that an intermediate step, a delegation, or a partial
endorsement occurred; it learns only the single outcome. An implementation that wakes the waiter on
any of the three has turned one intervention into several, and violates I1 as soon as a receipt is
minted for each.

**R2's guard is strict.** Once a person has begun answering — any delivery has reached `acted` — an
amendment MUST be refused with `409 request_in_progress` and the caller MUST supersede instead
(§6.6). A partial answer under §5.5 is not an amendment by a third party; it is the same person's
in-progress attempt, and it is permitted.

Every transition MUST emit its event **in the same transaction as the state change** (I12). A Server
MUST NOT emit state events on a best-effort path that can drop them relative to the state.

### 6.3 Two clocks: the request and the attempt

A Server MUST maintain two independent clocks.

| Clock | Bounds the claim | On lapse | Terminal? |
|---|---|---|---|
| **Attempt** — `attempt_expires_at` | "a specific person is expected to be doing this right now" | Nudge the waiter **once** with `attempt_lapsed`; the request stays `pending` and **returns to every inbox** | No |
| **Request** — `expires_at` | "this ask is still worth answering" | Run `ttl_policy` (§6.4) | Yes |

An attempt is armed when a delivery reaches `acted`, or explicitly by
`POST /v1/requests/{id}/attempt`. It MUST be re-armed **fresh** at each progressive-disclosure step
(§5.5) and MUST NOT inherit a near-expired countdown. The RECOMMENDED default attempt window is
`PT15M`, and a Server MUST apply the same default to every capability and every field set — a Server
MUST NOT vary the attempt window by interaction type.

> **Invariant I4: a `pending` request MUST always be listable and MUST always be answerable at its
> canonical URL.**

A lapsed *attempt* changes the request's `urgency_state` from `attention` to `waiting`. It MUST NOT
change the request's visibility. A Server MUST NOT filter attempt-lapsed requests out of a listing,
and MUST NOT withdraw their notifications. This is the specific failure the split exists to prevent:
a data layer that remains resumable behind a surface that shows nothing to click. Conformance: C-9.

`expires_at` is OPTIONAL. A request raised with no `ttl` never expires; it is bounded only by
cancellation, supersession, or a person answering.

### 6.4 Request TTL policies

Declared per request, defaulted per deployment.

| `on_expiry` | Behaviour at `expires_at` | Waiter receives | The honest claim |
|---|---|---|---|
| `escalate` | Advance the ladder and extend the TTL. Terminal only when rungs are exhausted, then falls through to the deployment's terminal policy | nothing yet | "we try harder before we give up" |
| `expire_and_deny` | → `expired`. Policy receipt, `effective: "deny"` | `{type: "expired", effective: "deny"}` | "unanswered means no" |
| `default` | → `expired`. Policy receipt carrying the declared `default_answer`, `actor.type = "policy"` | `{type: "expired", effective: "default", values: {…}}` | "unanswered means the pre-agreed default, and the record says a policy decided" |
| `park` | Never expires. Reminder deliveries on a declared cadence | nothing | "it waits until someone answers" |

When no policy is declared **and a `ttl` is present**, a Server MUST default to `escalate` falling
through to `expire_and_deny`. When no `ttl` is present, the request does not expire and the policy is
irrelevant.

The general rule the default expresses: **fail toward a typed terminal answer, never toward silence,
and when the typed answer must be guessed, guess "no."** Proceeding unasked is the expensive error.

`default` is the only policy that can produce an outcome without a person, and it therefore carries
the strongest record requirement: `default_answer` MUST have been declared **at raise time**, before
anyone knew the person would go quiet, and the receipt MUST record `actor.type = "policy"` so that no
audit can mistake it for consent. A Server MUST reject `on_expiry: "default"` without
`default_answer` with `400 invalid_request`.

### 6.5 Cancellation and supersession

| Operation | Who | Effect | Racing a landed answer |
|---|---|---|---|
| `cancel(reason)` | The requester principal, or the same `waiter_ref` | R7. Open deliveries cancelled; a person mid-answer receives `409 request_cancelled` carrying the reason | The answer wins (R11) |
| `supersede(by)` | The requester principal | R8. `superseded_by` linked both ways; the person is redirected to the successor | The answer wins |
| withdraw-on-waiter-death | Server | §8.4 | The answer wins |

**Amendment and supersession are different operations and MUST NOT be conflated.**

- *Amend* — "the same ask, better described". Preserves the `request_id`, the eventual receipt, and
  any in-progress attempt.
- *Supersede* — "a materially different ask". The amount changed; the recipient changed. It MUST mint
  a new request, so that a person who already saw the old one cannot inadvertently authorize the new
  one.

The mechanical guard: **if a change touches any field the answer is about, it is a supersession.** A
Client MUST NOT use amend to alter the substance of what is being authorized.

### 6.6 Reassignment and escalation are operations

`POST /v1/requests/{id}/reassign` and `POST /v1/requests/{id}/escalate` change **who is being asked**.
They MUST NOT change the request's state, MUST NOT mint a receipt, and MUST NOT mint a new request.

An answerer may also hand the decision on through the answer itself, with
`disposition: "delegate"` and `delegate_to`. A Server MUST record the delegation on the receipt and
MUST keep the request `pending` until an authorized principal decides. A Server MUST NOT treat a
delegation as a decision.

`disposition: "unable"` records that the person was asked, engaged, and could not complete the ask.
A Server MUST record it and MUST keep the request `pending` unless the policy says otherwise. "Unable
to do it" is a disposition, not a new state.

### 6.7 Duplicate answers

This is where approval systems silently corrupt, so the rule is stated completely.

1. The answer write MUST be conditional on `state = 'pending'` (R5).
2. When it affects no row, the Server MUST inspect the current state and return a **specific** error:
   `409 already_answered` with the existing `receipt_id`; or `409 request_expired`;
   or `409 request_cancelled`; or `409 request_superseded` with `superseded_by`.
3. When the caller supplied the **same** `Idempotency-Key` as the answer that landed, the Server MUST
   return `200` with the original receipt. A retried click is not a conflict.
4. A Server MUST NOT implement last-write-wins.
5. A Server MUST NOT return a `2xx` carrying a failure flag in the body. A conflict is a `409`.

Conformance: C-3, C-4.

---

## 7. The DELIVERY state machine

Delivery is a first-class tracked entity, not a side effect of a notification sweep.

### 7.1 States

```
  ∅ ─rung fires─▶ queued ──▶ suppressed        (policy: quiet hours, dedupe, consent missing)
                     │
                     └──▶ sending ──▶ dispatched ──▶ delivered ──▶ seen ──▶ acted
                             │  ▲          │   │                     ▲
                             │  │          │   └─────────────────────┘
                             ▼  │          │      (channel reports no delivery confirmation)
                          retrying         └──▶ bounced
                             │
                             └──▶ failed
```

Plus, at any point before `acted`: → `cancelled` when the request settles before dispatch, and →
`stale` when the request settles elsewhere after dispatch.

Terminal delivery states: `suppressed`, `failed`, `bounced`, `acted`, `stale`, `cancelled`.

**Grades are an ordered ladder, and advancement is monotone but MAY skip.** The order is
`dispatched < delivered < seen < acted`. A delivery MUST NOT move backwards down the ladder, and MUST
NOT exceed its channel's declared `max_grade`. It MAY skip a rung: a channel that never reports
delivery confirmation goes `dispatched → seen` the moment the person opens the surface, and that is a
higher grade honestly earned, not a gap to be filled in. A Server MUST NOT synthesize an intermediate
grade it did not observe — inferring `delivered` from a later `seen` would record evidence the channel
never produced.

### 7.2 Delivery grades — what a delivery actually proves

| Grade | Proves |
|---|---|
| `dispatched` | Our transport accepted it. **This is not evidence a person received anything.** |
| `delivered` | The channel reports it reached the person's endpoint |
| `seen` | The person opened the request surface, **authenticated** |
| `acted` | The person answered **through this delivery** |

Every channel adapter MUST declare `max_grade` and `can_authenticate_person`, and a Server MUST NOT
record a grade above a channel's declared `max_grade`. A Server MUST NOT treat `dispatched` as
`delivered`. A protocol that sells receipts must not blur the difference between "the API returned
200" and "a person got it".

The receipt records the grade the answering delivery reached (§9.2, `via.grade_reached`).

A Server MUST record the grade that delivery is **known to have reached**, and MUST NOT substitute a
weaker grade when none was recorded. If no grade was ever recorded, `grade_reached` is `null`. It is
specifically NOT `dispatched`: that value asserts that our transport accepted the message, so
writing it for a delivery nothing ever graded puts a send on the record that may never have
happened. Zero evidence and the weakest evidence are different claims, and a receipt is the one
artifact in this protocol that may not blur them.

### 7.3 Attempts within a delivery

A delivery owns an ordered list of attempts, each `{n, started_at, ended_at, outcome,
transport_status, error}`. Retry backoff MUST be exponential with jitter. The RECOMMENDED shape is
`2^n` seconds capped at 300, with a bounded attempt count.

> **A raise MUST NOT block on delivery.**

`POST /v1/requests` MUST return once the request is durable, with deliveries in `queued`. A Server
MUST NOT fail a raise because a channel is unavailable; where no channel could even be attempted, the
Server MUST still create the request and MAY report `503 delivery_unavailable` alongside it. A
human-intervention service that can take a caller's agent down with it has inverted its own purpose.

### 7.4 Routing and escalation ladders

> **The Client declares urgency; the Server decides the channel.**

A Client MAY supply `urgency` and a suggested assignee. The **ladder** is deployment policy, resolved
server-side at raise time and **snapshotted onto the request**, so that a policy edit mid-flight
cannot retroactively change what happened.

```json
"routing": {
  "targets": [{"kind": "role", "value": "editor"}],
  "ladder": [
    {"after": "PT0S",  "channels": ["inapp", "push"]},
    {"after": "PT5M",  "channels": ["chat", "email"]},
    {"after": "PT15M", "channels": ["voice"], "to": {"kind": "rotation", "value": "oncall"}}
  ]
}
```

Overriding the ladder on a request MUST require a **separate scope** from raising one. A compromised
key that can ask a question is a materially different blast radius from one that can page an on-call
engineer at 3 a.m.

> **Invariant I3: a ladder rung mints deliveries, never a request.**

Escalation is therefore invisible to the receipt except as escalation metadata. One intervention, one
decision, one record. Conformance: C-14.

### 7.5 Targets

```
target = {kind: "principal", value: …}   // a specific person
       | {kind: "role",      value: …}   // everyone holding a role
       | {kind: "group",     value: …}
       | {kind: "rotation",  value: …}   // resolved at rung-fire time, not raise time
       | {kind: "anyone",    value: …}   // anyone in scope
```

A Server MUST resolve every target kind through **one** target-resolution port returning a set of
principals. A Server MUST NOT branch per target kind anywhere else in the core.

A Server SHOULD address deliveries to individual people rather than to a place. An escalation ladder
that cannot name a person is a broadcast, and read state that is shared across a whole workspace
means one person clearing a notification clears everyone's.

---

## 8. The WAITER state machine

This is what makes resumption a protocol property rather than a runtime accident: **the wait is a
durable server-side row, not a loop inside the client's process.**

### 8.1 States

```
  ∅ ─raise─▶ armed ──▶ signalled ──▶ delivering ──▶ acked
               ▲   W2       │  ▲   W3        │  W4     │
               │            │  └─ transport ─┘         │
               │            │      failed (W5)         │
               │            │                          │
               │            └──────────────────────────┘
               │              W9: last signal acked while
               │              the request is still pending
               │
               ├──▶ released   (W8: request cancelled or superseded)
               │
               └──▶ orphaned ──reattach (W7)──▶ armed
```

`released` is reachable from `armed` and from `signalled`; `orphaned` from both as well. The
`signalled → armed` return edge is **W9** and exists only for the non-terminal nudge.

### 8.2 Transition table

| # | From | To | Trigger | Guard | Effect |
|---|---|---|---|---|---|
| **W1** | ∅ | `armed` | raise | — | Create a durable waiter keyed `(request_id, waiter_ref)`; store `resume_ref` / `resume_payload` verbatim if present |
| **W2** | `armed` | `signalled` | the request reached `answered` or `expired` (R5 / R6), or an attempt lapsed (R3) | — | Enqueue exactly one signal. **Signals are a queue, not a flag** |
| **W3** | `signalled` | `delivering` | a long poll attaches, or a callback relay claims the signal | Exclusive claim with a lease | — |
| **W4** | `delivering` | `acked` | ack with a valid `resume_token` | Token matches; signal not already acked | Stop redelivery; record `applied` and any `reason` |
| **W5** | `delivering` | `signalled` | transport failure or lease expiry | attempts below the maximum | Exponential backoff; re-queue |
| **W6** | `armed` or `signalled` | `orphaned` | the `waiter_ref` was reported terminal, or a leased waiter's heartbeat lapsed | — | Apply `on_waiter_terminal` |
| **W7** | `orphaned` | `armed` | reattach | `resume_token` or authenticated `waiter_ref` ownership | Return every unacked signal; re-arm the lease |
| **W8** | `armed` or `signalled` | `released` | the request was cancelled or superseded (R7 / R8) | — | Terminal signal delivered; no ack required |
| **W9** | `acked` | `armed` | the acked signal was non-terminal and no further signal is queued | The request is still `pending` | Re-arm the waiter to await the eventual terminal signal |

**W2 and W8 partition the terminal transitions and MUST NOT both fire for one request.** R5 and R6
(answered, expired) route through W2, because the runtime is expected to consume the outcome and ack
it. R7 and R8 (cancelled, superseded) route through W8, because the request was withdrawn by the side
that raised it and there is nothing for the runtime to acknowledge. A Server MUST NOT signal a waiter
twice for a single terminal transition.

**W9 is why `acked` is not a terminal waiter state.** `attempt_lapsed` is a nudge: once it is acked
and the request is still `pending`, the waiter returns to `armed` to await the real outcome. A Server
that retires the waiter on the first ack will silently drop the answer that arrives afterwards.

**W2 is why signals are a queue.** A non-terminal `attempt_lapsed` nudge MUST NOT be able to
overwrite, replace, or mask a subsequent terminal signal. A Server that models the waiter's pending
outcome as a single mutable field will lose answers, and MUST NOT do so.

**Every signal MUST carry a typed payload.** A Server MUST NOT satisfy a wait with a null or empty
decision. A Server MUST NOT satisfy any wait other than the one bound to the request whose state
changed: satisfying sibling waits indiscriminately retires listeners the runtime armed for unrelated
reasons.

### 8.3 Signals

Signal `type` is a closed set: `answered`, `expired`, `cancelled`, `superseded`, `attempt_lapsed`.
The first four are terminal for the request; `attempt_lapsed` is a nudge and leaves the request
`pending`.

Each signal carries a `sequence` number, monotonically increasing per `waiter_ref`, so that a
receiver can detect out-of-order or missing delivery. A Server MUST assign it; a Client MAY use it.

**Reading a signal MUST NOT consume it.** Consumption is the ack. This two-step is the
effectively-once hinge: at-least-once delivery plus an idempotent ack. A receiver that returns `2xx`
to a callback and then crashes before applying the decision has not received it, and the Server MUST
continue redelivering until an explicit ack arrives.

`applied: false` with a `reason` records that the runtime received the decision and could not act on
it. That is a fact the record should hold, not an error to swallow. A Server MUST accept it, MUST
stop redelivery, and MUST record it.

### 8.4 Liveness

| `liveness` | The waiter is | Death detected by | Default `on_waiter_terminal` |
|---|---|---|---|
| `durable` | A server-side parked run in the runtime | The runtime reporting it, or never | `keep` |
| `leased` | A live client process holding a long poll | Heartbeat lapse (RECOMMENDED 90 s) | `cancel` |

Under `keep`, the request MUST remain answerable and the Server MUST surface the waiter's `orphaned`
state on the request so a person can decide whether it is still worth answering. The receipt MUST
record that the answer arrived after the requester was gone.

Under `cancel`, the Server MUST transition the request via R7 — nobody is left to receive the answer,
so it MUST stop paging people.

### 8.5 Reattachment

`POST /v1/waiters/{waiter_ref}/reattach` returns the waiter's state, every open request, and every
**unacked** signal, and re-arms the lease.

This is the operation that makes a client's own process death survivable. The wait was never in that
process. A Server MUST NOT discard a signal because the client that raised the request has gone away.
Conformance: C-11.

---

## 9. RECEIPT

### 9.1 What a receipt is

An immutable record, minted **in the same transaction as the outcome**, that answers six questions.

| Question | Field |
|---|---|
| What was decided? | `decision` — the typed answer, with `secret` fields reduced to `{"provided": true}` |
| Who decided? | `actor` |
| When? | `decided_at` (Server clock) and `attempt_id` |
| What did they see? | `request_digest`, `rendered.digest`, `rendered.ref`, `request_version` |
| Through what? | `via` — delivery, channel, target, grade reached |
| Under what authority? | `authority` — `{required, satisfied}` |

A Server MUST mint the receipt atomically with the state transition. A Server MUST NOT mint receipts
asynchronously, and MUST NOT allow a receipt to be lost while its state change persists.

### 9.2 Required content

```json
{
  "id": "rcpt_01K3MB2R4Y8ZC4YRXB2N6VD9FT",
  "request_id": "req_01K3M7QW8ZC4YRXB2N6VD9FTHE",
  "org_id": "org_01K0A2QW8ZC4YRXB2N6VD9FTHE",
  "kind": "decision",
  "decision": {"values": {"decision": "approve", "note": "Confirmed with Acme on the phone."},
               "disposition": "decide"},
  "actor": {"type": "user", "principal_id": "usr_01J9M7QW8ZC4YRXB2N6VD9FTHE",
            "display": "Dana Okafor", "role_at_decision": "editor",
            "auth_strength": "session", "on_behalf_of": null},
  "decided_at": "2026-07-30T14:07:44Z",
  "request_version": 1,
  "request_digest": "sha256:…",
  "rendered": {"digest": "sha256:…", "ref": "rnd_01K3MB2R4Y8ZC4YRXB2N6VD9F"},
  "via": {"delivery_id": "dlv_01K3M7QW9A8ZC4YRXB2N6VD9FT", "channel": "inapp",
          "target": {"kind": "role", "value": "editor"}, "grade_reached": "acted"},
  "authority": {"required": {"min_role": "editor", "auth_strength": "session"},
                "satisfied": {"role": "editor", "auth_strength": "session"}},
  "steps": [],
  "capabilities_exercised": [],
  "clearance": {"source": "human_assertion", "actor": "usr_01J9M7QW8ZC4YRXB2N6VD9FTHE",
                "at": "2026-07-30T14:07:44Z"},
  "chain": {"height": 4211, "prev_digest": "sha256:…", "digest": "sha256:…"}
}
```

**`rendered.digest` is what the person actually saw.** A Server MUST compute it over the canonical
form of the request **as presented to this person at the step they decided on**, MUST retain a copy
addressable by `rendered.ref`, and MUST NOT re-derive it later from the request's current content. A
request may be amended in place; losing the prior renderings is what MUST NOT happen. Thirty-two
bytes converts "we have a log" into "the log cannot quietly be rewritten to say something else was
approved."

`actor.type` is one of `user`, `policy`, `runtime`, `anonymous_link`. A Server MUST NOT record
`user` unless it authenticated a person.

### 9.3 Presentation binding

A request MAY declare `presentation_binding`:

| Mode | Behaviour when the answer's `rendered_digest` does not match what the answerer was shown |
|---|---|
| `advisory` (default) | The answer is accepted and the divergence is recorded on the receipt |
| `strict` | The answer is rejected with `409 presentation_stale`; the person must re-read the current request |

A Client answering SHOULD echo the `rendered_digest` it displayed. A Server MUST record whichever
mode applied.

### 9.4 Immutability and tamper-evidence

A conforming Server MUST enforce receipt immutability in **three** layers:

1. **Application.** No code path may update or delete a receipt.
2. **Storage.** The storage engine itself MUST reject mutation and deletion of a receipt. This MUST
   be asserted from the storage layer directly, not from the application — application-level
   immutability is insufficient, because the threat includes the application. Conformance: C-15.
3. **Chain.** Each receipt MUST carry `chain.prev_digest` linking it to the previous receipt **in the
   same tenant**, forming a per-tenant hash chain. `chain.digest` is taken over the canonical form of
   the receipt including `prev_digest`. The chain head MUST be exportable.

Altering any historical receipt MUST invalidate the chain head. This gives tamper-evidence with no
key management at all. Detached signatures and external anchoring are OPTIONAL additions (§15,
`signing.md`) and MUST NOT replace the chain.

**What the chain does not detect, stated so that nobody has to discover it.** Height contiguity
catches an alteration anywhere and an excision from the middle, but **truncation of the tail is
undetectable from the chain alone**: removing the most recent receipts leaves a shorter chain that
verifies perfectly, because there is nothing left to point at what was removed. This is inherent to
an unanchored hash chain rather than a defect in it, and it is why the head MUST be exportable —
an exported head is the external anchor that makes truncation visible, and it is the only thing
that does.

A deployment that never records its head anywhere outside its own database has tamper-evidence
against alteration and against excision, and **no** evidence against a party who can delete the
newest rows. An implementation MUST NOT describe its chain as detecting truncation unless it also
anchors the head somewhere the same party cannot rewrite.

Note for implementers: once storage-level immutability is in place, a backfilling update is also
rejected. New receipt columns must therefore arrive as additive, defaulted columns.

### 9.5 Corrections

A receipt MUST NOT be edited. A correction is a **new receipt** with `kind: "correction"` and
`corrects` referencing the original. Both remain in the chain. This preserves I1: the request still
has exactly one *decision* receipt.

### 9.6 Policy receipts

An expiry under `expire_and_deny` or `default` MUST mint a receipt with `actor.type = "policy"` and
`authority.satisfied = "none"`.

It is a receipt because the waiter received an authoritative outcome and something may have happened
as a result: the audit must show it. It is visibly **not** a human decision. A record that cannot
distinguish a person from a policy from a passer-by is not a record.

### 9.7 Clearance provenance

Where an outcome depends on someone asserting that an out-of-band act is complete, the receipt MUST
record how that was established:

```json
"clearance": {"source": "human_assertion" | "runtime_inference" | "timeout",
              "actor": "usr_…" | null, "at": "…"}
```

> **Clearance MUST be asserted, never inferred.**

Observed state change — a URL change, a title change, a page transition — MAY prompt a surface to ask
a person to confirm, and a deployment MAY choose to auto-answer from it. In that case the Server MUST
record `source: "runtime_inference"` and `actor: null`. **A Server MUST NOT fabricate a person.**
Conformance: C-22.

A runtime SHOULD re-verify the asserted state against reality on resume, and where the observation
disagrees with the assertion, both MUST be recorded.

---

## 10. AUTHORIZATION

The receipt records what was decided. The authorization is what the runtime **spends**.

```json
{
  "id": "auth_01K3MB2R4Z8ZC4YRXB2N6VD9FT",
  "receipt_id": "rcpt_01K3MB2R4Y8ZC4YRXB2N6VD9FT",
  "grants": {"decision": "approve"},
  "single_use": true,
  "expires_at": "2026-07-31T14:07:44Z",
  "bound_to": {"waiter_ref": "run:0198f2a1", "effect_digest": "sha256:…"}
}
```

Rules:

1. **One answer mints exactly one authorization** (I10).
2. **Redemption is idempotent per `effect_key`.** A repeat with the same `effect_key` MUST return
   `200` with `first_redemption: false`. A retried turn MUST NOT be able to double-spend.
3. A `single_use` authorization redeemed with a **different** `effect_key` MUST be rejected with
   `409 authorization_spent`.
4. `expires_at` bounds how long a decision remains spendable. The RECOMMENDED default is 24 h. A
   redemption after it MUST be rejected with `409 authorization_expired`, which a Server MUST NOT
   conflate with `authorization_spent` or `authorization_not_found`: the decision was real and is on
   the record, it simply can no longer be spent, and an operator debugging a stalled runtime needs to
   be told which of the three happened.
5. `effect_digest` OPTIONALLY binds the authorization to the **shape** of the effect. When present, a
   redemption whose digest disagrees MUST be rejected with `409 effect_digest_mismatch`. An approval
   of "refund $2,400" cannot then be spent on "refund $24,000".

Conformance: C-13.

### 10.1 `advisory` and `gated` are modes of one design

| Mode | The runtime | The Server |
|---|---|---|
| `advisory` (default) | proceeds; the decision is typed input | records the decision, mints the authorization, never holds the effect |
| `gated` | must redeem before performing the effect | refuses redemption where there is no decision |

Both are the same code path with a different declared property. A Server MUST implement both and MUST
NOT fork its state machine between them.

> **Blocking is a property a request declares. It MUST NOT be a policy the platform imposes on a
> class of actions.**

A Server MUST NOT provide a deployment-level setting that forces requests matching some pattern into
`gated` mode. That is a different product — an interception gate — and it is outside this protocol.
Handoff is the mechanism by which a runtime that has concluded it cannot proceed asks for help; it is
not a mechanism for intercepting a runtime that did not ask.

---

## 11. Capabilities

### 11.1 A capability is an opaque handle

> **A capability MUST be carried as an opaque handle. The protocol MUST NOT carry a resolvable
> address, bearer URL, or credential by value — anywhere.**

Not in a request, not in a receipt, not in a delivery body, not in an event, not in a waiter signal,
and not in any message sent to a channel. Conformance: C-8, and the automatable scan of §16.

A grant handle MUST be generated from a cryptographically secure random source. It MUST NOT be
derived deterministically from a resource name, a shared secret, or any other recomputable input: a
derived handle cannot be rotated without rotating the input for every resource at once, and cannot be
revoked individually at all.

A grant carries:

| Field | Meaning |
|---|---|
| `handle` | The opaque public identifier, `hg_…` |
| `provider`, `resource_ref` | Registry coordinates. **The core MUST NOT match on these strings.** It looks the provider up and calls it |
| `type` | The capability kind, e.g. `interactive_surface` |
| `scope` | `view` or `drive` (§11.3) |
| `constraints` | Provider-enforced narrowing, opaque to the core |
| `blast_radius` | §11.5 |
| `expires_at`, `max_holders`, `bound_principal_id`, `revoked_at` | Lifecycle |

Adding a new capability — a remote desktop, a phone bridge, a document editor — MUST add **zero**
branches to the core. It registers a provider.

### 11.2 Resolution is a separate, authenticated call

```
POST /v1/grants/hg_…/sessions
Authorization: <the answering person's own session — never the handle>
{"scopes": ["view", "drive"], "accepted_blast_radius_digest": "sha256:…"}
```

A Server MUST perform these checks, in order, and MUST fail on the first that does not hold:

1. The request exists and is `pending`. A terminal request resolves nothing.
2. The grant is neither expired nor revoked.
3. The caller is a member of the grant's tenant and of any narrower scope the grant names.
4. The caller's role meets the minimum for **each requested scope** (§11.3).
5. Binding: the grant is unbound (bind now), or bound to this caller, or `max_holders > 1`.
6. `accepted_blast_radius_digest` matches the grant's current blast radius. On mismatch, respond
   `409 blast_radius_mismatch`.

The resolved session returns a `transport.url`. That URL MUST be minted at resolve time, MUST be
bound to this single session, MUST be short-lived, and MUST NOT be persisted in any protocol table,
appended to any event, or written to any message store. It is the only place in a conforming system
where a resolvable address exists.

A Server MUST NOT allow the agent runtime to resolve a capability. Resolution is performed by the
person's own client.

### 11.3 Scopes

| Scope | Meaning | Minimum authority | Enforced |
|---|---|---|---|
| `view` | Output only; no input accepted | the viewer role | **Server-side.** The relay or provider drops inbound input |
| `drive` | Full interactive control | the administrative role | **Server-side.** Only a `drive` session forwards input |

"Watch only" MUST be enforced at the grant and by the provider. A Server MUST NOT rely on a client-
side control — a disabled overlay, a non-interactive embed — to enforce `view`, because the same
token would permit both.

Narrowing beyond the two scopes MUST be expressed through `constraints`, which the core carries and
never inspects, and which the provider enforces. A Server MUST NOT grow a scope name per case.

### 11.4 Lifecycle

**Minting.** Server-side, inside the raise transaction, one grant per declared capability. The
runtime MUST NOT receive anything resolvable — at most the handle, and preferably only the
`request_id`.

**TTL.** A grant's `expires_at` SHOULD track the request's attempt deadline and be renewed whenever
the attempt is re-armed. A session lease is short (RECOMMENDED 120 s) and heartbeat-renewed. Two
clocks with different jobs: the grant tracks patience, the session tracks presence.

**Renewal** MUST re-check revocation, expiry, binding, and current authority, so that a role removed
mid-session takes effect within one lease period. A client that stops renewing MUST have its session
closed, and the receipt MUST record the real held duration.

**Revocation** MUST be a single operation on a single grant. Revoking one grant MUST NOT affect
other grants on the same resource, and MUST NOT require rotating a shared secret. A Server MUST
revoke all open grants implicitly when the request reaches a terminal state.

Conformance: a handle replayed after use, after expiry, after revocation, or by a different subject
MUST be rejected.

### 11.5 Blast radius MUST be declared

A person accepting a capability is accepting a scope of consequence. The grant MUST declare it, the
surface MUST show it before the accept control, and the receipt MUST record its digest.

```json
"blast_radius": {
  "summary": "Everything this workspace's browser is signed into",
  "shared_with": "space",
  "principals": 4,
  "identities": [{"origin": "mail.example.com", "label": "dana@example.com"}],
  "reversible": false,
  "note": "Actions you take here are indistinguishable from actions by the signed-in accounts."
}
```

Three rules and nothing else:

1. `summary` MUST be rendered to the person **before** the control that resolves the grant.
2. The resolve call MUST carry `accepted_blast_radius_digest`, and a mismatch MUST be a `409`. The
   person MUST NOT be handed something other than what they were shown.
3. The receipt MUST record `blast_radius_digest` only. The full content may be personal data and
   MUST NOT be required to live in the receipt.

`shared_with` ∈ `{isolated, request, space, org}` is core vocabulary because it is the one field a
Server must be able to **compare** for policy. Everything else in the object is opaque provider text.

### 11.6 What a capability record MUST contain

Where a capability was exercised, the receipt MUST record: the handle, the session, the scopes
granted, the role that justified them, when it was resolved and released, the held duration, a count
of input events, and the ordered list of navigations at origin granularity.

> **A Server MUST NOT record input content.**

Keystrokes, clipboard contents, and form values MUST NOT be logged. A person driving a live surface
types real passwords and one-time codes into it; an input log would reconstruct, inside the audit
trail, exactly the exposure §12 exists to prevent. Record **presence and effect**, never content.
"Who was handed control of what, when, and did they give it back" is an authorization question, and
it is answerable without recording a single keystroke.

---

## 12. Out-of-band secret custody

A request declares a **sink**; values travel to the sink; the answer carries only the fact of
provision.

```
POST /v1/sinks/snk_…/values          (the person's own session; TLS; never logged; never echoed)
{"values": {"email": "…", "password": "…"}}
→ 202 {"accepted": ["email", "password"], "state": "…"}

POST /v1/requests/req_…/answer
{"values": {"email": {"provided": true}, "password": {"provided": true}}}
```

Normative requirements:

1. **Declared-field allowlist.** A sink MUST reject any key that is not a declared field name of the
   request it belongs to, so that a compromised surface cannot smuggle arbitrary keys to the runtime.
2. **Values MUST NOT enter** the request, the receipt, the event record, a waiter signal, a delivery,
   a notification, a prompt sent to a model, an error message, or a screenshot. Only
   `{"provided": true}` travels. Conformance: C-7.
3. **Values MUST NOT appear in a URL, a query string, a path segment, a process argument vector, an
   environment variable, an HTTP header, a redirect target, a log line, a metric label, a trace
   attribute, or a crash report.** They MUST be transmitted in a request body over TLS and nowhere
   else. This sentence is normative and is testable: conformance test C-7 greps every artifact.
4. **The sink is supplied by the runtime, not by this protocol.** Handoff carries the *declaration*;
   the values travel a channel the runtime owns and can audit. A conforming implementation MUST
   provide or accept some such channel; this specification does not mandate a particular one.
5. **This specification defines no default sink implementation, and a conforming open
   implementation SHOULD NOT ship one.** A default sink is a credential-custody product wearing a
   protocol's clothes, and the only thing worse than no secret handling is secret handling nobody
   audited.
6. A Server MUST NOT echo a submitted value back in any response, including a validation error.

---

## 13. Errors

Every error response body has this shape:

```json
{"error": {"code": "already_answered",
           "message": "req_01K3M7… was answered at 14:07:44Z",
           "request_id": "req_01K3M7…",
           "receipt_id": "rcpt_01K3MB2R4Y…",
           "docs": "https://…/errors/already_answered"}}
```

`code` is stable and machine-readable. A Server MUST NOT change the meaning of a code within a major
version. `message` is for people and MAY change at any time; a Client MUST NOT parse it.

The complete taxonomy is enumerated in `openapi.yaml` as `ErrorCode`. The rules that are not obvious
from the list:

| Rule | Reason |
|---|---|
| `401 invalid_api_key` covers absent, malformed, revoked, and expired credentials as one code | A distinct "revoked" code tells an attacker which keys once existed |
| `404 …_not_found` is returned instead of `403` wherever existence is itself sensitive | §3.2 |
| `409` responses about a settled request carry the settling record — `receipt_id`, or `superseded_by` | A Client must be able to recover without a second round trip |
| `422 answer_validation_failed` carries per-field `{name, code, message}` | The surface must be able to mark the offending input |
| `503 delivery_unavailable` **never loses the request** | The request exists, the delivery failed, the ladder retries. Failing the raise would put the channel outage inside the caller's agent |

A Server MUST return rate-limit headers (`X-RateLimit-Limit`, `-Remaining`, `-Reset`) and MUST rate
limit per key. A Server MUST NOT open an API to long-lived external credentials without it.

A Server MUST use exactly one error envelope across its whole surface.

---

## 14. Conformance Level 2: the `continuation` extension

OPTIONAL. A raise MAY carry:

| Field | Rule |
|---|---|
| `resume_ref` | A URI the runtime owns. The Server MUST store it verbatim and MUST NOT dereference, parse, or interpret it |
| `resume_payload` | Opaque bytes, ≤ 64 KiB, base64-encoded. The Server MUST encrypt it at rest, MUST store it verbatim, and MUST NOT interpret it |

Both MUST be returned **byte-identical** in every signal for that request, and MUST appear nowhere
else — not in a listing, not in a receipt, not in an event, not in a log line. Conformance: C-17.

**The Server stores a pointer or a blob. It never stores meaning.** A runtime that can snapshot its
own state gets true continuation from its own snapshot; a runtime that cannot ignores these fields
and loses nothing that this protocol ever promised. This is the whole of the extension, and it is
deliberately the whole of it: continuation belongs to the runtime.

A Server implementing Level 2 MUST still satisfy every Level 1 requirement. A Level 1 Server MUST
accept and ignore these fields, or reject them with `400 invalid_request`; it MUST NOT store them
partially or return them altered.

---

## 15. Callbacks and signatures

Where a runtime's wait lives server-side, polling is wasteful. A request or a key MAY register a
callback endpoint, and the Server then POSTs each signal to it.

The signature scheme, the canonical string, the replay window, key identification and rotation, and
the worked test vectors are specified normatively in **`signing.md`**. The requirements that belong
here:

1. Every outbound callback MUST be signed, versioned, timestamped, and sequenced.
2. A receiver MUST reject a callback outside the freshness window, and MUST reject a valid signature
   replayed onto a different delivery.
3. **A valid signature proves the SENDER. It never proves the TENANT.** A receiver MUST resolve
   tenancy from its own stored state, keyed on the endpoint or the secret, and MUST NOT read tenancy
   from the callback body.
4. A `2xx` from a receiver marks the callback *dispatched*. It MUST NOT consume the signal;
   consumption is the ack (§8.3).
5. Retries MUST use exponential backoff with jitter and a bounded attempt count. Every attempt MUST
   be inspectable by the tenant. A repeatedly failing endpoint MUST eventually be disabled and the
   tenant notified: silent permanent retry is how queues die.
6. A callback MUST NOT carry a capability, a resolvable URL, or a secret value. Identifiers and typed
   values only.
7. Each attempt MUST have a request timeout. A hung receiver MUST NOT hold a delivery worker.

Conformance: C-18.

---

## 16. Security requirements

These twelve requirements are normative. Each is realized by numbered invariants in §17 and by tests
in §18.

1. **Server-minted, single-use, scoped, revocable grants — never bearer values.** §11. A conforming
   implementation MUST fail a test asserting that an observed grant string still works after use,
   after expiry, or after revocation.
2. **Requester ≠ decider, enforced by principal type.** §4.2.
3. **Receipts bind what the person saw** — a request digest *and* a rendered digest at the step
   decided on. §9.2.
4. **Receipts are append-only and tamper-evident**, asserted at the storage layer, with a per-tenant
   hash chain and an exportable head. §9.4.
5. **Out-of-band secret custody is normative, and the transport is part of it.** §12, including the
   explicit prohibition on URLs, argv, and every log-adjacent position.
6. **Decisions originate from principals, never from content.** §4.7.
7. **Clearance is asserted, never inferred.** §9.7.
8. **Request identity is tenant-scoped, never globally unique.** §3.2.
9. **Attempt lifetime is separate from request lifetime.** §6.3.
10. **Delivery is at-least-once and consumers dedupe**, stated explicitly rather than implied, with a
    per-(request, channel) delivery record. §7, §8.3.
11. **Outbound callbacks are signed, versioned, sequenced, and replay-rejectable**, and the signature
    proves the sender, not the tenant. §15.
12. **Grant blast radius is declared** and shown to the person before they accept. §11.5.

---

## 17. Invariants

**These numbers are stable.** The conformance suite references them, and they MUST NOT be renumbered.
Additions take the next free number.

| # | Invariant |
|---|---|
| **I1** | One request has many deliveries and **at most one** decision receipt. |
| **I2** | A receipt is immutable and records what the person **saw**, not what the request later became. |
| **I3** | Escalation, reminders, and channel fallback mint **deliveries**, never requests. |
| **I4** | A `pending` request is **always listable and always answerable** at its canonical URL. A lapsed attempt changes urgency, never visibility. |
| **I5** | Answering is **first-writer-wins**. A conflicting second answer is a `409` and changes nothing. A landed answer beats a racing cancel or expiry. |
| **I6** | Required authority is declared on the request and evaluated **at answer time** against the answerer's authenticated identity. **A delivery channel never confers authority.** |
| **I7** | `secret` values never enter the request, the receipt, the event record, a waiter signal, or any delivery. Only `{"provided": true}` travels. |
| **I8** | Capability grants are **opaque handles** resolved through an authenticated endpoint. The protocol never carries a resolvable address by value. |
| **I9** | The outcome is delivered to the waiter as **typed data**, retried until acked; the ack is idempotent. |
| **I10** | One answer mints **one** authorization. Redemption is idempotent per `effect_key`, and a single-use authorization cannot be spent twice. |
| **I11** | Every terminal transition produces a typed terminal signal. **A request never goes quiet.** |
| **I12** | Every state transition emits its event **in the same transaction** as the state change. |
| **I13** | Tenant binding is resolved from **stored state**, never from a request body. |
| **I14** | The core **never switches on a request kind**. New interaction types arrive as new field types or capability types, behind the declaration. |
| **I15** | A **requester principal can never answer** its own request. |
| **I16** | A decision originates from an authenticated **principal**. It is never derived from message content, and never inferred from observed state. Inference may be recorded as `runtime_inference` with no actor; it is never recorded as a person. |
| **I17** | Every identifier and every client-supplied key is **tenant-scoped**. Lookups are tenant-scoped; uniqueness is tenant-scoped; possession of an identifier is never authorization. |
| **I18** | Secret values never appear in a URL, query string, path, argv, environment variable, header, redirect, log line, metric label, trace attribute, or crash report. |
| **I19** | A capability grant declares its **blast radius**; the person is shown it before accepting; the accepted digest binds the resolve; the receipt records its digest. |
| **I20** | Every mutating operation is **idempotent** under a caller-supplied key, and every idempotency key is tenant-scoped. |
| **I21** | Unknown protocol versions, field types, and capability types **fail closed**. Nothing is created, and nothing degrades silently. |

---

## 18. Conformance suite

A Server is **Handoff v0.1 Level 1 compliant** when it passes all 25 Level 1 cases: C-1 through
C-16, plus C-6b and C-18 through C-25. Level 2 adds C-17. Each test is black-box,
against the HTTP API.

**Case identifiers are stable and MUST NOT be renumbered.** A case that is withdrawn keeps its
identifier and is marked withdrawn; new cases take the next free number. `C-6b` is a suffixed variant
of C-6 and is a distinct case, not a sub-step of it.

**Every invariant in §17 has at least one case.** The `Invariants` column below is the normative
mapping, and it is duplicated in machine-readable form at `conformance-map.json` so that an
implementation can iterate it without parsing this table. A Level 2 case is not required to map to a
Level 1 invariant, and C-17 is the only such case.

| Id | Level | Test | Invariants |
|---|---|---|---|
| C-1 | 1 | Raise twice with one `Idempotency-Key` → one request, `201` then `200`, identical body | I20 |
| C-2 | 1 | Raise twice with the same `dedupe_key` while pending → one request | I1, I20 |
| C-3 | 1 | Two concurrent answers → one `200` with a receipt, one `409 already_answered`; the request is answered exactly once | I5 |
| C-4 | 1 | Answer, then cancel → `409 already_answered`; the receipt is intact | I5 |
| C-5 | 1 | A machine key calls `/answer` → `403 requester_may_not_answer` | I15 |
| C-6 | 1 | Answer as a principal below `min_role` → `403 insufficient_authority` | I6 |
| C-6b | 1 | A deployment that forbids `link_only` rejects an answer at that grade; a deployment that has opted in accepts it and the receipt records `actor.type = "anonymous_link"` and names no principal | I6, I2 |
| C-7 | 1 | Answer carrying a raw value for a `secret` field → `422`. Then scan the request, receipt, event record, signal, logs, and every URL for the value → zero hits | I7, I18 |
| C-8 | 1 | No request, receipt, delivery, signal, or outbound message contains a resolvable grant. Resolve without a session → `401`. Two resolves return different URLs. A handle replayed after use, after expiry, after revocation, or by another subject → rejected. Resolving with a stale `accepted_blast_radius_digest` → `409 blast_radius_mismatch`, and the grant's declared blast radius is readable before the resolve control | I8, I19 |
| C-9 | 1 | Attempt TTL lapses → exactly one `attempt_lapsed` signal, ever; the request is still `pending` and **still listed** (R3) | I4 |
| C-10 | 1 | Request TTL lapses under `expire_and_deny` → `expired` plus a policy receipt with `actor.type = "policy"`; the waiter receives a typed terminal signal | I11 |
| C-11 | 1 | Kill the client mid-poll, restart, reattach (W7) → the signal is still there and still unacked | I9, I11 |
| C-12 | 1 | Ack twice → both `200`, redelivery stops once, no duplicate application | I9 |
| C-13 | 1 | Redeem twice with the same `effect_key` → `200` both, `first_redemption` true then false. A different `effect_key` on a single-use authorization → `409` | I10 |
| C-14 | 1 | Escalate through three rungs → three or more deliveries, still exactly one request and one receipt | I3, I1 |
| C-15 | 1 | Attempt to update or delete a receipt **at the storage layer** → rejected by storage. The chain verifies over the full history, and altering any historical entry invalidates the head | I2 |
| C-16 | 1 | Post `requires: {"v": 2, …}` to a v1 Server → `400 unsupported_requires_version`, and **no request is created**. Post an unknown field `type` → `400 unsupported_field_type` | I21 |
| C-17 | **2** | `resume_ref` and `resume_payload` are returned byte-identical in the signal and appear nowhere else, including in logs | — (Level 2, §14) |
| C-18 | 1 | A valid callback signature replayed onto a different delivery is rejected; a body altered by one byte is rejected; a timestamp outside the window is rejected; both secrets verify during a rotation overlap; sequence numbers are monotonic per waiter. A receiver resolving tenancy from the callback body rather than from stored state fails the case | I13 |
| C-19 | 1 | The same idempotency key used by **two different tenants** succeeds for both and each sees only its own; the same key twice within one tenant collapses to one | I17, I20 |
| C-20 | 1 | Every read returns exactly the caller's tenant's rows, asserted on **length and identity** (never `contains` — a query missing its tenant predicate returns a superset and `contains` passes). Every mutation by tenant A against tenant B's id fails, and B re-reads to prove its row is untouched. A body-supplied tenant identifier is ignored: tenancy comes from the credential | I17, I13 |
| C-21 | 1 | Channel content matching a decision format does not settle a request; a channel declaring `can_authenticate_person: false` produces a provisional answer only | I16, I6 |
| C-22 | 1 | All eight interaction patterns in `fixtures/use-cases/` are accepted and answerable with **no request-kind field anywhere in the wire traffic**. A page-state change alone produces no clearance receipt | I14, I16 |
| C-23 | 1 | Drive every transition in §6.2 and §8.2. For each one, the state change and its event are observable together: no state exists whose event is missing, and no event exists whose state change was rolled back. Kill the Server between the state write and the event write → after restart, either both are present or neither is | I12 |
| C-24 | 1 | Answer a `number` field with a non-integer, with `1e21`, and with `2^53` → each is `422 answer_validation_failed` naming the field, and no receipt is minted. Answer with `0`, `1`, and `9007199254740991` (2^53 − 1) → accepted. A canonicalizer MUST refuse a non-integer rather than render it. Canonicalize `fixtures/signing/receipt-core.json` and `fixtures/signing/callback-body.json` and reproduce their exact byte lengths and SHA-256 digests from `signing.md` | I2, I21 |
| C-25 | 1 | Answer request A with a key, then answer request B with the **same** key and an identical body → B is answered, naming B, with its own receipt and its own authorization. A retry of A with that key still replays A's receipt | I20, I10, I1 |

Four notes for implementers:

- **C-19 and C-20 are the tests that pass in development and fail in production.** A globally unique
  key does not error on collision; it drops the second tenant's row. A query missing its tenant
  predicate passes every test written with `contains`.
- **C-15 must be asserted from the storage layer**, not through the application. The application is
  inside the threat model.
- **C-7 and C-8 are scans, not unit tests.** They must search every artifact the system produced
  during the scenario, including logs.
- **C-23 is the case an implementation is most tempted to skip**, because emitting the event just
  after the commit passes every happy-path test. It only fails under a crash, and by then the record
  and the state disagree permanently.

A version of this protocol is "released" when the conformance suite passes. A change that cannot be
expressed as a conformance test is not a protocol change; it is an implementation detail.

---

## 19. Versioning and extension policy

| Rule | Detail |
|---|---|
| **Major version in the path** | `/v1`. A new major ships alongside the old; the old runs for at least 12 months past the successor's general availability |
| **Additive is not breaking** | New response fields, new optional request fields, new enum members in **response-only** positions. A Client MUST ignore unknown response fields |
| **Breaking** | Removing or renaming a field, narrowing a type, adding a required field, changing an error `code`, or changing a default in a way that alters behaviour |
| **Versioned envelopes fail closed** | `requires: {"v": 1, …}`. A Server that does not understand the declared version MUST reject with `400 unsupported_requires_version` and MUST create nothing. Partial acceptance is forbidden |
| **New field and capability types fail closed** | An unknown `type` MUST be a `400`, never a text box and never a silent drop. A capability nobody can render is a request nobody can answer |
| **Extension namespace** | Keys prefixed `x-` inside `metadata` and `requires` MUST be stored verbatim, returned verbatim, and never interpreted. Vendors extend there. A Server MUST NOT interpret an `x-` key, and a Client MUST NOT depend on a Server doing so |
| **Deprecation** | A deprecated surface MUST respond with `Handoff-Deprecation: true` and a `Sunset` date (RFC 8594), with at least 180 days' notice and a `CHANGELOG.md` entry |
| **Discovery** | `GET /v1/meta` MUST report the protocol version, the conformance level, the supported field types, the supported capability types, and the implemented extensions |

**Why fail-closed is the rule everywhere.** A request the Server only partly understands is a request
the person will be shown incompletely, and the receipt would then record consent to something nobody
saw. Rejecting is always recoverable; a misrepresented decision is not.

---

## Appendix A — A managed profile (non-normative)

Nothing in this appendix is required for conformance, and a conforming implementation MUST NOT depend
on any of it. It is included so that the normative text above can stay free of vendor concepts while
still showing how one hosted deployment fills in what the protocol deliberately leaves open.

A managed profile may:

- **Bind principals to an existing identity system.** The profile used by the reference hosted service
  requires every answerer to be a real member of the tenant, which means `auth_strength: link_only` is
  refused (§4.4, C-6b) and an external approver who is not a member is not yet expressible. That is a
  profile decision, not a protocol one; the grade and the actor union exist precisely so that adding
  an external-principal type later is additive under §19.
- **Mint machine credentials centrally.** A hosted deployment may have its control plane mint API keys
  and exchange them for short-lived signed tokens that the Handoff service verifies locally, so that
  key verification does not put another service on the hot path. A self-hosted deployment mints and
  verifies its own. One verification port, two issuers.
- **Operate the delivery fleet.** Channel adapters, per-person routing, quiet hours, and on-call
  rotations are operational concerns. The protocol defines the delivery model, the grades, and the
  ladder; it does not oblige an implementation to run any particular channel.
- **Mirror receipts into a tenant-wide audit read model.** The receipt itself stays in the Handoff
  store, because it is written in the outcome transaction and no transaction spans two services. A
  derived mirror carrying the receipt id, the digest, the actor, and the outcome — **never the answer
  payload and never the prompt** — may be emitted asynchronously from a durable outbox and retried
  until acked. Derived records may be delayed; they must not be silently dropped.
- **Meter and bill.** Where a hosted service meters interventions, the meter emission is subject to
  the same tenant-scoping rule as everything else (§3.2, C-19): a globally unique meter key means one
  tenant's usage silently absorbs another's.
- **Add tiered retention, detached receipt signatures, and external anchoring** on top of the hash
  chain of §9.4. These strengthen the chain; they MUST NOT replace it, because an open implementation
  with no such service must still be able to make the protocol's central claim.

The line this profile draws, stated once: **the open protocol defines the receipt and the chain; a
managed service operates the archive.**

---

## Appendix B — Claim language (non-normative)

Implementations and derived documentation are strongly encouraged to keep to the left column. The
right column overstates what any implementation of this specification can deliver.

| Defensible | Not defensible |
|---|---|
| "One human answer, delivered to your runtime as typed data, authorizing exactly one effect." | "Exactly-once delivery" without qualification |
| "Your runtime picks up from the wait it registered, with the answer in hand." | "Your agent resumes exactly where it stopped" |
| "Survives your process dying, your worker redeploying, and every retry in between." | "We snapshot your agent's memory" |
| "The decision arrives as typed data your code branches on, never as prose a model has to interpret." | "Perfect context preservation" |
| "If nobody answers, your runtime gets a typed terminal answer, never silence." | "Never times out" |

The distinction the protocol is built around: **an approval asks a person to reply; a handoff asks
them to act** — and the receipt records which of the two happened.

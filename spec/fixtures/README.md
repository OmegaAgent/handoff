# Fixtures

Canonical examples for the Handoff Protocol v0.1. These are **normative for conformance**: the SDKs
and the reference server are asserted against them, and two of them are byte-exact inputs to the
signature test vectors.

## How to use them

| Directory | What it is | How it is asserted |
|---|---|---|
| `*.json` (this directory) | One canonical object per file — a request body, a response body, or an error envelope | SDK serialization is asserted **byte-identical** after re-encoding with 2-space indent and the key order shown here. Deserialization is asserted field-for-field |
| `use-cases/*.json` | Scenario fixtures for the eight required interaction patterns | Driven end to end by conformance test **C-22**. Each file carries a `raise` body (or `null` where the pattern is not a request shape), the follow-up call bodies, and an `expect` block |
| `signing/*.json` | Byte-exact inputs to the test vectors in `signing.md` | Hashed and signed as-is. **Do not reformat these two files** — no trailing newline, no whitespace, keys sorted by code point |

## The two files that must not be touched

`signing/callback-body.json` (493 bytes) and `signing/receipt-core.json` (1125 bytes) are the exact
byte sequences the worked vectors in `signing.md` are computed over. Reformatting either one — adding
a trailing newline, pretty-printing, reordering a key — changes its SHA-256 and invalidates every
signature in that document.

They are also the check on your canonicalization. An implementation that reproduces their byte
lengths and hashes is doing RFC 8785 correctly for this protocol's value types; one that does not has
a bug, regardless of what its own tests say.

## Consistency guarantees these fixtures hold

These are asserted mechanically, and a change that breaks one is a bug in the fixture set:

1. `08-receipt-decision.json` is exactly `signing/receipt-core.json` plus its `chain` member. Its
   `chain.digest` is the value the worked vector in `signing.md` §2.5 computes and the Ed25519 test
   signature verifies over.
2. `05-signal-answered.json` and `signing/callback-body.json` describe the same signal. The signing
   copy is the canonical wire form; the other is the readable one.
3. `02-request-created.json`, `08-receipt-decision.json`, `09-receipt-policy.json`,
   `11-delivery.json`, and `19-tenant-policy.json` validate against `../schemas/*.schema.json`.
4. Every `use-cases/*.json` `raise.prompt`, `raise.requires`, and `raise.ttl_policy` validates against
   the corresponding definition in `../schemas/request.schema.json`.
5. No fixture anywhere contains a secret value, a resolvable capability address, or a request `kind`
   field. The absence of the third is the point of `use-cases/`.

## Index

| File | Object |
|---|---|
| `01-raise-request.json` | `POST /v1/requests` body |
| `02-request-created.json` | `201` response, deliveries queued, receipt null |
| `03-answer-request.json` | `POST /v1/requests/{id}/answer` body |
| `04-answer-result.json` | `200` response with receipt and authorization |
| `05-signal-answered.json` | A terminal `answered` signal |
| `06-signal-expired.json` | A terminal `expired` signal, `effective: "deny"` |
| `07-signal-attempt-lapsed.json` | The non-terminal nudge — `decision` is null and the request stays pending |
| `08-receipt-decision.json` | A decision receipt with its chain entry |
| `09-receipt-policy.json` | A policy receipt: `actor.type = "policy"`, `authority.satisfied = "none"` |
| `10-authorization.json` | A spent single-use authorization with its redemption |
| `11-delivery.json` | A delivery that reached grade `acted` |
| `12-capability-grant.json` | A grant with its full blast radius, as shown to the person before accepting |
| `13-grant-session.json` | A resolved session — the only place a resolvable address exists |
| `14-error-already-answered.json` | `409` carrying the settling receipt |
| `15-error-requester-may-not-answer.json` | `403` — the requester ≠ decider rule |
| `16-error-unsupported-requires-version.json` | `400` — fail closed, nothing created |
| `17-reattach-response.json` | Waiter reattach after client process death |
| `18-meta.json` | `GET /v1/meta` discovery document |
| `19-tenant-policy.json` | A complete tenant policy document |

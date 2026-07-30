# Handoff signatures — callbacks and receipts

**Status:** Normative. Part of the Handoff Protocol v0.1.
**Companion:** `handoff-protocol-v0.1.md` §9.4 (receipt chain), §15 (callbacks).

RFC 2119 keywords are used as defined in `handoff-protocol-v0.1.md` §1.1.

This document specifies two independent schemes:

| Scheme | Protects | Algorithm | Who can verify |
|---|---|---|---|
| **Callback signatures** | An outbound signal POSTed to a receiver's endpoint | HMAC-SHA-256 | The receiver, using a shared secret |
| **Receipt signatures** | A receipt and the tenant hash chain it belongs to | Ed25519, detached | Anyone, using the published verification key |

They are separate because they answer different questions. A callback needs a symmetric secret the
receiver already has and can rotate on its own schedule. A receipt needs to be verifiable by a party
who was never given a secret and must not be able to forge one — an auditor, a regulator, a customer
after they have left.

---

## 0. The claim a signature makes, and the one it does not

> **A valid signature proves the SENDER. It never proves the TENANT.**

A verified signature establishes that the payload was produced by a party holding the corresponding
key. It establishes nothing whatsoever about which tenant, organization, workspace, or customer the
payload concerns.

Therefore:

1. A receiver MUST resolve tenancy from its own stored state — keyed on the endpoint that received
   the callback, or on the secret that verified it — and MUST NOT read tenancy from any field in the
   callback body.
2. A Server MUST NOT accept a tenant identifier from an inbound body on any signed ingestion path.

Trusting the body would let any party holding a single valid key target an arbitrary tenant. This is
the same rule as invariant I13, restated here because a signature is exactly the thing that tempts an
implementer to relax it.

---

## 1. Callback signatures

### 1.1 Headers

Every outbound callback MUST carry:

| Header | Value | Required |
|---|---|---|
| `Handoff-Signature` | `t=<unix_seconds>,v1=<hex>[,v1=<hex>]` | MUST |
| `Handoff-Delivery` | The delivery identifier, `dlv_…` | MUST |
| `Handoff-Signal` | The signal identifier, `sig_…` | MUST |
| `Handoff-Version` | The signature scheme version. `1` for this document | MUST |
| `Handoff-Sequence` | The signal's `sequence`, mirroring the body field | MUST |
| `Handoff-Idempotency-Key` | The delivery identifier, so a receiver can dedupe without parsing the body | MUST |
| `Handoff-Signature-Key-Ids` | Comma-separated secret identifiers, in the same order as the `v1` values. Diagnostic only | MAY |
| `Content-Type` | `application/json` | MUST |

`Handoff-Sequence` is a convenience mirror. Its authoritative value is the `sequence` field inside the
body, which the body hash covers. A receiver that uses the header MUST check it against the body
value and MUST reject a mismatch.

### 1.2 The canonical string

```
canonical = version ‖ LF ‖ timestamp ‖ LF ‖ delivery_id ‖ LF ‖ body_sha256_hex
signature = lowercase_hex( HMAC_SHA256( secret_utf8, canonical_utf8 ) )
```

Precisely:

| Element | Definition |
|---|---|
| `version` | The value of `Handoff-Version`, as ASCII decimal. `1` for this document |
| `LF` | A single line feed, `0x0A`. Never `CRLF` |
| `timestamp` | The `t=` value: Unix time in **seconds**, ASCII decimal, no sign, no fraction, no padding |
| `delivery_id` | The value of `Handoff-Delivery`, verbatim |
| `body_sha256_hex` | `SHA-256` over the **exact bytes of the request body as transmitted**, rendered as 64 lowercase hex characters |
| `secret_utf8` | The UTF-8 bytes of the secret **exactly as issued, including its `whsec_` prefix** |

There is no trailing line feed. The canonical string has exactly three `LF` bytes.

Four properties this construction is chosen for, each deliberate:

| Property | Choice | Why |
|---|---|---|
| Algorithm | HMAC-SHA-256, lowercase hex | Symmetric, present in every standard library, and the shape most receivers already have code for |
| The body **hash**, not the body | `sha256(raw_body)` | A receiver can verify before buffering a large body, and canonicalization is unambiguous. Concatenating timestamp and body directly is unambiguous only until a body begins with a digit |
| `delivery_id` inside the signed string | Included | Binds the signature to **this** delivery, so a valid signature cannot be lifted onto a different delivery of the same payload |
| Explicit version element | `v1=` and `Handoff-Version` | Adding `v2=` alongside `v1=` is how the algorithm can ever change without a flag day |

### 1.3 Verification (receiver)

A receiver MUST perform all of these, and MUST reject on the first failure:

1. Parse `Handoff-Signature` into `t` and one or more `v1` values. Reject a malformed header.
2. Reject if `|now − t| > 300` seconds. The freshness window is **300 seconds**, receiver-enforced.
3. Compute `sha256` over the raw received body **before** any parsing, re-serialization, or
   normalization. A receiver MUST NOT re-serialize the body and hash the result.
4. Rebuild the canonical string from `Handoff-Version`, `t`, and `Handoff-Delivery` — never from
   values found inside the body.
5. For each configured **active** secret, compute the expected signature and compare it against each
   supplied `v1` value using a **constant-time** comparison. Accept if any pair matches.
6. Reject a body whose `sequence` disagrees with `Handoff-Sequence`.
7. Deduplicate on `Handoff-Delivery`. A repeated delivery MUST NOT produce a second application of
   the decision.

A receiver SHOULD additionally track the highest `sequence` seen per `waiter_ref` so that gaps and
reordering are detectable. A gap is not by itself an error — delivery is at-least-once and retries
reorder — but a receiver that never observes a gap closing SHOULD raise it operationally.

A receiver MUST NOT treat its own `2xx` as consumption of the signal. Consumption is
`POST /v1/signals/{id}/ack` (`handoff-protocol-v0.1.md` §8.3).

### 1.4 Key identification and rotation

A callback secret is identified by an opaque identifier of the form `whsec_<random>`. The identifier
**is** the secret string; there is no separate public id. Secrets MUST be generated from a
cryptographically secure random source and MUST be displayed to the operator exactly once.

Rotation is an **overlap**, not a cutover:

1. The operator adds a second secret. Both are now active.
2. While two secrets are active, the Server MUST sign every callback with **both** and emit both as
   separate `v1=` elements in one `Handoff-Signature` header.
3. The receiver, which accepts "any active secret matches", now verifies under either. There is no
   window in which valid callbacks fail.
4. The operator removes the old secret. The Server emits one `v1=` again.

A Server MUST support at least two simultaneously active secrets per endpoint. A Server MUST NOT
require the receiver to be updated between steps 1 and 4.

A receiver MUST NOT accept a signature from a secret that has been removed, and MUST NOT keep a
removed secret in its active set.

### 1.5 Delivery, retry, and inspection

| Rule | Value |
|---|---|
| Timeout per attempt | RECOMMENDED 10 s. A Server MUST set some timeout; a hung receiver MUST NOT hold a delivery worker |
| Backoff | Exponential, `2^n` seconds capped at 300, with jitter |
| Maximum attempts | RECOMMENDED 12, spanning roughly 24 hours |
| Inspection | Every attempt MUST be listed at `GET /v1/signals/{id}/attempts` with status, response code, and duration |
| Manual redelivery | `POST /v1/deliveries/{id}/redeliver` MUST be available to the tenant |
| Circuit breaking | An endpoint failing every attempt for 24 hours MUST be disabled and the tenant notified. Silent permanent retry is how queues die |

A callback MUST NOT carry a capability handle's resolved address, a bearer URL, or a secret value.
Identifiers and typed values only (I8, I18).

### 1.6 Worked test vectors — callbacks

An implementation is expected to reproduce every value below exactly.

**Secrets**

```
secret A : whsec_2f8a91c4e7b3d05a6c1e9f47b28d3a05
secret B : whsec_9d41c07be5a2f36819b4d0e7c5a81f62
```

**Body** — the exact 493 bytes transmitted, also stored byte-identical at
`fixtures/signing/callback-body.json` (no trailing newline):

```json
{"created_at":"2026-07-30T14:07:44Z","decision":{"authorization_id":"auth_01K3MB2R4Z8ZC4YRXB2N6VD9FT","outcome":"answered","receipt_id":"rcpt_01K3MB2R4Y8ZC4YRXB2N6VD9FT","source":"human","values":{"decision":"approve","note":"Confirmed with Acme on the phone."}},"id":"sig_01K3MB2R4X8ZC4YRXB2N6VD9FT","request_id":"req_01K3M7QW8ZC4YRXB2N6VD9FTHE","resume_payload":null,"resume_ref":null,"resume_token":"rt_01K3MB2R558ZC4YRXB2N6VD9FT","sequence":1,"type":"answered","waiter_ref":"run:0198f2a1"}
```

**Derived values**

```
body length            493 bytes
sha256(body)           fbd6ec4cacc7cb9c9371d2791f946535e3d391a0594a92b5a3a27dd34f5e94fa
version                1
timestamp (t)          1785592064
delivery_id            dlv_01K3MB2R6C8ZC4YRXB2N6VD9FT
```

**Canonical string** — shown with `\n` for the three line feeds; the byte sequence contains no other
whitespace and no trailing newline:

```
1\n1785592064\ndlv_01K3MB2R6C8ZC4YRXB2N6VD9FT\nfbd6ec4cacc7cb9c9371d2791f946535e3d391a0594a92b5a3a27dd34f5e94fa
```

**Signatures**

| Secret | `v1` value |
|---|---|
| A | `cae13126f8dcd1e918376aa373be2757db7281a3e5aaed2d83d716537e03de80` |
| B | `d86b3740bad654e46c1349614523a476be0eb7d6a30a798b2d475374f36c57eb` |

**The header, during a rotation overlap:**

```
Handoff-Signature: t=1785592064,v1=cae13126f8dcd1e918376aa373be2757db7281a3e5aaed2d83d716537e03de80,v1=d86b3740bad654e46c1349614523a476be0eb7d6a30a798b2d475374f36c57eb
Handoff-Delivery: dlv_01K3MB2R6C8ZC4YRXB2N6VD9FT
Handoff-Signal: sig_01K3MB2R4X8ZC4YRXB2N6VD9FT
Handoff-Version: 1
Handoff-Sequence: 1
Handoff-Idempotency-Key: dlv_01K3MB2R6C8ZC4YRXB2N6VD9FT
```

**Negative vectors.** A conforming receiver MUST reject all four.

| Case | Input change | Expected |
|---|---|---|
| Tampered body | `"approve"` → `"reject"`; body hash becomes `8d1b25a370b6de9d1a504ca1acfe97dc7abe10d4c12b0d33dfaf74f5114eb019` | The header's `v1` no longer matches. **Reject.** (A signature computed over the tampered body under secret A would be `621af1622c79ccb0d444ae046dae7db4a8e5b96c6ae0d9bd574ff8bc0be26a66` — an attacker without the secret cannot produce it) |
| Replayed onto another delivery | Same body and same `v1`, but `Handoff-Delivery: dlv_01K3MB2R6D8ZC4YRXB2N6VD9FT` | **Reject.** The valid signature for that delivery is `9a674a003d0507ad13369a6bd82713769116a276ec57f26eb2637b2af00f8e68` |
| Stale timestamp | `t=1785591763` (301 s earlier), signature recomputed and valid | **Reject** — outside the 300 s window |
| Retired secret | Signed under a secret removed from the active set | **Reject** |

**Reference verifier** (Python; the whole scheme is nine lines):

```python
import hmac, hashlib, time

def verify(raw_body: bytes, headers: dict, active_secrets: list[str], window: int = 300) -> bool:
    parts = dict(p.split("=", 1) for p in headers["Handoff-Signature"].split(","))  # first v1 only
    sigs  = [p.split("=", 1)[1] for p in headers["Handoff-Signature"].split(",") if p.startswith("v1=")]
    t     = int(parts["t"])
    if abs(time.time() - t) > window:
        return False
    canonical = f'{headers["Handoff-Version"]}\n{t}\n{headers["Handoff-Delivery"]}\n{hashlib.sha256(raw_body).hexdigest()}'
    for secret in active_secrets:
        expected = hmac.new(secret.encode(), canonical.encode(), hashlib.sha256).hexdigest()
        if any(hmac.compare_digest(expected, s) for s in sigs):
            return True
    return False
```

---

## 2. Receipt signatures

### 2.1 What signs what

Receipt integrity has two layers, and they are not interchangeable.

| Layer | Mechanism | Required |
|---|---|---|
| **Chain** | Per-tenant hash chain, `chain.prev_digest` → `chain.digest` | **MUST.** This is the base mechanism; it needs no key management at all |
| **Signature** | Detached Ed25519 over the chain digest | **MAY.** An additive strengthening |

A Server MUST implement the chain. A Server MAY implement signatures. A Server MUST NOT present
signatures as a substitute for the chain: a self-hosted implementation with no key infrastructure
must still be able to make the protocol's central claim.

The chain gives tamper-evidence within one store. The signature adds attribution to a signer and lets
a third party verify without access to the store.

### 2.2 The receipt core and the chain digest

Two canonicalizations, in order.

**Step 1 — the receipt core.** Take the receipt object **excluding** its `chain` member. Canonicalize
it with RFC 8785 (JCS) and encode as UTF-8. This byte sequence is the *receipt core*.

```
core_hash = lowercase_hex( SHA-256( receipt_core ) )
```

**Step 2 — the chain digest.**

```
chain_input  = height ‖ LF ‖ prev_digest ‖ LF ‖ core_hash
chain.digest = "sha256:" ‖ lowercase_hex( SHA-256( chain_input ) )
```

| Element | Definition |
|---|---|
| `height` | The receipt's 0-based position in the tenant's chain, ASCII decimal |
| `prev_digest` | The previous receipt's `chain.digest`, in full including the `sha256:` prefix. For the first receipt in a tenant, 64 ASCII zeros prefixed `sha256:` |
| `core_hash` | From step 1, **without** a `sha256:` prefix |

Including `height` binds a receipt to its position, so an entry cannot be excised and the remaining
entries re-linked without detection.

**Verifying a chain.** Recompute every `chain.digest` from `height`, the predecessor's digest, and
the core hash. Any historical alteration changes that receipt's core hash, which changes its digest,
which invalidates every subsequent digest and therefore the exported head. Conformance: C-15.

### 2.3 The detached signature

```
sig_input = "handoff-receipt-v1" ‖ LF ‖ kid ‖ LF ‖ chain.digest
signature = Ed25519( signing_key, sig_input )
```

Encoded as unpadded base64url. A signature is published alongside the receipt, never inside the
receipt core — signing the core in place would make the core's own digest depend on the signature.

```json
"signature": {
  "alg": "Ed25519",
  "kid": "rk_01K3MB2R4Y8ZC4YRXB2N6VD9FT",
  "sig": "av8Iq2KkysJR6J3na_k6GHTS26ajN3CNsT4iOyHcJUy9mTxvF1hD0moPcg4kFGkklv1u2cGiijm76V2icmwZCw"
}
```

Signing the **chain digest** rather than the core means one signature attests to both the receipt's
content and its position in the chain. Signing the head periodically — an anchor — then attests to
every receipt beneath it.

### 2.4 Key identification and rotation

| Rule | Detail |
|---|---|
| Identification | `kid`, an opaque identifier `rk_…`. A verifier MUST select the key by `kid` and MUST NOT assume a single key |
| Publication | Verification keys MUST be published at a stable, unauthenticated location as a JWKS document with `kty: "OKP"`, `crv: "Ed25519"`, `use: "sig"`, `alg: "EdDSA"` |
| Overlap | A retired key MUST remain published for at least as long as the receipts it signed are retained. Old receipts must stay verifiable forever |
| Algorithm agility | The prefix `handoff-receipt-v1` inside the signed input is the version. A future algorithm uses `handoff-receipt-v2` and a different `alg`, and both may be published for the same receipt |
| Private key | MUST NOT be exportable through any API surface defined by this specification |

A verifier MUST reject a signature whose `kid` is unknown. A verifier MUST NOT fall back to "any
published key" — that would make key retirement meaningless.

### 2.5 Worked test vectors — receipts

**Signing key.** Deterministic, published so that anyone can regenerate it. The 32-byte Ed25519 seed
is `SHA-256("handoff-spec-v0.1-test-vector-key")`:

```
seed (hex)        dbcb1a7a2012be306784fad7a454ac8fa398e42247df01153334576209b010c8
public key (hex)  fb83e7234defb5402d3123ce1753df2e30313285cf194f4b7651bf5530646f98
public key (b64u) -4PnI03vtUAtMSPOF1PfLjAxMoXPGU9LdlG_VTBkb5g
kid               rk_01K3MB2R4Y8ZC4YRXB2N6VD9FT
```

**This key is for test vectors only.** It MUST NOT be used by any deployment.

**Receipt core.** 1125 bytes, stored byte-identical at `fixtures/signing/receipt-core.json` (no
trailing newline). It is the receipt of §9.2 of the protocol document with the `chain` member removed
and JCS canonicalization applied.

```
sha256(receipt_core)   2763f39ef8a61d493106d3db302ec36cae5c024ca3da3a019d483ccc29704ad1
height                 4211
prev_digest            sha256:0000000000000000000000000000000000000000000000000000000000000000
```

**Chain input** — shown with `\n` for the two line feeds:

```
4211\nsha256:0000000000000000000000000000000000000000000000000000000000000000\n2763f39ef8a61d493106d3db302ec36cae5c024ca3da3a019d483ccc29704ad1
```

```
chain.digest   sha256:919f8870391849de4e7b1d5b249ccbaaa7d5a7d3f500f5571c5a92dd0c3909db
```

**Signature input** — two line feeds:

```
handoff-receipt-v1\nrk_01K3MB2R4Y8ZC4YRXB2N6VD9FT\nsha256:919f8870391849de4e7b1d5b249ccbaaa7d5a7d3f500f5571c5a92dd0c3909db
```

```
signature (b64url) av8Iq2KkysJR6J3na_k6GHTS26ajN3CNsT4iOyHcJUy9mTxvF1hD0moPcg4kFGkklv1u2cGiijm76V2icmwZCw
signature (hex)    6aff08ab62a4cac251e89de76bf93a1874d2dba6a337708db13e223b21dc254c
                   bd993c6f175843d26a0f720e2414692496fd6ed9c1a28a39bbe95da2726c190b
```

(The hex value is one 128-character string, wrapped here for width.)

**Negative vectors.** A conforming verifier MUST reject each.

| Case | Expected |
|---|---|
| Any byte of the receipt core altered | `core_hash` changes → `chain.digest` changes → signature does not verify, and every later chain digest is invalidated |
| `height` changed from `4211` to `4210` | `chain.digest` changes → signature does not verify |
| `prev_digest` replaced with any other value | `chain.digest` changes → signature does not verify |
| Correct signature presented with `kid: "rk_unknown"` | **Reject.** A verifier MUST NOT try other published keys |
| Signature verified against `handoff-receipt-v2` as the prefix | **Reject.** The version prefix is inside the signed input |

**Reference verifier** (Python, `cryptography`):

```python
import json, hashlib, base64
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey

def b64u_decode(s: str) -> bytes:
    return base64.urlsafe_b64decode(s + "=" * (-len(s) % 4))

def verify_receipt(receipt: dict, signature: dict, keys: dict[str, bytes]) -> bool:
    chain = receipt["chain"]
    core  = {k: v for k, v in receipt.items() if k != "chain"}
    # canonical_json must be RFC 8785 (JCS); json.dumps with sorted keys and no
    # whitespace matches it for the value types this protocol uses.
    core_bytes = json.dumps(core, sort_keys=True, separators=(",", ":"),
                            ensure_ascii=False).encode("utf-8")
    core_hash  = hashlib.sha256(core_bytes).hexdigest()
    chain_input = f'{chain["height"]}\n{chain["prev_digest"]}\n{core_hash}'.encode()
    expected    = "sha256:" + hashlib.sha256(chain_input).hexdigest()
    if expected != chain["digest"]:
        return False                      # chain broken; do not even check the signature
    if signature["kid"] not in keys:
        return False                      # unknown key: reject, never fall back
    sig_input = f'handoff-receipt-v1\n{signature["kid"]}\n{chain["digest"]}'.encode()
    try:
        Ed25519PublicKey.from_public_bytes(keys[signature["kid"]]).verify(
            b64u_decode(signature["sig"]), sig_input)
        return True
    except Exception:
        return False
```

---

## 3. Canonical JSON

Every digest in this document is taken over **RFC 8785 (JSON Canonicalization Scheme)** output,
UTF-8 encoded: object members sorted by code point, no insignificant whitespace, and the RFC's number
serialization.

An implementation MUST use a JCS implementation, or MUST guarantee equivalent output for the value
types this protocol uses. The fixtures in `fixtures/signing/` are the authority: an implementation
that reproduces their byte lengths and hashes is canonicalizing correctly, and one that does not has
a bug regardless of what its own tests say.

Two traps worth naming, because both produce a digest that is stable in one implementation and wrong
across two:

1. **Re-serializing a received body before hashing.** Callback verification hashes the bytes on the
   wire, never a re-encoding of the parsed object.
2. **Non-integer numbers.** JCS specifies a number serialization; a naive `str(float)` does not match
   it. This protocol's canonicalized objects avoid non-integer numbers, and an implementation SHOULD
   keep it that way rather than relying on agreement about floating-point formatting.

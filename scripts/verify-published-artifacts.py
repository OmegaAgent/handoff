#!/usr/bin/env python3
"""Check the artifacts a stranger would rely on, using nothing this project ships.

Everything here is computed from the specification prose and the published files. It imports no
`handoff` module and calls no code from `core/`, because a verifier that shares code with the
producer proves only self-consistency — the defect that produced R-1, and then N-1 underneath it.

Three things are checked, in increasing order of what they would have caught:

1. The worked vectors in `spec/signing.md` §2.5 reproduce from the prose alone, including the
   negative vectors a conforming verifier must reject.
2. The "Reference verifier (Python)" snippet published in `spec/signing.md` is EXTRACTED FROM THE
   DOCUMENT AND EXECUTED against the published fixtures. Nothing in this repository used to run it,
   which is exactly how it came to ship an incorrect sort order under a comment asserting it was
   safe. A reference implementation nobody executes is documentation with a false badge.
3. The member ordering that snippet implements actually differs from the naive one, on the input
   where RFC 8785 and code-point ordering disagree. Without this, a correct-looking fix that changed
   nothing would pass checks 1 and 2 unchanged.

Run: python3 scripts/verify-published-artifacts.py     (exit 0 = every check passed)
"""

import base64
import hashlib
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
SPEC = ROOT / "spec"
failures: list[str] = []


def check(label: str, ok: bool, detail: str = "") -> None:
    print(f"  {'ok  ' if ok else 'FAIL'}  {label}{('  — ' + detail) if detail and not ok else ''}")
    if not ok:
        failures.append(label)


# ---------------------------------------------------------------- 1. the worked vectors
print("spec/signing.md §2.5 — the worked vectors, recomputed from the prose")

core = (SPEC / "fixtures/signing/receipt-core.json").read_bytes()
check("receipt core is 1125 bytes with no trailing newline", len(core) == 1125 and not core.endswith(b"\n"), str(len(core)))

core_hash = hashlib.sha256(core).hexdigest()
check("sha256(receipt core)", core_hash == "2763f39ef8a61d493106d3db302ec36cae5c024ca3da3a019d483ccc29704ad1", core_hash)

GENESIS = "sha256:" + "0" * 64
digest = "sha256:" + hashlib.sha256(f"4211\n{GENESIS}\n{core_hash}".encode()).hexdigest()
check("chain digest at height 4211 over the genesis", digest == "sha256:919f8870391849de4e7b1d5b249ccbaaa7d5a7d3f500f5571c5a92dd0c3909db", digest)

# derived, not trusted: the document publishes the derivation so anyone can regenerate the key
seed = hashlib.sha256(b"handoff-spec-v0.1-test-vector-key").digest()
check("signing seed derives from its published sentence", seed.hex() == "dbcb1a7a2012be306784fad7a454ac8fa398e42247df01153334576209b010c8")

try:
    from cryptography.hazmat.primitives import serialization
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

    sk = Ed25519PrivateKey.from_private_bytes(seed)
    pk = sk.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)
    check("public key", pk.hex() == "fb83e7234defb5402d3123ce1753df2e30313285cf194f4b7651bf5530646f98", pk.hex())
    sig = sk.sign(f"handoff-receipt-v1\nrk_01K3MB2R4Y8ZC4YRXB2N6VD9FT\n{digest}".encode())
    b64u = base64.urlsafe_b64encode(sig).rstrip(b"=").decode()
    check("signature over the versioned input", b64u == "av8Iq2KkysJR6J3na_k6GHTS26ajN3CNsT4iOyHcJUy9mTxvF1hD0moPcg4kFGkklv1u2cGiijm76V2icmwZCw", b64u)
except ImportError:
    print("  skip  signature checks — `cryptography` is not installed")

# The negative vectors. Each must change the digest; a verifier that accepts any of them is broken.
for label, height, prev, ch in [
    ("altering one byte of the core", 4211, GENESIS, hashlib.sha256(core[:-1] + b" ").hexdigest()),
    ("changing height 4211 to 4210", 4210, GENESIS, core_hash),
    ("replacing prev_digest", 4211, "sha256:" + "1" * 64, core_hash),
]:
    got = "sha256:" + hashlib.sha256(f"{height}\n{prev}\n{ch}".encode()).hexdigest()
    check(f"negative vector — {label} changes the digest", got != digest)

body = (SPEC / "fixtures/signing/callback-body.json").read_bytes()
check("callback body is 493 bytes", len(body) == 493, str(len(body)))
bh = hashlib.sha256(body).hexdigest()
check("sha256(callback body)", bh == "fbd6ec4cacc7cb9c9371d2791f946535e3d391a0594a92b5a3a27dd34f5e94fa", bh)
try:
    import hmac

    canon = f"1\n1785592064\ndlv_01K3MB2R6C8ZC4YRXB2N6VD9FT\n{bh}".encode()
    for name, secret, want in [
        ("A", "whsec_2f8a91c4e7b3d05a6c1e9f47b28d3a05", "cae13126f8dcd1e918376aa373be2757db7281a3e5aaed2d83d716537e03de80"),
        ("B", "whsec_9d41c07be5a2f36819b4d0e7c5a81f62", "d86b3740bad654e46c1349614523a476be0eb7d6a30a798b2d475374f36c57eb"),
    ]:
        got = hmac.new(secret.encode(), canon, hashlib.sha256).hexdigest()
        check(f"callback v1 signature under rotation secret {name}", got == want, got)
except Exception as exc:  # pragma: no cover
    check("callback signatures", False, str(exc))


# ------------------------------------------- 2. the PUBLISHED reference verifier, actually executed
print("\nspec/signing.md — the published reference verifier, extracted and run")

blocks = [b for b in re.findall(r"```python\n(.*?)```", (SPEC / "signing.md").read_text(), re.S) if "core_hash" in b]
if len(blocks) != 1:
    check("exactly one reference-verifier block is published", False, f"found {len(blocks)}")
else:
    ns: dict = {}
    try:
        exec(compile(blocks[0], "spec/signing.md#reference-verifier", "exec"), ns)
        check("the published snippet executes as written", True)
    except Exception as exc:
        check("the published snippet executes as written", False, f"{type(exc).__name__}: {exc}")

    canonical_json = ns.get("canonical_json")
    if canonical_json:
        for name in sorted(p.name for p in SPEC.glob("fixtures/*receipt*.json")):
            doc = json.loads((SPEC / "fixtures" / name).read_text())
            if "chain" not in doc:
                continue
            body_core = {k: v for k, v in doc.items() if k not in ("chain", "signature")}
            ch = hashlib.sha256(canonical_json(body_core)).hexdigest()
            want = "sha256:" + hashlib.sha256(f'{doc["chain"]["height"]}\n{doc["chain"]["prev_digest"]}\n{ch}'.encode()).hexdigest()
            check(f"{name} verifies AS PUBLISHED under the published verifier", want == doc["chain"]["digest"], want)

        # ------------------------------------- 3. and the ordering it implements is the load-bearing one
        print("\nRFC 8785 member ordering — the correction is not cosmetic")
        # U+1F600 is non-BMP, so its UTF-16 surrogate pair begins at 0xD800 and sorts BELOW U+FF01,
        # while by code point it sorts above. U+D7FF is the last BMP code point below the surrogates.
        probe = {"a": 0, "퟿": 3, "\U0001F600": 2, "！": 1}
        correct = canonical_json(probe)
        naive = json.dumps(probe, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
        check("UTF-16 ordering differs from code-point ordering on a non-BMP key", correct != naive)
        check("the emoji key sorts before the fullwidth exclamation", correct.decode().index("\U0001F600") < correct.decode().index("！"))

print()
if failures:
    print(f"{len(failures)} check(s) failed: " + ", ".join(failures))
    sys.exit(1)
print("every published artifact checks out against code that shares nothing with the implementation")

#!/usr/bin/env python3
"""Handoff quickstart: raise → a person answers → typed outcome → redeem.

Run it against a real server:

    HANDOFF_URL=https://handoff.example.com HANDOFF_API_KEY=sk_... python3 quickstart.py

Run it with no server at all, which is what happens by default:

    python3 quickstart.py

What the offline run proves and does not prove
----------------------------------------------
With no `HANDOFF_URL` set, this starts the SDK's **test double** — a small in-memory server that
implements the handful of operations this script calls and none of the guarantees that make a
server conformant. There is no tenancy, no authority evaluation, no receipt chain, no storage
immutability, no delivery ladder.

So the offline run proves the *client* half end to end: that a raise produces a durable wait,
that a long poll is satisfied by a person's answer, that the answer arrives as typed data, that
the ack is what consumes it, and that redemption is idempotent per effect key. It proves nothing
about any server. For that, run `conformance/` against a real one.
"""

from __future__ import annotations

import os
import sys
import threading
import time
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "sdk" / "python"))

import handoff  # noqa: E402
from handoff import Client, fields  # noqa: E402


def main() -> int:
    base_url = os.environ.get("HANDOFF_URL")
    server = None
    if not base_url:
        sys.path.insert(0, str(REPO / "sdk" / "python" / "tests"))
        from fake_server import FakeServer  # a test double, not a conforming server

        server = FakeServer()
        server.__enter__()
        base_url = server.base_url
        print(f"no HANDOFF_URL set — using the bundled test double at {base_url}\n")

    client = Client(base_url, os.environ.get("HANDOFF_API_KEY", "demo-key"))

    # 1. Declare what you need. Not what kind of thing this is — what the answer must look like,
    #    what the person must be handed, and who is entitled to give it.
    pending = client.raise_request(
        waiter_ref="run:quickstart-0198f2a1",
        prompt=handoff.prompt(
            "Refund $2,400 to Acme Corp?",
            "Invoice INV-8821 was double-charged on 2026-07-28.",
            evidence=[handoff.evidence.link("Invoice INV-8821", "https://billing.internal/inv/8821")],
        ),
        requires=handoff.requires(
            [
                fields.choice("decision", "Decision", [("approve", "Refund it"), ("reject", "Don't refund")]),
                fields.text("note", "Add a note", required=False, max_len=500),
            ],
            authority=handoff.authority("editor", "session", reason="the refund leaves our account"),
        ),
        ttl="PT4H",
        ttl_policy=handoff.ttl_policy("expire_and_deny"),
        mode="gated",  # do not perform the effect without redeeming first
    )

    print(f"raised   {pending.id}")
    print(f"created  {pending.was_created}   (201 means 'I asked'; 200 means 'I already asked')")
    print(f"surface  {pending.surface_url}")
    print(f"waiter   {pending.waiter_ref}   <- the only thing that has to survive a crash\n")

    if server is not None:
        _simulate_a_person_answering(base_url, pending.id)

    # 2. Wait. The wait is a durable row on the server, not this loop — killing this process
    #    here loses nothing, and `handoff.resume(waiter_ref)` picks it up from anywhere.
    print("waiting for a person...")
    with pending.receive(timeout=120) as received:
        outcome = received.outcome

        # 3. The answer is typed data. No prose to interpret, no regex, no model in the path.
        print(f"\noutcome  {outcome.outcome}")
        print(f"source   {outcome.source}   (human, policy, or runtime_inference — never guessed)")
        print(f"values   {outcome.values}")
        print(f"receipt  {outcome.receipt_id}")

        if not outcome.decided_by_human:
            print("\nnobody decided this — a policy did. Not treating it as consent.")
            return 0
        if outcome.value("decision") != "approve":
            print("\nrejected. Nothing to spend.")
            return 0

        # 4. One answer authorizes exactly one effect. Redeem immediately before doing it, and
        #    act only when first_redemption is true — that is what stops a retried turn from
        #    refunding twice.
        spend = outcome.redeem("stripe:refund:ch_1B")
        print(f"\nredeem   first_redemption={spend.first_redemption}")
        if spend.first_redemption:
            print("         -> performing the refund now")
        else:
            print("         -> already refunded on an earlier attempt; doing nothing")

        replay = outcome.redeem("stripe:refund:ch_1B")
        print(f"replay   first_redemption={replay.first_redemption}   (the same effect, refused twice)")

    # The ack was sent when that block completed, and not before. Had the block raised, the
    # signal would still be queued for the next process to reattach and find.
    print("\nacked. The signal is consumed exactly once.")

    if server is not None:
        server.__exit__(None, None, None)
    return 0


def _simulate_a_person_answering(base_url: str, request_id: str) -> None:
    """Stands in for a human opening the surface and clicking.

    In a real deployment this is a person authenticated in your tenant. It cannot be the agent:
    a service-account principal calling /answer gets 403 requester_may_not_answer, by principal
    type and under no configuration (§4.2).
    """

    def answer() -> None:
        time.sleep(1.0)
        print("(a person opens the surface and approves)")
        Client(base_url, "a-persons-session").answer(
            request_id, {"decision": "approve", "note": "Confirmed with Acme on the phone."}
        )

    threading.Thread(target=answer, daemon=True).start()


if __name__ == "__main__":
    raise SystemExit(main())

# Night Hack — the original Handoff build

**This is prior art, kept as a worked example. It is not the reference implementation, it is not
maintained, and nothing here should be deployed.** The reference implementation is the Rust
workspace under [`core/`](../../core), and the protocol it implements is defined in
[`spec/`](../../spec).

## What this is

Handoff was built solo at founders.inc Night Hack on 2026-07-24, in under four hours: a Python
FastAPI server, a one-file Python SDK, a public handoff page, phone paging through Retell AI, and a
demo of an agent hitting a wall it could not pass. It worked, against a deployed server, and
[`DISCLOSURE.md`](../../DISCLOSURE.md) records exactly what was built that night and what already
existed — including what was measured and what was not.

It is preserved for three reasons, in order of weight:

1. **It is the evidence.** The protocol's central claim — that a person's action can be recorded as
   an asserted fact and handed back to a blocked agent — was demonstrated here before it was
   specified. A specification with no prior demonstration is a guess.
2. **It shows the failure mode the spec exists to fix.** Before this build, "the human is finished"
   was *inferred* from the page URL or title changing. That silently misses a wall cleared in
   place — a verification checkbox is the clean example — and the agent then waits until its
   deadline for a wall that is already gone. The fix, and the reason there is a protocol at all, is
   that **a human action is recorded, never detected.**
3. **Deleting it would erase the honesty trail.** `DISCLOSURE.md` only means something if what it
   describes is still readable.

## What is in here

| Path | What it is |
|---|---|
| `app/` | The hackathon FastAPI server: create request, long-poll, resolve, and the public pages. |
| `demo/` | The demo agent and the demo wall — a self-controlled login and verification page whose payoff is withheld until a human clears it. |
| `Dockerfile`, `fly.toml`, `requirements.txt` | How it was deployed that night. |
| `CONTRACT-DEMO.md` | The demo's own contract: what the demo asserts and how it was checked. |
| `PAGING-UX.md` | The phone-paging call design. |
| `RUNBOOK.md` | Operating notes for the hackathon deployment. |
| `SUBMISSION.md` | The hackathon submission. |
| `assets/flow.svg` | The flow as it worked that night. Describes this build, not the spec. |

![The agent calls clear_wall and blocks; Handoff holds the request and pages a phone; a person answers and drives the agent's own browser through a live view; pressing "I cleared it" resolves the request, posts to the agent's resume endpoint, and the blocked call returns.](assets/flow.svg)

## What it is not

Read this part before borrowing any of it.

- **Not the protocol.** Its wire shape (`kind`, `reason`, `cleared`, a boolean for whether the live
  view is the agent's own browser) predates the spec and does not match it. Where they differ, the
  spec is right and this is history.
- **Not durable.** State did not survive a process restart. A durable wait — one that outlives a
  crash — is a property of the reference implementation, and it is the property the prior art most
  visibly lacked.
- **Not multi-tenant, and not safe as-is.** Possession of a request identifier was sufficient to act
  on it, and paging had a single global destination, so every request would have reached one person.
  Both shapes are defects the conformance suite is written to catch.
- **Not double-resolve safe.** Resolving an already-resolved request returned HTTP 200 with an
  `{"ok": false}` body rather than a conflict. Quiet is the wrong answer here; the protocol requires
  loud.
- **Not the claim language, either.** These documents predate the specification's rule about what may
  be said, and they break it. `RUNBOOK.md` closes on *"It did not restart. It carried on from exactly
  where it stopped"*, and `SUBMISSION.md` says the agent *"picks right back up"* and *"resumes"*.
  Spec Appendix B lists *"your agent resumes exactly where it stopped"* as **not defensible**, because this
  protocol does not resume execution: it delivers an answer, and what the runtime does next is the
  runtime's own business. Read those lines as a record of how the project once described itself, not
  as a description of what it does.
- **`DISCLOSURE.md`'s "ten of ten assertions against production" is unverifiable here.** The only
  assertion set preserved in this tree is an offline `selftest()` in `demo/agent.py` with **eight**
  asserts over a hardcoded sample. The production run may well have happened; the artifact was not
  kept, which is precisely the failure `DISCLOSURE.md` exists to prevent. Treat the number as
  unsupported rather than as evidence.
- **Not maintained.** It is out of scope for security reports — see
  [`SECURITY.md`](../../SECURITY.md).

## Licence

MIT, `Copyright (c) 2026 Noureddin Bakir` — see [`LICENSE-MIT`](../../LICENSE-MIT). That grant was
made when this code was published and is not altered by the Apache-2.0 default elsewhere in this
repository.

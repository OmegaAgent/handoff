# Disclosure

Handoff was built solo at founders.inc Night Hack on 2026-07-24, in under four hours. The
hackathon asks for an honest split between what existed before kickoff and what was built
during the event. This is that split, written to be checkable rather than flattering.

## Existed before kickoff

Omega, the author's own project, already carried internal browser-agent infrastructure, and
tonight's build stood on it:

- **Fly.io / Sprites microVM sandboxes** that rent a browser and expose a CDP endpoint.
- **An in-sprite live-view service** (a single aiohttp app on port 8080) that streams CDP
  screencast frames over a WebSocket and relays mouse and keyboard input back, so a viewer can
  drive the browser rather than only watch it.
- **HMAC viewer-token minting** for that service, plus the public sprite URL it is served on.
- **A sprite-level `request_human_help(reason)` action** for browser-use, along with clearance
  polling that inferred "the human is finished" from the page url or title changing.
- **A sprite endpoint `POST /resume`** that no code path had ever called.

Personal API accounts also predate the event: Retell AI, Fly.io, Cloudflare, and AWS Bedrock.

## Built tonight

The whole standalone product, from nothing:

- **The `human` SDK** (one file): `ask()`, `clear_wall()`, `create_request()`, and the blocking
  long-poll loop that makes a human's decision look like a normal function return.
- **The hosted API**: create request, long-poll for resolution, resolve, and the routes that
  serve the public pages.
- **The public handoff page**: the agent's reason and context, the embedded live view, the typed
  answer field, and the resolve control.
- **Phone paging through Retell AI**: write the agent's reason into the call's opening message,
  then place the call to a real phone.
- **The demo wall** (a self-controlled login and CAPTCHA page) and the demo agent that drives it.
  Two modes: `--scripted`, which is the mode measured end to end tonight, and `--claude`, which
  runs the agent on Claude via AWS Bedrock but is fed stripped page text, so it does not
  demonstrate the gate below.
- **`GET /demo/statement?handoff=<id>`**, the gated payoff: it returns the demo's rebate statement
  only when a human resolved that exact handoff id, and 403 otherwise. It exists because serving
  the wall's HTML also serves the numbers inside it, so an agent could regex the total out of the
  page and never need a person. The agent holds the handoff id and has no way to resolve it itself,
  which is what makes the demo prove something.
- **`GET /try`**, a self-serve route that mints a demo handoff with paging off and redirects to its
  page, so anyone can see what a paged human sees without ringing a real phone.
- **The "I cleared it" to `POST /resume` path**, described below.

## The gap that closed tonight

Before tonight, clearance was only ever *inferred*: the sprite watched for the page url or title
to move and treated that as the human being done. That silently misses walls cleared in place. A
Cloudflare Turnstile checkbox is the clean example. The human ticks the box, the url and the
title do not change, and the agent waits until its deadline. `POST /resume` existed but had no
caller anywhere.

Tonight's handoff page gives the human an explicit "I cleared it" button, which calls `/resolve`,
which POSTs the agent browser's `resume_url`. Clearance stopped being a guess and became a
stated fact. That is the human-facing half of the product, and it is new.

Measured against the deployed server: the blocked long-poll returned 3 seconds after a human
resolved, inside a 25-second wait window, so the resume tracked the click rather than a polling
tick. `POST /resume` reached the browser sandbox with its bearer token. A real phone call fired from
the deployed server through Retell AI. In `--scripted` mode the demo agent ran the full loop and read
the payoff only after clearance, passing its scripted assertions against production **(number unverifiable — see below)**: the 403 while
pending names the state as `pending` rather than an unknown handoff, the agent process was confirmed
still blocked while it waited, and the wall was checked as production serves it over HTTPS, where a
scripted `.click()` is rejected and only a trusted CDP click reveals the payoff.

Source: https://github.com/OmegaAgent/handoff

---

## Corrections to this document

Kept rather than rewritten, because a disclosure that quietly edits itself is worth less than one
that shows what it got wrong.

- **"ten of ten assertions against production" is unsupported.** The only assertion set preserved
  in this repository is an offline `selftest()` in `examples/night-hack/demo/agent.py` with
  **eight** asserts over a hardcoded sample. The production run may well have happened; the
  artifact was not kept — which is precisely the failure this document exists to prevent. Treat
  the number as unsupported.
- **The live deployment is deliberately not linked here.** It had no authentication, and its own
  runbook recorded that anyone holding the URL could ring a real phone. See `SECURITY.md`.

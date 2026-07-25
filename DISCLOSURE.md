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
- **The demo wall** (a self-controlled login and CAPTCHA page) and the demo agent that drives it
  with Claude on AWS Bedrock.
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

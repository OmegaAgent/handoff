# BACKLOG — Handoff (Night Hack 2026-07-24)

Ranked. Top = handle first. Non-blockers get deferred here instead of stopping the build.

## Shipped tonight (verified in production, not just written)
- Hosted API: create request / long-poll / resolve. The long-poll returns the instant a human
  resolves (measured 3s against a 25s wait), so a blocked agent resumes immediately.
- Python SDK, one file, standard library only: `ask()`, `clear_wall()`, `create_request()`,
  `Handoff.wait()`. Timeout raises `HandoffTimeout`; `default=` swallows it.
- Public handoff page: reason, live-view iframe, "I cleared it" / answer box, and it refreshes
  itself when the request settles from somewhere else.
- **"I cleared it" → `POST /resume` on the agent's sandbox** — the gap this project set out to
  close. Verified: the resume lands with its bearer token and the agent's blocked poll returns.
- Phone paging via Retell, fired from the deployed server (`paged: "ringing"`). Best-effort by
  design: a paging failure never blocks request creation.
- Demo wall: a self-controlled portal whose verification step requires a genuinely trusted click,
  with the payoff held in a `<template>` so it is absent from the DOM until a human clears it.
- Public: https://handoff.omegas.dev · https://github.com/NoureddinBakir/handoff (MIT).

## Blockers / early (next session)
1. **Real sprite live view.** The iframe points at our own demo wall as a placeholder. H2
   (embedding omega's in-sprite CDP screencast from a foreign origin) is near-certain on paper but
   was not empirically confirmed tonight. Residual risk is `X-Frame-Options` / `frame-ancestors`
   on the sprite's `/live`.
2. **`demo/agent.py` browser-use mode is unverified.** `--scripted` is the fallback that must
   always work. Related unsolved point: an agent with no browser cannot observe a client-side-only
   reveal, so "a human cleared it" needs a server-side notion of clearance.
3. **State is in one process.** A redeploy drops pending requests. Fine for a hack, wrong for
   anything real — needs durable storage before anyone else can rely on it.
4. Nothing outstanding on hosting. `handoff.omegas.dev` is live on a Let's Encrypt cert, A and
   AAAA records pointing at the Fly app, DNS-only. Note for later: the old
   `CLOUDFLARE_DNS_API_TOKEN` is EXPIRED and `hipocampus/.env`'s `CLOUDFLARE_API_TOKEN` cannot see
   the zone — the working token is the one in `omega/.env.live` (named RED_LINE in Cloudflare).

## Deferred (non-blocking)
- **Paging-UX escalation ladder** — owner-approved direction, full spec in `PAGING-UX.md` (quiet
  hours hard rail, text-first consent "call now or tomorrow?", morning-digest batching, urgency
  levels, workflow tie-in). Post-demo v1 feature; mention in the submission's "potential" story.
- Auth / API keys for the hosted API. Today: unguessable request ids only, no tenancy.
- Rate limiting and abuse controls on paging — anyone holding the API URL can currently ring a
  phone. This is the first thing to fix before the link is shared widely.
- Voice answers back: capture speech on the call and return it as the `ask()` result.
- Multiple humans and an escalation ladder: ring the second person if the first does not answer.
- TypeScript SDK twin of the Python SDK.
- Framework adapters: LangGraph interrupt, a browser-use action, a Claude Agent SDK tool.
- Convex port for sponsor points.
- Landing page and logo beyond the current server-rendered page.

## Learnings log
- 2026-07-24 resource pass: ElevenLabs free tier has no phone numbers or agents, so outbound
  calling was impossible there. Railway's token lives only in omega CI, not locally.
- 2026-07-24 ~22:20: **Twilio is a dead end, do not revisit** — trial policy walls block Calls,
  OutgoingCallerIds and AvailablePhoneNumbers via the REST API regardless of key type, and a
  console-verified caller ID never surfaces to the API. Retell AI did the same job in minutes.
- 2026-07-24: paging must be fire-and-forget. An agent that blocks on a phone API is an agent that
  hangs when the phone API is down.
- 2026-07-24: long-poll plus one `asyncio.Event` per request is the entire blocking primitive. No
  queue, no broker, no websocket on the agent side.
- 2026-07-24: the expired Cloudflare DNS token cost us the vanity domain. Verify a credential by
  calling the API that will actually use it, not by finding it in a `.env` file.

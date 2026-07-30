# BACKLOG — Handoff

Ranked. Top = handle first. Non-blockers get deferred here instead of stopping the build.

## Stated gaps — things that do not exist, said plainly

An empty directory named after a package is worse than an absent one, because it implies a
deliverable. These are the things a reader might reasonably expect to find and will not.

- **`@handoffproto/types` is hand-written, not generated.** *(Corrected: an earlier revision of this
  file said the package did not exist. It does — that entry was written from a stale look at the
  tree and was wrong.)* `sdk/types/index.d.ts` covers all 87 schemas in `spec/openapi.yaml`, and
  `scripts/check-drift.mjs` enumerates the schema names from the spec and fails if a declaration is
  missing, with no YAML parser and no dependencies. That is a real guard, but it checks **presence,
  not shape**: a schema whose fields change while its name stays put will pass. Wiring a genuine
  generator, or extending the drift check to compare members, is still open.
- **No case exists for a Level 2 deployment that declines the continuation extension.** C-17 covers
  the positive path only.
- ~~The conformance CI job is still `continue-on-error`.~~ **Done.** Removed when the first cases
  landed. Until the reference server was green the job asserted the inverse property — that the
  suite reports red against a stub implementing nothing — and it fails the build if the suite ever
  reports conformance against that stub.

## Open now — H0, publish the contract

Landed in H0:
- Apache-2.0 repository default with MIT retained on `sdk/**` and on the preserved prior art.
  `NOTICE`, `TRADEMARKS.md`, `CONTRIBUTING.md` (DCO, no CLA), `GOVERNANCE.md`, `SECURITY.md`.
- The §2.2 tree: `core/` Rust workspace (six crates, builds clean), `sdk/python`,
  `examples/night-hack/` holding the hackathon build as prior art.
- `handoff-human` 0.2.0: module renamed `human` → `handoff`, with `human` kept as a deprecated
  re-export that warns. Removed in 0.3.0.
- CI with no secrets: fmt, clippy, build, test, docs; Python SDK lint and import surface; a wired
  conformance job; a spec-change-needs-a-conformance-case rule; secret hygiene.
- Repo identity standardized on `OmegaAgent/handoff`.

Still open before the first public push, in order:
1. **Reserve the names.** `handoff-protocol`, `handoff-core`, `handoff-store-postgres`,
   `handoff-adapters`, `handoff-server`, `handoff-conformance` on crates.io, and the npm scope for
   the TypeScript SDK. Fallbacks exist (`handoffproto-*`, `@handoff-protocol/*`) but the choice must
   be made before the spec is published, not after.
2. **`spec/` v0.1.** Concurrent work. The release gate for H0 is not met until it exists.
3. **Confirm the copyright holder going forward.** The published MIT grant says
   `Noureddin Bakir`; the company packages say `Omegas`. Pick the entity that will hold the
   trademark and use it consistently from here. Do not rewrite the published grant.
4. **Decide whether the conformance suite gates a managed deploy from day one or from v1.0.**
   Day one is stronger and slower. A gate added later is a gate that never gets added.

## H5 — the managed adapter and the cutover

Landed in `managed/` (closed, `UNLICENSED`, `publish = false`) and `docs/cutover-plan.md`. Eight seam
ports implemented against the Ωmegas control plane over HTTP; 76 tests; the outbox and reconciler are
tested against a real Postgres because durability is not a property a fake can demonstrate.

**Blocking, and needing an owner decision rather than a schedule:**

1. **Machine auth as specified cannot serve an out-of-repo Handoff.** It is an Axum extractor doing
   an indexed fetch against `api_keys` inside the omega repo, and `handoff.omegas.dev` has neither.
   `managed/…/auth.rs` implements the recommended client-credentials exchange, but until an owner
   rules, **Handoff's public API has no mechanism** — not merely no date. Everything else in the
   managed tier is downstream of this.
2. **Attestation is unbuilt in both tiers.** No key, no custody decision, no verification endpoint,
   no owner. `signer.rs` refuses rather than signing with something convenient. **Do not market
   independent attestation before it exists** — it is the strongest item on the upgrade list and the
   one we currently cannot deliver.
3. **The revocable viewer token is unbuilt**, so browser takeover cannot be offered. `takeover.rs`
   refuses and deliberately does **not** fall back to today's broadcast URL (defect B-2).

**Ordered follow-ups:**

4. **`Store::chain` is the wrong read for a reconciler.** It returns a tenant's whole chain, because
   it exists to serve the open verifier. Reconciling from it is O(chain) per pass rather than O(new),
   and it will not stay acceptable on a large tenant. The fix is a cursored read on the open store —
   an upstream port change, in the open repo.
5. **Tenant discovery has no home.** `Reconciler::run_for` takes one tenant; nothing enumerates them.
   Querying the open store's tables from the managed crate would couple the adapter to the core's
   schema, which is the coupling the whole arrangement exists to avoid. `Store::tenants()` upstream,
   or the deployment drives it from the set it already knows.
6. **The managed delivery fleet has no transports.** `delivery.rs` declares what the fleet should be
   and refuses the two shapes that must never ship; the reviewed Slack app, the warmed SES identity,
   and the numbers are operational assets, not code.
6a. **`advance_delivery_grade` has no HTTP route.** It is a store primitive, so later evidence — a
   provider's delivery receipt, a person opening the surface — can only be recorded in-process. A
   managed deployment drives it from its own webhook ingress and therefore needs a route, or every
   delivery stays at `dispatched` forever and the grade ladder is decorative. Ask H4 for the route.
7. **Wiring attestation needs an open port.** The receipt is sealed inside the answer transaction and
   nothing in `Store` takes a signer. It must be an upstream change, or the hosted tier produces
   receipts the open verifier cannot check — which is a vendor claim rather than evidence.
8. **`usage_events.idempotency_key` is globally unique, not per-org (B-10).** The adapter mints
   org-scoped keys and refuses any that are not, but a server assuming global uniqueness still
   collides across products. Fix on the ingest endpoint.
9. **`events` is not append-only at the database**, and two live `UPDATE events SET instance_id`
   statements must go before the audit mirror can be trusted. The API also drops `payload`, which is
   exactly the field a receipt summary lives in.

**A defect in our own tooling, found while verifying H5:**

10. ~~`core/dev/run-conformance.sh` is unsafe to run concurrently.~~ **Fixed (`baba8fe`).** Every
    resource the script owns now derives from one per-run token, and both ports are obtained by
    asking the kernel for a free one rather than derived arithmetically — an offset only makes a
    collision rarer. Proof: two suites started concurrently with **no environment variables at
    all**, both 24/24 exit 0, from the identical invocation that had produced 17/24, 19/24, 22/24
    and 23/24.

    Worth keeping the diagnosis, because the failure shape recurs: the harness had also been
    *measuring a server it did not start*. An orphan held the port, the newly launched server
    failed to bind and exited, and the readiness probe checked `curl` before checking whether its
    own process was alive — so a stranger's 200 read as a healthy start, and the suite then
    measured an older build against a stale database. **Not one of those failures was an assertion
    about the protocol.** A measurement tool that corrupts a peer does not look broken; it looks
    like a finding.

## Prior art — the hackathon build (Night Hack, 2026-07-24)

Preserved under `examples/night-hack/`. What follows is that build's own backlog, kept because the
direction section below is still the direction.

### Shipped that night (verified in production, not just written)
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
- Public: https://handoff.omegas.dev · https://github.com/OmegaAgent/handoff (MIT).

## Direction (owner, 2026-07-25): the multi-channel communication layer, person-centric

Handoff is not phone paging for stuck agents. It is **the communication layer between AI agents
and the humans they depend on** — an open-source framework any agent harness plugs into so it
stops owning *how* a person gets reached and stops owning the long-lived wait.

**Multi-channel is the identity, not a feature.** A phone call, an SMS, a Slack message, an email,
a calendar invite: one abstraction, so an agent never hardcodes a channel. Voice with live browser
takeover is simply the highest-bandwidth channel and the one that is built today; the rest is
direction, and nothing in the docs or the UI may imply otherwise.

Agents run 24/7, humans do not, and humans are still the accountability layer. That mismatch is
the product.

Four consequences, all decided:

1. **The unit is a person's attention queue, not a request.** Requests become items in that queue.
   One human contact can then resolve many blocked runs across many sessions, instead of the agent
   paging once per blocker. This is the reframe everything else depends on.
2. **Channels are pluggable and capability-first, never per-vendor.** No `match` over
   slack/email/sms/voice. A channel declares capabilities — can it carry rich actions, capture free
   text, interrupt someone, survive being ignored, what latency class — and the framework routes on
   requirements. Adding a provider must never add a branch in the core. Open question worth
   settling: drive an existing action registry (Pipedream Connect enumerates 3,000+ apps
   dynamically) rather than hand-writing integrations, and keep first-class ownership only of the
   genuinely special channels, voice and calendar.
3. **The agent negotiates modality and timing, with an LLM.** Not "send a message" but: how urgent
   is this, is this person awake, do I have their calendar, do I interrupt or batch, do I text
   first and ask "call now or tomorrow?". Needs a person model (channels, timezone, quiet hours,
   calendar access, learned preferences) reasoned over, not a config file. `PAGING-UX.md` is the
   seed of this.
4. **Durable state stops being a nice-to-have.** Handoffs outliving the agent process is the
   premise, not a limitation to fix later.

What survives from the hack build: the blocking primitive, the request object, the resolve-and-
resume wiring, and the live browser takeover — which stays the differentiator because it is the
only channel where the human can *act* rather than reply. Voice + takeover is simply the
highest-bandwidth channel, and it is the one that is built.

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

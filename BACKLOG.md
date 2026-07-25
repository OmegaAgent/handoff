# BACKLOG — human (Night Hack 2026-07-24)

Ranked. Top = handle first/early. Non-blockers get deferred here instead of stopping the build.

## Blockers / early
1. ~~Twilio setup~~ → **H1 DONE via Retell AI** (live call proven 2026-07-24 ~22:20; IDs + key in `~/human/.env`; Twilio path dead — see EXECPLAN H1, don't revisit). Paging recipe: update retell-llm begin_message → create-phone-call.
2. Wire the unbuilt "I cleared it" → POST /resume path into the public request page (recon found nothing calls /resume today; url/title-movement detection misses in-place walls like Turnstile). This is the headline "built tonight" item.
3. H7 spike: browser-use on Bedrock bearer (no Anthropic credits available).

## Deferred (non-blocking)
- Convex port for sponsor points (only if core lands early).
- H5 voice-answer via Twilio Gather.
- Landing page / logo.
- TypeScript SDK twin of the Python SDK.
- Auth/API keys for the hosted API (hack demo can run open with unguessable request IDs).
- Rate limiting, persistence beyond process memory.

## Learnings log
- 2026-07-24 resource pass: ElevenLabs = free tier, no phone numbers/agents → outbound calling impossible as-is; Twilio trial is the path. Railway token not locally available (lives in omega CI). Fly + Cloudflare tokens in hand.

# Demo runbook — Handoff

Live: **https://handoff.example.invalid** · Repo: **https://github.com/OmegaAgent/handoff**

Two minutes, one uninterrupted take. The whole story is: the agent stops, a phone rings, a person
takes the wheel, the agent finishes. Do not explain the architecture on stage — show the phone.

## Before you record

```bash
curl -s https://handoff.example.invalid/healthz     # expect {"ok":true,...}

# Rehearse silently first. --no-page means your phone stays quiet.
cd ~/human && HANDOFF_URL=https://handoff.example.invalid python3 demo/agent.py --scripted --no-page
```

**Freeze deploys before you go on stage.** Request state lives in the server process, so a deploy
or machine restart mid-run orphans the agent's pending handoff and the run hangs until it times
out. The app is pinned to exactly one machine for this reason: with two, a request created on one
is unknown to the other. Do not run `flyctl deploy` or `flyctl scale` until the demo is over.

Have ready, in this order, on one screen if possible:
1. A terminal in `~/human` (this is the agent's voice — its blocked print lands here).
2. Your phone, visible in frame, volume ON.
3. A browser tab, empty. The handoff page opens here.

## The take

**Beat 1 — the agent starts and hits a wall (~20s).**

```bash
# Recommended: drives a real browser the human can take over.
export SPRITES_API_TOKEN=...   # redacted: never name the file that holds a working key
cd ~/human && python3 demo/agent_sprite.py --page          # --page rings the phone

# Simpler fallback, no sandbox, but the live view is not its browser:
HANDOFF_URL=https://handoff.example.invalid python3 demo/agent.py --scripted
```

`agent_sprite.py` takes roughly 25 to 40 seconds to reach the wall because every browser step goes
through the sandbox exec API. It narrates each step, so it reads as an agent working rather than a
hang. Do not fill the silence by talking over it; let the narration carry the beat. Note that
`--no-page` is the DEFAULT for this one, so you must pass `--page` for the phone to ring on stage.

The terminal prints the wall it hit, the handoff URL, and a line showing the statement endpoint
answering **403 while the handoff is still pending** — the payoff is locked and the agent cannot
unlock it. Then it goes quiet. Say out loud: *"It is blocked. It cannot solve this one, and it has
not given up either. It is waiting."*

Use `--scripted`. There is also a `--claude` mode (Claude via Bedrock), but do not present it as
proof of the gate: it is fed the stripped page text, which exposes the template contents, so it can
read the total without help. `--scripted` is the mode that is honestly gated.

**Beat 2 — the phone rings (~25s).** Let it ring on camera before you answer. The voice states the
agent's own reason for stopping. This is the beat nobody else has. Do not talk over it.

**Beat 3 — the human takes the wheel (~40s).** Open the handoff page. Click the verification
checkbox in the live view. Say: *"The agent could not tick this box. I can."*

Be careful what you claim here, because it depends on which agent you ran:

- **`demo/agent_sprite.py`** (recommended for the take) drives a real Chrome inside a sandbox, and
  the live view is literally that browser. Verified end to end: a relayed click landed and the next
  screencast frame showed the wall going to "Verifying". Here you can say *"that is the agent's own
  browser, not a screenshot"* with no qualification. It is the strongest claim in the demo.
- **`demo/agent.py --scripted`** drives its own HTTP session, so the live view is the wall, not that
  agent's browser. Say *"this is the wall it is stuck on"*. Do NOT say "this is its browser".

Either way, what unlocks the payoff is your click plus your press of the button. That part is real
in both modes: the statement endpoint stays 403 until a person resolves that specific handoff.

### If a judge asks "couldn't the agent just click the box itself?"

Answer honestly, because the true answer is more interesting than a dodge: **yes, a synthetic
trusted event could tick this particular box.** The wall gates on `event.isTrusted`, and a CDP-
injected click is trusted. The demo agent deliberately does not do that — it clicks through
`Runtime.evaluate`, which is untrusted, and takes the rejection rather than cheating its own demo.
The real-world case this stands in for is a CAPTCHA or an SMS code that no synthetic event
satisfies. Do not claim the sandbox makes trusted clicks impossible; it does not, and a judge who
knows CDP will catch it.

**Beat 4 — hand it back (~15s).** Press **I cleared it**. Cut to the terminal: the blocked call has
returned and the agent finishes and prints the deliverable. Close on: *"It did not restart. It
carried on from exactly where it stopped."*

## If something breaks mid-take

- **Phone does not ring** — keep going, the handoff link still works; paging is best-effort by
  design and the page says so. Mention it rings and move on. Do not stop the take.
- **Live view is blank or missing** — the page falls back to showing the reason without a live view.
  Clear the wall in a normal tab instead, then press *I cleared it*. The point survives.
- **Agent errored out** — resolve any pending request by hand and rerun. State fresh in ~15s:
  ```bash
  curl -s https://handoff.example.invalid/v1/requests | python3 -m json.tool | head -30
  ```
- **Everything is on fire** — demo the page by itself. Create a request, let it ring, clear it. That
  alone shows the product.

## Making a request by hand

Paging (redacted — see the note below):
```bash
curl -s -X POST https://handoff.example.invalid/v1/requests \
  -H 'content-type: application/json' \
  -d '{"kind":"clear_wall",
       "reason":"A human-verification checkbox is blocking the Northwind partner portal",
       "agent":"demo-agent",
       "live_view_url":"https://handoff.example.invalid/demo/wall",
       "timeout_s":900,"page":false}'
```

**Redacted for publication.** The host is a placeholder and `page` is `false`. The original text carried a working request against a live, unauthenticated deployment that rang a real phone — publishing that is a denial-of-service handed to the first reader. The shape is kept because it is the historical record; the payload is not.

Add `"page":false` to rehearse silently. The response carries `page_url` — open it.

Ask a question instead of a wall:
```bash
curl -s -X POST https://handoff.example.invalid/v1/requests \
  -H 'content-type: application/json' \
  -d '{"kind":"question","question":"Which shipping address should I use?",
       "agent":"demo-agent","timeout_s":900,"page":false}'
```

## Letting a judge try it themselves

Send them to **https://handoff.example.invalid/try**. It mints a handoff and drops them on the exact page
a paged human sees, with the live view and the resolve button. Paging is off on that path on
purpose: a public button that rings a phone is a public button for waking someone up. Say that out
loud if a judge asks why their click did not ring anything.

## Reset between takes

Nothing to reset server-side; each request is independent and ids are unguessable. The demo wall
resets from its own restart link. Requests expire on their own `timeout_s`.

## What to say if a judge asks what is real

All of it is running in production, and be straight about the seams: state lives in one process, so
a redeploy drops pending requests; paging is best-effort and never blocks the agent; there is no
auth yet beyond unguessable request ids. `DISCLOSURE.md` lists what existed before tonight (Omega's
sandbox and live-view internals) versus what was built tonight (the SDK, the API, the handoff page,
the paging, and the resume path that nothing had ever called).

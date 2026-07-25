# Demo runbook — Handoff

Live: **https://handoff.omegas.dev** · Repo: **https://github.com/NoureddinBakir/handoff**

Two minutes, one uninterrupted take. The whole story is: the agent stops, a phone rings, a person
takes the wheel, the agent finishes. Do not explain the architecture on stage — show the phone.

## Before you record

```bash
curl -s https://handoff.omegas.dev/healthz     # expect {"ok":true,...}
```

Have ready, in this order, on one screen if possible:
1. A terminal in `~/human` (this is the agent's voice — its blocked print lands here).
2. Your phone, visible in frame, volume ON.
3. A browser tab, empty. The handoff page opens here.

## The take

**Beat 1 — the agent starts and hits a wall (~20s).**

```bash
cd ~/human
HANDOFF_URL=https://handoff.omegas.dev python3 demo/agent.py --scripted
```

The terminal prints the wall it hit and the handoff URL, then goes quiet. Say out loud: *"It is
blocked. It cannot solve this one, and it has not given up either. It is waiting."*

**Beat 2 — the phone rings (~25s).** Let it ring on camera before you answer. The voice states the
agent's own reason for stopping. This is the beat nobody else has. Do not talk over it.

**Beat 3 — the human takes the wheel (~40s).** Open the handoff page. Point at the live view: *"That
is the agent's actual browser, not a screenshot."* Click the verification checkbox in the live view.
Say: *"The agent could not tick this box. I can."*

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
  curl -s https://handoff.omegas.dev/v1/requests | python3 -m json.tool | head -30
  ```
- **Everything is on fire** — demo the page by itself. Create a request, let it ring, clear it. That
  alone shows the product.

## Making a request by hand

Paging ON (rings the phone):
```bash
curl -s -X POST https://handoff.omegas.dev/v1/requests \
  -H 'content-type: application/json' \
  -d '{"kind":"clear_wall",
       "reason":"A human-verification checkbox is blocking the Northwind partner portal",
       "agent":"demo-agent",
       "live_view_url":"https://handoff.omegas.dev/demo/wall",
       "timeout_s":900,"page":true}'
```

Add `"page":false` to rehearse silently. The response carries `page_url` — open it.

Ask a question instead of a wall:
```bash
curl -s -X POST https://handoff.omegas.dev/v1/requests \
  -H 'content-type: application/json' \
  -d '{"kind":"question","question":"Which shipping address should I use?",
       "agent":"demo-agent","timeout_s":900,"page":true}'
```

## Reset between takes

Nothing to reset server-side; each request is independent and ids are unguessable. The demo wall
resets from its own restart link. Requests expire on their own `timeout_s`.

## What to say if a judge asks what is real

All of it is running in production, and be straight about the seams: state lives in one process, so
a redeploy drops pending requests; paging is best-effort and never blocks the agent; there is no
auth yet beyond unguessable request ids. `DISCLOSURE.md` lists what existed before tonight (Omega's
sandbox and live-view internals) versus what was built tonight (the SDK, the API, the handoff page,
the paging, and the resume path that nothing had ever called).

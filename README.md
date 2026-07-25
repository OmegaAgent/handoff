# Handoff

**An `await human()` API for AI agents.** When an agent hits a wall it cannot pass, a real
human's phone rings, the human takes the wheel inside the agent's live browser, clears the
wall, and the agent's blocked call returns so it finishes the job.

MIT licensed (see `LICENSE`). Public page: **https://handoff.omegas.dev** · Repo: `handoff`

## The problem

Agents fail at walls: a CAPTCHA, a 2FA code, a login form, a judgment call the agent has no
standing to make. The common answer is to retry harder, or to give up and write a line in a
log, which kills the run and throws away everything the agent already did. The honest answer
is to ask a human, and keep the run alive while they help.

## Quickstart (30 seconds)

The SDK is one file. Install it, or just copy it in:

```bash
pip install -e .
# or:
cp -r human/ /path/to/your/project/
```

```python
import human

human.configure(base_url="https://handoff.omegas.dev")   # or set HANDOFF_URL

# Ask a question. Blocks until a human answers; returns their text.
address = human.ask("Which shipping address should I use?", timeout_s=600)

# Hand off a wall. Blocks until a human clears it; returns True if they did.
cleared = human.clear_wall(
    reason="Cloudflare Turnstile checkbox is blocking checkout",
    live_view_url="https://<sprite>.sprites.app/live?token=<hex>",
    resume_url="https://<sprite>.sprites.app/resume",
    resume_token="<hex>",
    timeout_s=600,
)
```

Both calls block by repeatedly long-polling the API. `live_view_url` and `resume_url` are
optional: pass them when your agent drives a browser you want the human to touch, and the
handoff page will embed that live view. Lower level, if you want to do your own waiting:

```python
req = human.create_request(reason="...", kind="clear_wall")
req.wait(timeout_s=600)
```

## HTTP API

| Endpoint | Purpose |
|---|---|
| `GET /healthz` | Liveness. Returns `{"ok": true}`. |
| `POST /v1/requests` | Create a handoff request. `kind` is `clear_wall` or `question`; body carries `reason`, `question`, `agent`, optional `live_view_url` / `resume_url` / `resume_token`, `timeout_s`, and `page` (set `false` to skip phone paging). Returns `201` with the id and its public `page_url`. |
| `GET /v1/requests/{id}?wait=25` | Long-poll. Returns as soon as status leaves `pending`, or after `wait` seconds. Carries `status`, `answer`, `cleared`, and timestamps. |
| `POST /v1/requests/{id}/resolve` | Resolve one request: `{"answer", "cleared", "by"}`. Side effect: if the request carried a `resume_url`, the server POSTs it with `resume_token` as a bearer. |
| `GET /r/{id}` | The public handoff page for one request. No auth; the id is a 22-character unguessable string. |
| `GET /` | Landing and status page. |

Request state lives in the server process, in memory, on a single machine. That is a
deliberate choice for a four-hour build, and it is the honest limitation: restart the server
and pending requests are gone. Durable state is first on the roadmap.

## How it works

1. The agent hits a wall and calls `human.clear_wall(...)` or `human.ask(...)`. The call blocks.
2. The SDK POSTs `/v1/requests` with the agent's reason, plus the live-view and resume URLs for
   the browser it is driving.
3. The API pages a human by phone through Retell AI. A voice reads the agent's reason out loud
   and tells the human to open the handoff link.
4. The human opens `/r/{id}` and sees the reason, the context, and a live view of the agent's
   actual browser: CDP screencast frames over a WebSocket, with mouse and keyboard relayed
   back, so they can drive it rather than only watch.
5. The human clears the wall or types an answer, then presses "I cleared it". That hits
   `/resolve`, which POSTs the agent browser's `resume_url`.
6. The agent's long-poll returns with `cleared=True` or the typed answer, and the run continues
   from where it stopped.

```
 agent + browser      handoff API       phone        human
        |                   |             |            |
        |-- clear_wall() -->|             |            |
        |     (blocks)      |-- rings --->|            |
        |                   |             |-- opens -->|
        |                   |<------ GET /r/<id> ------|
        |                   |             |            |
        |<==== live view: frames out, clicks in =======|
        |                   |<-- "I cleared it" -------|
        |<-- POST /resume --|             |            |
        |-- poll returns ->>|             |            |
        |  run continues    |             |            |
```

The live view is a direct connection between the human's page and the agent's browser sandbox.
The API's job is to hold the request, ring the phone, and carry the resolution back.

## How judges can test it

Open **https://handoff.omegas.dev** and start a demo request. You get the exact page a paged
human gets: the agent's stated reason, the live view of its browser, and the resolve control.
Clearing the wall there is what unblocks the agent.

We deliberately do not publish a `curl` that rings the phone. That would mean putting a working
key in the wild, and the number on the other end belongs to a person. The paging leg is shown
in the live demo and in the backup video.

## Sponsor tools used

- **Anthropic Claude** as the demo agent's brain, called through **AWS Bedrock** (we had no
  direct Anthropic API credits, so Bedrock carried the model).
- **Retell AI** places the phone call and speaks the agent's reason.
- **Fly.io** hosts the API and the handoff page.
- **Cloudflare** for DNS on `handoff.omegas.dev`.

## Prior art, and what is different

humanlayer.dev, gotoHuman, and LangGraph interrupts all have the same shape: a text approval in
chat. Handoff does the two things they do not. **Physical paging**, so the human is reachable
away from a keyboard, and **live browser takeover**, so the human acts inside the agent's own
session and hands it back. inkbox.ai (YC S26) runs the opposite direction, giving the agent its
own identity and comms; complementary, not a competitor.

## Roadmap

- TypeScript SDK with the same two calls.
- Durable state, so a restart cannot drop a pending request.
- API keys on request creation, and signed handoff-page links.
- Voice answers: the human speaks the answer on the call, speech-to-text returns it to the agent.
- Framework adapters for LangGraph, browser-use, and the Claude Agent SDK.
- Escalation policy: re-ring, fall back to a second contact, then SMS or Slack.

## Disclosure

`DISCLOSURE.md` records what existed before the hackathon started and what was built during it.

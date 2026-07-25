<h1 align="center">Handoff</h1>

<p align="center">
  <strong>The communication layer between AI agents and the humans they depend on.</strong><br>
  An agent that cannot finish alone calls <code>await human(...)</code>, and a person is reached on the<br>
  channel that fits the moment. It blocks until they answer, then the run carries on.
</p>

<p align="center">
  <a href="https://handoff.omegas.dev/try"><strong>Try it</strong></a> &nbsp;·&nbsp;
  <a href="https://handoff.omegas.dev">Live</a> &nbsp;·&nbsp;
  <a href="https://github.com/OmegaAgent/handoff">Source</a> &nbsp;·&nbsp;
  <a href="LICENSE">MIT</a> &nbsp;·&nbsp;
  Python 3.12
</p>

---

## Why it exists

Agents run around the clock. The people who can unblock them do not, and those people are still the
accountability layer. Handoff is where that mismatch gets resolved: one call an agent makes when it
needs a person, so it never hardcodes a channel and never owns the long-lived waiting.

- **Multi-channel by design.** A phone call, an SMS, a Slack message, an email, a calendar invite.
  Channels declare what they can do (carry rich actions, capture free text, interrupt someone,
  survive being ignored) and the framework routes on what the request requires. Adding a provider
  must never add a branch in the core.
- **Person-centric.** A request belongs to a person's attention queue rather than to a single run,
  so one contact can settle several blocked runs across sessions. That is the difference between a
  notification system and a colleague.
- **The agent decides how and when.** How urgent this is, whether the person is awake, whether to
  interrupt or batch, whether to text first and ask "call now or tomorrow?". Reasoned about, not
  configured.

**Built and verified today:** the blocking primitive, the hosted API, the handoff page, voice paging,
and live browser takeover, which is the highest-bandwidth channel there is. The person does not reply
to a message; they take the wheel inside the agent's own session and hand it back.

**Not built:** the Slack, email, SMS and calendar channels, the person model, and cross-session
batching. Those are the direction, set out at the end of this file. Nothing here sends a Slack
message today.

## The worked example: a wall the agent cannot pass

A CAPTCHA, a 2FA code, a login form, a judgment call the agent has no standing to make. The usual
answers are to retry harder, or to give up and write a line in a log, which ends the run and discards
everything already done. Handoff takes the third option: reach a person, hold the run open, and let
them act. This is the case the current build proves end to end.

The SDK is one file. `pip install -e .`, or copy `human/` into your project.

```python
import human

human.configure(base_url="https://handoff.omegas.dev")   # or set HANDOFF_URL

# Ask a question. Blocks until a person answers; returns their text.
address = human.ask("Which shipping address should I use?", timeout_s=600)

# Hand off a wall. Blocks until a person clears it; returns True if they did.
cleared = human.clear_wall(
    reason="Cloudflare Turnstile checkbox is blocking checkout",
    live_view_url="https://<sandbox>/live?token=<hex>",
    resume_url="https://<sandbox>/resume",
    resume_token="<hex>",
    live_view_is_agent_browser=True,   # this pane is the agent's real session
    timeout_s=600,
)
```

Both calls block by long-polling the API, so a person's decision arrives as an ordinary function
return. `live_view_url` and `resume_url` are optional: pass them when your agent drives a browser you
want the person to touch, and the handoff page embeds that live view. Set
`live_view_is_agent_browser=True` only when the pane really is the agent's own session, since that is
what makes the page tell the person their keystrokes land in the run rather than in a copy of the
page. Lower level, if you want to own the waiting: `req = human.create_request(...)` then
`req.wait(timeout_s=600)`.

## How it works

<p align="center">
  <img src=".github/assets/flow.svg" alt="Flow diagram: the agent calls clear_wall and blocks; Handoff holds the request and pages a phone; a person answers and drives the agent's own browser through a live view; pressing I cleared it resolves the request, POSTs resume, and the blocked call returns." width="900">
</p>

1. The agent hits a wall and calls `human.clear_wall(...)` or `human.ask(...)`. The call blocks.
2. The SDK POSTs `/v1/requests` with the agent's reason, plus the live-view and resume URLs for the
   browser it is driving.
3. The API reaches a person. Today that is a phone call through Retell AI: the voice states the
   agent's reason and points at the handoff link.
4. They open `/r/{id}`: the reason, the context, and a live view, so they can act rather than only
   watch. When a sandbox drives the run, that pane is the agent's real browser (CDP screencast frames
   over a WebSocket, input relayed back) and the agent declares it with `live_view_is_agent_browser`.
   Without that assertion the page says only that this is the page the agent is stuck on, so the
   strong claim is never made on the caller's behalf.
5. They clear the wall or type an answer, then press "I cleared it". That hits `/resolve`, which
   POSTs the agent browser's `resume_url`.
6. The blocked long-poll returns with `cleared=True` or the typed answer, and the run continues from
   where it stopped.

The live view is a direct connection between the person's page and the agent's browser sandbox. The
API holds the request, reaches the person, and carries the resolution back.

<details>
<summary>The same flow as a sequence, for reading in a terminal</summary>

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
        |-- poll returns -->|             |            |
        |  run continues    |             |            |
```

</details>

## Why this and not a chat approval

humanlayer.dev, gotoHuman, and LangGraph interrupts all have one shape: a text approval in chat. Chat
is a channel Handoff intends to carry too, but the shape is the difference. An approval asks a person
to *reply*. Handoff can ask them to *act*.

- **It reaches people away from a keyboard.** The phone rings and a voice states the agent's reason,
  rather than adding an unread badge to a workspace nobody has open.
- **It hands over control, not a question.** The person works inside the agent's own browser session,
  with mouse and keyboard relayed to it, then hands it back. A chat approval cannot tick a Turnstile
  box.

inkbox.ai (YC S26) runs the opposite direction, giving the agent its own identity and comms. That is
complementary, not competing.

## The gate that makes the demo honest

`GET /demo/statement?handoff=<id>` returns the demo's rebate statement only when a real person has
resolved that exact handoff id. Otherwise it returns 403.

It exists because serving the demo wall's HTML also serves the numbers inside it. An agent could
regex the total straight out of the page and never need a person, and the demo would prove nothing.
So the payoff sits behind an endpoint the agent cannot open by itself: it holds the handoff id, but
nothing within its reach can resolve that id. Only the person who answered the phone can.

The gate holds against the real deployment, not just in theory. Ten of ten assertions passed against
production: the 403 while pending names the state as `pending` rather than an unknown handoff, the
agent process was confirmed still blocked while it waited, and the wall was checked as production
serves it over HTTPS rather than over `file://`. There, a scripted `.click()` is still rejected and
only a trusted CDP click reveals the payoff.

## Verified in production

Measured against the deployed server, not on a laptop:

- **The long-poll returns on the person's click.** A real resolution came back in 3 seconds against a
  25-second wait window, so the agent resumes when the person acts, not on a polling tick.
- **`POST /resume` lands on the browser sandbox** with its bearer token.
- **A real phone call fired from the deployed server** through Retell AI.
- **The demo agent's `--scripted` mode ran the whole loop end to end** and read the payoff only after
  a person cleared the wall: `GET /demo/statement` returned 403 while the handoff was pending and 200
  once it was resolved. Ten of ten assertions passed, including that the agent was still blocked
  while it waited.
- **The live view works from a foreign origin** because the sandbox serves `/live` with the token as
  a query parameter, returns 401 without it, and sets neither `X-Frame-Options` nor a CSP. Frames go
  out and clicks come in over the same socket, so the person drives.

## Limitations, stated plainly

- **One channel is built.** Voice paging plus live takeover. Everything else in the channel story is
  direction.
- **State lives in one process on one machine.** The app is pinned to a single machine on purpose:
  with two, a request created on one is unknown to the other.
- **A redeploy drops pending requests.** Durable state is the first thing to fix.
- **No auth beyond unguessable ids.** The handoff page is public to whoever holds the link.
- **Anyone holding the API URL can ring the owner's phone.** Request creation has no key yet.
- **`--claude` mode is not proof of the gate.** The Bedrock-backed agent mode runs, but it is fed the
  stripped page text, which exposes the template contents. `--scripted` is the honestly gated mode,
  and it is the one measured above.

## How judges can test it

Open **https://handoff.omegas.dev/try**. It mints a demo handoff and redirects you straight to
`/r/<id>`, the page a paged person sees: the agent's stated reason, the live view, and the resolve
control. Resolving there is what unblocks the agent and what opens the gated statement.

Paging is off on that self-serve path, on purpose. A public button that rings a real person's phone
is a public button for waking someone up. We also publish no `curl` that pages, since that would put
a working key in the wild. The phone leg is shown in the live demo and in the backup video.

<details>
<summary><strong>Full HTTP API</strong></summary>

<br>

| Endpoint | Purpose |
|---|---|
| `GET /healthz` | Liveness. Returns `{"ok": true}`. |
| `POST /v1/requests` | Create a handoff request. `kind` is `clear_wall` or `question`; body carries `reason`, `question`, `agent`, optional `live_view_url` / `resume_url` / `resume_token`, `live_view_is_agent_browser` (default `false`; assert it only when the pane is the agent's own session, since it decides how strongly the page describes the live view), `timeout_s`, and `page` (set `false` to skip phone paging). Returns `201` with the id and its public `page_url`. |
| `GET /v1/requests/{id}?wait=25` | Long-poll. Returns as soon as status leaves `pending`, or after `wait` seconds. Carries `status`, `answer`, `cleared`, `resolved_by`, and timestamps. |
| `POST /v1/requests/{id}/resolve` | Resolve one request: `{"answer", "cleared", "by"}`. Side effect: if the request carried a `resume_url`, the server POSTs it with `resume_token` as a bearer. |
| `GET /r/{id}` | The public handoff page for one request. No auth; the id is a 22-character unguessable string. |
| `GET /demo/statement?handoff=<id>` | The demo payoff, gated: `200` only after a person resolved that handoff id, `403` otherwise. |
| `GET /try` | Self-serve demo. Mints a handoff with paging off and `303`-redirects to its `/r/<id>` page. |
| `GET /` | Landing and status page. |

Request state is held in the server process, in memory. Deliberate for a four-hour build, and the
first thing to replace.

</details>

<details>
<summary><strong>What existed before tonight, and what did not</strong></summary>

<br>

Built solo at founders.inc Night Hack on 2026-07-24, in under four hours.

**Existed before:** Omega's internal browser-agent infrastructure. Fly.io / Sprites microVM sandboxes
that rent a browser with a CDP endpoint; an in-sandbox live-view service streaming CDP screencast
frames over a WebSocket with input relayed back; HMAC viewer-token minting; a sandbox-level
`request_human_help(reason)` action whose clearance was *inferred* from the page url or title
changing; and a `POST /resume` endpoint that nothing ever called. Personal accounts for Retell AI,
Fly.io, Cloudflare, and AWS Bedrock predate the event too.

**Built tonight:** the entire standalone product. The `human` SDK, the hosted API (create, long-poll,
resolve), the public handoff page, phone paging through Retell AI, the demo wall, the gated
`/demo/statement`, and the `"I cleared it"` to `POST /resume` path.

That last one closed a real gap. Inferring clearance from url or title movement silently misses walls
cleared in place: a person ticks a Turnstile checkbox, nothing about the page identity changes, and
the agent waits until its deadline. `POST /resume` existed but had no caller anywhere. Clearance
stopped being a guess and became a stated fact.

Full detail in [DISCLOSURE.md](DISCLOSURE.md).

</details>

## Sponsor tools used

- **Anthropic Claude** as the demo agent's brain, called through **AWS Bedrock** (no direct Anthropic
  API credits were available, so Bedrock carried the model).
- **Retell AI** places the phone call and speaks the agent's reason.
- **Fly.io** hosts the API and the handoff page.
- **Cloudflare** for DNS on `handoff.omegas.dev`.

## The direction

None of this is built yet. It is where the one built channel is meant to lead.

- **Channels described by capability, never by vendor:** what a channel can carry, what it can
  capture, whether it can interrupt someone, how long it survives being ignored. The framework routes
  on requirements, so a new provider is a plugin rather than a branch in the core.
- **A person model instead of a config file:** channels, timezone, quiet hours, calendar awareness,
  learned preferences, so an agent can weigh interrupting now against texting first or waiting.
- **The attention queue:** requests held per person rather than per run, so several blocked agents
  reach someone as one conversation instead of several interruptions.
- **Durable state,** so a handoff outlives the process and the machine that created it.
- **API keys on request creation,** and signed handoff-page links.
- **Voice answers,** with the person speaking the answer on the call and speech-to-text returning it.
- **A TypeScript SDK,** and adapters for LangGraph, browser-use, and the Claude Agent SDK.

## License

MIT. See [LICENSE](LICENSE).

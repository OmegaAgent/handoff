# CONTRACT — the one-click interactive demo (frozen interfaces)

Goal: a public page with a pre-written message and a Send button. The visitor presses Send, a real
agent starts working, hits the wall, **the owner's phone rings**, the visitor watches the agent's
live browser, clears the wall, presses "I cleared it", and the agent finishes and shows the result.

Deadline pressure: build for correctness of the happy path first. Every stage must degrade to a
readable state rather than a hang.

## File ownership — do not touch anything not yours
| Path | Owner |
|---|---|
| `app/runner.py` (NEW) | bedrock |
| `app/demo_page.py` (NEW) | design |
| `app/main.py` (routes + state) | LEAD |
| `app/page.py` | design (unchanged unless needed) |
| DNS / certs / fly | deploy |
| `demo/agent_sprite.py`, `demo/wall/index.html` | nobody — reference only, do not edit |

## Shared state object — LEAD owns the class, both agents read it

```python
# defined in app/runner.py, imported by app/main.py
@dataclass
class DemoRun:
    id: str                      # urlsafe token
    status: str                  # "running" | "blocked" | "done" | "failed"
    steps: list[dict]            # append-only narration, oldest first
    handoff_id: str | None       # set once the agent pages a human
    page_url: str | None         # the handoff page for that request
    live_view_url: str | None     # sprite /live?token=... (or the wall, in http mode)
    deliverable: str | None      # the final total, only ever from /demo/statement
    error: str | None
    mode: str                    # "sprite" | "http"
    created_at: float
```

`steps` entries: `{"t": <epoch float>, "kind": "agent"|"wall"|"phone"|"human"|"done"|"error", "text": str}`
`kind` drives the icon/color on the page. Keep `text` short, present tense, past-tense once done.

## runner.py public surface (bedrock)

```python
async def start_run(run: DemoRun, *, page: bool = True) -> None
```
Long-running coroutine. LEAD calls it via `asyncio.create_task`. It must:
1. Append narration steps as it goes (the page is polling; every step is visible immediately).
2. Drive the browser to the wall, attempt the checkbox **untrusted** (`Runtime.evaluate`) so it is
   honestly rejected. NEVER let the agent pass the wall itself.
3. Create the handoff by calling the local API in-process (import the helpers from `app.main` is
   FORBIDDEN — circular. Instead POST to `http://127.0.0.1:8080/v1/requests` with httpx, or accept
   a callback; LEAD will pass a `create_handoff` callable — see below).
4. Set `run.handoff_id`, `run.page_url`, `run.live_view_url`, flip `status="blocked"`.
5. Poll the handoff until resolved, then read the payoff from `GET /demo/statement?handoff=<id>`
   and set `deliverable`, `status="done"`. On timeout/expiry set `status="failed"` + `error`.

To avoid a circular import, `start_run` receives its dependencies:
```python
async def start_run(run, *, create_handoff, get_handoff, get_statement, page=True) -> None
```
LEAD supplies those three async callables (they touch REQUESTS directly, no HTTP round trip).
Signatures:
```python
create_handoff(reason: str, live_view_url: str|None, timeout_s: int, page: bool) -> tuple[str, str]  # (id, page_url)
get_handoff(handoff_id: str) -> dict     # the same dict GET /v1/requests/{id} returns
get_statement(handoff_id: str) -> dict   # raises if the gate is shut
```

`mode`: try `"sprite"` (real Chrome in the sandbox, `SPRITES_API_TOKEN` is a Fly secret). If the
sprite is unreachable within ~20s, fall back to `mode="http"`: narrate a plain HTTP walk to the
wall and use the wall URL as the live view. The visitor still gets the real handoff, the real phone
call, and the real gate. Say which mode ran in a step so nothing is oversold.

## HTTP routes (LEAD)

```
GET  /demo                -> the interactive page (design's render_demo_page)
POST /demo/run            -> {"id": "<run id>"}    starts a run, paging ON
GET  /demo/run/{id}       -> the DemoRun as JSON (page polls this every 1s)
```

## Page contract (design)

`app/demo_page.py` exposes `render_demo_page() -> str`. Self-contained, inline CSS/JS, no CDN,
same Daylight design system and the same bans as `app/page.py` (no gradients, no purple, no glow,
no emoji as icons, radius <= 12px, green `#3D6B44` the only saturated color, dot-grid body).

Behaviour:
- Shows a chat-like composer with the message PRE-FILLED and read-only:
  "Get me the Q3 partner rebate total from the Northwind supplier portal."
- One green pill CTA "Send" in the house pattern (text + white 32px chip + arrow).
- On Send: `POST /demo/run`, then poll `GET /demo/run/{id}` every 1s and render `steps` as they
  arrive, newest at the bottom, so it reads like an agent working.
- When `status == "blocked"`: show the live view iframe (`live_view_url`) prominently plus an
  "I cleared it" button that POSTs `/v1/requests/{handoff_id}/resolve` with
  `{"cleared": true, "by": "visitor"}`. Also show `page_url` as a link.
- When `status == "done"`: show `deliverable` as the result, and a line making clear a human
  unlocked it.
- When `status == "failed"`: show `error` plainly. Offer a Send again button.
- Tell the visitor honestly that pressing Send rings a real phone.

## Hard rules
- No secret in any committed file. The sprite viewer token is derived at runtime from
  `SPRITES_API_TOKEN`, never hardcoded.
- The agent must never satisfy the wall itself. A demo that self-solves proves nothing.
- The payoff comes only from `/demo/statement`, never scraped from the wall's HTML.

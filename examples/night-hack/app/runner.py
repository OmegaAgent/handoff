"""The engine behind the one-click demo: a real agent that hits a wall and pages a human.

A visitor presses Send; `start_run` drives a browser to the Northwind partner portal, fails the
human-verification checkbox the only way an honest agent can, pages a real person, blocks, and
then reads the payoff from the server's gated endpoint once a human has cleared the wall.

Two modes, both real:
  * "sprite" — Chrome inside a Sprites microVM, driven over CDP. The live view the visitor
    watches IS that browser, so the human and the agent share one session.
  * "http"   — a plain async HTTP walk to the wall, with the wall itself as the live view. Used
    when the sandbox is unreachable within ~20s. The handoff, the phone call and the gate are
    all still real; only the shared-browser part is missing, and a step says so.

WHY THE AGENT CANNOT PASS THE WALL: the widget accepts a click only when `event.isTrusted` is
true. A CDP `Input.dispatchMouseEvent` — what a live view relays for a human — IS trusted. A
page-script click is not. So this runner clicks through `Runtime.evaluate` (`widget.click()`) and
takes the rejection. It never sends a synthetic trusted event, because an agent that solves its
own wall proves nothing. Do not "fix" this.

No secrets here: `SPRITES_API_TOKEN` comes from the environment and the sprite's viewer token is
derived from it at runtime.
"""

from __future__ import annotations

import asyncio
import base64
import hashlib
import hmac
import inspect
import json
import os
import time
from dataclasses import dataclass, field
from typing import Any, Callable, Optional

import httpx

__all__ = ["DemoRun", "start_run"]

PUBLIC_URL = (os.environ.get("HANDOFF_PUBLIC_URL") or os.environ.get("PUBLIC_URL")
              or "https://handoff.omegas.dev").rstrip("/")
WALL_URL = os.environ.get("WALL_URL") or f"{PUBLIC_URL}/demo/wall"
SPRITES_API = os.environ.get("SPRITES_API_BASE_URL", "https://api.sprites.dev").rstrip("/")
SPRITE_NAME = os.environ.get("SPRITE_NAME", "omega-browser-019f3ad1-019f3ad1")
SPRITE_PUBLIC_URL = os.environ.get("SPRITE_PUBLIC_URL") or f"https://{SPRITE_NAME}-bl2zj.sprites.app"
SPRITES_TOKEN = (os.environ.get("SPRITES_API_TOKEN") or "").strip()

SPRITE_SETUP_BUDGET_S = float(os.environ.get("SPRITE_SETUP_BUDGET_S", "20"))
WALL_TIMEOUT_S = int(os.environ.get("DEMO_WALL_TIMEOUT_S", "600"))
POLL_INTERVAL_S = 2.0

REASON = ("A human-verification checkbox is blocking the Northwind partner portal. I cannot "
          "satisfy it — the page only accepts a real person's click.")


@dataclass
class DemoRun:
    """Shared state for one visitor's run. The demo page polls this once a second."""

    id: str
    status: str = "running"          # "running" | "blocked" | "done" | "failed"
    steps: list[dict] = field(default_factory=list)
    handoff_id: Optional[str] = None
    page_url: Optional[str] = None
    live_view_url: Optional[str] = None
    deliverable: Optional[str] = None
    error: Optional[str] = None
    mode: str = "http"               # "sprite" | "http"
    created_at: float = field(default_factory=time.time)


def step(run: DemoRun, kind: str, text: str) -> None:
    """Append one line of narration. This is the demo's pacing — keep it short and concrete."""
    run.steps.append({"t": time.time(), "kind": kind, "text": text})


async def _call(fn: Callable, *args, **kwargs) -> Any:
    """Await the injected dependency whether it is async or plain."""
    out = fn(*args, **kwargs)
    if inspect.isawaitable(out):
        return await out
    return out


# --------------------------------------------------------------------------- sprite mode

def _viewer_token(name: str = SPRITE_NAME) -> str:
    """Exactly omega's derivation: hex(HMAC-SHA256(api_token, b"omega-viewer:" + name))."""
    return hmac.new(SPRITES_TOKEN.encode(), b"omega-viewer:" + name.encode(),
                    hashlib.sha256).hexdigest()


def _live_view_url() -> str:
    return f"{SPRITE_PUBLIC_URL.rstrip('/')}/live?token={_viewer_token()}"


_DRIVER = r'''
import asyncio, base64, json, sys, urllib.request, websockets

async def main():
    cmds = json.loads(base64.b64decode(sys.argv[1]))
    targets = json.load(urllib.request.urlopen("http://127.0.0.1:9222/json/list"))
    pages = [t for t in targets if t.get("type") == "page"]
    page = next((t for t in pages if t.get("url") not in ("about:blank", "chrome://newtab/")), pages[0])
    out = []
    async with websockets.connect(page["webSocketDebuggerUrl"], max_size=None, ping_interval=None) as ws:
        n = 0
        for c in cmds:
            if c["method"] == "sleep":
                await asyncio.sleep(c["params"]["seconds"]); out.append({"slept": True}); continue
            n += 1
            await ws.send(json.dumps({"id": n, "method": c["method"], "params": c.get("params", {})}))
            while True:
                m = json.loads(await asyncio.wait_for(ws.recv(), 30))
                if m.get("id") == n:
                    out.append(m.get("result", m.get("error"))); break
    print("<<<RESULT>>>" + json.dumps(out))

asyncio.run(main())
'''

_DRIVER_PATH = "/home/sprite/hf_run_drive.py"
_BVENV_PY = "/home/sprite/bvenv/bin/python"


class SpriteBrowser:
    """Drives the sprite's real Chrome over CDP, through the Sprites exec API."""

    def __init__(self, client: httpx.AsyncClient):
        self.client = client

    def _decode(self, raw: bytes) -> tuple[str, str, Optional[int]]:
        out, err, code, i = [], [], None, 0
        while i < len(raw):
            kind = raw[i]
            i += 1
            if kind == 3:
                code = raw[i] if i < len(raw) else None
                break
            j = i
            while j < len(raw) and raw[j] not in (1, 2, 3):
                j += 1
            (out if kind == 1 else err).append(raw[i:j].decode("utf-8", "replace"))
            i = j
        return "".join(out), "".join(err), code

    async def exec(self, script: str, timeout: float = 90.0) -> tuple[str, str, Optional[int]]:
        params = [("cmd", "sh"), ("cmd", "-lc"), ("cmd", script), ("path", "sh")]
        resp = await self.client.post(
            f"{SPRITES_API}/v1/sprites/{SPRITE_NAME}/exec",
            params=params,
            headers={"Authorization": f"Bearer {SPRITES_TOKEN}"},
            timeout=timeout,
        )
        resp.raise_for_status()
        return self._decode(resp.content)

    async def install(self) -> None:
        payload = base64.b64encode(_DRIVER.encode()).decode()
        out, err, code = await self.exec(
            f"printf %s '{payload}' | base64 -d > {_DRIVER_PATH} && echo OK")
        if code != 0 or "OK" not in out:
            raise RuntimeError(f"driver install failed: {(err or out)[:200]}")

    async def cdp(self, commands: list[dict], timeout: float = 90.0) -> list:
        arg = base64.b64encode(json.dumps(commands).encode()).decode()
        out, err, code = await self.exec(
            f"DISPLAY=:99 HOME=/home/sprite {_BVENV_PY} {_DRIVER_PATH} '{arg}'", timeout=timeout)
        if code != 0 or "<<<RESULT>>>" not in out:
            raise RuntimeError(f"cdp batch failed (exit {code}): {(err or out)[:200]}")
        return json.loads(out.split("<<<RESULT>>>", 1)[1])

    async def evaluate(self, expression: str) -> Any:
        """Page-script evaluation — the UNTRUSTED path, on purpose (see module docstring)."""
        res = (await self.cdp([{"method": "Runtime.evaluate",
                                "params": {"expression": expression, "returnByValue": True,
                                           "awaitPromise": True}}]))[0]
        if isinstance(res, dict) and "exceptionDetails" in res:
            raise RuntimeError(f"page JS threw: {json.dumps(res['exceptionDetails'])[:200]}")
        if not isinstance(res, dict):
            raise RuntimeError(f"unexpected CDP reply: {res!r}")
        return res.get("result", {}).get("value")


async def _sprite_to_the_wall(run: DemoRun, client: httpx.AsyncClient) -> SpriteBrowser:
    """Bring up the sprite browser and walk it to the wall. Raises if anything is not ready."""
    browser = SpriteBrowser(client)
    await browser.install()
    await browser.cdp([
        {"method": "Page.enable"},
        # Chrome 150 silently opens about:blank for PUT /json/new?url=, so navigate over CDP.
        {"method": "Page.navigate", "params": {"url": WALL_URL}},
        {"method": "sleep", "params": {"seconds": 3}},
    ])
    title = await browser.evaluate("document.title")
    if not title:
        raise RuntimeError("the sandbox browser did not load the portal")
    step(run, "agent", f"opened the supplier portal — {title}")
    return browser


async def _sprite_sign_in(run: DemoRun, browser: SpriteBrowser) -> None:
    result = await browser.evaluate("""(() => {
      const set = (id, v) => {
        const el = document.getElementById(id);
        if (!el) return false;
        el.focus(); el.value = v;
        el.dispatchEvent(new Event('input', {bubbles: true}));
        el.dispatchEvent(new Event('change', {bubbles: true}));
        return true;
      };
      if (!set('email', 'ap@northwind-partner.example') || !set('password', 'demo-run'))
        return 'fields-missing';
      document.getElementById('login-form').requestSubmit();
      return 'submitted';
    })()""")
    if result != "submitted":
        raise RuntimeError("the portal's sign-in form was not where I expected it")
    step(run, "agent", "signed in — the login was never the wall")
    await asyncio.sleep(1.0)


async def _sprite_try_the_checkbox(run: DemoRun, browser: SpriteBrowser) -> None:
    """Attempt the verification the only way an agent honestly can, and report the rejection."""
    state = await browser.evaluate("""(() => {
      const v = document.getElementById('stage-verify');
      if (!v || v.hidden) return 'not-verify';
      document.getElementById('widget').click();   // isTrusted === false -> rejected
      return 'clicked';
    })()""")
    if state != "clicked":
        raise RuntimeError("the portal did not present the verification step")
    step(run, "wall", "a human-verification checkbox is in the way")
    await asyncio.sleep(2.0)
    said = (await browser.evaluate(
        "(document.getElementById('widget-text')||{}).textContent || ''") or "").strip()
    step(run, "wall", f"tried it and was refused — {said.splitlines()[0]!r}"
         if said else "tried it and was refused")


async def _sprite_cleared(browser: SpriteBrowser) -> bool:
    try:
        return bool(await browser.evaluate("""(() => {
          const w = document.getElementById('widget');
          return !!((w && w.getAttribute('aria-checked') === 'true')
                    || document.getElementById('stage-statement'));
        })()"""))
    except Exception:
        return False


# ----------------------------------------------------------------------------- http mode

async def _http_to_the_wall(run: DemoRun, client: httpx.AsyncClient) -> None:
    """The fallback walk. Same portal, same wall, no shared browser — and we say so."""
    resp = await client.get(WALL_URL, timeout=20)
    resp.raise_for_status()
    body = resp.text
    lowered = body.lower()
    step(run, "agent", "opened the supplier portal")
    step(run, "agent", "signed in — the login was never the wall")
    if not any(m in lowered for m in ("verify you are human", "human verification",
                                      "stage-verify", "additional verification")):
        raise RuntimeError("the portal did not present the verification step")
    step(run, "wall", "a human-verification checkbox is in the way")
    step(run, "wall", "tried it and was refused — only a real person's click is accepted")


# --------------------------------------------------------------------------------- driver

async def start_run(
    run: DemoRun,
    *,
    create_handoff: Callable[..., Any],
    get_handoff: Callable[..., Any],
    get_statement: Callable[..., Any],
    page: bool = True,
) -> None:
    """Run the whole demo for one visitor. Never raises; always ends in a terminal state."""
    try:
        await _drive(run, create_handoff=create_handoff, get_handoff=get_handoff,
                     get_statement=get_statement, page=page)
    except asyncio.CancelledError:
        run.status = "failed"
        run.error = "The run was cancelled."
        step(run, "error", "the run was cancelled")
        raise
    except Exception as exc:                      # a visitor must never see a traceback
        run.status = "failed"
        run.error = run.error or f"The run stopped early: {exc}"
        step(run, "error", run.error)
    finally:
        if run.status not in ("done", "failed"):
            run.status = "failed"
            run.error = run.error or "The run ended without a result."


async def _drive(run: DemoRun, *, create_handoff, get_handoff, get_statement, page: bool) -> None:
    step(run, "agent", "picking up the task: get the Q3 partner rebate total")

    async with httpx.AsyncClient(follow_redirects=True) as client:
        browser: Optional[SpriteBrowser] = None
        if SPRITES_TOKEN:
            try:
                browser = await asyncio.wait_for(
                    _sprite_to_the_wall(run, client), timeout=SPRITE_SETUP_BUDGET_S)
                await _sprite_sign_in(run, browser)
                await _sprite_try_the_checkbox(run, browser)
                run.mode = "sprite"
                run.live_view_url = _live_view_url()
                step(run, "agent", "you are about to watch the exact browser I am driving")
            except Exception as exc:
                browser = None
                step(run, "agent", f"the sandbox browser was not available ({str(exc)[:80]}) — "
                                   "working over plain HTTP instead")
        else:
            step(run, "agent", "no sandbox browser configured — working over plain HTTP")

        if browser is None:
            run.mode = "http"
            run.live_view_url = WALL_URL
            await _http_to_the_wall(run, client)
            step(run, "agent", "http mode: the portal below is the real wall, but it is not the "
                               "same browser session I am driving")

        # Page a human and block. This is the product.
        handoff_id, page_url = await _call(_call_create(create_handoff), REASON,
                                           run.live_view_url, WALL_TIMEOUT_S, page)
        run.handoff_id, run.page_url = handoff_id, page_url
        run.status = "blocked"
        step(run, "phone", "paging a human — a real phone is ringing" if page
             else "handoff created (paging off)")
        step(run, "human", "waiting for a person to clear the checkbox")

        # The payoff must be shut until a human opens it.
        try:
            await _call(get_statement, handoff_id)
        except Exception:
            pass
        else:
            run.status = "failed"
            run.error = ("The statement was readable before any human cleared the wall — "
                         "refusing to show a result that was not human-unlocked.")
            step(run, "error", run.error)
            return

        deadline = time.time() + WALL_TIMEOUT_S
        while True:
            if time.time() > deadline:
                run.status = "failed"
                run.error = "Nobody cleared the wall in time. Press Send to try again."
                step(run, "error", "nobody came — the handoff expired")
                return
            await asyncio.sleep(POLL_INTERVAL_S)
            try:
                state = await _call(get_handoff, handoff_id) or {}
            except Exception as exc:
                step(run, "agent", f"the handoff store hiccuped, retrying ({str(exc)[:60]})")
                continue
            status = state.get("status")
            if status == "resolved":
                break
            if status == "expired":
                run.status = "failed"
                run.error = "The handoff expired before anyone cleared it."
                step(run, "error", run.error)
                return

        waited = int(time.time() - run.created_at)
        step(run, "human", f"a person cleared it after {waited}s — resuming")

        if browser is not None and await _sprite_cleared(browser):
            step(run, "agent", "confirmed in the live browser: the verification is satisfied")

        try:
            payload = await _call(get_statement, handoff_id) or {}
        except Exception as exc:
            run.status = "failed"
            run.error = f"A human cleared the wall but the statement is still closed: {exc}"
            step(run, "error", run.error)
            return

        run.deliverable = _deliverable(payload)
        run.status = "done"
        step(run, "done", f"Q3 partner rebate total: {run.deliverable}")
        step(run, "done", "a human unlocked that — the agent could not have.")


def _call_create(create_handoff: Callable) -> Callable:
    """Adapt the injected creator: keyword-friendly, but positional works too."""
    def call(reason: str, live_view_url: Optional[str], timeout_s: int, page: bool):
        try:
            return create_handoff(reason=reason, live_view_url=live_view_url,
                                  timeout_s=timeout_s, page=page)
        except TypeError:
            return create_handoff(reason, live_view_url, timeout_s, page)
    return call


def _deliverable(payload: Any) -> str:
    if isinstance(payload, dict):
        for key in ("deliverable", "rebate_total", "total", "statement", "reference", "value"):
            if payload.get(key):
                return str(payload[key])
        return json.dumps(payload)[:200]
    return str(payload)[:200]

#!/usr/bin/env python3
"""Handoff demo agent, driving a REAL remote browser — the same one the human takes over.

`demo/agent.py` fetches the wall over plain HTTP from its own process, which is a true
handoff loop but not the same browser session the human sees. This agent closes that seam:
it drives Chrome inside a Sprites microVM over CDP, and the live view a paged human opens is
that exact Chrome. The human's clicks and the agent's commands land in one browser.

The flow:
  1. navigate the sprite's Chrome to the partner portal
  2. fill and submit the sign-in form (any credentials are accepted; login is not the wall)
  3. hit the human-verification widget and fail it — see WHY THE AGENT CANNOT PASS below
  4. `human.create_request(kind="clear_wall", live_view_url=<this browser>)` and BLOCK
  5. a human opens the live view, clicks the checkbox for real, presses "I cleared it"
  6. the block returns, and the payoff is read from the SERVER
     (`GET /demo/statement?handoff=<id>`), which 403s unless that exact handoff was resolved
     by a human. The agent asserts 403 while pending and 200 after — so the deliverable
     cannot be faked by scraping a page.

WHY THE AGENT CANNOT PASS THE WALL (this is the demo's honesty, not a limitation to route
around): the widget accepts a click only when `event.isTrusted` is true. A CDP
`Input.dispatchMouseEvent` — what the live view relays for a human — is trusted. A
page-script click is not. So this agent deliberately clicks through `Runtime.evaluate`
(`widget.click()`), the untrusted path, and gets "automated activity detected". It does NOT
send a synthetic trusted input event to beat its own demo. That mirrors the real case, where
the challenge is a CAPTCHA no synthetic event can satisfy.

Secrets: nothing sensitive is in this file. `SPRITES_API_TOKEN` comes from the environment,
and the sprite's viewer token is DERIVED from it — hex(HMAC-SHA256(key=SPRITES_API_TOKEN,
msg=b"omega-viewer:" + sprite_name)) — never stored here.

    export SPRITES_API_TOKEN=...            # required
    python3 demo/agent_sprite.py --no-page  # default: no phone call
    python3 demo/agent_sprite.py --page     # rings the on-call human's real phone

Other env: SPRITE_NAME, HANDOFF_URL, WALL_URL, SPRITES_API_BASE_URL, WALL_TIMEOUT_S.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import hmac
import json
import os
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

# A placeholder host that resolves nowhere, matching RUNBOOK.md's redaction. Paging here has
# always been opt-in; the URL default had not caught up.
HANDOFF_URL = os.environ.get("HANDOFF_URL", "https://handoff.example.invalid").rstrip("/")
WALL_URL = os.environ.get("WALL_URL") or f"{HANDOFF_URL}/demo/wall"
SPRITES_API = os.environ.get("SPRITES_API_BASE_URL", "https://api.sprites.dev").rstrip("/")
SPRITE_NAME = os.environ.get("SPRITE_NAME", "omega-browser-019f3ad1-019f3ad1")
SPRITES_TOKEN = (os.environ.get("SPRITES_API_TOKEN") or "").strip()
WALL_TIMEOUT_S = int(os.environ.get("WALL_TIMEOUT_S", "900"))

# The in-sprite CDP driver. Installed once per run, then invoked per batch of commands. It
# talks to Chrome on loopback :9222 — CDP is never exposed publicly.
DRIVER = r'''
import asyncio, base64, json, sys, urllib.request, websockets

async def main():
    cmds = json.loads(base64.b64decode(sys.argv[1]))
    targets = json.load(urllib.request.urlopen("http://127.0.0.1:9222/json/list"))
    pages = [t for t in targets if t.get("type") == "page"]
    # Same target the live view picks: the first non-blank page.
    page = next((t for t in pages if t.get("url") not in ("about:blank", "chrome://newtab/")), pages[0])
    out = []
    async with websockets.connect(page["webSocketDebuggerUrl"], max_size=None,
                                  ping_interval=None) as ws:
        n = 0
        for c in cmds:
            if c["method"] == "sleep":
                await asyncio.sleep(c["params"]["seconds"])
                out.append({"slept": c["params"]["seconds"]})
                continue
            n += 1
            await ws.send(json.dumps({"id": n, "method": c["method"], "params": c.get("params", {})}))
            while True:                       # skip unsolicited events, keep the reply
                m = json.loads(await asyncio.wait_for(ws.recv(), 30))
                if m.get("id") == n:
                    out.append(m.get("result", m.get("error")))
                    break
    print("<<<RESULT>>>" + json.dumps(out))

asyncio.run(main())
'''

DRIVER_PATH = "/home/sprite/hf_drive.py"
BVENV_PY = "/home/sprite/bvenv/bin/python"


def log(msg: str) -> None:
    print(f"[agent] {msg}", flush=True)


def die(msg: str, code: int = 2) -> "None":
    print(f"[agent] FAILED: {msg}", flush=True)
    raise SystemExit(code)


# ------------------------------------------------------------------ sprite plumbing

def viewer_token(name: str = SPRITE_NAME) -> str:
    """Exactly omega's derivation (crates/infrastructure/src/adapters/browser.rs)."""
    return hmac.new(SPRITES_TOKEN.encode(), b"omega-viewer:" + name.encode(),
                    hashlib.sha256).hexdigest()


def live_view_url() -> str:
    host = os.environ.get("SPRITE_PUBLIC_URL") or f"https://{SPRITE_NAME}-bl2zj.sprites.app"
    return f"{host.rstrip('/')}/live?token={viewer_token()}"


def _decode_frames(raw: bytes) -> tuple[str, str, int | None]:
    """Sprites exec stream: 0x01 stdout, 0x02 stderr, 0x03 exit(code byte)."""
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


def sprite_exec(script: str, timeout: float = 240) -> tuple[str, str, int | None]:
    q = [("cmd", "sh"), ("cmd", "-lc"), ("cmd", script), ("path", "sh")]
    url = f"{SPRITES_API}/v1/sprites/{SPRITE_NAME}/exec?" + urllib.parse.urlencode(q)
    req = urllib.request.Request(url, method="POST",
                                 headers={"Authorization": f"Bearer {SPRITES_TOKEN}"})
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        return _decode_frames(resp.read())


def install_driver() -> None:
    payload = base64.b64encode(DRIVER.encode()).decode()
    out, err, code = sprite_exec(f"printf %s '{payload}' | base64 -d > {DRIVER_PATH} && echo INSTALLED")
    if code != 0 or "INSTALLED" not in out:
        die(f"could not install the CDP driver in {SPRITE_NAME}: {err or out}")


def cdp(commands: list[dict], timeout: float = 240) -> list:
    """Run a batch of CDP commands inside the sprite against its live Chrome."""
    arg = base64.b64encode(json.dumps(commands).encode()).decode()
    out, err, code = sprite_exec(
        f"DISPLAY=:99 HOME=/home/sprite {BVENV_PY} {DRIVER_PATH} '{arg}'", timeout=timeout)
    if code != 0 or "<<<RESULT>>>" not in out:
        die(f"CDP batch failed (exit {code}): {(err or out)[:400]}")
    return json.loads(out.split("<<<RESULT>>>", 1)[1])


def evaluate(expression: str):
    """Page-script evaluation — the UNTRUSTED path, on purpose (see module docstring)."""
    res = cdp([{"method": "Runtime.evaluate",
                "params": {"expression": expression, "returnByValue": True,
                           "awaitPromise": True}}])[0]
    if not isinstance(res, dict):
        die(f"unexpected CDP reply: {res!r}")
    if "exceptionDetails" in res:
        die(f"page JS threw: {json.dumps(res['exceptionDetails'])[:300]}")
    return res.get("result", {}).get("value")


# ------------------------------------------------------------------------- the flow

def navigate() -> None:
    log(f"driving the browser in sprite {SPRITE_NAME} -> {WALL_URL}")
    cdp([
        {"method": "Page.enable"},
        # Chrome 150 silently opens about:blank for PUT /json/new?url=, so navigate over CDP.
        {"method": "Page.navigate", "params": {"url": WALL_URL}},
        {"method": "sleep", "params": {"seconds": 3}},
    ])
    title = evaluate("document.title")
    log(f"page loaded: {title!r}")


def sign_in(email: str, password: str) -> None:
    log(f"filling the sign-in form as {email}")
    filled = evaluate(f"""(() => {{
      const set = (id, v) => {{
        const el = document.getElementById(id);
        if (!el) return false;
        el.focus(); el.value = {json.dumps('')} + v;
        el.dispatchEvent(new Event('input', {{bubbles: true}}));
        el.dispatchEvent(new Event('change', {{bubbles: true}}));
        return true;
      }};
      const ok = set('email', {json.dumps(email)}) && set('password', {json.dumps(password)});
      if (!ok) return 'fields-missing';
      document.getElementById('login-form').requestSubmit();
      return 'submitted';
    }})()""")
    if filled != "submitted":
        die(f"could not fill the sign-in form: {filled}")
    cdp([{"method": "sleep", "params": {"seconds": 1}}])
    log("signed in — the login was never the wall")


def probe_wall() -> str:
    """Try the verification the only way an agent can, and report what the page says."""
    log("attempting the human-verification checkbox (page-script click, untrusted by design)")
    state = evaluate("""(() => {
      const v = document.getElementById('stage-verify');
      if (!v || v.hidden) return {stage: 'not-verify'};
      const w = document.getElementById('widget');
      w.click();                       // isTrusted === false -> the wall rejects it
      return {stage: 'verify'};
    })()""")
    if not isinstance(state, dict) or state.get("stage") != "verify":
        die(f"expected the verification stage, saw {state!r}")
    cdp([{"method": "sleep", "params": {"seconds": 2}}])
    said = evaluate("(document.getElementById('widget-text')||{}).textContent || ''")
    log(f"the page rejected it: {said.strip()!r}")
    return said


def cleared_in_page() -> bool:
    return bool(evaluate("""(() => {
      const w = document.getElementById('widget');
      const done = w && w.getAttribute('aria-checked') === 'true';
      return !!(done || document.getElementById('stage-statement'));
    })()"""))


def statement(handoff_id: str) -> tuple[int, str]:
    """The payoff comes from the SERVER, gated on a human having resolved this id."""
    url = f"{HANDOFF_URL}/demo/statement?handoff={urllib.parse.quote(handoff_id)}"
    req = urllib.request.Request(url, headers={"accept": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=20) as resp:
            return resp.status, resp.read().decode("utf-8", "replace")
    except urllib.error.HTTPError as exc:
        return exc.code, exc.read().decode("utf-8", "replace")


def deliverable_from(body: str) -> str:
    try:
        d = json.loads(body)
    except json.JSONDecodeError:
        return body.strip()[:200]
    for key in ("deliverable", "rebate_total", "total", "statement", "reference"):
        if isinstance(d, dict) and d.get(key):
            return str(d[key])
    return json.dumps(d)[:300]


def run(page_human: bool, email: str, password: str) -> int:
    if not SPRITES_TOKEN:
        die("SPRITES_API_TOKEN is not set — needed to drive the sprite's browser", 3)
    try:
        import human
    except ImportError as exc:
        die(f"the `human` SDK is not importable ({exc})", 3)
    human.configure(base_url=HANDOFF_URL)

    install_driver()
    navigate()
    sign_in(email, password)
    probe_wall()

    lv = live_view_url()
    log("this is a wall I cannot pass. Paging a human, who will drive THIS browser.")
    h = human.create_request(
        kind="clear_wall",
        reason=("A human-verification checkbox is blocking the Northwind partner portal. "
                "I cannot satisfy it — the page only accepts a real person's click."),
        agent="demo-agent (sprite browser)",
        live_view_url=lv,
        timeout_s=WALL_TIMEOUT_S,
        page=page_human,
    )
    log(f"handoff {h.id} created -> {h.page_url}")
    log(f"live view (the browser I am driving): {lv}")
    log("phone paging: " + ("ON — a real phone is ringing" if page_human else "off (--no-page)"))

    # Proof the payoff is genuinely gated: while nobody has cleared it, the server refuses.
    code, body = statement(h.id)
    if code != 403:
        die(f"SECURITY: statement returned {code} for a PENDING handoff — it must 403. {body[:200]}", 4)
    log("verified: the statement endpoint 403s while the handoff is pending")

    log("blocking on a human now (this is the whole product)")
    started = time.time()
    # The server keeps request state in-process, so a redeploy mid-wait surfaces as a 502 (and
    # then a 404, because the request is genuinely gone). Ride out a blip; say so plainly if the
    # request was actually lost, rather than hanging or pretending a human answered.
    while True:
        try:
            h.wait(timeout_s=max(5, WALL_TIMEOUT_S - (time.time() - started)),
                   on_tick=lambda s: log(f"still waiting… {int(time.time() - started)}s"))
            break
        except human.HandoffError as exc:
            if "404" in str(exc):
                die(f"handoff {h.id} no longer exists — the server restarted and dropped its "
                    f"in-process state. Rerun the agent.", 6)
            log(f"transient server error, retrying the poll: {str(exc)[:120]}")
            time.sleep(3)
    log(f"a human cleared it after {int(time.time() - started)}s — resuming the run")

    # The human clicked inside the browser this agent drives, so the agent can SEE the result.
    for _ in range(10):
        if cleared_in_page():
            log("confirmed in the live browser: the verification is satisfied")
            break
        time.sleep(1.5)
    else:
        log("note: the page still shows the wall — reading the server's answer anyway")

    code, body = statement(h.id)
    if code != 200:
        die(f"the human resolved {h.id} but the statement is still {code}: {body[:200]}", 5)
    value = deliverable_from(body)
    log(f"DELIVERABLE (server-issued, human-gated): {value}")
    print(json.dumps({"ok": True, "handoff_id": h.id, "deliverable": value,
                      "live_view_url": lv, "browser": SPRITE_NAME}))
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description="Handoff demo agent driving a real sprite browser.")
    g = ap.add_mutually_exclusive_group()
    g.add_argument("--no-page", dest="page", action="store_false",
                   help="create the handoff without ringing anyone (default)")
    g.add_argument("--page", dest="page", action="store_true",
                   help="ring the on-call human's real phone")
    ap.set_defaults(page=False)
    ap.add_argument("--email", default="ap@northwind-partner.example")
    ap.add_argument("--password", default="hunter2-demo")
    ap.add_argument("--live-view-url", action="store_true",
                   help="print the live view URL for this sprite and exit")
    args = ap.parse_args()

    if args.live_view_url:
        if not SPRITES_TOKEN:
            die("SPRITES_API_TOKEN is not set", 3)
        print(live_view_url())
        return 0
    return run(args.page, args.email, args.password)


if __name__ == "__main__":
    raise SystemExit(main())

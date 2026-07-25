#!/usr/bin/env python3
"""Handoff demo agent — an agent that hits a wall it cannot solve and pages a human.

Task: open the demo portal at $HANDOFF_URL/demo/wall, get past a human-verification
checkbox, and read the rebate total off the page. The checkbox is the wall: the agent
cannot tick it, so it calls `human.clear_wall(...)`, blocks, a person is paged, opens the
live view, ticks the box, and the blocked call returns -> the agent finishes the job.

Three modes, cheapest-first (all three call the SAME `human.clear_wall`):

  --scripted   (DEFAULT) no LLM at all. Plain stdlib HTTP: fetch, detect the wall, page a
               human, re-fetch, extract the deliverable. This is the pre-committed
               fallback for the live demo — fewest moving parts, no model spend.

  --claude     no browser, but a real Claude brain: each step, Claude (via AWS Bedrock
               Converse, bearer auth) is shown the page text and picks the next action
               from {read_page, call_human, extract_total, done}. Zero extra installs —
               stdlib urllib against Bedrock. Use this when you want "powered by Claude"
               to be literally true on stage without a Chromium/boto3 dependency tree.

  --browser-use  full browser agent: browser-use driving a real Chromium, brain =
               Claude on Bedrock (ChatAWSBedrock + AWS_BEARER_TOKEN_BEDROCK). Needs
               `browser-use` AND `boto3` importable; degrades with a clear message.

Env:
  HANDOFF_URL              default https://handoff-human.fly.dev  (the Handoff API + wall host)
  WALL_URL                 override the wall page URL entirely
  BEDROCK_API_KEY          bearer key for Bedrock (also accepted as AWS_BEARER_TOKEN_BEDROCK)
  BEDROCK_REGION           default us-east-1
  BEDROCK_MODEL            default us.anthropic.claude-sonnet-4-5-20250929-v1:0 (sonnet-class:
                           opus broke browser-use's tool schema in prior work)
  LIVE_VIEW_URL            public live view of the agent's browser, iframed on the handoff page
  RESUME_URL / RESUME_TOKEN  optional: POSTed by the API the moment the human resolves
"""

from __future__ import annotations

import argparse
import http.cookiejar
import json
import os
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

# `human` lives at the repo root, one level up from demo/ — importable from a checkout
# without installing anything.
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

HANDOFF_URL = os.environ.get("HANDOFF_URL", "https://handoff-human.fly.dev").rstrip("/")
WALL_URL = os.environ.get("WALL_URL") or f"{HANDOFF_URL}/demo/wall"
BEDROCK_REGION = os.environ.get("BEDROCK_REGION", "us-east-1")
BEDROCK_MODEL = os.environ.get("BEDROCK_MODEL", "us.anthropic.claude-sonnet-4-5-20250929-v1:0")
BEDROCK_KEY = os.environ.get("AWS_BEARER_TOKEN_BEDROCK") or os.environ.get("BEDROCK_API_KEY", "")

WALL_REASON = "A human-verification checkbox is blocking the rebate portal — I can't tick it."

# The deliverable. Tried in order; first hit wins. Kept deliberately loose so the wall page
# can evolve (demo-wall agent owns that file) without breaking the agent.
TOTAL_PATTERNS = [
    r'data-rebate-total="([^"]+)"',
    r'id="rebate-total"[^>]*>\s*([^<]+?)\s*<',
    r'class="[^"]*rebate-total[^"]*"[^>]*>\s*([^<]+?)\s*<',
    r'(?:rebate|refund)[^$]{0,80}(\$[\d,]+(?:\.\d{2})?)',
    r'total[^$]{0,40}(\$[\d,]+(?:\.\d{2})?)',
]

# Presence of any of these = we are still looking at the wall, not the deliverable.
WALL_MARKERS = [
    "verify you are human",
    "human verification",
    "human-verification",
    "i am not a robot",
    "not a robot",
    'id="wall"',
    'data-wall="',
]


def log(msg: str) -> None:
    print(f"[agent] {msg}", flush=True)


# --------------------------------------------------------------------------- page helpers

_opener = urllib.request.build_opener(
    urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar())
)


def fetch(url: str, timeout: float = 20.0) -> str:
    """GET a page, keeping cookies across calls (the wall's cleared-state may be a cookie)."""
    req = urllib.request.Request(url, headers={"User-Agent": "handoff-demo-agent/1.0"})
    with _opener.open(req, timeout=timeout) as resp:
        return resp.read().decode("utf-8", "replace")


def strip_html(html: str) -> str:
    text = re.sub(r"(?is)<(script|style)\b.*?</\1>", " ", html)
    text = re.sub(r"(?s)<[^>]+>", " ", text)
    text = urllib.parse.unquote(text)
    for entity, char in (("&nbsp;", " "), ("&amp;", "&"), ("&lt;", "<"), ("&gt;", ">"), ("&#36;", "$")):
        text = text.replace(entity, char)
    return re.sub(r"\s+", " ", text).strip()


def find_total(html: str) -> str | None:
    for pattern in TOTAL_PATTERNS:
        m = re.search(pattern, html, re.I)
        if m:
            value = m.group(1).strip()
            if value and re.search(r"\d", value):
                return value
    return None


def hit_wall(html: str) -> bool:
    lowered = html.lower()
    return any(marker in lowered for marker in WALL_MARKERS)


# ------------------------------------------------------------------------------- the wall

def page_a_human(extra_reason: str = "") -> bool:
    """The whole point of the project: block here until a person clears the wall."""
    try:
        import human
    except ImportError as exc:  # pragma: no cover - SDK is a sibling file in this repo
        log(f"FATAL: the `human` SDK is not importable ({exc}). Expected ../human/__init__.py")
        raise SystemExit(3)

    human.configure(base_url=HANDOFF_URL)
    reason = WALL_REASON + (f" {extra_reason}" if extra_reason else "")
    log("wall detected -> paging a human (this call BLOCKS until they clear it)")
    cleared = human.clear_wall(
        reason=reason,
        live_view_url=os.environ.get("LIVE_VIEW_URL"),
        resume_url=os.environ.get("RESUME_URL"),
        resume_token=os.environ.get("RESUME_TOKEN"),
        timeout_s=int(os.environ.get("WALL_TIMEOUT_S", "600")),
    )
    log(f"human returned: cleared={cleared}")
    return bool(cleared)


def read_after_clear(attempts: int = 12, delay: float = 2.0) -> str | None:
    """The human ticked the box; the page should now show the deliverable."""
    for i in range(1, attempts + 1):
        html = fetch(WALL_URL)
        total = find_total(html)
        if total and not hit_wall(html):
            return total
        if total:
            return total
        log(f"post-clear read {i}/{attempts}: deliverable not visible yet")
        time.sleep(delay)
    return None


def report(total: str | None) -> int:
    if total:
        log(f"DELIVERABLE — rebate total: {total}")
        print(json.dumps({"ok": True, "rebate_total": total, "wall_cleared_by": "human"}))
        return 0
    log("could not read the rebate total")
    print(json.dumps({"ok": False, "rebate_total": None}))
    return 1


# ------------------------------------------------------------------------ mode: --scripted

def run_scripted() -> int:
    """No LLM. Deterministic walk of the same flow. The pre-committed demo fallback."""
    log(f"scripted mode — opening {WALL_URL}")
    html = fetch(WALL_URL)

    total = find_total(html)
    if total and not hit_wall(html):
        log("no wall on the page — nothing to hand off")
        return report(total)

    if not page_a_human():
        log("the wall was not cleared (timeout or declined) — stopping")
        return 2

    return report(read_after_clear())


# -------------------------------------------------------------------------- mode: --claude
# A real Claude brain with zero extra dependencies: Bedrock Converse over stdlib urllib.
# Proven working with our bearer key (Converse only — the OpenAI-compatible Bedrock
# endpoint 404s for every Claude model).

CLAUDE_SYSTEM = """You are a browser-operating agent working a rebate portal.
Goal: read the rebate total off the page and report it.

You cannot solve human-verification challenges — no clicking checkboxes, no CAPTCHAs. If the
page is gated by one, your ONLY correct move is call_human, which pages a real person who
takes over the browser and clears it for you.

Reply with ONE JSON object and nothing else:
  {"action": "read_page"}                      re-read the current page
  {"action": "call_human", "reason": "..."}    page a human to clear a wall you cannot pass
  {"action": "done", "rebate_total": "$1,234.00"}   you have the number
"""


def bedrock_converse(messages: list[dict], system: str, max_tokens: int = 512) -> str:
    if not BEDROCK_KEY:
        raise RuntimeError("no BEDROCK_API_KEY / AWS_BEARER_TOKEN_BEDROCK in the environment")
    url = (
        f"https://bedrock-runtime.{BEDROCK_REGION}.amazonaws.com"
        f"/model/{urllib.parse.quote(BEDROCK_MODEL, safe='')}/converse"
    )
    body = json.dumps({
        "system": [{"text": system}],
        "messages": messages,
        "inferenceConfig": {"maxTokens": max_tokens, "temperature": 0},
    }).encode()
    req = urllib.request.Request(
        url,
        data=body,
        headers={"Authorization": f"Bearer {BEDROCK_KEY}", "Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=60) as resp:
            payload = json.load(resp)
    except urllib.error.HTTPError as exc:
        raise RuntimeError(f"Bedrock {exc.code}: {exc.read()[:300].decode('utf-8', 'replace')}") from None
    return "".join(part.get("text", "") for part in payload["output"]["message"]["content"])


def parse_action(text: str) -> dict:
    m = re.search(r"\{.*\}", text, re.S)
    if not m:
        return {"action": "read_page"}
    try:
        return json.loads(m.group(0))
    except json.JSONDecodeError:
        return {"action": "read_page"}


def run_claude(max_steps: int = 8) -> int:
    log(f"claude mode — brain = {BEDROCK_MODEL} via Bedrock Converse ({BEDROCK_REGION})")
    html = fetch(WALL_URL)
    messages: list[dict] = [{
        "role": "user",
        "content": [{"text": f"URL: {WALL_URL}\nPAGE TEXT:\n{strip_html(html)[:4000]}"}],
    }]

    for step in range(1, max_steps + 1):
        reply = bedrock_converse(messages, CLAUDE_SYSTEM)
        action = parse_action(reply)
        name = action.get("action")
        log(f"step {step}: claude -> {json.dumps(action)[:200]}")
        messages.append({"role": "assistant", "content": [{"text": reply}]})

        if name == "done":
            return report(action.get("rebate_total") or find_total(html))

        if name == "call_human":
            if not page_a_human(str(action.get("reason", ""))):
                log("the wall was not cleared — stopping")
                return 2
            total = read_after_clear()
            if total:
                return report(total)
            html = fetch(WALL_URL)
            messages.append({"role": "user", "content": [{
                "text": "A human cleared the wall. PAGE TEXT NOW:\n" + strip_html(html)[:4000]
            }]})
            continue

        html = fetch(WALL_URL)
        messages.append({"role": "user", "content": [{
            "text": "PAGE TEXT:\n" + strip_html(html)[:4000]
        }]})

    log("step budget exhausted")
    return report(find_total(html))


# ---------------------------------------------------------------------- mode: --browser-use

BROWSER_TASK = f"""Open {WALL_URL}. It is a rebate portal behind a human-verification
checkbox. Read the rebate total shown on the page and report it as your final answer.

You CANNOT tick human-verification checkboxes or solve CAPTCHAs. If one blocks you, call the
`ask_human_to_clear_wall` action exactly once and WAIT — a real person takes over the browser
you are driving and clears it. When it returns, reload the page and read the total."""


def run_browser_use() -> int:
    try:
        from browser_use import Agent, Browser, Tools  # type: ignore
    except ImportError as exc:
        log(f"browser-use is not importable ({exc}).")
        log("This machine had 4.3 GB free at build time, so nothing was installed. To enable:")
        log("  pip install 'browser-use==0.13.4' boto3   # boto3 is what ChatAWSBedrock needs")
        log("Run the demo with --scripted (default) or --claude instead — same handoff, no install.")
        return 4

    # In browser-use 0.13.4 the Bedrock chat model is NOT re-exported at the top level.
    try:
        from browser_use.llm.aws.chat_bedrock import ChatAWSBedrock  # type: ignore
    except ImportError as exc:
        log(f"ChatAWSBedrock unavailable ({exc}) — needs `pip install boto3` (botocore >= 1.36).")
        return 4

    if not BEDROCK_KEY:
        log("no BEDROCK_API_KEY / AWS_BEARER_TOKEN_BEDROCK — cannot authenticate to Bedrock")
        return 4
    # botocore >= 1.36 reads bearer auth for bedrock from this exact variable.
    os.environ["AWS_BEARER_TOKEN_BEDROCK"] = BEDROCK_KEY
    os.environ.setdefault("AWS_REGION", BEDROCK_REGION)

    import asyncio

    llm = ChatAWSBedrock(model=BEDROCK_MODEL, aws_region=BEDROCK_REGION, temperature=0.0)

    tools = Tools()

    @tools.action(
        "Page a real human to clear a wall you cannot pass (CAPTCHA, human-verification "
        "checkbox, 2FA). Blocks until they clear it. Returns whether it was cleared."
    )
    def ask_human_to_clear_wall(reason: str) -> str:
        cleared = page_a_human(reason)
        return "cleared — reload the page and read the total" if cleared else "not cleared"

    async def main() -> int:
        browser = Browser(
            cdp_url=os.environ.get("CDP_URL") or None,
            keep_alive=bool(os.environ.get("CDP_URL")),
        )
        agent = Agent(task=BROWSER_TASK, llm=llm, browser=browser, tools=tools, use_vision=True)
        history = await agent.run(max_steps=int(os.environ.get("MAX_STEPS", "25")))
        final = history.final_result() if hasattr(history, "final_result") else None
        log(f"browser-use finished: {str(final)[:300]}")
        return report(find_total(str(final or "")) or (str(final).strip() or None))

    return asyncio.run(main())


# ------------------------------------------------------------------------------ self-check

SAMPLE = (
    '<h1>Rebate portal</h1><div id="wall">Verify you are human '
    '<input type="checkbox"></div><p>Q3 rebate total: <span id="rebate-total">$4,182.60</span></p>'
)


def selftest() -> int:
    """Offline: no network, no model. Proves the parsers work on a wall-shaped page."""
    assert hit_wall(SAMPLE), "wall marker not detected"
    assert find_total(SAMPLE) == "$4,182.60", find_total(SAMPLE)
    assert find_total('<b data-rebate-total="$99.00">x</b>') == "$99.00"
    assert find_total("<p>nothing here</p>") is None
    assert not hit_wall("<p>Rebate total: $1.00</p>")
    assert parse_action('sure!\n{"action": "call_human", "reason": "checkbox"}')["action"] == "call_human"
    assert parse_action("no json at all")["action"] == "read_page"
    assert "Verify you are human" in strip_html(SAMPLE)
    print("selftest OK — wall detection, deliverable extraction, action parsing")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    mode = ap.add_mutually_exclusive_group()
    mode.add_argument("--scripted", action="store_true", help="no LLM (default, demo fallback)")
    mode.add_argument("--claude", action="store_true", help="Claude brain via Bedrock Converse, no browser")
    mode.add_argument("--browser-use", action="store_true", help="browser-use + Claude on Bedrock")
    ap.add_argument("--selftest", action="store_true", help="offline parser check, no network")
    args = ap.parse_args()

    if args.selftest:
        return selftest()
    if args.claude:
        return run_claude()
    if args.browser_use:
        return run_browser_use()
    return run_scripted()


if __name__ == "__main__":
    raise SystemExit(main())

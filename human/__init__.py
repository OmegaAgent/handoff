"""`await human()` for AI agents — the client half of Handoff.

    import human
    human.configure(base_url="https://handoff.omegas.dev")

    # Blocks until a person clears the wall in a live view of your agent's browser.
    human.clear_wall(reason="A verification checkbox is blocking checkout",
                     live_view_url=..., resume_url=...)

    # Blocks until a person types an answer.
    address = human.ask("Which shipping address should I use?")

One file, standard library plus `requests` if present (falls back to urllib). Copy it into
your project or `pip install -e .`.
"""

from __future__ import annotations

import json
import os
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from typing import Optional

__all__ = [
    "configure",
    "ask",
    "clear_wall",
    "create_request",
    "Handoff",
    "HandoffTimeout",
    "HandoffError",
]

DEFAULT_BASE_URL = "https://handoff.omegas.dev"
_POLL_WAIT_S = 25  # server caps a single long-poll at 30s

_config = {"base_url": os.environ.get("HANDOFF_URL", DEFAULT_BASE_URL).rstrip("/")}


class HandoffError(RuntimeError):
    """The Handoff service could not be reached or refused the request."""


class HandoffTimeout(TimeoutError):
    """No human resolved the request before it expired."""


def configure(base_url: Optional[str] = None) -> None:
    """Point the SDK at a Handoff server. Defaults to $HANDOFF_URL."""
    if base_url:
        _config["base_url"] = base_url.rstrip("/")


def _request(method: str, path: str, body: Optional[dict] = None, timeout: float = 40) -> dict:
    url = _config["base_url"] + path
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("content-type", "application/json")
    req.add_header("accept", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return json.loads(resp.read() or b"{}")
    except urllib.error.HTTPError as exc:
        detail = exc.read().decode(errors="replace")[:300]
        raise HandoffError(f"{method} {path} -> {exc.code}: {detail}") from exc
    except urllib.error.URLError as exc:
        raise HandoffError(f"{method} {path} -> unreachable: {exc.reason}") from exc


@dataclass
class Handoff:
    """A pending ask. `wait()` blocks until a human settles it."""

    id: str
    page_url: str
    status: str = "pending"
    answer: Optional[str] = None
    cleared: bool = False

    def poll(self, wait: float = 0) -> dict:
        state = _request("GET", f"/v1/requests/{self.id}?wait={wait}", timeout=wait + 15)
        self.status = state.get("status", self.status)
        self.answer = state.get("answer")
        self.cleared = bool(state.get("cleared"))
        return state

    def wait(self, timeout_s: float = 600, on_tick=None) -> dict:
        """Block until resolved or expired. Raises HandoffTimeout if nobody answered."""
        deadline = time.time() + timeout_s
        while True:
            remaining = deadline - time.time()
            if remaining <= 0:
                raise HandoffTimeout(f"nobody resolved {self.id} within {timeout_s}s ({self.page_url})")
            state = self.poll(wait=min(_POLL_WAIT_S, max(1, int(remaining))))
            if state.get("status") == "resolved":
                return state
            if state.get("status") == "expired":
                raise HandoffTimeout(f"handoff {self.id} expired ({self.page_url})")
            if on_tick:
                on_tick(state)


def create_request(
    kind: str = "clear_wall",
    reason: str = "",
    question: Optional[str] = None,
    agent: str = "agent",
    live_view_url: Optional[str] = None,
    resume_url: Optional[str] = None,
    resume_token: Optional[str] = None,
    timeout_s: int = 600,
    page: bool = True,
) -> Handoff:
    """Create the request and page a human. Returns immediately; nothing blocks yet."""
    created = _request(
        "POST",
        "/v1/requests",
        {
            "kind": kind,
            "reason": reason,
            "question": question,
            "agent": agent,
            "live_view_url": live_view_url,
            "resume_url": resume_url,
            "resume_token": resume_token,
            "timeout_s": timeout_s,
            "page": page,
        },
        timeout=25,
    )
    return Handoff(id=created["id"], page_url=created["page_url"])


def clear_wall(
    reason: str,
    live_view_url: Optional[str] = None,
    resume_url: Optional[str] = None,
    resume_token: Optional[str] = None,
    timeout_s: int = 600,
    agent: str = "agent",
    page: bool = True,
    verbose: bool = True,
) -> bool:
    """Page a human to clear a wall your agent cannot pass. Blocks until they do.

    Returns True once a person says they cleared it. Raises HandoffTimeout if nobody came.
    """
    h = create_request(
        kind="clear_wall",
        reason=reason,
        agent=agent,
        live_view_url=live_view_url,
        resume_url=resume_url,
        resume_token=resume_token,
        timeout_s=timeout_s,
        page=page,
    )
    if verbose:
        print(f"[human] blocked: {reason}\n[human] a human is being paged: {h.page_url}", flush=True)
    h.wait(timeout_s=timeout_s)
    if verbose:
        print("[human] cleared by a human, resuming", flush=True)
    return True


def ask(
    question: str,
    timeout_s: int = 600,
    agent: str = "agent",
    live_view_url: Optional[str] = None,
    default: Optional[str] = None,
    page: bool = True,
    verbose: bool = True,
) -> str:
    """Ask a person a question and block until they answer. Returns their text.

    If `default` is given, a timeout returns it instead of raising.
    """
    h = create_request(
        kind="question",
        reason=question,
        question=question,
        agent=agent,
        live_view_url=live_view_url,
        timeout_s=timeout_s,
        page=page,
    )
    if verbose:
        print(f"[human] asking a person: {question}\n[human] {h.page_url}", flush=True)
    try:
        state = h.wait(timeout_s=timeout_s)
    except HandoffTimeout:
        if default is not None:
            if verbose:
                print(f"[human] nobody answered, using default: {default}", flush=True)
            return default
        raise
    answer = state.get("answer") or ""
    if verbose:
        print(f"[human] answered: {answer}", flush=True)
    return answer

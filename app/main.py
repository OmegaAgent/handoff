"""Handoff — the hosted half of the `await human()` API.

An agent POSTs a request when it hits a wall it cannot pass. The call blocks on a
long-poll while a real human is paged by phone, opens the public handoff page, clears
the wall (or types an answer) and resolves it. Then the agent's call returns.

State is in-process on purpose: one machine, no coordination, a hack deadline.
"""

from __future__ import annotations

import asyncio
import os
import secrets
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Literal, Optional

import httpx
from fastapi import FastAPI, HTTPException
from fastapi.responses import HTMLResponse, JSONResponse, RedirectResponse
from pydantic import BaseModel

from app.page import render_landing, render_request_page

# The one-click demo is built from two modules that may not exist yet during a partial
# deploy. A missing demo must never stop the API that a blocked agent is long-polling.
try:
    from app.runner import DemoRun, start_run
    from app.demo_page import render_demo_page

    DEMO_READY = True
    DEMO_IMPORT_ERROR = ""
except Exception as _exc:  # noqa: BLE001
    DEMO_READY = False
    DEMO_IMPORT_ERROR = f"{type(_exc).__name__}: {_exc}"

PUBLIC_URL = os.environ.get("HANDOFF_PUBLIC_URL", "http://localhost:8080").rstrip("/")
REPO_ROOT = Path(__file__).resolve().parent.parent

RETELL_API_KEY = os.environ.get("RETELL_API_KEY", "")
RETELL_FROM_NUMBER = os.environ.get("RETELL_FROM_NUMBER", "")
RETELL_AGENT_ID = os.environ.get("RETELL_AGENT_ID", "")
RETELL_LLM_ID = os.environ.get("RETELL_LLM_ID", "")
# The human on call. TWILIO_TO_NUMBER is kept as an alias: it is where the owner's
# verified personal number already lived before the Twilio path was abandoned.
TO_NUMBER = os.environ.get("HANDOFF_TO_NUMBER") or os.environ.get("TWILIO_TO_NUMBER", "")

MAX_WAIT_S = 30.0

app = FastAPI(title="Handoff", docs_url="/api", redoc_url=None)


# --------------------------------------------------------------------------- state


@dataclass
class HandoffRequest:
    id: str
    kind: Literal["clear_wall", "question"]
    reason: str
    question: Optional[str]
    agent: str
    live_view_url: Optional[str]
    resume_url: Optional[str]
    resume_token: Optional[str]
    timeout_s: int
    created_at: float
    status: str = "pending"
    answer: Optional[str] = None
    cleared: bool = False
    resolved_at: Optional[float] = None
    resolved_by: Optional[str] = None
    paged: Optional[str] = None  # "ringing" | "failed: ..." | "skipped"
    resume_posted: Optional[bool] = None
    _event: asyncio.Event = field(default_factory=asyncio.Event, repr=False)

    @property
    def expired(self) -> bool:
        return self.status == "pending" and (time.time() - self.created_at) > self.timeout_s

    def settle(self) -> None:
        """Flip a timed-out request to expired and wake anyone waiting."""
        if self.expired:
            self.status = "expired"
            self.resolved_at = time.time()
            self._event.set()

    def public(self) -> dict:
        self.settle()
        return {
            "id": self.id,
            "status": self.status,
            "kind": self.kind,
            "reason": self.reason,
            "question": self.question,
            "agent": self.agent,
            "answer": self.answer,
            "cleared": self.cleared,
            "live_view_url": self.live_view_url,
            "has_resume": bool(self.resume_url),
            "resume_posted": self.resume_posted,
            "paged": self.paged,
            "timeout_s": self.timeout_s,
            "created_at": self.created_at,
            "resolved_at": self.resolved_at,
            "resolved_by": self.resolved_by,
            "page_url": f"{PUBLIC_URL}/r/{self.id}",
            "age_s": round(time.time() - self.created_at, 1),
        }


REQUESTS: dict[str, HandoffRequest] = {}


def _get(request_id: str) -> HandoffRequest:
    req = REQUESTS.get(request_id)
    if req is None:
        raise HTTPException(status_code=404, detail="no such request")
    return req


# ------------------------------------------------------------------------- paging


async def page_human(req: HandoffRequest) -> None:
    """Make a real phone ring. Never let a paging failure break the request."""
    if not (RETELL_API_KEY and RETELL_FROM_NUMBER and RETELL_AGENT_ID and TO_NUMBER):
        req.paged = "skipped: missing Retell config"
        return

    what = req.question if req.kind == "question" else req.reason
    spoken = (
        f"Hi, this is Handoff calling on behalf of your agent {req.agent}. "
        f"It is blocked and needs a human. {what}. "
        f"Open the handoff link we generated to take over the browser and clear it. "
        f"The agent is waiting on the line."
    )
    headers = {"Authorization": f"Bearer {RETELL_API_KEY}"}
    try:
        async with httpx.AsyncClient(timeout=20) as client:
            if RETELL_LLM_ID:
                # The agent's reason becomes the first thing the voice says.
                await client.patch(
                    f"https://api.retellai.com/update-retell-llm/{RETELL_LLM_ID}",
                    headers=headers,
                    json={"begin_message": spoken},
                )
            resp = await client.post(
                "https://api.retellai.com/v2/create-phone-call",
                headers=headers,
                json={
                    "from_number": RETELL_FROM_NUMBER,
                    "to_number": TO_NUMBER,
                    "override_agent_id": RETELL_AGENT_ID,
                    "metadata": {"handoff_id": req.id, "page_url": f"{PUBLIC_URL}/r/{req.id}"},
                },
            )
        req.paged = "ringing" if resp.status_code < 300 else f"failed: {resp.status_code} {resp.text[:160]}"
    except Exception as exc:  # noqa: BLE001 - paging is best effort by design
        req.paged = f"failed: {type(exc).__name__}: {exc}"[:200]


async def post_resume(req: HandoffRequest) -> bool:
    """Tell the agent's browser sandbox the wall is gone.

    This is the path that did not exist before tonight: clearance used to be inferred
    only from the page url or title moving, which silently misses an in-place wall
    like a verification checkbox.
    """
    if not req.resume_url:
        return False
    headers = {}
    if req.resume_token:
        headers["Authorization"] = f"Bearer {req.resume_token}"
    try:
        async with httpx.AsyncClient(timeout=15) as client:
            resp = await client.post(
                req.resume_url,
                headers=headers,
                json={"source": "handoff", "handoff_id": req.id, "answer": req.answer},
            )
        return resp.status_code < 400
    except Exception:  # noqa: BLE001
        return False


# --------------------------------------------------------------------------- api


class CreateBody(BaseModel):
    kind: Literal["clear_wall", "question"] = "clear_wall"
    reason: str = ""
    question: Optional[str] = None
    agent: str = "agent"
    live_view_url: Optional[str] = None
    resume_url: Optional[str] = None
    resume_token: Optional[str] = None
    timeout_s: int = 600
    page: bool = True


class ResolveBody(BaseModel):
    answer: Optional[str] = None
    cleared: bool = True
    by: str = "human"


@app.get("/healthz")
async def healthz() -> dict:
    pending = sum(1 for r in REQUESTS.values() if r.status == "pending" and not r.expired)
    return {"ok": True, "requests": len(REQUESTS), "pending": pending}


@app.post("/v1/requests", status_code=201)
async def create_request(body: CreateBody) -> JSONResponse:
    if body.kind == "question" and not (body.question or body.reason):
        raise HTTPException(status_code=422, detail="question requires question or reason")

    req = HandoffRequest(
        id=secrets.token_urlsafe(16),
        kind=body.kind,
        reason=body.reason or (body.question or ""),
        question=body.question,
        agent=body.agent,
        live_view_url=body.live_view_url,
        resume_url=body.resume_url,
        resume_token=body.resume_token,
        timeout_s=max(10, min(body.timeout_s, 3600)),
        created_at=time.time(),
    )
    REQUESTS[req.id] = req

    if body.page:
        asyncio.create_task(page_human(req))
    else:
        req.paged = "skipped: page=false"

    return JSONResponse(
        status_code=201,
        content={"id": req.id, "page_url": f"{PUBLIC_URL}/r/{req.id}", "status": "pending"},
    )


@app.get("/v1/requests/{request_id}")
async def get_request(request_id: str, wait: float = 0) -> dict:
    req = _get(request_id)
    req.settle()
    if req.status == "pending" and wait > 0:
        try:
            await asyncio.wait_for(req._event.wait(), timeout=min(wait, MAX_WAIT_S))
        except asyncio.TimeoutError:
            pass
        req.settle()
    return req.public()


@app.post("/v1/requests/{request_id}/resolve")
async def resolve_request(request_id: str, body: ResolveBody) -> dict:
    req = _get(request_id)
    req.settle()
    if req.status != "pending":
        return {"ok": False, "status": req.status, "detail": "already settled"}

    req.answer = body.answer
    req.cleared = body.cleared
    req.resolved_by = body.by
    req.resolved_at = time.time()
    req.status = "resolved"

    req.resume_posted = await post_resume(req)
    req._event.set()  # the blocked agent's long-poll returns now
    return {"ok": True, "resume_posted": req.resume_posted}


@app.get("/v1/requests")
async def list_requests() -> dict:
    for r in REQUESTS.values():
        r.settle()
    newest = sorted(REQUESTS.values(), key=lambda r: r.created_at, reverse=True)[:25]
    return {"requests": [r.public() for r in newest]}


# --------------------------------------------------------------------------- pages


@app.get("/", response_class=HTMLResponse)
async def landing() -> HTMLResponse:
    for r in REQUESTS.values():
        r.settle()
    recent = sorted(REQUESTS.values(), key=lambda r: r.created_at, reverse=True)[:8]
    return HTMLResponse(render_landing([r.public() for r in recent]))


@app.get("/r/{request_id}", response_class=HTMLResponse)
async def request_page(request_id: str) -> HTMLResponse:
    req = _get(request_id)
    return HTMLResponse(render_request_page(req.public()))


@app.get("/try")
async def try_it() -> RedirectResponse:
    """Self-serve: mint a demo handoff and drop the visitor on the page a paged human sees.

    Paging is off here on purpose. The number on the other end belongs to a person, and a
    public button that rings it is a public button for waking someone up.
    """
    req = HandoffRequest(
        id=secrets.token_urlsafe(16),
        kind="clear_wall",
        reason="A human-verification checkbox is blocking the Northwind partner portal",
        question=None,
        agent="demo-agent",
        live_view_url=os.environ.get("HANDOFF_DEMO_LIVE_VIEW") or f"{PUBLIC_URL}/demo/wall",
        resume_url=None,
        resume_token=None,
        timeout_s=1800,
        created_at=time.time(),
    )
    req.paged = "skipped: self-serve demo does not ring a real phone"
    REQUESTS[req.id] = req
    return RedirectResponse(url=f"/r/{req.id}", status_code=303)


def _statement_for(handoff_id: str) -> dict:
    """The demo payoff, or an HTTPException if no human has cleared this handoff."""
    req = REQUESTS.get(handoff_id)
    if req is None:
        raise HTTPException(status_code=403, detail="unknown handoff; no human has cleared this session")
    req.settle()
    if req.status != "resolved" or not req.cleared:
        raise HTTPException(status_code=403, detail=f"handoff is {req.status}; no human has cleared this session")
    return {
        "reference": "NWS-Q3-REBATE-48,210.00",
        "total": "48,210.00",
        "currency": "USD",
        "period": "Q3",
        "cleared_by": req.resolved_by,
        "cleared_at": req.resolved_at,
    }


# ------------------------------------------------------------------ one-click demo

DEMO_RUNS: dict = {}
DEMO_MAX_CONCURRENT = int(os.environ.get("DEMO_MAX_CONCURRENT", "4"))
DEMO_RUN_MAX_AGE_S = float(os.environ.get("DEMO_RUN_MAX_AGE_S", "900"))


async def _demo_create_handoff(
    reason: str, live_view_url: Optional[str], timeout_s: int, page: bool
) -> tuple[str, str]:
    """Mint a handoff in-process, so the runner needs no HTTP round trip to ourselves."""
    req = HandoffRequest(
        id=secrets.token_urlsafe(16),
        kind="clear_wall",
        reason=reason,
        question=None,
        agent="demo-agent",
        live_view_url=live_view_url,
        resume_url=None,
        resume_token=None,
        timeout_s=timeout_s,
        created_at=time.time(),
    )
    REQUESTS[req.id] = req
    if page:
        asyncio.create_task(page_human(req))
    else:
        req.paged = "skipped: page=false"
    return req.id, f"{PUBLIC_URL}/r/{req.id}"


async def _demo_get_handoff(handoff_id: str) -> dict:
    return _get(handoff_id).public()


async def _demo_get_statement(handoff_id: str) -> dict:
    return _statement_for(handoff_id)


@app.get("/demo", response_class=HTMLResponse)
async def demo_page() -> HTMLResponse:
    if not DEMO_READY:
        raise HTTPException(status_code=503, detail=f"demo not built: {DEMO_IMPORT_ERROR}")
    return HTMLResponse(render_demo_page())


@app.post("/demo/run", status_code=201)
async def demo_run_start() -> dict:
    """Start a real run. This rings a real phone, which the page says before you press Send."""
    if not DEMO_READY:
        raise HTTPException(status_code=503, detail=f"demo not built: {DEMO_IMPORT_ERROR}")

    # A run nobody finishes would otherwise hold a slot until its handoff times out, so one
    # abandoned visitor locks out the next. Count only runs that are genuinely still alive.
    now = time.time()
    live = 0
    for r in DEMO_RUNS.values():
        if r.status not in ("running", "blocked"):
            continue
        if now - r.created_at > DEMO_RUN_MAX_AGE_S:
            r.status = "failed"
            r.error = "This run was abandoned and timed out."
            continue
        handoff = REQUESTS.get(r.handoff_id) if r.handoff_id else None
        if handoff is not None:
            handoff.settle()
            if handoff.status != "pending":
                continue  # settled elsewhere; the runner will notice on its next poll
        live += 1

    if live >= DEMO_MAX_CONCURRENT:
        raise HTTPException(
            status_code=429,
            detail="Someone else is running the demo right now. Try again in a minute.",
        )

    run = DemoRun(
        id=secrets.token_urlsafe(12),
        status="running",
        steps=[],
        handoff_id=None,
        page_url=None,
        live_view_url=None,
        deliverable=None,
        error=None,
        mode="http",
        created_at=time.time(),
    )
    DEMO_RUNS[run.id] = run
    asyncio.create_task(
        start_run(
            run,
            create_handoff=_demo_create_handoff,
            get_handoff=_demo_get_handoff,
            get_statement=_demo_get_statement,
            page=True,
        )
    )
    return {"id": run.id}


@app.get("/demo/run/{run_id}")
async def demo_run_state(run_id: str) -> dict:
    run = DEMO_RUNS.get(run_id)
    if run is None:
        raise HTTPException(status_code=404, detail="no such run")
    return {
        "id": run.id,
        "status": run.status,
        "steps": run.steps,
        "handoff_id": run.handoff_id,
        "page_url": run.page_url,
        "live_view_url": run.live_view_url,
        "deliverable": run.deliverable,
        "error": run.error,
        "mode": run.mode,
        "age_s": round(time.time() - run.created_at, 1),
    }


@app.get("/demo/statement")
async def demo_statement(handoff: str = "") -> dict:
    """The demo's payoff, gated on a real person having cleared a real handoff.

    Serving the wall's HTML also serves the numbers inside it, so an agent with no
    browser could regex the total straight out of the page and never need anyone. That
    would make the demo a lie. The deliverable lives here instead, behind a check that
    only a human resolving this exact handoff id can satisfy: the agent holds the id but
    has no way to resolve it itself.
    """
    return _statement_for(handoff)


@app.get("/demo/wall", response_class=HTMLResponse)
async def demo_wall() -> HTMLResponse:
    path = REPO_ROOT / "demo" / "wall" / "index.html"
    if not path.exists():
        raise HTTPException(status_code=404, detail="demo wall not built")
    return HTMLResponse(path.read_text())

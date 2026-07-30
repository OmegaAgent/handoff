"""A minimal in-memory Handoff server, for testing this SDK over a real socket.

**This is a test double, not a conforming implementation.** It implements the handful of
operations these tests exercise and none of the guarantees that make a server conformant: no
tenancy, no authority evaluation, no receipt chain, no storage-level immutability, no delivery
ladder. The reference server is `core/`; the conformance suite is `conformance/`.

It exists so the resumable-client tests run against real HTTP with a real long poll, which is
the only way to test that killing a client mid-wait loses nothing.
"""

from __future__ import annotations

import json
import random
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any, Optional
from urllib.parse import parse_qs, unquote, urlparse

_CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"


def _ulid(prefix: str) -> str:
    value = (int(time.time() * 1000) << 80) | random.getrandbits(80)
    out = []
    for _ in range(26):
        out.append(_CROCKFORD[value & 0x1F])
        value >>= 5
    return prefix + "_" + "".join(reversed(out))


class State:
    """Everything the double remembers. The point of the exercise: the wait lives here, not in
    any client process."""

    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.requests: dict[str, dict[str, Any]] = {}
        self.signals: dict[str, dict[str, Any]] = {}
        self.by_waiter: dict[str, list[str]] = {}
        self.sequence: dict[str, int] = {}
        self.authorizations: dict[str, dict[str, Any]] = {}
        self.redemptions: dict[str, dict[str, str]] = {}
        self.reattach_calls: list[str] = []
        self.ack_calls: list[tuple[str, bool, Optional[str]]] = []

    # -- protocol operations ----------------------------------------------------------------

    def raise_request(self, body: dict[str, Any]) -> dict[str, Any]:
        request_id = _ulid("req")
        request = {
            "id": request_id,
            "state": "pending",
            "version": 1,
            "org_id": "org_TEST",
            "waiter_ref": body["waiter_ref"],
            "prompt": body["prompt"],
            "requires": body["requires"],
            "created_at": "2026-07-30T14:02:11Z",
            "surface_url": f"https://handoff.test/requests/{request_id}",
            "deliveries": [],
            "receipt": None,
            "authorization": None,
            "waiter": {"state": "armed", "liveness": body.get("liveness", "durable")},
            "metadata": body.get("metadata"),
        }
        with self.lock:
            self.requests[request_id] = request
            self.by_waiter.setdefault(body["waiter_ref"], [])
        return request

    def enqueue(self, request_id: str, type: str, decision: Optional[dict[str, Any]]) -> dict[str, Any]:
        """Signals are a queue, not a flag (W2): a nudge never overwrites a later terminal signal."""
        with self.lock:
            request = self.requests[request_id]
            waiter_ref = request["waiter_ref"]
            self.sequence[waiter_ref] = self.sequence.get(waiter_ref, 0) + 1
            signal = {
                "id": _ulid("sig"),
                "request_id": request_id,
                "waiter_ref": waiter_ref,
                "type": type,
                "sequence": self.sequence[waiter_ref],
                "resume_token": _ulid("rt"),
                "decision": decision,
                "resume_ref": request.get("resume_ref"),
                "resume_payload": request.get("resume_payload"),
                "attempts": 1,
                "created_at": "2026-07-30T14:07:44Z",
                "acked_at": None,
            }
            self.signals[signal["id"]] = signal
            self.by_waiter.setdefault(waiter_ref, []).append(signal["id"])
            return signal

    def answer(self, request_id: str, body: dict[str, Any]) -> dict[str, Any]:
        with self.lock:
            request = self.requests[request_id]
            if request["state"] != "pending":
                raise Conflict("already_answered", f"{request_id} is {request['state']}")
            request["state"] = "answered"
            request["answered_at"] = "2026-07-30T14:07:44Z"
            receipt_id = _ulid("rcpt")
            authorization_id = _ulid("auth")
            self.authorizations[authorization_id] = {
                "id": authorization_id,
                "receipt_id": receipt_id,
                "request_id": request_id,
                "grants": dict(body.get("values") or {}),
                "single_use": True,
                "expires_at": "2026-07-31T14:07:44Z",
            }
        self.enqueue(
            request_id,
            "answered",
            {
                "outcome": "answered",
                "values": body.get("values") or {},
                "source": "human",
                "effective": None,
                "receipt_id": receipt_id,
                "authorization_id": authorization_id,
                "superseded_by": None,
            },
        )
        return {
            "request": {"id": request_id, "state": "answered", "answered_at": "2026-07-30T14:07:44Z"},
            "receipt": {"id": receipt_id, "digest": "sha256:" + "0" * 64},
            "authorization": {"id": authorization_id, "single_use": True, "expires_at": "2026-07-31T14:07:44Z"},
        }

    def unacked(self, waiter_ref: str) -> list[dict[str, Any]]:
        with self.lock:
            return [
                self.signals[sid]
                for sid in self.by_waiter.get(waiter_ref, [])
                if self.signals[sid]["acked_at"] is None
            ]

    def ack(self, signal_id: str, body: dict[str, Any]) -> dict[str, Any]:
        with self.lock:
            signal = self.signals.get(signal_id)
            if signal is None:
                raise NotFound("signal_not_found", f"{signal_id} does not exist")
            if body.get("resume_token") != signal["resume_token"]:
                raise Forbidden("insufficient_scope", "resume_token does not match")
            first = signal["acked_at"] is None
            self.ack_calls.append((signal_id, bool(body.get("applied")), body.get("reason")))
            if first:
                signal["acked_at"] = "2026-07-30T14:07:50Z"
            return {"acked_at": signal["acked_at"], "first_ack": first}

    def redeem(self, authorization_id: str, body: dict[str, Any]) -> dict[str, Any]:
        effect_key = body["effect_key"]
        with self.lock:
            if authorization_id not in self.authorizations:
                raise NotFound("authorization_not_found", "no such authorization")
            spent = self.redemptions.setdefault(authorization_id, {})
            if effect_key in spent:
                return {"redeemed_at": spent[effect_key], "first_redemption": False}
            if spent and self.authorizations[authorization_id]["single_use"]:
                raise Conflict("authorization_spent", "single-use authorization already spent")
            spent[effect_key] = "2026-07-30T14:07:46Z"
            return {"redeemed_at": spent[effect_key], "first_redemption": True}


class HttpError(Exception):
    status = 400

    def __init__(self, code: str, message: str):
        super().__init__(message)
        self.code = code
        self.message = message


class NotFound(HttpError):
    status = 404


class Conflict(HttpError):
    status = 409


class Forbidden(HttpError):
    status = 403


class _Handler(BaseHTTPRequestHandler):
    state: State

    def log_message(self, *args: Any) -> None:  # keep the test output readable
        pass

    def _send(self, status: int, payload: Any = None) -> None:
        body = b"" if payload is None else json.dumps(payload).encode()
        self.send_response(status)
        if body:
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        if body:
            self.wfile.write(body)

    def _read(self) -> dict[str, Any]:
        length = int(self.headers.get("Content-Length") or 0)
        return json.loads(self.rfile.read(length) or b"{}")

    def _route(self, method: str) -> None:
        parsed = urlparse(self.path)
        parts = [unquote(p) for p in parsed.path.strip("/").split("/")]
        query = parse_qs(parsed.query)
        try:
            if method == "GET" and parts == ["v1", "meta"]:
                return self._send(
                    200,
                    {
                        "protocol_version": "0.1",
                        "conformance_level": 1,
                        "extensions": [],
                        "field_types": ["choice", "text", "number", "boolean", "secret", "attestation"],
                        "capability_types": ["interactive_surface"],
                        "max_wait_seconds": 30,
                    },
                )
            if method == "POST" and parts == ["v1", "requests"]:
                return self._send(201, self.state.raise_request(self._read()))
            if method == "GET" and len(parts) == 3 and parts[:2] == ["v1", "requests"]:
                return self._send(200, self.state.requests[parts[2]])
            if method == "POST" and len(parts) == 4 and parts[3] == "answer":
                return self._send(200, self.state.answer(parts[2], self._read()))
            if method == "GET" and len(parts) == 4 and parts[1] == "waiters" and parts[3] == "signals":
                wait = int((query.get("wait") or ["0"])[0])
                deadline = time.time() + wait
                while True:
                    pending = self.state.unacked(parts[2])
                    if pending:
                        return self._send(200, {"data": pending, "has_more": False})
                    if time.time() >= deadline:
                        return self._send(204)
                    time.sleep(0.05)
            if method == "POST" and len(parts) == 4 and parts[1] == "waiters" and parts[3] == "reattach":
                self.state.reattach_calls.append(parts[2])
                return self._send(
                    200,
                    {
                        "waiter_ref": parts[2],
                        "state": "signalled" if self.state.unacked(parts[2]) else "armed",
                        "open_requests": [
                            r["id"]
                            for r in self.state.requests.values()
                            if r["waiter_ref"] == parts[2] and r["state"] == "pending"
                        ],
                        "signals": self.state.unacked(parts[2]),
                    },
                )
            if method == "POST" and len(parts) == 4 and parts[1] == "signals" and parts[3] == "ack":
                return self._send(200, self.state.ack(parts[2], self._read()))
            if method == "POST" and len(parts) == 4 and parts[1] == "authorizations" and parts[3] == "redeem":
                return self._send(200, self.state.redeem(parts[2], self._read()))
            return self._send(404, {"error": {"code": "request_not_found", "message": self.path}})
        except HttpError as exc:
            return self._send(exc.status, {"error": {"code": exc.code, "message": exc.message}})
        except KeyError as exc:
            return self._send(404, {"error": {"code": "request_not_found", "message": str(exc)}})

    def do_GET(self) -> None:
        self._route("GET")

    def do_POST(self) -> None:
        self._route("POST")


class FakeServer:
    """Runs the double on a free port for the life of a ``with`` block."""

    def __init__(self) -> None:
        self.state = State()
        handler = type("_Bound", (_Handler,), {"state": self.state})
        self.httpd = ThreadingHTTPServer(("127.0.0.1", 0), handler)
        self.httpd.daemon_threads = True
        self.thread = threading.Thread(target=self.httpd.serve_forever, daemon=True)

    @property
    def base_url(self) -> str:
        host, port = self.httpd.server_address[:2]
        return f"http://{host}:{port}"

    def __enter__(self) -> "FakeServer":
        self.thread.start()
        return self

    def __exit__(self, *exc: Any) -> None:
        self.httpd.shutdown()
        self.httpd.server_close()

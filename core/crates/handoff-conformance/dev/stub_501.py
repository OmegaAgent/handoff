#!/usr/bin/env python3
"""A Handoff server that implements nothing.

Every route answers `501 Not Implemented` with a well-formed error envelope. It exists for one
purpose: to prove the conformance suite fails loudly. A suite that cannot report `0/22 passing`
against a server that does nothing is not measuring anything, and would report the same green
against a server that does the wrong thing.

    python3 core/crates/handoff-conformance/dev/stub_501.py --port 8080
    cargo run -p handoff-conformance -- --base-url http://127.0.0.1:8080/v1
"""

import argparse
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


class Stub(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def respond(self):
        # Drain the body so keep-alive stays in sync; the content is irrelevant to a stub.
        length = int(self.headers.get("Content-Length") or 0)
        if length:
            self.rfile.read(length)

        body = json.dumps(
            {
                "error": {
                    "code": "invalid_request",
                    "message": (
                        f"{self.command} {self.path} is not implemented. This is the conformance "
                        "suite's stub server: it exists to prove the suite fails."
                    ),
                }
            }
        ).encode()

        self.send_response(501)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    do_GET = do_POST = do_PUT = do_PATCH = do_DELETE = do_HEAD = respond

    def log_message(self, *args):
        pass


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="A Handoff server that implements nothing.")
    parser.add_argument("--port", type=int, default=8080)
    parser.add_argument("--host", default="127.0.0.1")
    args = parser.parse_args()

    server = ThreadingHTTPServer((args.host, args.port), Stub)
    print(f"stub 501 server on http://{args.host}:{args.port}", flush=True)
    server.serve_forever()

"""Stdlib HTTP surface for genaid-audio liveness and readiness.

Importing this module does not bind a socket. ``serve_loopback`` is the
opt-in listener. ``/health`` is liveness and stays 200 when the model is
unloaded. ``/ready`` is the lazy-load readiness probe.
"""

from __future__ import annotations

import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from gen_audio.serve import ROUTE_PREFIX, health_payload, ready_payload


def dispatch(method: str, path: str, *, model_loaded: bool = False) -> tuple[int, dict[str, object]]:
    """Return ``(status, json)`` for one request. Unknown routes are 404."""
    if method != "GET":
        return 405, {"error": "method not allowed"}
    route = path.split("?", 1)[0].rstrip("/") or "/"
    if route == f"{ROUTE_PREFIX}/health":
        body: dict[str, object] = dict(health_payload())
        body["model_loaded"] = model_loaded
        return 200, body
    if route == f"{ROUTE_PREFIX}/ready":
        return 200, dict(ready_payload(model_loaded=model_loaded))
    return 404, {"error": "not found"}


def serve_loopback(host: str = "127.0.0.1", port: int = 8002) -> ThreadingHTTPServer:
    """Bind a loopback listener. The caller starts ``serve_forever``."""

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self) -> None:  # noqa: N802
            status, payload = dispatch("GET", self.path)
            raw = json.dumps(payload).encode("utf-8")
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)

        def log_message(self, fmt: str, *args: object) -> None:
            return

    return ThreadingHTTPServer((host, port), Handler)

"""scripts/mcp-call.ps1 -OutFile, against an in-test local JSON-RPC listener.

The listener answers initialize as Gen-Audio and tools/call with a canned
body, so the script's own discovery, file write and exit code are what run.
Needs PowerShell (pwsh or Windows PowerShell); skips without one.
"""

from __future__ import annotations

import json
import shutil
import subprocess
import threading
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parents[1]
MCP_CALL = REPO / "scripts" / "mcp-call.ps1"
SHELL = shutil.which("pwsh") or shutil.which("powershell")
needs_shell = pytest.mark.skipif(SHELL is None, reason="needs PowerShell")

INIT = {"jsonrpc": "2.0", "id": 1, "result": {"protocolVersion": "2025-03-26", "serverInfo": {"name": "gen-audio", "version": "t"}}}
# Non-ASCII on purpose: a BOM or a non-UTF-8 write shows up in these bytes.
OK = {"jsonrpc": "2.0", "id": 2, "result": {"content": [{"type": "text", "text": "glyph \u2803\u2817 ok"}], "isError": False}}
ERROR = {"jsonrpc": "2.0", "id": 2, "error": {"code": -32602, "message": "unknown tool \u2803"}}


@contextmanager
def _server(tools_call: dict):
    body = {"initialize": json.dumps(INIT).encode("utf-8"), "tools/call": json.dumps(tools_call, ensure_ascii=False).encode("utf-8")}

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):  # noqa: N802 (http.server API)
            request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            data = body[request["method"]]
            self.send_response(200)
            self.send_header("Content-Type", "application/json; charset=utf-8")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def log_message(self, *args):
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield server.server_address[1], body["tools/call"]
    finally:
        server.shutdown()
        server.server_close()


def _call(port: int, *extra: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run([SHELL, "-NoProfile", "-NonInteractive", "-File", str(MCP_CALL), "-Tool", "viewport_get", "-Port", str(port), *extra],
                          capture_output=True, text=True, encoding="utf-8", timeout=120)


def _convertfrom_json(path: Path) -> subprocess.CompletedProcess[str]:
    command = f"$ErrorActionPreference = 'Stop'; (Get-Content -LiteralPath '{path}' -Raw -Encoding UTF8 | ConvertFrom-Json | ConvertTo-Json -Depth 32 -Compress)"
    return subprocess.run([SHELL, "-NoProfile", "-NonInteractive", "-Command", command], capture_output=True, text=True, encoding="utf-8",
                          timeout=120)


@needs_shell
def test_outfile_holds_only_the_ok_body_as_utf8_without_bom(tmp_path):
    out = tmp_path / "reply.json"
    with _server(OK) as (port, sent):
        run = _call(port, "-OutFile", str(out))
    assert run.returncode == 0, run.stdout + run.stderr
    data = out.read_bytes()
    assert data == sent, "the file is the response body, byte for byte"
    assert not data.startswith(b"\xef\xbb\xbf")
    with out.open(encoding="utf-8") as handle:  # plain utf-8, so a BOM would make json.load fail
        assert json.load(handle) == OK
    parsed = _convertfrom_json(out)
    assert parsed.returncode == 0, parsed.stderr
    assert json.loads(parsed.stdout) == OK
    assert run.stdout.strip() == "", "with -OutFile nothing goes to stdout"
    assert f"MCP addr=127.0.0.1:{port} tool=viewport_get" in run.stderr and "exit=0" in run.stderr
    assert "MCP addr=" not in data.decode("utf-8")


@needs_shell
def test_outfile_on_a_jsonrpc_error_is_still_parseable_json_and_exits_nonzero(tmp_path):
    out = tmp_path / "reply.json"
    with _server(ERROR) as (port, sent):
        run = _call(port, "-OutFile", str(out))
    assert run.returncode != 0, run.stdout + run.stderr
    assert out.read_bytes() == sent and not sent.startswith(b"\xef\xbb\xbf")
    with out.open(encoding="utf-8") as handle:
        assert json.load(handle)["error"]["code"] == -32602
    parsed = _convertfrom_json(out)
    assert parsed.returncode == 0, parsed.stderr
    assert json.loads(parsed.stdout) == ERROR
    assert run.stdout.strip() == ""
    assert "JSON-RPC error -32602" in run.stderr and "exit=1" in run.stderr


@needs_shell
def test_without_outfile_header_and_body_go_to_stdout_as_before(tmp_path):
    with _server(OK) as (port, sent):
        ok = _call(port)
    assert ok.returncode == 0, ok.stdout + ok.stderr
    assert ok.stdout.splitlines()[:2] == [f"MCP addr=127.0.0.1:{port} tool=viewport_get", sent.decode("utf-8")]
    assert "exit=" not in ok.stdout + ok.stderr
    with _server(ERROR) as (port, sent):
        bad = _call(port)
    assert bad.returncode == 1 and sent.decode("utf-8") in bad.stdout and "JSON-RPC error -32602" in bad.stderr
    assert not list(tmp_path.iterdir()), "no file is written without -OutFile"

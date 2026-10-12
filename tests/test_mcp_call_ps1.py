"""scripts/mcp-call.ps1 -OutFile, against an in-test local JSON-RPC listener.

The listener answers initialize as Gen-Audio and tools/call with a canned
body, so the script's own discovery, file write and exit code are what run.
Needs PowerShell (pwsh or Windows PowerShell); skips without one.
"""

from __future__ import annotations

import json
import importlib.util
import os
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
TOKEN = "ab" * 32

INIT = {"jsonrpc": "2.0", "id": 1, "result": {"protocolVersion": "2025-03-26", "serverInfo": {"name": "gen-audio", "version": "t"}}}
# Non-ASCII on purpose: a BOM or a non-UTF-8 write shows up in these bytes.
OK = {"jsonrpc": "2.0", "id": 2, "result": {"content": [{"type": "text", "text": "glyph \u2803\u2817 ok"}], "isError": False}}
ERROR = {"jsonrpc": "2.0", "id": 2, "error": {"code": -32602, "message": "unknown tool \u2803"}}


@contextmanager
def _server(tools_call: dict, seen: list | None = None, redirect: bool = False):
    body = {"initialize": json.dumps(INIT).encode("utf-8"), "tools/call": json.dumps(tools_call, ensure_ascii=False).encode("utf-8")}

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):  # noqa: N802 (http.server API)
            request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            if seen is not None:
                seen.append((self.path, request["method"], self.headers.get("Authorization")))
            if self.headers.get("Authorization") != "Bearer " + TOKEN:
                self.send_error(401, "bearer required")
                return
            if redirect:
                self.send_response(307)
                self.send_header("Location", "/unexpected")
                self.send_header("Content-Length", "0")
                self.end_headers()
                return
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


def _call(port: int, *extra: str, token: str | None = TOKEN, shell: str | None = None, env_extra: dict | None = None) -> subprocess.CompletedProcess[str]:
    env = {key: value for key, value in os.environ.items() if key not in {
        "GEN_AUDIO_MCP_TOKEN", "GEN_AUDIO_MCP_ADDR", "GEN_AUDIO_MCP_CONNECTION_FILE",
    }}
    if token is not None:
        env["GEN_AUDIO_MCP_TOKEN"] = token
    env.update(env_extra or {})
    return subprocess.run([shell or SHELL, "-NoProfile", "-NonInteractive", "-File", str(MCP_CALL), "-Tool", "viewport_get", "-Port", str(port), *extra],
                          capture_output=True, text=True, encoding="utf-8", timeout=120, env=env)


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


@pytest.mark.parametrize("shell_name", ["powershell", "pwsh"])
def test_authentication_uses_environment_or_owned_export_without_leaking(tmp_path, shell_name):
    shell = shutil.which(shell_name)
    if shell is None:
        pytest.skip(f"needs {shell_name}")
    seen = []
    with _server(OK, seen) as (port, _):
        env_run = _call(port, "-OutFile", str(tmp_path / "env.json"), shell=shell)
        config = tmp_path / "connection.json"
        config.write_text(json.dumps({"addr": f"127.0.0.1:{port}", "token": TOKEN, "handshake_ok": True}), encoding="utf-8")
        config_run = _call(0, "-ConnectionFile", str(config), "-OutFile", str(tmp_path / "file.json"), shell=shell, token=None)
        env_config_run = _call(0, "-OutFile", str(tmp_path / "env-file.json"), shell=shell, token=None,
                              env_extra={"GEN_AUDIO_MCP_CONNECTION_FILE": str(config)})
    for run in (env_run, config_run, env_config_run):
        assert run.returncode == 0, run.stdout + run.stderr
        assert TOKEN not in run.stdout + run.stderr
    assert seen == [("/mcp", method, "Bearer " + TOKEN) for method in ["initialize", "tools/call"] * 3]
    assert config.is_file(), "a caller-owned export is not silently deleted"


@needs_shell
@pytest.mark.parametrize("token", [None, "", "not-a-token", "ab" * 31])
def test_missing_or_invalid_auth_fails_before_any_http_request(token):
    seen = []
    with _server(OK, seen) as (port, _):
        run = _call(port, token=token)
    assert run.returncode != 0 and "GEN_AUDIO_MCP_TOKEN" in run.stdout + run.stderr
    assert not seen


@needs_shell
def test_wrong_bearer_is_rejected_without_tool_calls_or_credential_output():
    seen = []
    wrong = "cd" * 32
    with _server(OK, seen) as (port, _):
        run = _call(port, token=wrong)
    assert run.returncode != 0
    assert seen == [("/mcp", "initialize", "Bearer " + wrong)]
    assert wrong not in run.stdout + run.stderr


@needs_shell
@pytest.mark.parametrize("address", ["example.com:8765", "127.0.0.1:8765?token=secret", "user:secret@127.0.0.1:8765", "127.0.0.1:65536"])
def test_environment_discovery_rejects_nonloopback_or_credential_urls(address):
    run = _call(0, env_extra={"GEN_AUDIO_MCP_ADDR": address})
    assert run.returncode != 0
    assert "GEN_AUDIO_MCP_ADDR must be a loopback" in run.stdout + run.stderr
    assert address not in run.stdout + run.stderr


@needs_shell
def test_http_redirect_cannot_forward_the_bearer():
    seen = []
    with _server(OK, seen, redirect=True) as (port, _):
        run = _call(port)
    assert run.returncode != 0
    assert len(seen) == 1 and seen[0][0] == "/mcp"
    assert TOKEN not in run.stdout + run.stderr


@needs_shell
def test_connection_export_rejects_untrusted_or_malformed_data(tmp_path):
    file = tmp_path / "connection.json"
    for config in (
        {"addr": "127.0.0.1:8765", "token": TOKEN, "handshake_ok": False},
        {"addr": "example.com:8765", "token": TOKEN, "handshake_ok": True},
        {"addr": "127.0.0.1:8765?token=" + TOKEN, "token": TOKEN, "handshake_ok": True},
        {"addr": "127.0.0.1:8765", "token": TOKEN[:-1], "handshake_ok": True},
        {"addr": "127.0.0.1:8765", "token": TOKEN},
    ):
        file.write_text(json.dumps(config), encoding="utf-8")
        run = _call(0, "-ConnectionFile", str(file), token=None)
        assert run.returncode != 0
        assert TOKEN not in run.stdout + run.stderr


def _bridge():
    spec = importlib.util.spec_from_file_location("connect_desktop_mcp", REPO / "scripts" / "connect-desktop-mcp.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_installed_bridge_consumes_private_export_and_cleans_its_scratch(tmp_path, monkeypatch):
    bridge = _bridge()
    exe = tmp_path / "installed app.exe"
    exe.write_bytes(b"mock executable")
    monkeypatch.setenv("GEN_AUDIO_DESKTOP", str(exe))
    monkeypatch.delenv("LOCALAPPDATA", raising=False)
    monkeypatch.chdir(tmp_path)
    exported = []
    config = {"addr": "127.0.0.1:8765", "token": TOKEN, "handshake_ok": True}

    def launch(args, **kwargs):
        assert args[:2] == [str(exe), "--mcp-connection-file"]
        path = Path(args[2])
        assert path.is_relative_to(tmp_path / "target" / "mcp-connections")
        assert not path.exists()
        exported.append(path)
        path.write_text(json.dumps(config), encoding="utf-8")

    monkeypatch.setattr(bridge.subprocess, "Popen", launch)
    assert bridge.connect() == config
    assert exported and not exported[0].exists()


def test_installed_bridge_validates_the_owned_address_and_token():
    bridge = _bridge()
    for address in ("127.0.0.1:8765", "localhost:80", "[::1]:65535"):
        assert bridge.valid_connection({"addr": address, "token": TOKEN, "handshake_ok": True})
    for address in ("127.0.0.1:65536", "0.0.0.0:8765", "example.com:80", "127.0.0.1:8765?token=" + TOKEN):
        assert not bridge.valid_connection({"addr": address, "token": TOKEN, "handshake_ok": True})
    assert not bridge.valid_connection(None)
    assert not bridge.valid_connection({"addr": "localhost:80", "token": 12, "handshake_ok": True})

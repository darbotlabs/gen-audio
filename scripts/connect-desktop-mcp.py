"""Forward newline-delimited MCP JSON-RPC to the installed app's owned listener.

Run with Python 3.12+. The transient bearer is obtained through the native
single-instance command into a new owner-only file, never stdout.
"""
from __future__ import annotations
import http.client
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time


def valid_connection(data: object) -> bool:
    if not isinstance(data, dict) or data.get("handshake_ok") is not True:
        return False
    token, addr = data.get("token"), data.get("addr")
    if not isinstance(token, str) or not re.fullmatch(r"[0-9a-fA-F]{64}", token):
        return False
    if not isinstance(addr, str):
        return False
    match = re.fullmatch(r"(?:127\.0\.0\.1|localhost|\[::1\]):([1-9]\d{0,4})", addr)
    return bool(match and int(match[1]) <= 65535)


def desktop_executable() -> Path:
    configured = os.environ.get("GEN_AUDIO_DESKTOP")
    local = os.environ.get("LOCALAPPDATA")
    exe = Path(configured) if configured else None
    if exe is None and os.name == "nt":
        # Prefer the registered MSIX over an older NSIS executable. Only query
        # this package/publisher; no caller-controlled text enters PowerShell.
        query = ("Get-AppxPackage -Name labs.darbot.genaudio | "
                 "Where-Object { $_.Publisher -eq 'CN=Darbot Labs' } | "
                 "Sort-Object { [version]$_.Version } -Descending | Select-Object -First 1 | "
                 "Select-Object -ExpandProperty InstallLocation")
        try:
            result = subprocess.run(["powershell.exe", "-NoProfile", "-NonInteractive", "-Command", query],
                                    capture_output=True, text=True, timeout=10, creationflags=subprocess.CREATE_NO_WINDOW)
            if result.returncode == 0 and result.stdout.strip():
                exe = Path(result.stdout.strip()) / "gen-audio.exe"
        except (OSError, subprocess.TimeoutExpired):
            pass
    if exe is None and local:
        exe = Path(local) / "Gen-Audio" / "gen-audio.exe"
    if exe is None:
        raise RuntimeError("Set GEN_AUDIO_DESKTOP to the installed desktop executable")
    if not exe.is_file():
        raise RuntimeError("Install Gen-Audio before connecting this MCP bridge")
    return exe


def connect() -> dict:
    exe = desktop_executable()
    scratch = Path.cwd() / "target" / "mcp-connections"
    scratch.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="connection-", dir=scratch) as directory:
        file = Path(directory) / "connection.json"
        subprocess.Popen([str(exe), "--mcp-connection-file", str(file)],
                         creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            if file.is_file() and 0 < file.stat().st_size <= 65536:
                try:
                    data = json.loads(file.read_text(encoding="utf-8"))
                    if valid_connection(data):
                        return data
                except (OSError, ValueError):
                    pass
            time.sleep(0.1)
        raise RuntimeError("The desktop did not export a usable owned MCP connection")

def main() -> int:
    # MCP stdio is UTF-8 even when the Windows console uses a legacy code page.
    sys.stdin.reconfigure(encoding="utf-8")
    sys.stdout.reconfigure(encoding="utf-8", newline="\n")
    sys.stderr.reconfigure(encoding="utf-8")
    config = connect()
    host, raw_port = config["addr"].rsplit(":", 1)
    host = host.removeprefix("[").removesuffix("]")
    while line := sys.stdin.readline(1024 * 1024 + 1):
        if len(line.encode("utf-8")) > 1024 * 1024:
            raise RuntimeError("MCP input exceeds 1 MiB")
        if not line.strip():
            continue
        message = json.loads(line)
        conn = http.client.HTTPConnection(host, int(raw_port), timeout=630)
        try:
            conn.request("POST", "/mcp", json.dumps(message), {"Content-Type": "application/json", "Authorization": "Bearer " + config["token"]})
            response = conn.getresponse()
            body = response.read(1024 * 1024 + 1)
            if response.status not in (200, 202, 204) or len(body) > 1024 * 1024:
                raise RuntimeError(f"Owned MCP request failed (HTTP {response.status})")
            if response.status in (202, 204) and "id" in message:
                raise RuntimeError("Owned MCP request returned no response for a request ID")
            if body.strip() and "id" in message:
                json.loads(body)
                print(body.decode("utf-8"), flush=True)
        finally:
            conn.close()
    return 0

if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, RuntimeError) as exc:
        print(f"Gen-Audio MCP connection failed: {exc}", file=sys.stderr)
        raise SystemExit(1)

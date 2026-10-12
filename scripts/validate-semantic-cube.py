"""Exercise metric imports, native projection, colors, export and audio isolation.

Uses the production frontend, native CSP and an isolated owned MCP process.
No external services, data feeds, asset overrides or user-session credentials.
"""
import functools
import http.client
import http.server
import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import tempfile
import threading
import time
from playwright.sync_api import sync_playwright, expect

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "artifacts/validation/semantic-cube"


def verify(page, report):
    page.wait_for_selector("#cube-metrics", state="attached")
    page.get_by_role("tab", name="Cube", exact=True).click()
    page.locator("#cube-source").select_option("lib-misaki-kokoro")
    expect(page.locator("#cube-compare-toggle")).to_be_enabled()
    page.locator("#cube-compare-toggle").click()
    expect(page.locator(".cube-stage")).to_have_class("cube-stage is-compare", timeout=15000)
    examples = json.loads((ROOT / "schemas/examples/semantic-cubes.json").read_text(encoding="utf-8"))
    for index, doc in enumerate(examples):
        page.locator("#cube-metrics").click()
        page.get_by_label("Metric schema example").select_option(str(index))
        # Selecting the existing option need not emit change, so fill the first
        # template explicitly when a prior document is already loaded.
        page.locator("#semantic-document").fill(json.dumps(doc))
        page.locator("#semantic-render").click()
        expect(page.locator("#semantic-editor")).not_to_be_visible(timeout=15000)
        expect(page.locator("#cube-title")).to_have_text(doc["title"])
        expect(page.locator("#cube-badge")).to_contain_text(doc["domain"] + " · example")
        expect(page.locator(".cube-layer-row")).to_have_count(len(doc["layers"]))
        expect(page.locator("#cube-play")).to_be_disabled()
        expect(page.locator("#cube-compare-toggle")).to_be_disabled()
        expect(page.locator("#cube-compare-toggle")).to_have_text("Compare")
        expect(page.locator(".cube-stage")).not_to_have_class("cube-stage is-compare")
        assert page.locator("#cube-viewport").evaluate("canvas => Boolean(canvas.getContext('webgl'))")
    # Invalid import is transactional: it cannot clear or replace the cube.
    page.locator("#cube-metrics").click()
    invalid = json.loads(page.locator("#semantic-document").input_value())
    invalid["records"][0]["metrics"]["low"] = 999
    page.locator("#semantic-document").fill(json.dumps(invalid))
    page.locator("#semantic-render").click()
    expect(page.locator(".semantic-feedback")).to_contain_text("OHLC")
    expect(page.locator("#cube-title")).to_have_text(examples[-1]["title"])
    page.get_by_role("button", name="Close", exact=True).click()
    color = page.locator('.cube-layer-row input[type="color"]').first
    color.fill("#33ccaa")
    page.locator("#cube-metrics").click()
    current = json.loads(page.locator("#semantic-document").input_value())
    assert current["layers"][0]["color"] == "#33ccaa"
    with page.expect_download() as transfer:
        page.locator("#semantic-export").click()
    downloaded = json.loads(Path(transfer.value.path()).read_text(encoding="utf-8"))
    assert downloaded == current
    # File import retains a real document, then native validation applies it.
    imported = json.loads(json.dumps(examples[0])); imported["title"] = "Imported sessions"
    page.get_by_label("Import metric cube JSON").set_input_files({"name":"sessions.json","mimeType":"application/json","buffer":json.dumps(imported).encode()})
    expect(page.locator(".semantic-feedback")).to_contain_text("Imported.")
    page.locator("#semantic-render").click()
    expect(page.locator("#semantic-editor")).not_to_be_visible()
    expect(page.locator("#cube-title")).to_have_text("Imported sessions")
    # Audio can continue while metric-axis scrubbing changes only the cube.
    page.get_by_role("tab", name="Audio clips", exact=True).click()
    clip = page.locator('.library-tile[data-id="lib-cube-explainer"]')
    clip.locator(".transport-toggle").click()
    page.wait_for_function("() => !document.querySelector('.library-tile[data-id=lib-cube-explainer] audio').paused")
    page.get_by_role("tab", name="Cube", exact=True).click()
    page.locator("#cube-metrics").click()
    page.locator("#semantic-document").fill(json.dumps(imported))
    page.locator("#semantic-render").click()
    expect(page.locator("#semantic-editor")).not_to_be_visible()
    before = clip.locator("audio").evaluate("audio => audio.currentTime")
    page.locator("#cube-scrub").fill("900")
    page.wait_for_timeout(300)
    after = clip.locator("audio").evaluate("audio => audio.currentTime")
    assert 0 <= after-before < 3, (before, after)
    expect(page.locator("#cube-title")).to_have_text("Imported sessions")
    expect(page.locator("#cube-clock")).to_contain_text("54")
    page.locator("#fp-close").click()
    page.screenshot(path=str(OUT / "metric-cube.png"))
    # Returning to a source reestablishes ordinary audio cube behavior.
    page.locator("#cube-source").select_option("lib-cube-explainer")
    expect(page.locator("#cube-title")).not_to_have_text("Imported sessions")
    expect(page.locator(".cube-layer-row")).to_have_count(4)
    expect(page.locator("#cube-play")).to_be_enabled()
    report.update(domains=[doc["domain"] for doc in examples], color_export_roundtrip=True, invalid_import_preserves_view=True,
                  native_validation=True, file_import=True, audio_clock_isolated=True, audio_cube_return=True)


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0)); port = sock.getsockname()[1]
    token = secrets.token_hex(32)
    with tempfile.TemporaryDirectory(prefix="gen-audio-semantic-qa-") as scratch:
        env = dict(os.environ, GEN_AUDIO_MCP_TOKEN=token, GEN_AUDIO_WORK_DIR=scratch,
                   LOCALAPPDATA=str(Path(scratch)/"appdata"), GEN_AUDIO_REPO=str(ROOT),
                   GEN_AUDIO_PYTHON=str(ROOT/".venv-app/Scripts/python.exe"),
                   GEN_AUDIO_KOKORO_MODEL=str(ROOT/"models/kokoro-onnx/kokoro-v1.0.onnx"),
                   GEN_AUDIO_KOKORO_VOICES=str(ROOT/"models/kokoro-onnx/voices-v1.0.bin"))
        env.pop("GEN_AUDIO_ARTIFACT_DIR", None)
        backend = subprocess.Popen([str(ROOT/"target/debug/gen-audio-mcp.exe"), "--http", f"127.0.0.1:{port}"],
                                   cwd=ROOT, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, creationflags=subprocess.CREATE_NO_WINDOW)
        csp = json.loads((ROOT/"apps/desktop/src-tauri/tauri.conf.json").read_text())["app"]["security"]["csp"]
        class Handler(http.server.SimpleHTTPRequestHandler):
            def copyfile(self, source, output):
                try: super().copyfile(source, output)
                except (BrokenPipeError, ConnectionResetError): pass
            def end_headers(self):
                self.send_header("Content-Security-Policy", csp); super().end_headers()
            def log_message(self, *args): pass
        web = None
        try:
            for _ in range(100):
                try:
                    connection = http.client.HTTPConnection("127.0.0.1",port,timeout=1)
                    connection.request("GET","/health",headers={"Authorization":"Bearer "+token}); response = connection.getresponse(); response.read(); connection.close()
                    if response.status == 200: break
                except OSError: time.sleep(0.1)
            else: raise RuntimeError("Owned MCP did not start")
            web = http.server.ThreadingHTTPServer(("127.0.0.1",1420),functools.partial(Handler,directory=str(ROOT/"apps/desktop/dist")))
            threading.Thread(target=web.serve_forever,daemon=True).start()
            report = {"production_frontend":True,"native_csp":True,"errors":[]}
            with sync_playwright() as p:
                browser = p.chromium.launch(channel="msedge",headless=True)
                try:
                    page = browser.new_page(viewport={"width":1536,"height":1000},reduced_motion="reduce")
                    page.add_init_script("window.__GEN_AUDIO_MCP__="+json.dumps({"addr":f"127.0.0.1:{port}","token":token})+";")
                    page.on("pageerror",lambda error: report["errors"].append(str(error)))
                    page.goto("http://localhost:1420",wait_until="domcontentloaded")
                    verify(page,report)
                    assert not report["errors"],report["errors"]
                finally: browser.close()
            (OUT/"browser-validation.json").write_text(json.dumps(report,indent=2)+"\n",encoding="utf-8")
            print(json.dumps(report))
        finally:
            if web: web.shutdown(); web.server_close()
            backend.terminate(); backend.wait(timeout=10)


if __name__ == "__main__": main()

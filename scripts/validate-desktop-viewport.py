"""Exercise the production frontend with an owned MCP and the native CSP.

Run after npm run build and cargo build -p gen-audio-mcp. Requires Playwright
and Edge. Uses isolated application data; never connects to a user's session.
"""
from __future__ import annotations

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
OUT = ROOT / "artifacts/validation/viewport-cleanup"


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    token = secrets.token_hex(32)
    with tempfile.TemporaryDirectory(prefix="gen-audio-viewport-qa-") as scratch:
        env = dict(os.environ, GEN_AUDIO_MCP_TOKEN=token, GEN_AUDIO_WORK_DIR=scratch,
                   LOCALAPPDATA=str(Path(scratch) / "appdata"), GEN_AUDIO_REPO=str(ROOT),
                   GEN_AUDIO_PYTHON=str(ROOT / ".venv-app/Scripts/python.exe"),
                   GEN_AUDIO_MODEL_DIR=str(ROOT / "models/kokoro-onnx"),
                   GEN_AUDIO_KOKORO_MODEL=str(ROOT / "models/kokoro-onnx/kokoro-v1.0.onnx"),
                   GEN_AUDIO_KOKORO_VOICES=str(ROOT / "models/kokoro-onnx/voices-v1.0.bin"))
        env.pop("GEN_AUDIO_ARTIFACT_DIR", None)
        backend = subprocess.Popen([str(ROOT / "target/debug/gen-audio-mcp.exe"), "--http", f"127.0.0.1:{port}"],
                                   cwd=ROOT, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        web = None
        csp = json.loads((ROOT / "apps/desktop/src-tauri/tauri.conf.json").read_text())["app"]["security"]["csp"]

        class Handler(http.server.SimpleHTTPRequestHandler):
            def copyfile(self, source, output):
                try:
                    super().copyfile(source, output)
                except (BrokenPipeError, ConnectionResetError):
                    pass

            def end_headers(self):
                self.send_header("Content-Security-Policy", csp)
                super().end_headers()

            def log_message(self, *args):
                pass

        try:
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                try:
                    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=1)
                    connection.request("GET", "/health", headers={"Authorization": "Bearer " + token})
                    response = connection.getresponse()
                    response.read()
                    connection.close()
                    if response.status == 200:
                        break
                except OSError:
                    time.sleep(0.1)
            else:
                raise RuntimeError("Owned test MCP did not start")
            web = http.server.ThreadingHTTPServer(("127.0.0.1", 1420), functools.partial(Handler, directory=str(ROOT / "apps/desktop/dist")))
            threading.Thread(target=web.serve_forever, daemon=True).start()
            report = {"production_frontend": True, "native_csp": True, "errors": []}
            with sync_playwright() as playwright:
                browser = playwright.chromium.launch(executable_path="C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe", headless=True)
                try:
                    page = browser.new_page(viewport={"width": 1536, "height": 1000}, reduced_motion="reduce")
                    page.add_init_script("window.__GEN_AUDIO_MCP__=" + json.dumps({"addr": f"127.0.0.1:{port}", "token": token}) + ";")
                    page.on("pageerror", lambda error: report["errors"].append(str(error)))
                    page.goto("http://localhost:1420", wait_until="domcontentloaded")
                    page.wait_for_selector(".profile-tile")
                    tabs = page.locator("#layer-switch [role=tab]")
                    slides = page.locator("#board > .slide")
                    names = tabs.all_text_contents()
                    assert names == ["Voice models", "Agent Voice Profiles", "Audio clips", "Connectors", "Pipeline", "Video", "Cube"], names
                    assert tabs.count() == slides.count() == 7
                    for i, name in enumerate(names):
                        tabs.nth(i).click()
                        assert tabs.nth(i).get_attribute("aria-selected") == "true", name
                        expect(page.locator("#slide-label")).to_have_text(f"Slide {i + 1}/7")
                        assert tabs.nth(i).get_attribute("aria-controls") == slides.nth(i).get_attribute("id")
                        assert abs(slides.nth(i).bounding_box()["y"] - page.locator("#board").bounding_box()["y"]) < 3
                    assert page.locator("#slide-dots [aria-current=true]").count() == 1
                    tabs.last.press("Home")
                    assert tabs.first.get_attribute("aria-selected") == "true"
                    tabs.first.press("ArrowRight")
                    assert tabs.nth(1).get_attribute("aria-selected") == "true"
                    assert page.locator("footer, .lede").count() == 0
                    assert "readiness is checked" not in page.locator("header").inner_text()
                    assert page.locator(".slide-chrome").bounding_box()["y"] > page.locator("#layer-switch").bounding_box()["y"] + page.locator("#layer-switch").bounding_box()["height"]
                    report["tabs"] = names
                    report["keyboard_and_indicator"] = True

                    page.get_by_role("tab", name="Audio clips", exact=True).click()
                    clip = page.locator('.library-tile[data-id="lib-cube-explainer"]')
                    flip = clip.locator(".flip-toggle").first
                    assert flip.bounding_box()["y"] + flip.bounding_box()["height"] <= clip.bounding_box()["y"] + clip.bounding_box()["height"]
                    clip.locator(".transport-toggle").click()
                    page.wait_for_function("() => !document.querySelector('.library-tile[data-id=lib-cube-explainer] audio').paused")
                    assert page.locator("#floating-playback").is_visible()
                    page.locator("#fp-playpause").click()
                    assert page.locator("#floating-playback").is_visible()
                    assert clip.locator("audio").evaluate("audio => audio.paused")
                    page.locator("#fp-playpause").click()
                    page.locator("#layer-switch").get_by_role("tab", name="Cube", exact=True).click()
                    assert page.locator("#floating-playback").is_visible()
                    before = clip.locator("audio").evaluate("audio => audio.currentTime")
                    page.locator("#fp-goto").click()
                    assert page.get_by_role("tab", name="Audio clips", exact=True).get_attribute("aria-selected") == "true"
                    assert clip.locator(".transport-toggle").evaluate("button => document.activeElement === button")
                    after = clip.locator("audio").evaluate("audio => ({paused:audio.paused, time:audio.currentTime})")
                    assert not after["paused"] and after["time"] >= before, {"before": before, "after": after, "errors": report["errors"]}
                    page.screenshot(path=str(OUT / "desktop-player.png"), full_page=True)
                    page.locator("#fp-close").click()
                    assert page.locator("#floating-playback").is_hidden()
                    assert clip.locator("audio").evaluate("audio => audio.paused && audio.currentTime === 0")
                    clip.locator("audio").evaluate("audio => audio.dispatchEvent(new Event('timeupdate'))")
                    assert page.locator("#floating-playback").is_hidden()
                    report["player"] = {"pause_resume": True, "cube_visible": True, "arrow_reveals_source_without_restart": True, "close_stops_and_stays_closed": True}

                    page.get_by_role("tab", name="Agent Voice Profiles", exact=True).click()
                    search = page.get_by_role("searchbox", name="Search Agent Voice Profiles")
                    search.fill("Alice")
                    card = page.locator(".profile-tile[data-persona-id=alice]")
                    card.locator(".flip-toggle").first.click()
                    card.locator("[data-action=audition-reference]").first.click()
                    page.wait_for_function("() => !document.querySelector('.profile-tile[data-persona-id=alice] .profile-sample audio').paused", timeout=20000)
                    page.get_by_role("tab", name="Voice models", exact=True).click()
                    page.locator("#fp-goto").click()
                    assert page.get_by_role("tab", name="Agent Voice Profiles", exact=True).get_attribute("aria-selected") == "true"
                    assert card.locator(".profile-sample audio").first.evaluate("audio => document.activeElement === audio")
                    page.locator("#fp-close").click()
                    search.fill("")
                    page.screenshot(path=str(OUT / "desktop-profiles.png"), full_page=True)
                    report["reference_player"] = True
                    page.locator("#prompt").fill("Hello. This is Alice testing the audio player. The arrow should return to the playing audio and preserve the current position. Pause and resume should stay available, and closing the player should stop the sound. This recording also checks the podcast master.")
                    page.locator("#generate-podcast").click()
                    page.wait_for_selector("#speech-output:not([hidden])", timeout=120000)
                    generated = page.locator("#speech-audio")
                    generated.evaluate("audio => audio.play()")
                    expect(page.locator("#floating-playback")).to_be_visible()
                    page.locator("#fp-goto").click()
                    assert generated.evaluate("audio => document.activeElement === audio && !audio.paused")
                    page.locator("#fp-close").click()
                    assert generated.evaluate("audio => audio.paused")
                    page.locator("#master-audio").click()
                    page.wait_for_function("() => !document.querySelector('#master-audio').disabled", timeout=120000)
                    assert page.locator("#master-player").is_visible(), page.locator("#master-info").inner_text()
                    mastered = page.locator("#master-player")
                    mastered.evaluate("audio => audio.play()")
                    expect(page.locator("#floating-playback")).to_be_visible()
                    page.locator("#fp-goto").click()
                    assert mastered.evaluate("audio => document.activeElement === audio && !audio.paused")
                    page.locator("#fp-close").click()
                    assert mastered.evaluate("audio => audio.paused")
                    report["real_kokoro_and_master_player"] = True
                    page.set_viewport_size({"width": 390, "height": 844})
                    tabs.nth(1).click()
                    assert tabs.nth(1).is_visible()
                    chrome = page.locator(".slide-chrome").bounding_box()
                    buttons = page.locator("#layer-switch").bounding_box()
                    assert chrome["y"] >= buttons["y"] + buttons["height"]
                    assert page.locator(".profile-tile[data-persona-id=anton]").bounding_box()["height"] >= 280
                    page.screenshot(path=str(OUT / "mobile-profiles.png"), full_page=True)
                    report["mobile_layout"] = True
                    assert not report["errors"], report["errors"]
                finally:
                    browser.close()
            (OUT / "browser-validation.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
            print(json.dumps(report))
        finally:
            if web:
                web.shutdown()
                web.server_close()
            backend.terminate()
            backend.wait(timeout=10)


if __name__ == "__main__":
    main()

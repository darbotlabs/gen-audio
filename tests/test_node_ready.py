from __future__ import annotations

from gen_audio.gateway import gateway_health, gateway_ready, podcast_headers, tts_proxy_error
from gen_audio.node_http import dispatch


def test_health_is_live_when_the_model_is_unloaded():
    status, body = dispatch("GET", "/genaid-audio/health", model_loaded=False)
    assert status == 200
    assert body["status"] == "ok"
    assert body["service"] == "genaid-audio"
    assert body["model_loaded"] is False


def test_ready_stays_ready_for_lazy_load():
    status, body = dispatch("GET", "/genaid-audio/ready", model_loaded=False)
    assert status == 200
    assert body["ready"] is True
    assert body["speech_generation"] == "load_required"


def test_gateway_ready_is_503_when_speech_is_down():
    assert gateway_health(None)[0] == 200
    status, payload = gateway_ready(None)
    assert status == 503
    assert payload["status"] == "unavailable"


def test_tts_proxy_log_shape_and_headers():
    class TimeoutException(Exception):
        pass

    code, detail = tts_proxy_error(TimeoutException())
    assert code == 504
    assert "text" not in detail.lower() or "timed out" in detail.lower()
    headers = podcast_headers("line\r\nSet-Cookie: x", "Alice", "/tmp/out.wav")
    assert "\r" not in headers["X-Podcast-Script"]
    assert "\n" not in headers["X-Podcast-Script"]

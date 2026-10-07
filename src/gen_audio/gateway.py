"""Gateway readiness and TTS error classes.

This is the gen-audio stand-in for the dlm_cluster gateway router. It does not
import FastAPI or Ray. ``/health`` stays HTTP 200 when speech is down so a
portal can still offer script generation. ``/ready`` is 503 in that case.
TTS proxy logs record status and exception type, not a prefix of the user text.
"""

from __future__ import annotations

import asyncio
import logging
from collections.abc import Callable
from typing import Any, TypeVar

from gen_audio.guards import classify_upstream_failure, sanitize_header_value

logger = logging.getLogger("gen_audio.gateway")

T = TypeVar("T")


def gateway_health(backend: dict[str, Any] | None) -> tuple[int, dict[str, Any]]:
    """Liveness. A missing backend is still HTTP 200 with ``status: unavailable``."""
    if backend is None:
        return 200, {
            "status": "unavailable",
            "service": "genaid-audio",
            "script_generation": "available",
            "speech_generation": "unavailable",
        }
    return 200, dict(backend)


def gateway_ready(backend: dict[str, Any] | None) -> tuple[int, dict[str, Any]]:
    """Readiness. Unreachable speech is 503. A live backend is 200."""
    _status, payload = gateway_health(backend)
    if str(payload.get("status", "")).lower() == "unavailable":
        return 503, payload
    return 200, {"ready": True, **payload}


def tts_proxy_error(exc: BaseException) -> tuple[int, str]:
    """Map a proxied TTS failure and log without the request text."""
    code, detail = classify_upstream_failure(exc)
    logger.warning("TTS proxy failed status=%s error_type=%s", code, type(exc).__name__)
    return code, detail


def podcast_headers(script: str, speakers: str, path: str) -> dict[str, str]:
    """Header values with CR/LF removed. The script is not written to the log."""
    return {
        "X-Podcast-Script": sanitize_header_value(script.replace("\n", " | ")),
        "X-Podcast-Speakers": sanitize_header_value(speakers),
        "X-Podcast-File": sanitize_header_value(path),
    }


def run_blocking(work: Callable[[], T]) -> T:
    """Run disk or unload work on this thread when no event loop is running."""
    try:
        asyncio.get_running_loop()
    except RuntimeError:
        return work()
    raise RuntimeError("await run_off_loop() so disk and unload work leave the event loop")


async def run_off_loop(work: Callable[[], T]) -> T:
    """Podcast writes and model unload belong off the event loop."""
    return await asyncio.to_thread(work)

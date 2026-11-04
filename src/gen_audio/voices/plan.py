"""What an adapter can tell a synthesizer without loading weights."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Mapping


@dataclass(frozen=True)
class SynthesisPlan:
    """Resolved parameters for one voice.

    ``executable`` is true only when this process has the files and the runtime
    the wired synthesizer needs. Other frameworks stay false and name the gap
    in ``missing``. Nothing here writes audio.
    """

    framework: str
    voice_id: str
    engine_id: str | None
    kind: str
    wired: bool
    executable: bool
    missing: tuple[str, ...]
    params: Mapping[str, Any]
    resolved_files: Mapping[str, str]

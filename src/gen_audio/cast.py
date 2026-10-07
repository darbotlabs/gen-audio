"""Parse ``Speaker N`` scripts and resolve voices from a cast map.

Script form::

    # comments and blank lines are ignored
    Speaker 1: Text on the same line.
    Speaker 2 (Frank):
    Text continues on following lines until the next Speaker header.

The name in parentheses is recorded and is not used to pick a voice.
Voice, language, and speed come from the cast map.
"""

from __future__ import annotations

import json
import re
from dataclasses import dataclass
from pathlib import Path

_HEADER = re.compile(
    r"^Speaker\s+(?P<id>\d+)\s*(?:\((?P<name>[^)]*)\))?\s*:\s*(?P<text>.*)$",
    re.IGNORECASE,
)

_KOKORO_SPEED_MIN = 0.5
_KOKORO_SPEED_MAX = 2.0


@dataclass(frozen=True)
class Turn:
    """One spoken turn in script order."""

    speaker_id: str
    text: str
    script_name: str | None
    line: int


@dataclass(frozen=True)
class VoiceAssignment:
    """Resolved voice for one speaker id."""

    speaker_id: str
    name: str
    voice: str
    lang: str
    speed: float


@dataclass(frozen=True)
class CastMap:
    """Speaker-id to voice table for one engine."""

    engine: str
    default_lang: str
    default_speed: float
    speakers: dict[str, VoiceAssignment]
    source: Path | None = None


def parse_script(text: str) -> list[Turn]:
    """Parse a podcast script into ordered turns.

    Raises ``ValueError`` when a header has no text, a line sits outside a
    turn, or the file has no turns.
    """
    turns: list[Turn] = []
    speaker_id: str | None = None
    script_name: str | None = None
    start_line = 0
    parts: list[str] = []

    def flush() -> None:
        nonlocal speaker_id, script_name, start_line, parts
        if speaker_id is None:
            return
        body = " ".join(part.strip() for part in parts if part.strip())
        body = " ".join(body.split())
        if not body:
            raise ValueError(f"Speaker {speaker_id} at line {start_line} has no text")
        turns.append(
            Turn(
                speaker_id=speaker_id,
                text=body,
                script_name=script_name,
                line=start_line,
            )
        )
        speaker_id = None
        script_name = None
        start_line = 0
        parts = []

    for lineno, raw in enumerate(text.splitlines(), start=1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        match = _HEADER.match(line)
        if match:
            flush()
            speaker_id = str(int(match.group("id")))
            name = match.group("name")
            script_name = name.strip() if name and name.strip() else None
            start_line = lineno
            inline = (match.group("text") or "").strip()
            parts = [inline] if inline else []
            continue
        if speaker_id is None:
            raise ValueError(f"line {lineno} is outside a Speaker turn: {line}")
        parts.append(line)
    flush()
    if not turns:
        raise ValueError("script contains no Speaker turns")
    return turns


def read_script(path: Path | str) -> list[Turn]:
    """Read a UTF-8 script and parse it."""
    text = Path(path).read_text(encoding="utf-8-sig")
    return parse_script(text)


def load_cast_map(path: Path | str) -> CastMap:
    """Load a cast map JSON file.

    Top-level keys that start with ``_`` are ignored so example files can
    carry a note. Speaker ids are normalized (``"01"`` and ``"1"`` match).
    """
    source = Path(path)
    try:
        payload = json.loads(source.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise ValueError(f"cast map is not valid JSON: {source}") from exc
    if not isinstance(payload, dict):
        raise ValueError("cast map root must be an object")

    engine = _required_str(payload, "engine")
    default_lang = _optional_str(payload, "default_lang", "en-us")
    default_speed = _optional_float(payload, "default_speed", 1.0)
    _check_speed(default_speed, "default_speed")

    raw_speakers = payload.get("speakers")
    if not isinstance(raw_speakers, dict) or not raw_speakers:
        raise ValueError("cast map speakers must be a non-empty object")

    speakers: dict[str, VoiceAssignment] = {}
    for raw_id, raw_value in raw_speakers.items():
        speaker_id = _normalize_speaker_id(raw_id)
        if not isinstance(raw_value, dict):
            raise ValueError(f"speaker {speaker_id} must be an object")
        voice = _required_str(raw_value, "voice", label=f"speaker {speaker_id}")
        name = _optional_str(raw_value, "name", f"Speaker {speaker_id}")
        lang = _optional_str(raw_value, "lang", default_lang)
        speed = _optional_float(raw_value, "speed", default_speed)
        _check_speed(speed, f"speaker {speaker_id} speed")
        if speaker_id in speakers:
            raise ValueError(f"duplicate speaker id {speaker_id}")
        speakers[speaker_id] = VoiceAssignment(
            speaker_id=speaker_id,
            name=name,
            voice=voice,
            lang=lang,
            speed=speed,
        )
    return CastMap(
        engine=engine,
        default_lang=default_lang,
        default_speed=default_speed,
        speakers=speakers,
        source=source,
    )


def resolve_turn(turn: Turn, cast: CastMap) -> VoiceAssignment:
    """Return the cast-map voice for a turn.

    Raises ``KeyError`` listing the known speaker ids when the script uses
    a speaker the map does not define.
    """
    try:
        return cast.speakers[turn.speaker_id]
    except KeyError:
        known = ", ".join(sorted(cast.speakers, key=int))
        raise KeyError(
            f"Speaker {turn.speaker_id} at line {turn.line} is not in the cast map; "
            f"known speakers: {known}"
        ) from None


def _normalize_speaker_id(raw_id: object) -> str:
    text = str(raw_id).strip()
    if not text.isdigit():
        raise ValueError(f"speaker id must be an integer, got {raw_id!r}")
    return str(int(text))


def _check_speed(speed: float, label: str) -> None:
    if speed < _KOKORO_SPEED_MIN or speed > _KOKORO_SPEED_MAX:
        raise ValueError(
            f"{label} must be between {_KOKORO_SPEED_MIN} and {_KOKORO_SPEED_MAX} "
            f"to match kokoro-onnx, got {speed}"
        )


def _required_str(payload: dict, key: str, *, label: str = "cast map") -> str:
    value = payload.get(key)
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"{label} field {key!r} must be a non-empty string")
    return value.strip()


def _optional_str(payload: dict, key: str, default: str) -> str:
    value = payload.get(key, default)
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"field {key!r} must be a non-empty string")
    return value.strip()


def _optional_float(payload: dict, key: str, default: float) -> float:
    value = payload.get(key, default)
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ValueError(f"field {key!r} must be a number")
    return float(value)

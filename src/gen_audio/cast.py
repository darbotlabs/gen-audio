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
import math
import re
from dataclasses import dataclass, field
from pathlib import Path
from types import MappingProxyType
from typing import Mapping

from gen_audio.guards import RequestLimitError, enforce_text_limit, max_script_segments, max_total_script_chars

_HEADER = re.compile(
    r"^Speaker\s+(?P<id>\d+)\s*(?:\((?P<name>[^)]*)\))?\s*:\s*(?P<text>.*)$",
    re.IGNORECASE,
)

_KOKORO_SPEED_MIN = 0.5
_KOKORO_SPEED_MAX = 2.0
_KOKORO_FRAMEWORKS = {"kokoro", "kokoro_onnx"}
_SCHEMA_VERSION = "1.0.0"
_SLUG = re.compile(r"^[a-z][a-z0-9_]{0,31}\Z")
_LANG = re.compile(r"^[a-z]{2,3}(-[a-z0-9]{2,8})?\Z")
_VOICE_DOC_ID = re.compile(r"^[a-z][a-z0-9_]{0,31}\.[A-Za-z0-9][A-Za-z0-9_.-]{0,63}\Z")
_V1_KEYS = {
    "schema_version",
    "id",
    "note",
    "engine",
    "default_lang",
    "default_speed",
    "default_framework",
    "source",
    "characters",
    "speakers",
}
_SPEAKER_KEYS = {"name", "voice", "lang", "speed", "framework", "voice_id", "character"}
_CHARACTER_KEYS = {"display_name", "voices"}


@dataclass(frozen=True)
class Turn:
    """One spoken turn in script order."""

    speaker_id: str
    text: str
    script_name: str | None
    line: int


@dataclass(frozen=True)
class VoiceAssignment:
    """Resolved voice for one speaker id.

    ``framework`` is empty when the caller built the assignment by hand; the
    cast engine is the framework in that case. ``speed`` is the Kokoro speed
    for Kokoro frameworks. Other frameworks keep a finite number so older cast
    maps still load; it is not a synthesis parameter for those frameworks.
    """

    speaker_id: str
    name: str
    voice: str
    lang: str
    speed: float
    framework: str = ""
    voice_id: str | None = None
    character: str | None = None


@dataclass(frozen=True)
class CharacterCast:
    """One character's voice ids, one per framework."""

    character_id: str
    display_name: str
    voices: Mapping[str, str]


@dataclass(frozen=True)
class CastMap:
    """Speaker-id to voice table.

    ``engine`` is the default framework. A speaker may name another framework.
    ``characters`` maps a character onto catalog voice ids across frameworks.
    """

    engine: str
    default_lang: str
    default_speed: float
    speakers: dict[str, VoiceAssignment]
    source: Path | None = None
    schema_version: str | None = None
    characters: Mapping[str, CharacterCast] = field(default_factory=dict)
    cast_id: str | None = None
    note: str | None = None
    declared_source: str | None = None


def assignment_framework(assignment: VoiceAssignment, cast: CastMap) -> str:
    """Framework for one speaker. A blank assignment framework uses the cast engine."""
    return assignment.framework or cast.engine


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
    _enforce_script_limits(turns)
    return turns


def _enforce_script_limits(turns: list[Turn]) -> None:
    """Apply the GenAID Audio ceilings to a parsed Speaker script."""
    segment_cap = max_script_segments()
    if len(turns) > segment_cap:
        raise RequestLimitError(f"script exceeds {segment_cap} segments")
    total = 0
    for turn in turns:
        enforce_text_limit(turn.text, "segment")
        total += len(turn.text)
    total_cap = max_total_script_chars()
    if total > total_cap:
        raise RequestLimitError(f"script exceeds {total_cap} characters")


def read_script(path: Path | str) -> list[Turn]:
    """Read a UTF-8 script and parse it."""
    text = Path(path).read_text(encoding="utf-8-sig")
    return parse_script(text)


def load_cast_map(path: Path | str) -> CastMap:
    """Load a cast map JSON file.

    Keys that start with ``_`` are ignored on a legacy map (no ``schema_version``)
    so an example can carry a note. A ``schema_version`` of 1.0.0 rejects unknown
    keys. Speaker ids are normalized (``"01"`` and ``"1"`` match). Kokoro
    frameworks keep the 0.5–2.0 speed range. Other frameworks accept a finite
    speed greater than 0 and at most 4 so an older map that always wrote
    ``speed`` still loads; that number is not a model parameter for them.
    """
    source = Path(path)
    try:
        payload = json.loads(source.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise ValueError(f"cast map is not valid JSON: {source}") from exc
    if not isinstance(payload, dict):
        raise ValueError("cast map root must be an object")

    version = payload.get("schema_version")
    versioned = version is not None
    if versioned:
        if version != _SCHEMA_VERSION:
            raise ValueError(f"cast map schema_version must be {_SCHEMA_VERSION}")
        extra = sorted(set(payload) - _V1_KEYS)
        if extra:
            raise ValueError(f"cast map has unknown fields: {', '.join(extra)}")

    engine = _required_str(payload, "engine")
    if versioned and _SLUG.fullmatch(engine) is None:
        raise ValueError("cast map engine must be a lowercase slug")
    default_framework = payload.get("default_framework", engine)
    if not isinstance(default_framework, str) or not default_framework.strip():
        raise ValueError("default_framework must be a non-empty string")
    default_framework = default_framework.strip()
    if default_framework != engine:
        raise ValueError("default_framework must match engine")
    default_lang = _optional_str(payload, "default_lang", "en-us")
    if versioned and _LANG.fullmatch(default_lang) is None:
        raise ValueError("default_lang must be a language tag")
    default_speed = _optional_float(payload, "default_speed", 1.0)
    _speed_for(engine, default_speed, "default_speed")

    characters = _load_characters(payload.get("characters"), versioned=versioned)
    raw_speakers = payload.get("speakers")
    if not isinstance(raw_speakers, dict) or not raw_speakers:
        raise ValueError("cast map speakers must be a non-empty object")
    if versioned and len(raw_speakers) > 8:
        raise ValueError("cast map supports at most 8 speakers")

    speakers: dict[str, VoiceAssignment] = {}
    for raw_id, raw_value in raw_speakers.items():
        speaker_id = _normalize_speaker_id(raw_id)
        if not isinstance(raw_value, dict):
            raise ValueError(f"speaker {speaker_id} must be an object")
        if versioned:
            extra = sorted(set(raw_value) - _SPEAKER_KEYS)
            if extra:
                raise ValueError(f"speaker {speaker_id} has unknown fields: {', '.join(extra)}")
        voice = _required_str(raw_value, "voice", label=f"speaker {speaker_id}")
        name = _optional_str(raw_value, "name", f"Speaker {speaker_id}")
        lang = _optional_str(raw_value, "lang", default_lang)
        if versioned and _LANG.fullmatch(lang) is None:
            raise ValueError(f"speaker {speaker_id} lang must be a language tag")
        framework = _optional_str(raw_value, "framework", engine)
        if versioned and _SLUG.fullmatch(framework) is None:
            raise ValueError(f"speaker {speaker_id} framework must be a lowercase slug")
        explicit_speed = "speed" in raw_value
        speed = _optional_float(raw_value, "speed", default_speed)
        if explicit_speed or framework in _KOKORO_FRAMEWORKS:
            _speed_for(framework, speed, f"speaker {speaker_id} speed")
        voice_id = raw_value.get("voice_id")
        if voice_id is not None:
            if not isinstance(voice_id, str) or _VOICE_DOC_ID.fullmatch(voice_id.strip()) is None:
                raise ValueError(f"speaker {speaker_id} voice_id must be framework.local-id")
            voice_id = voice_id.strip()
            if versioned and not voice_id.startswith(framework + "."):
                raise ValueError(f"speaker {speaker_id} voice_id must start with {framework}.")
        character = raw_value.get("character")
        if character is not None:
            if not isinstance(character, str) or _SLUG.fullmatch(character.strip()) is None:
                raise ValueError(f"speaker {speaker_id} character must be a lowercase slug")
            character = character.strip()
            if character not in characters:
                raise ValueError(f"speaker {speaker_id} character {character} is not in the cast map")
        if speaker_id in speakers:
            raise ValueError(f"duplicate speaker id {speaker_id}")
        speakers[speaker_id] = VoiceAssignment(
            speaker_id=speaker_id,
            name=name,
            voice=voice,
            lang=lang,
            speed=speed,
            framework=framework,
            voice_id=voice_id,
            character=character,
        )
    declared = payload.get("source")
    declared_source = declared.strip() if isinstance(declared, str) and declared.strip() else None
    cast_id = payload.get("id")
    if cast_id is not None and (not isinstance(cast_id, str) or not cast_id.strip()):
        raise ValueError("cast map id must be a non-empty string")
    note = payload.get("note")
    if note is not None and not isinstance(note, str):
        raise ValueError("cast map note must be a string")
    return CastMap(
        engine=engine,
        default_lang=default_lang,
        default_speed=default_speed,
        speakers=speakers,
        source=source,
        schema_version=version if versioned else None,
        characters=characters,
        cast_id=cast_id.strip() if isinstance(cast_id, str) else None,
        note=note,
        declared_source=declared_source,
    )


def resolve_character(cast: CastMap, character_id: str, framework: str) -> str:
    """Return the catalog voice id for one character on one framework."""
    try:
        character = cast.characters[character_id]
    except KeyError:
        known = ", ".join(sorted(cast.characters)) or "(none)"
        raise KeyError(f"character {character_id} is not in the cast map; known: {known}") from None
    try:
        return character.voices[framework]
    except KeyError:
        known = ", ".join(sorted(character.voices))
        raise KeyError(
            f"character {character_id} has no {framework} voice; known frameworks: {known}"
        ) from None


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


def _speed_for(framework: str, speed: float, label: str) -> None:
    if framework in _KOKORO_FRAMEWORKS:
        _check_speed(speed, label)
        return
    if not math.isfinite(speed) or speed <= 0 or speed > 4:
        raise ValueError(f"{label} must be greater than 0 and at most 4, got {speed}")


def _load_characters(raw: object, *, versioned: bool) -> Mapping[str, CharacterCast]:
    if raw is None:
        return MappingProxyType({})
    if not isinstance(raw, dict) or not raw:
        raise ValueError("cast map characters must be a non-empty object")
    if len(raw) > 32:
        raise ValueError("cast map has too many characters")
    found: dict[str, CharacterCast] = {}
    for raw_id, raw_value in raw.items():
        character_id = str(raw_id).strip()
        if _SLUG.fullmatch(character_id) is None:
            raise ValueError(f"character id {raw_id!r} must be a lowercase slug")
        if not isinstance(raw_value, dict):
            raise ValueError(f"character {character_id} must be an object")
        if versioned:
            extra = sorted(set(raw_value) - _CHARACTER_KEYS)
            if extra:
                raise ValueError(f"character {character_id} has unknown fields: {', '.join(extra)}")
        display = _required_str(raw_value, "display_name", label=f"character {character_id}")
        voices_raw = raw_value.get("voices")
        if not isinstance(voices_raw, dict) or not voices_raw:
            raise ValueError(f"character {character_id} voices must be a non-empty object")
        if len(voices_raw) > 8:
            raise ValueError(f"character {character_id} lists too many frameworks")
        voices: dict[str, str] = {}
        for raw_framework, raw_voice in voices_raw.items():
            framework = str(raw_framework).strip()
            if _SLUG.fullmatch(framework) is None:
                raise ValueError(f"character {character_id} framework {raw_framework!r} must be a slug")
            if not isinstance(raw_voice, str) or _VOICE_DOC_ID.fullmatch(raw_voice.strip()) is None:
                raise ValueError(f"character {character_id} voice for {framework} must be a voice id")
            voice_id = raw_voice.strip()
            if not voice_id.startswith(framework + "."):
                raise ValueError(f"character {character_id} {framework} voice id must start with {framework}.")
            voices[framework] = voice_id
        found[character_id] = CharacterCast(character_id, display, MappingProxyType(voices))
    return MappingProxyType(found)


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

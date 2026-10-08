"""Turn a user prompt into a Speaker script and a cast map.

The default path copies the user's words and does not add any. An empty
prompt is an error. The example cast file is never opened.
"""

from __future__ import annotations

import json
import re
from dataclasses import dataclass
from pathlib import Path

from gen_audio.cast import parse_script

_SPEAKER_LINE = re.compile(r"(?i)^speaker\s+\d+\b")
_CHARS_PER_SEC = 13.0
_SPEED_MIN = 0.5
_SPEED_MAX = 2.0

def _catalog_path() -> Path:
    import os

    root = Path(os.environ["GEN_AUDIO_REPO"]) if os.environ.get("GEN_AUDIO_REPO") else Path(__file__).resolve().parents[2]
    return root / "crates" / "gen-audio-core" / "src" / "catalog.rs"


def _personas_from_catalog(text: str) -> dict[str, dict[str, str]]:
    """Display names and kokoro pack ids from the Rust persona catalog."""
    found: dict[str, dict[str, str]] = {}
    for block in re.findall(r"Persona\s*\{(.*?)\n\s*\}", text, re.S):
        id_match = re.search(r'id:\s*"([^"]+)"', block)
        name_match = re.search(r'name:\s*"([^"]+)"', block)
        refs_match = re.search(r"refs:\s*&\[(.*?)\]", block, re.S)
        if id_match is None or name_match is None:
            continue
        refs = re.findall(r'"([^"]+)"', refs_match.group(1) if refs_match else "")
        kokoro = ""
        for ref in refs:
            if ref.startswith("kokoro-pack:"):
                kokoro = ref.split(":", 1)[1]
        found[id_match.group(1)] = {"name": name_match.group(1), "kokoro_onnx": kokoro}
    if len(found) < 9:
        raise RuntimeError(f"Rust catalog parser found {len(found)} personas")
    return found


# Kokoro pack ids come only from catalog.rs refs. Other engines have no pack ref.
PERSONAS: dict[str, dict[str, str]] = _personas_from_catalog(_catalog_path().read_text(encoding="utf-8"))

ENGINE_LANG = {
    "kokoro_onnx": "en-us",
    "pocket_tts": "en",
    "vibevoice": "en",
    "magpie": "en",
}


class PromptError(ValueError):
    """The prompt cannot become a script without inventing words."""


@dataclass(frozen=True)
class PreparedScript:
    """Script text, cast-map JSON, and the words that will be spoken."""

    script_text: str
    cast: dict
    spoken_text: str
    duration_s: float
    speed: float


def voice_for(engine: str, persona_id: str) -> str:
    """Return the speaker id for one persona on one engine."""
    persona = PERSONAS.get(persona_id)
    if persona is None:
        known = ", ".join(sorted(PERSONAS))
        raise PromptError(f"unknown persona {persona_id!r}; known: {known}")
    if engine not in ENGINE_LANG:
        raise PromptError(
            f"unknown voice engine {engine!r}; known: {', '.join(sorted(ENGINE_LANG))}"
        )
    voice = persona.get(engine) or ""
    if not voice:
        raise PromptError(f"{engine} has no speaker id for persona {persona_id} in the Rust catalog")
    return voice


def choose_speed(text: str, duration_s: float) -> float:
    """Pick a kokoro-legal speed so speech aims at ``duration_s`` without new words."""
    estimate = max(len(text), 1) / _CHARS_PER_SEC
    raw = estimate / max(float(duration_s), 0.5)
    return min(_SPEED_MAX, max(_SPEED_MIN, raw))


def prepare_prompt(
    prompt: str,
    personas: list[str],
    engine: str,
    duration_s: float,
) -> PreparedScript:
    """Build a script and cast from the user's text and the selected personas."""
    text = prompt.replace("\r\n", "\n").replace("\r", "\n")
    if not text.strip():
        raise PromptError("empty prompt; no sample script is used")
    if not personas:
        raise PromptError("at least one persona is required")
    if len(personas) > 8:
        raise PromptError("at most 8 personas")
    if len(set(personas)) != len(personas):
        raise PromptError("duplicate persona")
    for persona_id in personas:
        voice_for(engine, persona_id)
    if duration_s < 0.5 or duration_s > 1800:
        raise PromptError("duration must be from 0.5 to 1800 seconds")

    if _has_speaker_headers(text):
        turns = parse_script(text)
        script_text = _format_authored(turns)
        spoken = " ".join(turn.text for turn in turns)
        speaker_ids = [turn.speaker_id for turn in turns]
    else:
        words = text.split()
        if not words:
            raise PromptError("empty prompt; no sample script is used")
        spans = _contiguous_spans(words, len(personas))
        lines: list[str] = []
        spoken_parts: list[str] = []
        speaker_ids = []
        for index, (persona_id, span) in enumerate(zip(personas, spans, strict=True), start=1):
            if not span:
                continue
            body = " ".join(span)
            name = PERSONAS[persona_id]["name"]
            lines.append(f"Speaker {index} ({name}): {body}")
            spoken_parts.append(body)
            speaker_ids.append(str(index))
        if not lines:
            raise PromptError("prompt produced no spoken words")
        script_text = "\n".join(lines) + "\n"
        spoken = " ".join(spoken_parts)
        _assert_words_are_from_prompt(spoken, text)

    speed = choose_speed(spoken, duration_s)
    cast = _cast_map(engine, personas, speed)
    # Authored scripts may use speaker ids the cast must still contain.
    for speaker_id in speaker_ids:
        if speaker_id not in cast["speakers"]:
            raise PromptError(
                f"Speaker {speaker_id} is not one of the {len(personas)} selected personas"
            )
    return PreparedScript(
        script_text=script_text,
        cast=cast,
        spoken_text=spoken,
        duration_s=float(duration_s),
        speed=speed,
    )


def write_prepared(prepared: PreparedScript, script_path: Path, cast_path: Path) -> None:
    """Write the script and cast map. Callers choose paths inside the work directory."""
    script_path.parent.mkdir(parents=True, exist_ok=True)
    script_path.write_text(prepared.script_text, encoding="utf-8")
    cast_path.write_text(json.dumps(prepared.cast, indent=2) + "\n", encoding="utf-8")


def _has_speaker_headers(text: str) -> bool:
    return any(_SPEAKER_LINE.match(line.strip()) for line in text.splitlines())


def _format_authored(turns) -> str:
    lines = []
    for turn in turns:
        name = f" ({turn.script_name})" if turn.script_name else ""
        lines.append(f"Speaker {turn.speaker_id}{name}: {turn.text}")
    return "\n".join(lines) + "\n"


def _contiguous_spans(words: list[str], count: int) -> list[list[str]]:
    """Split words into ``count`` ordered spans. No word is added or dropped."""
    base, extra = divmod(len(words), count)
    spans: list[list[str]] = []
    cursor = 0
    for index in range(count):
        take = base + (1 if index < extra else 0)
        spans.append(words[cursor : cursor + take])
        cursor += take
    return spans


def _assert_words_are_from_prompt(spoken: str, prompt: str) -> None:
    """Spoken tokens must be a subsequence of the prompt tokens."""
    prompt_words = prompt.split()
    cursor = 0
    for word in spoken.split():
        while cursor < len(prompt_words) and prompt_words[cursor] != word:
            cursor += 1
        if cursor >= len(prompt_words):
            raise PromptError("script contains a word that is not in the prompt")
        cursor += 1


def _cast_map(engine: str, personas: list[str], speed: float) -> dict:
    speakers = {}
    for index, persona_id in enumerate(personas, start=1):
        speakers[str(index)] = {
            "name": PERSONAS[persona_id]["name"],
            "voice": voice_for(engine, persona_id),
            "lang": ENGINE_LANG[engine],
            "speed": speed,
        }
    return {
        "engine": engine,
        "default_lang": ENGINE_LANG[engine],
        "default_speed": speed,
        "speakers": speakers,
        "source": "persona-voice-map",
    }

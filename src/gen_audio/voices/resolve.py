"""Tie a cast map to catalog voice ids and framework speaker limits."""

from __future__ import annotations

from gen_audio.cast import CastMap, assignment_framework
from gen_audio.voices.catalog import Catalog
from gen_audio.voices.errors import VoiceError
from gen_audio.voices.registry import ensure_builtins, get_framework


def validate_cast(cast: CastMap, catalog: Catalog | None = None) -> None:
    """Check framework ids, character links, and (when given) the catalog.

    A legacy cast with no ``voice_id`` still loads. Speaker limits apply when
    the speaker's framework is one this package registers.
    """
    ensure_builtins()
    counts: dict[str, int] = {}
    for speaker in cast.speakers.values():
        framework = assignment_framework(speaker, cast)
        counts[framework] = counts.get(framework, 0) + 1
        if speaker.voice_id is not None and not speaker.voice_id.startswith(framework + "."):
            raise VoiceError(
                f"speaker {speaker.speaker_id} voice_id {speaker.voice_id} is not a {framework} voice"
            )
        if speaker.character is not None:
            character = cast.characters.get(speaker.character)
            if character is None:
                raise VoiceError(f"speaker {speaker.speaker_id} character {speaker.character} is not in the cast")
            if speaker.voice_id is not None and character.voices.get(framework) != speaker.voice_id:
                raise VoiceError(
                    f"speaker {speaker.speaker_id} voice_id does not match character {speaker.character}"
                )
        if catalog is not None and speaker.voice_id is not None:
            voice = catalog.voice(speaker.voice_id)
            if voice.framework != framework:
                raise VoiceError(f"speaker {speaker.speaker_id} framework does not match {voice.id}")
            _token_matches(speaker.voice, voice)
    for character in cast.characters.values():
        for framework, voice_id in character.voices.items():
            if not voice_id.startswith(framework + "."):
                raise VoiceError(f"character {character.character_id} {framework} voice id does not match that framework")
            if catalog is not None:
                voice = catalog.voice(voice_id)
                if voice.framework != framework:
                    raise VoiceError(f"character {character.character_id} maps {framework} to {voice.id}")
    for framework, count in counts.items():
        spec = get_framework(framework)
        if spec.kind == "speech_to_speech":
            raise VoiceError(
                f"{framework} is speech-to-speech and cannot fill a text-to-speech cast slot",
                code="not_tts",
            )
        if spec.speaker_limit is not None and count > spec.speaker_limit:
            raise VoiceError(f"{framework} supports at most {spec.speaker_limit} speakers", code="speaker_limit")


def _token_matches(token: str, voice) -> None:
    params = voice.params
    if "blend" in params and "voice_id" not in params:
        raise VoiceError(f"{voice.id} is a blend and has no single pack id for a speaker slot")
    if "voice_id" in params and token != params["voice_id"]:
        raise VoiceError(f"speaker voice {token} does not match {voice.id}")
    if "file_stem" in params and token != params["file_stem"]:
        raise VoiceError(f"speaker voice {token} does not match {voice.id}")
    if "voice_code" in params and token != params["voice_code"]:
        raise VoiceError(f"speaker voice {token} does not match {voice.id}")

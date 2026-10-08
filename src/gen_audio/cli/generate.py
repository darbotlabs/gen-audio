"""CLI for prompt → synth → analysis. Prints one JSON object on stdout."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

from gen_audio.adapters import EngineRefusal, synthesize
from gen_audio.assets import asset_object
from gen_audio.cast import load_cast_map, read_script
from gen_audio.pipeline import run_pipeline
from gen_audio.prompt_script import PromptError, prepare_prompt, write_prepared


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Generate a clip from a user prompt and run the analysis pipeline.")
    parser.add_argument("--prompt-file", type=Path, required=True)
    parser.add_argument("--personas", required=True, help="Comma-separated persona ids, at most 8")
    parser.add_argument("--engine", required=True)
    parser.add_argument("--duration-s", type=float, required=True)
    parser.add_argument("--out-dir", type=Path, required=True)
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    out_dir = args.out_dir.resolve()
    prompt_path = args.prompt_file.resolve()
    try:
        prompt_path.relative_to(out_dir)
    except ValueError:
        _emit(out_dir, {"ok": False, "synthesizedSpeech": False, "reason": "prompt file is outside the work directory"})
        return 2
    personas = [part.strip() for part in args.personas.split(",") if part.strip()]
    try:
        prompt = prompt_path.read_text(encoding="utf-8")
        prepared = prepare_prompt(prompt, personas, args.engine, args.duration_s)
        script_path = out_dir / "scripts" / "script.txt"
        cast_path = out_dir / "cast.json"
        write_prepared(prepared, script_path, cast_path)
        script_asset = asset_object(script_path, kind="script", derived_from=[], duration_s=None)
        turns = read_script(script_path)
        cast = load_cast_map(cast_path)
        raw_path = out_dir / "raw.wav"
        synthesize(args.engine, turns, cast, raw_path)
        manifest = run_pipeline(
            raw_path,
            out_dir=out_dir,
            engine=args.engine,
            reference_text=prepared.spoken_text,
            duration_target_s=prepared.duration_s,
            script_asset=script_asset,
        )
        manifest["spokenText"] = prepared.spoken_text
        manifest["speed"] = prepared.speed
        manifest["castSource"] = "persona-voice-map"
        manifest["sampleScript"] = False
    except (PromptError, EngineRefusal, OSError, ValueError, KeyError, RuntimeError) as exc:
        _emit(out_dir, {
            "ok": False,
            "synthesizedSpeech": False,
            "synthesized": False,
            "engine": args.engine,
            "reason": str(exc),
            "sampleScript": False,
        })
        return 0
    _emit(out_dir, manifest)
    return 0


def _emit(out_dir: Path, manifest: dict) -> None:
    out_dir.mkdir(parents=True, exist_ok=True)
    (out_dir / "manifest.json").write_text(json.dumps(manifest) + "\n", encoding="utf-8")
    summary = {
        "ok": bool(manifest.get("ok")),
        "synthesizedSpeech": manifest.get("synthesizedSpeech") is True,
        "manifest": "manifest.json",
        "reason": manifest.get("reason", ""),
        "uid": (manifest.get("assets") or {}).get("wav", {}).get("uid", ""),
        "duration_s": manifest.get("duration_s"),
        "durationHonoured": manifest.get("durationHonoured"),
        "speech_s": manifest.get("speech_s"),
    }
    print(json.dumps(summary))


if __name__ == "__main__":
    raise SystemExit(main())

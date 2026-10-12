"""CLI: synthesize a Speaker script with kokoro-onnx."""

from __future__ import annotations

import argparse
from contextlib import redirect_stdout
import json
import os
import shutil
import sys
from pathlib import Path

import numpy as np
import soundfile as sf

from gen_audio.artifacts import check_destinations, publication_path, resolve_artifact_dir, staged_files
from gen_audio.audio_io import read_wav, validate_frame_count
from gen_audio.audio_metadata import tag_wav
from gen_audio.cast import load_cast_map, read_script
from gen_audio.library_manifest import OUTSIDE, file_facts, label_work_dir_paths, outside_repo_label
from gen_audio.guards import max_generation_seconds, sha256_file
from gen_audio.synthesis import SynthResult, validate_generated_audio, validate_request
from gen_audio.synth_kokoro_onnx import KokoroOnnxSynthesizer, resolve_model_paths


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Synthesize a multi-speaker script with kokoro-onnx. Models are local files; nothing is downloaded."
    )
    parser.add_argument("script", type=Path, help="UTF-8 script with Speaker N turns")
    parser.add_argument("--cast-map", type=Path, required=True, help="cast map JSON (engine kokoro_onnx)")
    parser.add_argument("-o", "--output", type=Path, required=True, help="output WAV path")
    parser.add_argument("--model", help="path to the kokoro ONNX model (or GEN_AUDIO_KOKORO_MODEL)")
    parser.add_argument("--voices", help="path to the kokoro voices bin (or GEN_AUDIO_KOKORO_VOICES)")
    parser.add_argument("--gap", type=float, default=0.35, help="silence between turns, in seconds")
    parser.add_argument("--json", action="store_true", help="end stdout with a versioned JSON success receipt")
    parser.add_argument("--include-text", action="store_true", help="opt in to raw turn text in the manifest")
    parser.add_argument("--max-duration", type=float, help="tighten the maximum output duration (ceiling: 600s)")
    parser.add_argument(
        "--generation-timeout", type=float,
        help="cooperative inference deadline in seconds (ceiling: 600s); hosts must enforce hard cancellation",
    )
    parser.add_argument(
        "--improve",
        action="store_true",
        help="also run the 24 kHz publish chain on the concatenated render",
    )
    parser.add_argument(
        "--artifact-dir",
        help="copy the WAV and manifest here (or set GEN_AUDIO_ARTIFACT_DIR)",
    )
    return parser


# Default repo root for the sidecar's input paths: the checkout this module runs from.
REPO_ROOT = Path(__file__).resolve().parents[3]


def sidecar_payload(manifest: dict, subtype: str, inputs: dict[str, Path], repo_root: Path) -> dict:
    """The JSON written next to the WAV: the synth result, the WAV subtype, and
    the input files as provenance: repo-relative inside the repo, an
    ``<outside-repo>/...`` label outside it (never ``../`` or absolute: the
    sidecar can ship in dist), with each outside file's sha256 and size in
    ``outside_repo``."""
    payload = dict(manifest)
    payload["wav_subtype"] = subtype
    labels = {name: outside_repo_label(os.path.abspath(os.fspath(path)), os.path.abspath(os.fspath(repo_root))) for name, path in inputs.items()}
    payload["inputs"] = labels
    outside = {labels[name]: file_facts(Path(path)) for name, path in inputs.items() if labels[name].startswith(OUTSIDE + "/")}
    if outside:
        payload["outside_repo"] = dict(sorted(outside.items()))
    # C1: the writer holds the same rule the sync does: every recorded path
    # resolves from the repo root or is a label with facts.
    problems = label_work_dir_paths(payload, Path(repo_root), None, {}, "sidecar")
    if problems:
        raise ValueError("; ".join(problems))
    return payload


def _destinations(output: Path, artifact_dir: str | None) -> list[Path]:
    if output.suffix.lower() != ".wav":
        raise ValueError("output must have a .wav extension")
    wavs = [publication_path(output)]
    root = resolve_artifact_dir(artifact_dir)
    if root is not None:
        durable = publication_path(root / output.name)
        if durable != wavs[0]:
            wavs.append(durable)
    return check_destinations(wavs + [wav.with_suffix(".json") for wav in wavs])


def publish_synthesis(
    result: SynthResult, destinations: list[Path], *, include_text: bool = False,
    inputs: dict[str, Path] | None = None,
) -> dict:
    """Commit WAVs before matching manifests; collisions never overwrite existing files."""
    if len(destinations) not in {2, 4}:
        raise ValueError("publication requires one or two WAV/manifest pairs")
    from gen_audio.guards import max_audio_duration
    validate_generated_audio(result.audio, result.sample_rate, max_audio_duration())
    count = len(destinations) // 2
    subtype = "PCM_16" if result.improved else "FLOAT"
    with staged_files(destinations) as staged:
        sf.write(staged[0], result.audio, result.sample_rate, subtype=subtype, format="WAV")
        metadata = tag_wav(staged[0], list(dict.fromkeys(turn.name for turn in result.turns)))
        audio, rate = read_wav(staged[0])
        validate_frame_count(audio.size, rate)
        if rate != result.sample_rate or audio.size != result.audio.size or not np.any(audio):
            raise ValueError("stored WAV does not match the non-empty speech render")
        digest = sha256_file(staged[0])
        payload = sidecar_payload(result.manifest(include_text=include_text), subtype, inputs or {}, REPO_ROOT)
        payload.update({
            "wav_subtype": subtype, "sha256": digest, "bytes": staged[0].stat().st_size,
            "sample_rate": rate, "samples": int(audio.size), "duration_s": audio.size / rate,
            "metadata": metadata,
        })
        for index in range(count):
            if index:
                shutil.copyfile(staged[0], staged[index])
                if sha256_file(staged[index]) != digest:
                    raise OSError("artifact copy checksum mismatch")
            payload["output"] = str(destinations[index])
            payload["manifest"] = str(destinations[count + index])
            output_label = outside_repo_label(str(destinations[index]), REPO_ROOT)
            payload["inputs"]["output"] = output_label
            if output_label.startswith(OUTSIDE + "/"):
                payload.setdefault("outside_repo", {})[output_label] = {
                    "sha256": digest, "bytes": staged[index].stat().st_size,
                }
            staged[count + index].write_text(
                json.dumps(payload, indent=2, allow_nan=False) + "\n", encoding="utf-8",
            )
        receipt = {
            "schema_version": "1.0", "ok": True, "synthesizedSpeech": True,
            "engine": result.engine, "output": str(destinations[count - 1]),
            "manifest": str(destinations[-1]), "sample_rate": rate, "samples": int(audio.size),
            "duration_s": audio.size / rate, "sha256": digest,
            "metadata": metadata,
        }
    return receipt


def main(argv: list[str] | None = None, *, resident=None) -> int:
    args = build_parser().parse_args(argv)
    try:
        turns = read_script(args.script)
        cast = load_cast_map(args.cast_map)
        validate_request(turns, cast, "kokoro_onnx", args.gap, args.max_duration)
        timeout = max_generation_seconds(args.generation_timeout)
        destinations = _destinations(args.output, args.artifact_dir)
        model_path, voices_path = resolve_model_paths(args.model, args.voices)
        with redirect_stdout(sys.stderr):
            synth = resident if resident is not None else KokoroOnnxSynthesizer(model_path, voices_path)
            if resident is not None:
                synth.validate_loaded_assets(model_path, voices_path)
            result = synth.synthesize_turns(
                turns, cast, gap_s=args.gap, improve_publish=args.improve,
                max_duration_s=args.max_duration, generation_timeout_s=timeout,
            )
        receipt = publish_synthesis(
            result, destinations, include_text=args.include_text,
            inputs={"script": args.script, "cast_map": args.cast_map, "model": model_path, "voices": voices_path},
        )
    except (OSError, ValueError, KeyError, RuntimeError, ImportError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    if args.json:
        print(json.dumps(receipt, allow_nan=False))
    else:
        print(
            f"wrote {receipt['output']} ({result.sample_rate} Hz, {result.audio.size} samples, "
            f"{len(result.turns)} turns); manifest {receipt['manifest']}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

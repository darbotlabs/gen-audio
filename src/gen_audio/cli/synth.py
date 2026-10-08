"""CLI: synthesize a Speaker script with kokoro-onnx."""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

from gen_audio.artifacts import publish_copy
from gen_audio.audio_io import write_wav
from gen_audio.cast import load_cast_map, read_script
from gen_audio.library_manifest import OUTSIDE, file_facts, label_work_dir_paths, outside_repo_label
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


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        turns = read_script(args.script)
        cast = load_cast_map(args.cast_map)
        model_path, voices_path = resolve_model_paths(args.model, args.voices)
        synth = KokoroOnnxSynthesizer(model_path, voices_path)
        result = synth.synthesize_turns(turns, cast, gap_s=args.gap, improve_publish=args.improve)
        # Raw engine samples can sit outside [-1, 1]. PCM_16 would clip them
        # before improve() gets a chance to set the 0.89 peak. Float keeps the
        # render; the publish chain writes PCM_16 once the peak is limited.
        subtype = "PCM_16" if result.improved else "FLOAT"
        write_wav(args.output, result.audio, result.sample_rate, subtype=subtype)
        manifest_path = args.output.with_suffix(".json")
        inputs = {"script": args.script, "cast_map": args.cast_map, "model": model_path, "voices": voices_path, "output": args.output}
        payload = sidecar_payload(result.manifest(), subtype, inputs, REPO_ROOT)
        manifest_path.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
        durable_wav = publish_copy(args.output, args.artifact_dir)
        durable_manifest = publish_copy(manifest_path, args.artifact_dir)
    except (OSError, ValueError, KeyError, RuntimeError, ImportError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    print(
        f"wrote {durable_wav} ({result.sample_rate} Hz, {result.audio.size} samples, "
        f"{len(result.turns)} turns); manifest {durable_manifest}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

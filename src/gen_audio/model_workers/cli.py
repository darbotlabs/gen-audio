"""One JSON receipt on stdout, backend diagnostics on stderr; exit 2 on blockers."""

from __future__ import annotations

import argparse
from contextlib import redirect_stdout
import json
from pathlib import Path
import sys

from .contracts import MAX_TEXT_BYTES, Request, WorkerError
from .preflight import local_file, preflight
from .runtime import execute


class Parser(argparse.ArgumentParser):
    def error(self, message):
        raise WorkerError("invalid_arguments", message)


def parser() -> argparse.ArgumentParser:
    result = Parser(description=__doc__, epilog=(
        "CPU, CUDA and ROCm use one execution contract in isolated Python 3.12 runtimes. "
        "WAV inputs/references must be mono 24 kHz PCM16. Speaker N maps to the Nth "
        "--voice-ref (1..4). PersonaPlex generates a response, not verbatim TTS. "
        "The caller should enforce an OS subprocess timeout in addition to this "
        "worker's cooperative --timeout-seconds checks."
    ))
    result.add_argument("--engine", choices=("vibevoice", "personaplex"), required=True)
    result.add_argument("--model-dir", type=Path, required=True)
    result.add_argument("--output", type=Path)
    text = result.add_mutually_exclusive_group()
    text.add_argument("--text")
    text.add_argument("--input-text", type=Path, help="UTF-8 plain text or Speaker N: lines")
    result.add_argument("--input-audio", type=Path, help="PersonaPlex user audio, not a TTS script")
    result.add_argument("--persona", help="PersonaPlex system persona, not words to read verbatim")
    result.add_argument("--voice-ref", type=Path, action="append", default=[])
    result.add_argument("--tokenizer", type=Path, help="VibeVoice local tokenizer directory or PersonaPlex SentencePiece file")
    result.add_argument("--device", default="cpu")
    result.add_argument("--execution", help="JSON execution object: backend, device, device_index, precision, fallback")
    result.add_argument("--max-new-tokens", type=int, default=256,
                        help="VibeVoice autoregressive tokens or PersonaPlex input stream steps")
    result.add_argument("--max-seconds", type=float, default=30.0, help="Audio duration cap, not a requested duration")
    result.add_argument("--timeout-seconds", type=float, default=600.0)
    result.add_argument("--seed", type=int, default=0)
    result.add_argument("--agent-name", action="append", default=[], help="Agent display name for output Artist attribution")
    result.add_argument("--preflight", action="store_true", help="No model weights loaded; accelerator selection probes its runtime")
    return result


def main(argv: list[str] | None = None, *, resident=None) -> int:
    engine = None
    try:
        args = parser().parse_args(argv)
        engine = args.engine
        from gen_audio.audio_metadata import agent_metadata
        agent_metadata(args.agent_name)
        text = args.text
        if args.input_text is not None:
            path = local_file(args.input_text, max_bytes=MAX_TEXT_BYTES)
            with path.open("rb") as handle:
                raw = handle.read(MAX_TEXT_BYTES + 1)
            if len(raw) > MAX_TEXT_BYTES:
                raise WorkerError("resource_limit", "Input text exceeds the byte limit")
            text = raw.decode("utf-8-sig")
        request = Request(
            engine=args.engine, model_dir=args.model_dir, output=args.output, text=text,
            input_audio=args.input_audio, persona=args.persona, voice_refs=tuple(args.voice_ref),
            tokenizer=args.tokenizer, device=args.device, max_new_tokens=args.max_new_tokens,
            max_seconds=args.max_seconds, timeout_seconds=args.timeout_seconds, seed=args.seed,
            agent_names=tuple(args.agent_name),
            execution=json.loads(args.execution) if args.execution is not None else None,
        )
        if resident is not None and (not request.model_dir.samefile(resident["model_dir"])
                                    or request.tokenizer is None
                                    or not request.tokenizer.samefile(resident["tokenizer_path"])):
            raise WorkerError("runtime_changed", "Loaded runtime differs from the configured request; reload it")
        if resident is not None:
            from .execution import preferences
            from dataclasses import asdict
            if resident.get("execution", {}).get("requested") != asdict(preferences(request)):
                raise WorkerError("runtime_changed", "Execution configuration differs from the loaded model; reload it")
        with redirect_stdout(sys.stderr):
            receipt = preflight(request, resident=resident is not None) if args.preflight else (
                execute(request, resident=resident) if resident is not None else execute(request))
        print(json.dumps(receipt, allow_nan=False))
        return 0 if receipt["ok"] else 2
    except (WorkerError, OSError, ValueError, RuntimeError, ImportError) as exc:
        receipt = {
            "schema_version": "1.0", "ok": False, "synthesizedSpeech": False, "engine": engine,
            "errors": [{"code": getattr(exc, "code", "worker_failed"),
                        "message": str(exc) if isinstance(exc, WorkerError) else
                        f"Local worker failed ({type(exc).__name__}): {str(exc)[:2000]}. No speech was published"}],
        }
        print(json.dumps(receipt, allow_nan=False))
        return 2

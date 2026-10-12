"""Record reviewed source parity and reproducible source fingerprints.

This is a review ledger, not an inference benchmark. Missing execution evidence
never becomes a passing engine result merely because an adapter exists.
"""
from __future__ import annotations

import ast
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCE = Path(r"\\fschia\darbot\Gen.AID.Audio")


def feature(name, status, source, target, finding):
    return dict(feature=name, status=status, source=source, target=target, finding=finding)


FEATURES = [
    feature("One model owner", "partial", ["api_server.py"], ["crates/gen-audio-core/src/runtime_owner.rs", "src/gen_audio/runtime_owner.py"],
            "Native-owned resident Kokoro/VibeVoice/PersonaPlex workers exist. Repeated native engine_load still replaces the resident process. The desktop MCP sidecar itself is not yet protected by the model-worker Windows job mechanism."),
    feature("Multi-speaker script generation", "implemented_unvalidated_vibevoice", ["api_server.py", "inference_from_file.py"], ["crates/gen-audio-core/src/studio.rs", "src/gen_audio/model_workers/vibevoice.py", "apps/desktop/src/speech-request.ts"],
            "Desktop supplied scripts, 1..4 speakers, explicit managed references, local runtime and checkpoint validation are wired to VibeVoice. Real generation remains blocked on this machine by available CPU memory."),
    feature("Managed voice library", "implemented", ["api_server.py", "genaid-audio.py"], ["voices/manifest.json", "crates/gen-audio-core/src/voice_library.rs", "apps/desktop/src/agent-profile-library.ts"],
            "Nine agent profiles, seven engine descriptors per profile and nine hash-pinned reference recordings. Profile tab, search, engine filters, samples, linked projects and JSON/Adaptive Card/schema downloads are present. Native artist metadata comes from AgentName."),
    feature("Persistent profile authoring", "partial", ["genaid-audio.py"], ["crates/gen-audio-mcp/src/control.rs", "apps/desktop/src/profiles.ts"],
            "Built-in profile browsing/export is available. Persistent user-created profiles, microphone sample capture and reference editing are not complete; library_harvest updates process state only."),
    feature("VibeVoice CFG and diffusion controls", "missing", ["api_server.py", "genaid-audio.py"], ["src/gen_audio/model_workers/vibevoice.py"],
            "The source framework exposes cfg_scale and inference_steps. Unified worker currently fixes cfg_scale=1.3 and DDPM steps=10; no desktop/MCP configuration path exists."),
    feature("Audio streaming and stop", "partial", ["api_server.py", "genaid-audio.py"], ["src/gen_audio/model_workers/vibevoice.py", "crates/gen-audio-mcp/src/control.rs", "crates/gen-audio-core/src/model_worker.rs"],
            "Source FastAPI streams native PCM chunks and supports cooperative generation stop. Unified desktop waits for a complete validated WAV. Prompt jobs have cancellation, but direct synth/model/load calls do not expose equivalent cancellation or chunk streaming."),
    feature("Duration and token controls", "partial", ["api_server.py"], ["src/gen_audio/model_workers/contracts.py", "src/gen_audio/model_workers/vibevoice.py"],
            "Unified worker enforces an output ceiling of 60 seconds/1024 tokens and marks capped output incomplete. Source framework also supports target duration with terminal-token reserve and a higher bounded token limit; those semantics are not equivalent."),
    feature("Tokenizer validation", "partial", ["api_server.py"], ["src/gen_audio/model_workers/vibevoice.py", "src/gen_audio/model_workers/checkpoints.py"],
            "Unified worker loads only a local Qwen2-compatible tokenizer and validates token/vocabulary bounds and checkpoint shapes. The source framework's additional ordinary-text encoding parity and declared tokenizer revision checks are not fully carried over."),
    feature("Generation telemetry and turn timing", "partial", ["api_server.py"], ["src/gen_audio/model_workers/runtime.py", "src/gen_audio/model_workers/vibevoice.py"],
            "Unified manifests retain hashes, engine/runtime/dependencies, references, seed, tokens, caps and output metadata. Source chunk latency, CPU/RSS/RTF metrics, Prometheus endpoint and inferred silence-aligned per-turn timing are not integrated."),
    feature("Audio mastering", "implemented", ["api_server.py"], ["crates/gen-audio-core/src/mastering.rs", "apps/desktop/src/authoring-panel.ts"],
            "Actual FFmpeg filtering/compression and two-pass loudness mastering preserve the source WAV and Artist, with independent final measurement and downloadable report. Exposed as an explicit desktop/MCP action."),
    feature("Saved audio and provenance", "implemented", ["api_server.py"], ["crates/gen-audio-core/src/generation_store.rs", "src/gen_audio/audio_metadata.py", "crates/gen-audio-core/src/audio_metadata.rs"],
            "Unified immutable audio/manifests reopen through a validated local asset library. WAV IART and JSON Artist retain exact AgentName values, including multiple agents."),
    feature("Script authoring", "partial", ["genaid-audio.py"], ["crates/gen-audio-core/src/authoring.rs", "crates/gen-audio-mcp/src/authoring.rs", "apps/desktop/src/authoring-panel.ts"],
            "Script analysis, speaker indexing conversion, draft planning and provider-backed drafting are wired. Source local Phi-3-mini/Phi-4 model ownership/generation is not implemented in the unified runtime."),
    feature("Existing HTTP clients", "missing", ["api_server.py", "gradio_api_client.py", "generate_genaid_audio.py"], ["crates/gen-audio-mcp/src/http.rs"],
            "Unified app exposes authenticated MCP and artifacts. Existing /tts, /tts/multi-speaker/stream, /generations and telemetry API contracts are not compatibility endpoints in the new backend."),
    feature("Qwen3 codec evaluation", "missing", ["benchmark_qwen3_codec.py"], [],
            "Source includes a separately pinned Qwen3 speech-codec encode/decode benchmark and reconstruction evidence. It is not a replacement for VibeVoice's text or speech tokenizer and is not exposed in the unified framework."),
    feature("PersonaPlex", "worker_only_unvalidated", [], ["src/gen_audio/model_workers/personaplex.py", "crates/gen-audio-core/src/model_worker.rs", "crates/gen-audio-core/src/runtime_owner.rs"],
            "Local speech-to-speech worker, model loading and MCP generation contracts exist. Desktop conversation/microphone/session controls are absent. PersonaPlex has no engine card and its catalog note is stale. No isolated PersonaPlex environment is installed here, and CPU memory is insufficient for its estimated 40 GiB minimum."),
    feature("Kokoro ONNX", "real_inference_verified", [], ["src/gen_audio/synth_kokoro_onnx.py", "src/gen_audio/synthesis.py", "crates/gen-audio-core/src/bridge.rs"],
            "Real local ONNX inference, multi-speaker script assembly, owned model loading, checksummed WAV/manifest publication, AgentName Artist, save/reopen and mastering are implemented. ONNX is the execution format/runtime for this Kokoro adapter, not a separate voice persona."),
    feature("Kokoro Torch / Misaki", "partial", [], ["src/gen_audio/engines.py", "src/gen_audio/adapters.py", "crates/gen-audio-core/src/catalog.rs"],
            "Imported Kokoro Torch and Misaki-to-Kokoro audio/cubes are preserved. There is no unified Kokoro Torch generation adapter or direct Misaki G2P route; Kokoro ONNX uses its package phonemizer."),
    feature("Magpie / Pocket TTS", "missing_adapters", [], ["src/gen_audio/engines.py", "crates/gen-audio-core/src/catalog.rs"],
            "Profiles describe these engines, but there are no unified live inference adapters. Imported examples/descriptors are not evidence of generation support."),
]


def fingerprint(root, name):
    path = root / name
    if not path.is_file():
        return {"path": name, "missing": True}
    raw = path.read_bytes()
    text = raw.decode("utf-8-sig")
    result = {"path": name, "sha256": hashlib.sha256(raw).hexdigest(), "lines": len(text.splitlines())}
    if path.suffix == ".py":
        tree = ast.parse(text, filename=name)
        result["symbols"] = [{"name": item.name, "line": item.lineno, "endLine": item.end_lineno}
                             for item in ast.walk(tree) if isinstance(item, (ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef))]
    return result


def main():
    source = sorted({path for item in FEATURES for path in item["source"]})
    target = sorted({path for item in FEATURES for path in item["target"]})
    report = {
        "reviewedAt": datetime.now(timezone.utc).isoformat(),
        "fullyIntegrated": False,
        "basis": "Reviewed executable source and current local validation. Status is per capability, not inferred from labels, profiles or imported WAVs.",
        "sourceRoot": str(SOURCE), "targetRoot": str(ROOT),
        "features": FEATURES,
        "sourceSnapshot": [fingerprint(SOURCE, path) for path in source],
        "targetSnapshot": [fingerprint(ROOT, path) for path in target],
        "evidence": ["artifacts/validation/viewport-cleanup/browser-validation.json",
                     "artifacts/validation/viewport-cleanup/vibevoice-preflight.json",
                     "artifacts/validation/agent-profiles/mcp-artist-validation.json"],
        "currentChanges": ["Tabs derive from rendered slides; seven release slides have seven buttons.",
                           "Removed explanatory header/footer copy and moved indicators under tabs.",
                           "Playing-audio arrow, stop/close X, persistent pause/resume, and one active transport across library, reference, generated and mastered audio.",
                           "Correlated native playback events prevent stale local echoes undoing Pause/Close.",
                           "Frontend model loading/preflight timeout allows the existing native operation to finish.",
                           "Enabled exact JSON float round trips after reproducing intact mastering report rejection at a 240004-sample WAV duration; strict persistence checks remain enabled."],
    }
    assert all(not row.get("missing") for rows in (report["sourceSnapshot"], report["targetSnapshot"]) for row in rows)
    out = ROOT / "docs/framework-integration-audit.json"
    out.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(json.dumps({"report": str(out), "fullyIntegrated": False, "capabilities": len(FEATURES), "sourceFiles": len(source), "targetFiles": len(target)}))


if __name__ == "__main__":
    main()

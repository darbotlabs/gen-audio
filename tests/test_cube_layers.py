"""Four-layer Library cube (gen_audio.cube_layers, cube_revision.py layers)."""

from __future__ import annotations

import hashlib
import json
import shutil
import subprocess
from pathlib import Path

import numpy as np
import pytest

from gen_audio.audio_io import read_wav, write_wav
from gen_audio.cli.cube import main as cube_main
from gen_audio.cube_layers import (
    GENERATOR_PATH,
    LAYER_METHOD,
    LAYER_NAMES,
    LIBRARY_PARAMS,
    LIBRARY_REVISION,
    REPO_ROOT,
    CubeParams,
    check_generator,
    compute_layers,
    cube_json_name,
    cube_png_name,
    generator_sha256,
    library_cube,
    pipeline_r2_stft,
)
from gen_audio.library_manifest import generator_commit_for, normalized_sha256, record_generator_commits
from gen_audio.cube_revision import measure

LIBRARY = Path(__file__).resolve().parents[1] / "apps" / "desktop" / "public" / "library"
SHA = "0" * 64


def _speechish(seconds: float = 3.0, sr: int = 24000) -> np.ndarray:
    t = np.arange(int(seconds * sr)) / sr
    env = 0.5 + 0.5 * np.sin(2 * np.pi * 3 * t)
    tone = sum(np.sin(2 * np.pi * f * t) / k for k, f in enumerate((220, 440, 880, 1760, 3520), start=1))
    noise = np.random.default_rng(0).normal(0, 0.01, len(t))
    return (0.3 * env * tone / 2.3 + noise).astype(np.float32)


def test_library_cube_shape_and_scrub_mapping():
    audio, sr = _speechish(), 24000
    doc, _cloud = library_cube(audio, sr, stem="tone", engine="fixture", source_sha256=SHA)
    params = CubeParams()
    nf, nt = doc["cube_shape_f_t"]
    sf, st = doc["downsample_sf_st"]
    assert nf == (params.n_fft // 2 + 1) // sf and nt <= params.max_t
    assert doc["bin_seconds"] == st * params.hop / sr
    assert doc["cube_covers_s"] == pytest.approx(nt * doc["bin_seconds"])
    assert doc["cube_covers_s"] <= doc["duration_s"] + doc["bin_seconds"]  # STFT padding
    assert doc["cube_revision"] == 3 and doc["wavUrl"] == "/library/tone.wav"
    # layer_method is provenance, not a top-level cube key.
    assert "layer_method" not in doc
    assert doc["provenance"]["layer_method"] == "library_r3"
    assert set(doc["layers"]) == set(LAYER_NAMES)
    assert 0 < doc["inv_hdr"] < 1 and 0 < doc["layer_score"] < 1
    assert doc["inv_hdr"] == pytest.approx(measure(audio, sr).inv_hdr)
    for point in doc["points_preview"]:
        assert 0 <= point["t"] <= 1 and 0 <= point["f"] <= 1 and 0 <= point["v"] <= 1
    assert doc["n_points"] == len(doc["points_preview"]) <= params.per_layer * 4


def test_library_cube_is_deterministic():
    audio = _speechish(1.5)
    first, _ = library_cube(audio, 24000, stem="a", engine="e", source_sha256=SHA)
    second, _ = library_cube(audio.copy(), 24000, stem="a", engine="e", source_sha256=SHA)
    assert json.dumps(first) == json.dumps(second)


def test_pipeline_r2_reproduces_archived_pr4_statistics():
    doc, _ = library_cube(_speechish(), 24000, stem="tone", engine="fixture",
                          source_sha256=SHA, method="pipeline_r2")
    golden = {
        "signal": [0.07769936901840238, 0.14810095425377648, 0.041461231175063576, 0.047991161636554605, 0.058823529411764705],
        "tonality": [0.36317973976193213, 0.0023611153035942673, 0.36355081156938784, 0.3659553289829641, 1.0],
        "confidence": [0.5194238779828132, 0.11192286081505017, 0.4888717837944977, 0.5254118540396466, 1.0],
        "quality": [0.2204395543901673, 0.07406380961061054, 0.20243245185227643, 0.20597433867168702, 0.8762254901960784],
    }
    assert doc["cube_shape_f_t"] == [102, 8]
    assert doc["downsample_sf_st"] == [5, 33]
    assert doc["bin_seconds"] == 33 * 256 / 24000
    assert doc["inv_hdr"] == measure(_speechish(), 24000).inv_hdr
    assert doc["cube_revision"] == 2
    assert doc["provenance"]["layer_method"] == "pipeline_r2"
    assert doc["provenance"]["generator_sha256"] == generator_sha256()
    assert doc["layer_method_source"].startswith("darbotlabs/gen-audio@204d0de")
    for name, expected in golden.items():
        got = doc["layers"][name]
        assert got["shape"] == [102, 8]
        assert [got[key] for key in ("mean", "std", "p50", "p90", "active_frac")] == pytest.approx(expected, rel=0, abs=1e-12)


def test_pipeline_r2_formulas_and_default_method():
    audio = _speechish(2)
    mag = pipeline_r2_stft(audio)
    layers = compute_layers(mag, 24000, 1024, method="pipeline_r2")
    nf, nt = mag.shape[0] // 5, mag.shape[1] // 33
    grid = mag[:nf * 5, :nt * 33].reshape(nf, 5, nt, 33).mean(axis=(1, 3))
    np.testing.assert_array_equal(layers["signal"], grid / grid.max())
    np.testing.assert_array_equal(layers["confidence"], grid / (grid + np.percentile(grid, 70) + 1e-12))
    np.testing.assert_array_equal(layers["quality"], 0.5 * layers["signal"] + 0.5 * layers["tonality"])
    default, _ = library_cube(audio, 24000, stem="x", engine="fixture", source_sha256=SHA)
    explicit, _ = library_cube(audio, 24000, stem="x", engine="fixture", source_sha256=SHA, method="library_r3")
    assert default == explicit
    with pytest.raises(ValueError, match="unknown layer_method"):
        library_cube(audio, 24000, stem="x", engine="fixture", source_sha256=SHA, method="unknown")
    with pytest.raises(ValueError, match="too short"):
        library_cube(audio[:2000], 24000, stem="x", engine="fixture", source_sha256=SHA, method="pipeline_r2")


@pytest.mark.parametrize(("stem", "shape", "means"), [
    ("genaid_full_misaki_kokoro", [102, 395],
     [0.01915429562396228, 0.49622658792574514, 0.35640301962363075, 0.2576904417748537]),
    ("bitdot_braille_vibevoice", [102, 701],
     [0.02465064335400211, 0.5794847820936125, 0.29731470571381385, 0.30206771272380734]),
])
def test_pipeline_r2_matches_archived_library_wav_statistics(stem, shape, means):
    wav = _need_wav(stem)
    audio, rate = read_wav(wav)
    doc, _ = library_cube(audio, rate, stem=stem, engine="fixture",
                          source_sha256=hashlib.sha256(wav.read_bytes()).hexdigest(), method="pipeline_r2")
    assert doc["cube_shape_f_t"] == shape
    assert [doc["layers"][name]["mean"] for name in LAYER_NAMES] == pytest.approx(means, rel=0, abs=1e-12)


def test_pipeline_r2_cli_and_names(tmp_path):
    wav = write_wav(tmp_path / "tone.wav", _speechish(1), 24000)
    out, png = tmp_path / "comparison.json", tmp_path / "comparison.png"
    assert cube_main(["layers", str(wav), str(out), "--stem", "tone", "--engine", "fixture",
                      "--method", "pipeline_r2", "--png", str(png)]) == 0
    doc = json.loads(out.read_text(encoding="utf-8"))
    assert doc["source_sha256"] == hashlib.sha256(wav.read_bytes()).hexdigest()
    assert doc["layer_method"] == doc["provenance"]["layer_method"] == "pipeline_r2"
    assert doc["compare_to"] == "/library/library_tone_cube3d.json"
    assert doc["pngUrl"] == "/library/tone_pipeline_r2_cube3d.png"
    assert cube_png_name("tone", "pipeline_r2") == "tone_pipeline_r2_cube3d.png"
    assert cube_json_name("library_kokoro_onnx") == "library_kokoro_onnx_cube3d.json"
    assert cube_json_name("tone", "pipeline_r2") == "library_tone_pipeline_r2_cube3d.json"
    assert png.read_bytes().startswith(b"\x89PNG")
    with pytest.raises(SystemExit):
        cube_main(["layers", str(wav), str(out), "--stem", "tone", "--engine", "fixture", "--method", "unknown"])


def test_manifest_discovery_does_not_replace_default_with_comparison(tmp_path):
    from gen_audio.library_manifest import sync_manifest

    for method in ("library_r3", "pipeline_r2"):
        doc, _ = library_cube(_speechish(1), 24000, stem="tone", engine="fixture",
                              source_sha256=SHA, method=method)
        (tmp_path / cube_json_name("tone", method)).write_text(json.dumps(doc), encoding="utf-8")
    manifest = tmp_path / "manifest.json"
    manifest.write_text(json.dumps({"clips": [{"id": "tone", "status": "ok", "wavUrl": "/library/tone.wav"}]}))
    sync_manifest(manifest)
    clip = json.loads(manifest.read_text())["clips"][0]
    assert clip["cube"]["jsonUrl"] == "/library/library_tone_cube3d.json"
    assert clip["cube"]["cube_revision"] == 3


def test_library_cube_rejects_empty_audio():
    with pytest.raises(ValueError):
        library_cube(np.zeros(0, np.float32), 24000, stem="x", engine="e", source_sha256=SHA)


def test_library_cube_records_rev3_provenance_and_rejects_bad_ids():
    doc, _ = library_cube(_speechish(1.0), 24000, stem="a", engine="e", source_sha256=SHA)
    assert doc["cube_revision"] == LIBRARY_REVISION == 3
    assert doc["source_sha256"] == SHA
    assert doc["provenance"] == {
        "generator": "src/gen_audio/cube_layers.py",
        "generator_sha256": generator_sha256(),
        "layer_method": "library_r3",
        "params": {"n_fft": 1024, "hop": 256, "max_f": 128, "max_t": 400, "thresh": 0.12, "per_layer": 900},
    }
    assert LIBRARY_PARAMS == CubeParams()
    with pytest.raises(ValueError):
        library_cube(_speechish(1.0), 24000, stem="a", engine="e", source_sha256="ABC")


def test_generator_path_exists():
    """provenance.generator is a real repo path (E4)."""
    assert (REPO_ROOT / GENERATOR_PATH).is_file()
    assert (REPO_ROOT / GENERATOR_PATH).resolve() == Path(__file__).resolve().parents[1] / "src" / "gen_audio" / "cube_layers.py"


def test_generator_sha256_is_the_crlf_normalized_hash_of_the_module():
    raw = (REPO_ROOT / GENERATOR_PATH).read_bytes()
    assert generator_sha256() == hashlib.sha256(raw.replace(b"\r\n", b"\n")).hexdigest()
    lf = raw.replace(b"\r\n", b"\n")
    assert normalized_sha256(lf.replace(b"\n", b"\r\n")) == normalized_sha256(lf), "a CRLF checkout keeps the identity"
    assert normalized_sha256(lf + b"# edit\n") != normalized_sha256(lf)


def test_generator_mismatch_fails_with_regenerate_cubes():
    doc, _ = library_cube(_speechish(1.0), 24000, stem="a", engine="e", source_sha256=SHA)
    check_generator(doc)
    stale = json.loads(json.dumps(doc))
    stale["provenance"]["generator_sha256"] = "f" * 64
    with pytest.raises(ValueError, match="regenerate cubes"):
        check_generator(stale)
    del stale["provenance"]["generator_sha256"]
    with pytest.raises(ValueError, match="regenerate cubes"):
        check_generator(stale)


def test_layers_cli_writes_json_and_png(tmp_path):
    wav = write_wav(tmp_path / "tone.wav", _speechish(2.0), 24000)
    out, png = tmp_path / "cube.json", tmp_path / "cube.png"
    assert cube_main(["layers", str(wav), str(out), "--stem", "tone", "--engine", "fixture", "--png", str(png)]) == 0
    doc = json.loads(out.read_text(encoding="utf-8"))
    assert doc["engine"] == "fixture" and doc["cube_revision"] == 3
    assert doc["source_sha256"] == hashlib.sha256(wav.read_bytes()).hexdigest()
    assert doc["provenance"]["generator_sha256"] == generator_sha256()
    assert "generator_commit" not in doc["provenance"], "no history in the cube bytes (uid)"
    assert png.read_bytes()[:8] == b"\x89PNG\r\n\x1a\n"


# (wav stem, cube JSON, engine, label): every Library cube, all rev 3 (E4).
GENERATED = [
    ("bitdot_braille_vibevoice", "library_bitdot_braille_vibevoice_cube3d.json", "vibevoice", "Bitdot braille (VibeVoice-1.5B)"),
    ("genaid_full_misaki_kokoro", "library_genaid_full_misaki_kokoro_cube3d.json", "misaki_kokoro", "misaki\u2192kokoro"),
    ("library_cube_explainer_kokoro_onnx", "library_cube_explainer_kokoro_onnx_cube3d.json", "kokoro_onnx", "Cube explainer (kokoro-onnx)"),
    ("library_kokoro_onnx", "library_kokoro_onnx_cube3d.json", "kokoro_onnx", "kokoro-onnx"),
    ("library_kokoro", "library_kokoro_cube3d.json", "kokoro_dayour", "dayour/kokoro"),
]
LIBRARY_CUBES = [(stem, cube) for stem, cube, _engine, _label in GENERATED]


def _need_wav(stem: str) -> Path:
    wav = LIBRARY / f"{stem}.wav"
    if not wav.exists():
        pytest.skip(f"{wav.name} is gitignored and not staged here (present on SMAX and the box)")
    return wav


def test_every_library_cube_json_is_listed_here():
    docs = [(path.name, json.loads(path.read_text(encoding="utf-8"))) for path in LIBRARY.glob("*_cube3d.json")]
    generated = [name for name, doc in docs if (doc.get("provenance") or {}).get("generator") == GENERATOR_PATH]
    manifest = json.loads((LIBRARY / "manifest.json").read_text(encoding="utf-8"))
    comparisons = [
        entry["jsonUrl"].rsplit("/", 1)[-1]
        for clip in manifest["clips"] for entry in (clip.get("cube") or {}).get("compare", [])
    ]
    assert sorted(generated) == sorted([cube for _s, cube, _e, _l in GENERATED] + comparisons)


@pytest.mark.parametrize(("stem", "cube", "engine", "label"), GENERATED)
def test_reproduces_shipped_cube_byte_for_byte(stem, cube, engine, label):
    """The regen-diff gate for cubes: same WAV + generator bytes -> same bytes."""
    wav = _need_wav(stem)
    shipped = (LIBRARY / cube).read_text(encoding="utf-8")
    audio, sr = read_wav(wav)
    doc, _ = library_cube(audio, sr, stem=stem, engine=engine, source_sha256=hashlib.sha256(wav.read_bytes()).hexdigest(), label=label)
    assert json.dumps(doc, ensure_ascii=False) == shipped, "regenerate: python scripts/cube_revision.py layers ..."


@pytest.mark.parametrize(("stem", "cube", "engine", "label"), GENERATED)
def test_cube_provenance_is_rev3_from_this_generators_bytes(stem, cube, engine, label):
    """No WAV and no git history needed: identity is the generator's content."""
    doc = json.loads((LIBRARY / cube).read_text(encoding="utf-8"))
    assert doc["cube_revision"] == LIBRARY_REVISION and doc["engine"] == engine
    assert doc["provenance"]["layer_method"] == LAYER_METHOD
    assert doc["provenance"]["generator"] == GENERATOR_PATH
    assert doc["provenance"]["params"] == {k: getattr(LIBRARY_PARAMS, k) for k in doc["provenance"]["params"]}
    assert len(doc["source_sha256"]) == 64
    assert "generator_commit" not in doc["provenance"], f"{cube}: a commit SHA in the cube JSON would put history in its uid"
    assert doc["provenance"]["generator_sha256"] == generator_sha256(), f"{cube}: {GENERATOR_PATH} changed since; regenerate cubes"
    check_generator(doc)


def _generator_blob_sha(commit: str) -> str | None:
    """sha256 of GENERATOR_PATH at ``commit``, CRLF normalized. None if absent."""
    blob = subprocess.run(
        ["git", "-C", str(REPO_ROOT), "show", f"{commit}:{GENERATOR_PATH}"],
        capture_output=True,
        check=False,
    )
    if blob.returncode != 0:
        return None
    return normalized_sha256(blob.stdout)


def test_shipped_cubes_record_generator_commit_outside_the_uid(tmp_path):
    """The flag writes generator_commit on the manifest block, never into the cube JSON.

    Identity is generator_sha256. Each shipped block names a commit whose
    generator file hashes to that value. Stripping the key and running
    record_generator_commits puts back a commit that still hashes, and does
    not touch the cube JSON. Forbidding the key made the documented
    --record-generator-commit step illegal, and that assertion already held
    before the test existed.
    """
    manifest = json.loads((LIBRARY / "manifest.json").read_text(encoding="utf-8"))
    historical = generator_commit_for(REPO_ROOT, GENERATOR_PATH, generator_sha256())
    seen = 0
    for clip in manifest["clips"]:
        block = clip.get("cube") or {}
        if not block:
            continue
        name = block["jsonUrl"].rsplit("/", 1)[-1]
        doc = json.loads((LIBRARY / name).read_text(encoding="utf-8"))
        assert "generator_commit" not in doc, clip["id"]
        assert "generator_commit" not in doc.get("provenance", {}), clip["id"]
        sha = doc["provenance"]["generator_sha256"]
        assert sha == generator_sha256(), clip["id"]
        commit = block.get("generator_commit")
        if historical is None:
            assert commit is None, "uncommitted generator bytes must not claim an older commit"
        else:
            assert isinstance(commit, str) and _generator_blob_sha(commit) == sha, clip["id"]
        seen += 1
    assert seen == 5, seen

    library = tmp_path / "library"
    library.mkdir()
    for path in LIBRARY.glob("*_cube3d.json"):
        (library / path.name).write_bytes(path.read_bytes())
    stripped = json.loads((LIBRARY / "manifest.json").read_text(encoding="utf-8"))
    for clip in stripped["clips"]:
        block = clip.get("cube")
        if isinstance(block, dict):
            block.pop("generator_commit", None)
    manifest_path = library / "manifest.json"
    manifest_path.write_text(json.dumps(stripped), encoding="utf-8")
    changed = record_generator_commits(manifest_path, REPO_ROOT, library)
    assert len(changed) == (5 if historical is not None else 0), changed
    restored = json.loads(manifest_path.read_text(encoding="utf-8"))
    for clip in restored["clips"]:
        block = clip.get("cube") or {}
        if not block:
            continue
        name = block["jsonUrl"].rsplit("/", 1)[-1]
        doc = json.loads((library / name).read_text(encoding="utf-8"))
        if historical is None:
            assert "generator_commit" not in block
        else:
            assert _generator_blob_sha(block["generator_commit"]) == doc["provenance"]["generator_sha256"], clip["id"]
    for path in LIBRARY.glob("*_cube3d.json"):
        assert (library / path.name).read_bytes() == path.read_bytes(), path.name


@pytest.mark.skipif(shutil.which("git") is None, reason="needs git")
def test_record_generator_commits_finds_the_commit_with_these_bytes(tmp_path):
    repo, gen = tmp_path / "repo", "src/gen.py"
    (repo / "src").mkdir(parents=True)
    lib = repo / "lib"
    lib.mkdir()

    def git(*args):
        return subprocess.run(["git", "-c", "user.name=t", "-c", "user.email=t@example.invalid", *args], cwd=repo,
                              check=True, capture_output=True, text=True).stdout.strip()

    git("init", "-q")
    (repo / gen).write_bytes(b"v1\n")
    git("add", ".")
    git("commit", "-q", "-m", "v1")
    first = git("rev-parse", "HEAD")
    (repo / gen).write_bytes(b"v2\r\n")
    sha = normalized_sha256(b"v2\n")
    (lib / "c_cube3d.json").write_text(json.dumps({"provenance": {"generator": gen, "generator_sha256": sha}}), encoding="utf-8")
    manifest = lib / "manifest.json"
    manifest.write_text(json.dumps({"clips": [{"id": "c", "cube": {"jsonUrl": "/library/c_cube3d.json", "generator_commit": first}}]}),
                        encoding="utf-8")
    assert record_generator_commits(manifest, repo) == ["c"], "a commit with other bytes is dropped"
    assert "generator_commit" not in json.loads(manifest.read_text(encoding="utf-8"))["clips"][0]["cube"]
    git("add", ".")
    git("commit", "-q", "-m", "v2")
    second = git("rev-parse", "HEAD")
    assert record_generator_commits(manifest, repo) == ["c"]
    assert json.loads(manifest.read_text(encoding="utf-8"))["clips"][0]["cube"]["generator_commit"] == second
    assert record_generator_commits(manifest, repo) == [], "a commit that holds the bytes is kept"


@pytest.mark.parametrize(("stem", "cube"), LIBRARY_CUBES)
def test_library_cube_inv_hdr_is_rms_over_peak_of_its_wav(stem, cube):
    """inv_hdr in every Library cube JSON is rms/peak of its WAV (cube_revision.measure)."""
    audio, sr = read_wav(_need_wav(stem))
    doc = json.loads((LIBRARY / cube).read_text(encoding="utf-8"))
    assert doc["inv_hdr"] == pytest.approx(measure(audio, sr).inv_hdr, abs=1e-6), cube
    assert doc["duration_s"] == pytest.approx(len(audio) / sr, abs=1e-9)


# --- manifest cube mirror (gen_audio.library_manifest) ---------------------
# Needs no WAV: it reads only the committed cube JSON and manifest.

def test_committed_manifest_mirrors_every_generated_cube(tmp_path):
    from gen_audio.library_manifest import sync_manifest

    copy = tmp_path / "manifest.json"
    copy.write_bytes((LIBRARY / "manifest.json").read_bytes())
    assert sync_manifest(copy, LIBRARY) == [], "run: python scripts/cube_revision.py manifest"
    assert copy.read_bytes() == (LIBRARY / "manifest.json").read_bytes()


def test_manifest_sync_repairs_drift_wires_new_cubes_and_leaves_legacy_cubes_alone(tmp_path):
    from gen_audio.library_manifest import render, sync_manifest

    library = tmp_path / "library"
    library.mkdir()
    for path in LIBRARY.glob("*_cube3d.json"):
        (library / path.name).write_bytes(path.read_bytes())
    # A cube without cube_revision stands for the retired generator.
    legacy = json.loads((library / "library_kokoro_onnx_cube3d.json").read_text(encoding="utf-8"))
    del legacy["cube_revision"]
    (library / "library_kokoro_onnx_cube3d.json").write_text(json.dumps(legacy), encoding="utf-8")
    manifest = json.loads((LIBRARY / "manifest.json").read_text(encoding="utf-8"))
    by_id = {clip["id"]: clip for clip in manifest["clips"]}
    by_id["lib-misaki-kokoro"]["cube"]["cube_revision"] = 2
    by_id["lib-misaki-kokoro"]["cube"]["note"] = "Inverse-HDR bitdot cube rev 2"
    by_id["lib-kokoro"]["cube"] = None
    legacy_before = json.dumps(by_id["lib-kokoro-onnx"]["cube"])
    path = library / "manifest.json"
    path.write_text(render(manifest), encoding="utf-8")
    assert sync_manifest(path) == ["lib-kokoro", "lib-misaki-kokoro"]
    rewired = {clip["id"]: clip for clip in json.loads(path.read_text(encoding="utf-8"))["clips"]}["lib-kokoro"]["cube"]
    shipped = {clip["id"]: clip for clip in json.loads((LIBRARY / "manifest.json").read_text(encoding="utf-8"))["clips"]}
    # A freshly wired block has no generator_commit (information only, recorded
    # from history by --record-generator-commit); everything else matches.
    assert "generator_commit" not in rewired
    assert rewired == {k: v for k, v in shipped["lib-kokoro"]["cube"].items() if k != "generator_commit"}
    repaired = {clip["id"]: clip for clip in json.loads(path.read_text(encoding="utf-8"))["clips"]}
    misaki = repaired["lib-misaki-kokoro"]["cube"]
    assert misaki.get("generator_commit") == shipped["lib-misaki-kokoro"]["cube"].get("generator_commit"), "sync keeps it"
    doc = json.loads((LIBRARY / "library_genaid_full_misaki_kokoro_cube3d.json").read_text(encoding="utf-8"))
    assert misaki["cube_revision"] == doc["cube_revision"] == 3
    assert misaki["note"].startswith("Inverse-HDR cube rev 3 over the full 139.375 s WAV")
    assert "bitdot" not in misaki["note"]
    assert json.dumps(repaired["lib-kokoro-onnx"]["cube"]) == legacy_before


def test_generated_cube_titles_name_the_clip_not_the_point_style():
    for _stem, cube, _engine, label in GENERATED:
        doc = json.loads((LIBRARY / cube).read_text(encoding="utf-8"))
        assert doc["title"] == f"Inverse-HDR cube \u2014 {label}"

//! Regenerate the v1 asset catalog from the v0 library data.
//!
//! cargo run -p gen-audio-core --example build_assets [-- --from-lock]
//!
//! Reads manifest.json, the catalog voice models, viewport.example.json,
//! voice_profile.optimus.json, the cube JSON files (each clip's cube and its
//! `cube.compare[]` comparison cubes) and the spectrogram
//! sidecars, hashes every referenced media file under
//! apps/desktop/public/library, and writes the outputs below.
//!
//! The WAVs are gitignored. Their identity facts (sha256, bytes, frames, rate,
//! channels) are pinned in schemas/asset-object/media.lock.json (PR #5
//! verification E2):
//!   default      every WAV must be on disk (SMAX, the box). A missing WAV is
//!                an error, never a skip. media.lock.json is rewritten.
//!   --from-lock  CI, which has no WAVs: a missing WAV takes its facts from
//!                media.lock.json; a WAV that is present must match its lock
//!                entry; every cube JSON's source_sha256 must be its clip's
//!                locked sha256; every cube file (JSON + PNG) must match the
//!                bytes media.lock.json "cubes" pins (a mode that can't
//!                regenerate verifies what was generated). media.lock.json
//!                is read, not written.
//! Any other missing media (cube JSON/PNG, strips, profile) is an error.
//! Then `git diff --exit-code` over the outputs is the regen gate.
//!
//! Writes:
//!   apps/desktop/public/library/assets.json        (release v1 envelopes: no dev fixtures)
//!   schemas/asset-object/fixtures/assets.dev.json   (dev/test-only envelopes, never shipped)
//!   schemas/asset-object/vectors/fixtures_v1.json   (legacy_id -> uid pins, release + dev)
//!   schemas/examples/viewport.example.json          (card "uid" alongside "id"; dev deck)
//!   schemas/examples/viewport.release.json          (the shipped deck: example minus dev cards)
//!   schemas/asset-object/media.lock.json            (WAV identity facts + cube file sha256; default mode only)

use std::fs;
use std::path::{Path, PathBuf};

use gen_audio_core::asset::{is_dev_fixture, sha256_hex, validate_set};
use gen_audio_core::asset_migrate::{migrate_to_v1, verify_cube_lock};
use gen_audio_core::catalog;
use serde_json::{json, Map, Value};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("repo root")
}

/// Source bytes with every CRLF pair replaced by LF (generator identity).
fn crlf_to_lf(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    for (i, &byte) in bytes.iter().enumerate() {
        if !(byte == b'\r' && bytes.get(i + 1) == Some(&b'\n')) {
            out.push(byte);
        }
    }
    out
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// (frames, sample_rate, channels) from a RIFF/WAVE header.
fn wav_facts(bytes: &[u8]) -> Option<(u64, u64, u64)> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut offset = 12;
    let (mut rate, mut channels, mut block) = (0u64, 0u64, 0u64);
    while offset + 8 <= bytes.len() {
        let tag = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().ok()?) as usize;
        let body = offset + 8;
        if tag == b"fmt " && body + 16 <= bytes.len() {
            channels = u64::from(u16::from_le_bytes([bytes[body + 2], bytes[body + 3]]));
            rate = u64::from(u32::from_le_bytes(bytes[body + 4..body + 8].try_into().ok()?));
            block = u64::from(u16::from_le_bytes([bytes[body + 12], bytes[body + 13]]));
        } else if tag == b"data" && block > 0 {
            return Some((size as u64 / block, rate, channels));
        }
        offset = body + size + (size & 1);
    }
    None
}

fn fail(message: String) -> ! {
    eprintln!("build_assets: {message}");
    std::process::exit(1);
}

fn main() {
    let from_lock = match std::env::args().skip(1).collect::<Vec<_>>().as_slice() {
        [] => false,
        [flag] if flag == "--from-lock" => true,
        other => fail(format!("unknown arguments {other:?}; usage: build_assets [--from-lock]")),
    };
    let root = repo();
    let library = root.join("apps/desktop/public/library");
    let lock_path = root.join("schemas/asset-object/media.lock.json");
    let lock_doc = if from_lock { read_json(&lock_path) } else { json!({}) };
    let lock: Map<String, Value> = lock_doc["media"].as_object().cloned().unwrap_or_default();
    let locked_cubes: Map<String, Value> = lock_doc["cubes"].as_object().cloned().unwrap_or_default();
    let manifest = read_json(&library.join("manifest.json"));
    let viewport_path = root.join("schemas/examples/viewport.example.json");
    let viewport = read_json(&viewport_path);
    let cards = viewport["cards"].as_array().cloned().unwrap_or_default();

    let mut media = Map::new();
    let mut missing: Vec<String> = Vec::new();
    let mut hash = |path: &str| {
        let full = library.join(path);
        let wav = path.ends_with(".wav");
        let Ok(bytes) = fs::read(&full) else {
            match lock.get(path) {
                Some(locked) if wav => {
                    media.insert(path.to_string(), locked.clone());
                }
                _ if wav && from_lock => missing.push(format!("{path} (not on disk and not in media.lock.json)")),
                _ if wav => missing.push(format!("{path} (stage the WAV; CI uses --from-lock)")),
                _ => missing.push(path.to_string()),
            }
            return;
        };
        let mut info = json!({"sha256": sha256_hex(&bytes), "bytes": bytes.len()});
        if wav {
            if let Some((frames, rate, channels)) = wav_facts(&bytes) {
                info["frames"] = json!(frames);
                info["sample_rate"] = json!(rate);
                info["channels"] = json!(channels);
            }
            if let Some(locked) = lock.get(path) {
                if locked != &info {
                    missing.push(format!("{path} differs from media.lock.json; rerun build_assets without --from-lock"));
                }
            }
        }
        media.insert(path.to_string(), info);
    };

    let mut cube_docs = Map::new();
    let mut cube_files: Vec<String> = Vec::new();
    for clip in manifest["clips"].as_array().cloned().unwrap_or_default() {
        if let Some(path) = clip["wavUrl"].as_str().and_then(|url| url.strip_prefix("/library/")) {
            hash(path);
        }
        // The clip's cube plus any comparison cubes (cube.compare[], other layer_method).
        let mut cube_blocks = vec![clip["cube"].clone()];
        cube_blocks.extend(clip["cube"]["compare"].as_array().cloned().unwrap_or_default());
        for block in &cube_blocks {
            for key in ["jsonUrl", "pngUrl"] {
                if let Some(path) = block[key].as_str().and_then(|url| url.strip_prefix("/library/")) {
                    hash(path);
                    cube_files.push(path.to_string());
                    if key == "jsonUrl" {
                        cube_docs.insert(path.to_string(), read_json(&library.join(path)));
                    }
                }
            }
        }
    }

    let mut spectrograms = Vec::new();
    let mut names: Vec<_> = fs::read_dir(&library).expect("library dir").flatten().map(|entry| entry.file_name()).collect();
    names.sort();
    for name in names {
        let name = name.to_string_lossy().to_string();
        if name.ends_with("_spec2d.json") {
            let sidecar = read_json(&library.join(&name));
            hash(sidecar["path"].as_str().unwrap_or_default());
            spectrograms.push(sidecar);
        }
    }

    let optimus_path = "voice_profile.optimus.json";
    hash(optimus_path);
    drop(hash);
    if !missing.is_empty() {
        fail(format!("missing or mismatched media:\n  {}", missing.join("\n  ")));
    }
    let cube_facts: Map<String, Value> = cube_files
        .iter()
        .filter_map(|path| media.get(path).map(|info| (path.clone(), json!({"sha256": info["sha256"], "bytes": info["bytes"]}))))
        .collect();
    if from_lock {
        if let Err(errors) = verify_cube_lock(&locked_cubes, &cube_facts) {
            fail(format!("cube bytes differ from media.lock.json:\n  {}", errors.join("\n  ")));
        }
    }
    // The cube regen check without WAVs: each cube JSON names the sha256 of
    // the WAV it was made from; it must be the WAV (or locked WAV) of its clip.
    for clip in manifest["clips"].as_array().cloned().unwrap_or_default() {
        let (Some(wav), Some(cube)) = (
            clip["wavUrl"].as_str().and_then(|url| url.strip_prefix("/library/")),
            clip["cube"]["jsonUrl"].as_str().and_then(|url| url.strip_prefix("/library/")),
        ) else {
            continue;
        };
        let want = media.get(wav).and_then(|info| info["sha256"].as_str()).unwrap_or_default();
        match cube_docs.get(cube).and_then(|doc| doc["source_sha256"].as_str()) {
            Some(got) if got == want => {}
            Some(got) => fail(format!("{cube} was made from WAV sha256 {got}, but {wav} is {want}; regenerate the cube")),
            None if from_lock => fail(format!("{cube} has no source_sha256, so CI cannot tie it to {wav}; regenerate it with cube_revision.py layers")),
            None => {}
        }
    }
    // Cube identity (item 2): a cube JSON names the CRLF-normalized sha256 of
    // its generator source; it must be the generator in this checkout.
    for (name, doc) in &cube_docs {
        let (Some(generator), Some(recorded)) =
            (doc.pointer("/provenance/generator").and_then(Value::as_str), doc.pointer("/provenance/generator_sha256").and_then(Value::as_str))
        else {
            continue;
        };
        let source = fs::read(root.join(generator)).unwrap_or_else(|e| fail(format!("{name}: generator {generator}: {e}")));
        let current = sha256_hex(&crlf_to_lf(&source));
        if current != recorded {
            fail(format!("{name} was made by {generator} sha256 {recorded}, but it now hashes to {current} (CRLF->LF); regenerate cubes with cube_revision.py layers"));
        }
    }
    let mut profiles = vec![json!({"path": optimus_path, "profile": read_json(&library.join(optimus_path))})];
    for card in &cards {
        if card["kind"] == "VoiceProfile" && card["body"]["personaId"] != "optimus" {
            profiles.push(json!({"profile": card["body"]}));
        }
    }

    let bundle = json!({
        "manifest": manifest,
        "voice_models": serde_json::to_value(catalog::voice_models()).expect("voice models"),
        "cards": cards,
        "profiles": profiles,
        "cube_docs": cube_docs,
        "spectrograms": spectrograms,
        "media": media,
    });
    let migrated = migrate_to_v1(&bundle).unwrap_or_else(|e| panic!("migration failed: {e}"));
    let assets = migrated["assets"].as_array().cloned().unwrap_or_default();
    validate_set(&assets).unwrap_or_else(|e| panic!("migrated set is invalid: {e}"));

    let pretty = |value: &Value| serde_json::to_string_pretty(value).expect("json") + "\n";
    // Release vs dev split (PR #5 review, fix 5): fixture-tone and
    // reference-only assets never reach public/library (and so never dist).
    let (dev_assets, release_assets): (Vec<Value>, Vec<Value>) = assets.iter().cloned().partition(is_dev_fixture);
    validate_set(&release_assets).unwrap_or_else(|e| panic!("release set is invalid without the dev fixtures: {e}"));
    let dev_uids: Vec<&str> = dev_assets.iter().filter_map(|asset| asset["uid"].as_str()).collect();
    let split_index = |dev: bool| -> Map<String, Value> {
        migrated["legacy_index"]
            .as_object()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|(_, uid)| dev_uids.contains(&uid.as_str().unwrap_or_default()) == dev)
            .collect()
    };
    let mut release = migrated.clone();
    release["assets"] = json!(release_assets);
    release["legacy_index"] = json!(split_index(false));
    fs::write(library.join("assets.json"), pretty(&release)).expect("write assets.json");
    let dev_doc = json!({
        "schema_version": migrated["schema_version"],
        "note": "Dev/test-only assets (honesty.fixture or fixture_tone / reference_only / sample_content claims). Never shipped; the desktop shows these cards only with VITE_GEN_AUDIO_FIXTURES=1. Regenerate with build_assets.",
        "assets": dev_assets,
        "legacy_index": split_index(true),
    });
    let dev_dir = root.join("schemas/asset-object/fixtures");
    fs::create_dir_all(&dev_dir).expect("fixtures dir");
    fs::write(dev_dir.join("assets.dev.json"), pretty(&dev_doc)).expect("write assets.dev.json");
    let fixtures = json!({
        "schema_version": migrated["schema_version"],
        "note": "Pinned uid of every migrated v0 fixture. Regenerate with `cargo run -p gen-audio-core --example build_assets`.",
        "legacy_index": migrated["legacy_index"],
    });
    fs::write(root.join("schemas/asset-object/vectors/fixtures_v1.json"), pretty(&fixtures)).expect("write fixtures");

    // Card uid alongside id in viewport.example.json (line-level edit keeps the file's layout).
    let index = migrated["legacy_index"].as_object().cloned().unwrap_or_default();
    let text = fs::read_to_string(&viewport_path).expect("viewport");
    let mut out = String::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim_start().starts_with("\"uid\": \"ga:card:") {
            continue;
        }
        out.push_str(line);
        out.push('\n');
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("\"id\": \"") {
            let id = rest.trim_end_matches(',').trim_end_matches('"');
            if let Some(uid) = index.get(&format!("card:{id}")).and_then(Value::as_str) {
                let indent = &line[..line.len() - trimmed.len()];
                out.push_str(&format!("{indent}\"uid\": \"{uid}\",\n"));
            }
        }
    }
    fs::write(&viewport_path, &out).expect("write viewport");

    // The shipped deck: the example minus every card whose asset is a dev fixture.
    let dev_card_ids: Vec<String> = dev_assets
        .iter()
        .filter(|asset| asset["kind"] == "card")
        .filter_map(|asset| asset.pointer("/fields/card_id").and_then(Value::as_str).map(str::to_string))
        .collect();
    let mut release_viewport: Value = serde_json::from_str(&out).expect("viewport json");
    release_viewport["cards"]
        .as_array_mut()
        .expect("cards")
        .retain(|card| !dev_card_ids.iter().any(|id| card["id"] == id.as_str()));
    fs::write(root.join("schemas/examples/viewport.release.json"), pretty(&release_viewport)).expect("write viewport.release.json");
    if !from_lock {
        let wavs: Map<String, Value> = media.iter().filter(|(path, _)| path.ends_with(".wav")).map(|(k, v)| (k.clone(), v.clone())).collect();
        let lock_doc = json!({
            "note": "Identity facts of the gitignored library WAVs, and the sha256 of every cube file made from them, written by build_assets when every WAV is on disk. CI has no WAVs and runs build_assets --from-lock, which takes these facts for a missing WAV, checks each cube JSON's source_sha256 against them, and fails when a cube file's bytes differ from \"cubes\".",
            "media": wavs,
            "cubes": cube_facts,
        });
        fs::write(&lock_path, pretty(&lock_doc)).expect("write media.lock.json");
    }
    println!(
        "{}: release: {} assets, {} cards; dev-only: {dev_card_ids:?}",
        if from_lock { "from media.lock.json" } else { "from the WAVs on disk" },
        release_assets.len(),
        release_viewport["cards"].as_array().map_or(0, Vec::len)
    );
    println!("{} assets; legacy_index {} entries", assets.len(), index.len());
}

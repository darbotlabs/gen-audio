//! Regenerate the v1 asset catalog from the v0 library data.
//!
//! cargo run -p gen-audio-core --example build_assets
//!
//! Reads manifest.json, the catalog voice models, viewport.example.json,
//! voice_profile.optimus.json, the cube JSON files and the spectrogram
//! sidecars, hashes every referenced media file under
//! apps/desktop/public/library (the gitignored WAVs must be staged there),
//! and writes:
//!   apps/desktop/public/library/assets.json        (release v1 envelopes: no dev fixtures)
//!   schemas/asset-object/fixtures/assets.dev.json   (dev/test-only envelopes, never shipped)
//!   schemas/asset-object/vectors/fixtures_v1.json   (legacy_id -> uid pins, release + dev)
//!   schemas/examples/viewport.example.json          (card "uid" alongside "id"; dev deck)
//!   schemas/examples/viewport.release.json          (the shipped deck: example minus dev cards)

use std::fs;
use std::path::{Path, PathBuf};

use gen_audio_core::asset::{is_dev_fixture, sha256_hex, validate_set};
use gen_audio_core::asset_migrate::migrate_to_v1;
use gen_audio_core::catalog;
use serde_json::{json, Map, Value};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("repo root")
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

fn main() {
    let root = repo();
    let library = root.join("apps/desktop/public/library");
    let manifest = read_json(&library.join("manifest.json"));
    let viewport_path = root.join("schemas/examples/viewport.example.json");
    let viewport = read_json(&viewport_path);
    let cards = viewport["cards"].as_array().cloned().unwrap_or_default();

    let mut media = Map::new();
    let mut hash = |path: &str| {
        let full = library.join(path);
        let Ok(bytes) = fs::read(&full) else {
            eprintln!("missing media (not hashed): {path}");
            return;
        };
        let mut info = json!({"sha256": sha256_hex(&bytes), "bytes": bytes.len()});
        if path.ends_with(".wav") {
            if let Some((frames, rate, channels)) = wav_facts(&bytes) {
                info["frames"] = json!(frames);
                info["sample_rate"] = json!(rate);
                info["channels"] = json!(channels);
            }
        }
        media.insert(path.to_string(), info);
    };

    let mut cube_docs = Map::new();
    for clip in manifest["clips"].as_array().cloned().unwrap_or_default() {
        if let Some(path) = clip["wavUrl"].as_str().and_then(|url| url.strip_prefix("/library/")) {
            hash(path);
        }
        for key in ["jsonUrl", "pngUrl"] {
            if let Some(path) = clip["cube"][key].as_str().and_then(|url| url.strip_prefix("/library/")) {
                hash(path);
                if key == "jsonUrl" {
                    cube_docs.insert(path.to_string(), read_json(&library.join(path)));
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
    println!("release: {} assets, {} cards; dev-only: {dev_card_ids:?}", release_assets.len(), release_viewport["cards"].as_array().map_or(0, Vec::len));
    println!("{} assets; legacy_index {} entries", assets.len(), index.len());
}

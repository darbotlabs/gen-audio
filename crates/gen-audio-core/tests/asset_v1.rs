//! Asset object model v1: golden vectors, schema parity and the library catalog.

use std::path::Path;

use gen_audio_core::asset::{
    bin_frames_inferred, check_media_path, glyph_from_uid, is_dev_fixture, mint, ms_from_frames, normalize_nfc, parse_uid, round_half_up, validate_envelope,
    validate_set, verify_media, MediaCheck, MediaDigest,
};
use gen_audio_core::asset_migrate::{migrate_to_v1, verify_cube_lock};
use serde_json::{Map, Value};

const VECTORS: &str = include_str!("../../../schemas/asset-object/vectors/v1.json");
const FIXTURES: &str = include_str!("../../../schemas/asset-object/vectors/fixtures_v1.json");
const SCHEMA: &str = include_str!("../../../schemas/asset-object.schema.json");
const CARD_SCHEMA: &str = include_str!("../../../schemas/card-viewport.schema.json");
const PROFILE_SCHEMA: &str = include_str!("../../../schemas/voice_profile.schema.json");
const ASSETS: &str = include_str!("../../../apps/desktop/public/library/assets.json");
const VIEWPORT: &str = include_str!("../../../schemas/examples/viewport.example.json");
const RELEASE_VIEWPORT: &str = include_str!("../../../schemas/examples/viewport.release.json");
const DEV_ASSETS: &str = include_str!("../../../schemas/asset-object/fixtures/assets.dev.json");
const MEDIA_LOCK: &str = include_str!("../../../schemas/asset-object/media.lock.json");

fn json(text: &str) -> Value {
    serde_json::from_str(text).expect("json")
}

fn code<T>(result: Result<T, gen_audio_core::asset::AssetError>) -> Value {
    match result {
        Ok(_) => Value::Null,
        Err(error) => Value::String(error.code.into()),
    }
}

fn validator() -> jsonschema::Validator {
    let card = json(CARD_SCHEMA);
    let profile = json(PROFILE_SCHEMA);
    jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .with_resource(card["$id"].as_str().unwrap(), jsonschema::Resource::from_contents(card.clone()).unwrap())
        .with_resource(profile["$id"].as_str().unwrap(), jsonschema::Resource::from_contents(profile.clone()).unwrap())
        .build(&json(SCHEMA))
        .expect("asset-object.schema.json compiles")
}

#[test]
fn mint_vectors_match() {
    let doc = json(VECTORS);
    for vector in doc["mint"].as_array().unwrap() {
        let name = vector["name"].as_str().unwrap();
        let mut fields = json(vector["fields_json"].as_str().unwrap());
        if vector["normalize_nfc"] == true {
            fields = normalize_nfc(&fields);
        }
        let media: Vec<MediaDigest> = vector["media"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| MediaDigest { role: m["role"].as_str().unwrap().into(), sha256: m["sha256"].as_str().unwrap().into() })
            .collect();
        let src: Vec<String> = vector["src"].as_array().unwrap().iter().map(|s| s.as_str().unwrap().into()).collect();
        let minted = mint(vector["kind"].as_str().unwrap(), &fields, &media, &src).unwrap_or_else(|e| panic!("{name}: {e}"));
        let expect = &vector["expect"];
        assert_eq!(minted.canonical, expect["canonical"], "{name} canonical");
        assert_eq!(minted.preimage_hex, expect["preimage_hex"], "{name} preimage");
        assert_eq!(minted.digest_hex, expect["digest_hex"], "{name} digest");
        assert_eq!(minted.uid, expect["uid"], "{name} uid");
        assert_eq!(minted.glyph, expect["glyph"], "{name} glyph");
        assert_eq!(glyph_from_uid(&minted.uid).unwrap(), minted.glyph, "{name} glyph from uid text");
        assert!(minted.preimage_hex.starts_with("67612d61737365742d763100"), "{name} domain tag ga-asset-v1\\0");
    }
    let uid = |name: &str| doc["mint"].as_array().unwrap().iter().find(|v| v["name"] == name).unwrap()["expect"]["uid"].clone();
    assert_eq!(uid("nfc_cafe"), uid("nfd_cafe_normalized_at_ingest"));
    assert_eq!(uid("nfc_cafe"), uid("key_order_shuffle"));
    assert_ne!(uid("media_a"), uid("media_b_changes_uid"));
}

#[test]
fn canonical_rejects_match() {
    for vector in json(VECTORS)["canonical_reject"].as_array().unwrap() {
        let name = vector["name"].as_str().unwrap();
        let got = match serde_json::from_str::<Value>(vector["fields_json"].as_str().unwrap()) {
            Err(_) => Value::String("lone_surrogate".into()),
            Ok(fields) => code(mint(vector["kind"].as_str().unwrap(), &fields, &[], &[])),
        };
        assert_eq!(got, vector["error"], "{name}");
    }
}

fn digests(media: &Value) -> Vec<MediaDigest> {
    media
        .as_array()
        .unwrap()
        .iter()
        .map(|m| MediaDigest { role: m["role"].as_str().unwrap().into(), sha256: m["sha256"].as_str().unwrap().into() })
        .collect()
}

#[test]
fn identity_rejects_match() {
    for vector in json(VECTORS)["identity_reject"].as_array().unwrap() {
        let src: Vec<String> = vector["src"].as_array().unwrap().iter().map(|s| s.as_str().unwrap().into()).collect();
        let got = code(mint(vector["kind"].as_str().unwrap(), &vector["fields"], &digests(&vector["media"]), &src));
        assert_eq!(got, vector["error"], "{}", vector["name"]);
    }
}

#[test]
fn rounding_vectors_match() {
    let doc = json(VECTORS);
    for vector in doc["rounding"].as_array().unwrap() {
        let got = match vector["op"].as_str().unwrap() {
            "ms_from_frames" => ms_from_frames(vector["frames"].as_u64().unwrap(), vector["rate"].as_u64().unwrap()),
            "bin_frames_inferred" => bin_frames_inferred(
                vector["duration_s"].as_f64().unwrap(),
                vector["sample_rate_hz"].as_u64().unwrap(),
                vector["time_bins"].as_u64().unwrap(),
            )
            .unwrap(),
            _ => round_half_up(vector["value"].as_f64().unwrap() * vector["scale"].as_f64().unwrap()).unwrap(),
        };
        assert_eq!(Value::from(got), vector["expect"], "{vector}");
    }
    for vector in doc["mint"].as_array().unwrap() {
        if let Some(from) = vector.get("derived_from") {
            let fields = json(vector["fields_json"].as_str().unwrap());
            let want = ms_from_frames(from["frames"].as_u64().unwrap(), from["sample_rate_hz"].as_u64().unwrap());
            assert_eq!(fields["duration_ms"], want, "{}", vector["name"]);
        }
    }
    assert_eq!(ms_from_frames(24_008, 16_000), 1501, "exact .5 tie rounds up");
    // B1' near-tie: binary64 two-step gives 39283; the exact frame count
    // (3,771,216 / 96 = 39283.5) would round to 39284.
    assert_eq!(bin_frames_inferred(157.134, 24_000, 96).unwrap(), 39_283);
    assert_eq!((3_771_216u64 * 2 + 96) / (2 * 96), 39_284);
    // E4: every Library cube is rev 3 with downsample_sf_st and a hop, so none
    // takes the inferred branch any more; the formula stays pinned by the vectors.
    let catalog = json(ASSETS);
    for cube in catalog["assets"].as_array().unwrap().iter().filter(|asset| asset["kind"] == "cube_ihdr") {
        assert_eq!(cube["provenance"]["params"]["bins_inferred_from_shape"], false, "{}", cube["legacy_id"]);
    }
}

#[test]
fn uid_and_path_vectors_match() {
    let doc = json(VECTORS);
    for vector in doc["uid_parse"].as_array().unwrap() {
        assert_eq!(code(parse_uid(vector["uid"].as_str().unwrap())), vector["error"], "{}", vector["uid"]);
    }
    for vector in doc["media_path"].as_array().unwrap() {
        assert_eq!(code(check_media_path(vector["path"].as_str().unwrap())), vector["error"], "{}", vector["path"]);
    }
}

#[test]
fn envelope_vectors_match_validator_and_schema() {
    let schema = validator();
    for vector in json(VECTORS)["envelopes"].as_array().unwrap() {
        let name = vector["name"].as_str().unwrap();
        assert_eq!(code(validate_envelope(&vector["envelope"])), vector["error"], "{name} validator");
        assert_eq!(schema.is_valid(&vector["envelope"]), vector["schema_valid"] == true, "{name} jsonschema");
    }
}

#[test]
fn set_and_migration_vectors_match() {
    let doc = json(VECTORS);
    for vector in doc["sets"].as_array().unwrap() {
        let members = vector["members"].as_array().unwrap().clone();
        assert_eq!(code(validate_set(&members)), vector["error"], "{}", vector["name"]);
    }
    for vector in doc["migrations"].as_array().unwrap() {
        let name = vector["name"].as_str().unwrap();
        match migrate_to_v1(&vector["before"]) {
            Ok(after) => assert_eq!(after, vector["after"], "{name}"),
            Err(error) => assert_eq!(Value::String(error.code.into()), vector["error"], "{name}"),
        }
    }
}

#[test]
fn library_catalog_is_valid_and_pinned() {
    let catalog = json(ASSETS);
    let assets = catalog["assets"].as_array().unwrap().clone();
    validate_set(&assets).expect("assets.json is a valid set");
    let schema = validator();
    for asset in &assets {
        let errors: Vec<String> = schema.iter_errors(asset).map(|e| format!("{e} at {}", e.instance_path)).collect();
        assert!(errors.is_empty(), "{} fails the schema: {errors:?}", asset["uid"]);
        assert_eq!(glyph_from_uid(asset["uid"].as_str().unwrap()).unwrap(), asset["display"]["glyph"]);
    }
    let fixtures = json(FIXTURES);
    // fixtures_v1.json pins release + dev; assets.json carries the release half.
    let mut union = catalog["legacy_index"].as_object().unwrap().clone();
    union.extend(json(DEV_ASSETS)["legacy_index"].as_object().unwrap().clone());
    assert_eq!(Value::Object(union), fixtures["legacy_index"], "fixtures_v1.json is stale; rerun build_assets");
    // viewport.example.json keeps id and carries the card uid alongside.
    let index = fixtures["legacy_index"].as_object().unwrap();
    for card in json(VIEWPORT)["cards"].as_array().unwrap() {
        let id = card["id"].as_str().unwrap();
        assert_eq!(card["uid"], index[&format!("card:{id}")], "card {id} uid");
    }
}

#[test]
fn release_catalog_and_deck_leave_out_dev_fixtures() {
    let release = json(ASSETS)["assets"].as_array().unwrap().clone();
    assert!(release.iter().all(|asset| !is_dev_fixture(asset)), "release assets.json carries a dev fixture");
    let dev = json(DEV_ASSETS)["assets"].as_array().unwrap().clone();
    assert!(!dev.is_empty() && dev.iter().all(is_dev_fixture));
    let mut dev_cards: Vec<String> =
        dev.iter().filter_map(|asset| asset.pointer("/fields/card_id").and_then(Value::as_str).map(str::to_string)).collect();
    dev_cards.sort_unstable();
    assert_eq!(dev_cards, ["bench-ref", "cast-sample", "cube-fixture", "serve-gateway", "serve-node", "spec-fixture"]);
    // Dev + release is the full migrated set and is valid as one set.
    let mut all = release.clone();
    all.extend(dev);
    validate_set(&all).expect("release + dev is a valid set");
    let example = json(VIEWPORT);
    let shipped = json(RELEASE_VIEWPORT);
    let expected: Vec<Value> =
        example["cards"].as_array().unwrap().iter().filter(|card| !dev_cards.iter().any(|id| card["id"] == id.as_str())).cloned().collect();
    assert_eq!(shipped["cards"], Value::Array(expected), "viewport.release.json is stale; rerun build_assets");
    for key in ["version", "title", "columns"] {
        assert_eq!(shipped[key], example[key]);
    }
}

#[test]
fn library_media_hashes_verify_where_present() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/desktop/public/library");
    let catalog = json(ASSETS);
    let (mut verified, mut missing) = (0, Vec::new());
    for asset in catalog["assets"].as_array().unwrap() {
        for media in asset["media"].as_array().unwrap() {
            match verify_media(&root, media).unwrap_or_else(|e| panic!("{}: {e}", asset["uid"])) {
                MediaCheck::Verified => verified += 1,
                MediaCheck::Missing => missing.push(media["path"].as_str().unwrap().to_string()),
            }
        }
    }
    // WAVs are gitignored; everything committed must verify.
    assert!(missing.iter().all(|path| path.ends_with(".wav")), "missing committed media: {missing:?}");
    if !missing.is_empty() {
        eprintln!("skipped (not staged here): {missing:?}");
    }
    assert!(verified > 0);
}

/// E2: media.lock.json pins exactly the WAVs the release catalog hashes, so CI
/// (no WAVs, build_assets --from-lock) mints the same clip uids.
#[test]
fn media_lock_pins_every_library_wav() {
    let lock = json(MEDIA_LOCK)["media"].as_object().unwrap().clone();
    let catalog = json(ASSETS);
    let mut wavs = 0;
    for asset in catalog["assets"].as_array().unwrap() {
        for media in asset["media"].as_array().unwrap().iter().filter(|media| media["role"] == "wav") {
            let path = media["path"].as_str().unwrap();
            let locked = &lock[path];
            assert_eq!(locked["sha256"], media["sha256"], "{path}");
            assert_eq!(locked["bytes"], media["bytes"], "{path}");
            assert!(locked["bytes"].as_u64().unwrap() < 25 * 1024 * 1024, "{path} is over 25 MB");
            wavs += 1;
        }
    }
    assert_eq!(wavs, lock.len(), "media.lock.json has entries no clip uses");
}

/// Every cube file (JSON + PNG) the library manifest names, with its on-disk
/// facts; `edit` may rewrite one file's bytes first (a hand edit).
fn cube_file_facts(edit: impl Fn(&str, Vec<u8>) -> Vec<u8>) -> Map<String, Value> {
    let library = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/desktop/public/library");
    let manifest: Value = json(&std::fs::read_to_string(library.join("manifest.json")).unwrap());
    let mut facts = Map::new();
    for clip in manifest["clips"].as_array().unwrap() {
        for key in ["jsonUrl", "pngUrl"] {
            if let Some(path) = clip["cube"][key].as_str().and_then(|url| url.strip_prefix("/library/")) {
                let bytes = edit(path, std::fs::read(library.join(path)).unwrap());
                facts.insert(path.into(), serde_json::json!({"sha256": gen_audio_core::asset::sha256_hex(&bytes), "bytes": bytes.len()}));
            }
        }
    }
    facts
}

/// -FromLock verifies the cube bytes: the WAV-backed regen pins every cube
/// file it made in media.lock.json "cubes", byte for byte.
#[test]
fn media_lock_pins_every_cube_file_byte_for_byte() {
    let locked = json(MEDIA_LOCK)["cubes"].as_object().cloned().unwrap_or_default();
    let actual = cube_file_facts(|_, bytes| bytes);
    assert_eq!(actual.len(), 10, "5 cubes x (JSON, PNG)");
    assert_eq!(verify_cube_lock(&locked, &actual), Ok(()));
}

/// The M8 repro (PR5_VERIFICATION v4, regen-gate-and-mutations-noWAV.log:80-93):
/// a hand-edited layer_score in library_kokoro_cube3d.json passed
/// build_assets --from-lock with exit 0. A mode that can't regenerate must at
/// least verify what was generated.
#[test]
fn from_lock_refuses_a_hand_edited_layer_score() {
    let locked = json(MEDIA_LOCK)["cubes"].as_object().cloned().unwrap_or_default();
    let actual = cube_file_facts(|path, bytes| {
        if path != "library_kokoro_cube3d.json" {
            return bytes;
        }
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("\"layer_score\": 0.2396223396062851"), "repro anchor moved");
        text.replacen("\"layer_score\": 0.2396223396062851", "\"layer_score\": 0.2996223396062851", 1).into_bytes()
    });
    let errors = verify_cube_lock(&locked, &actual).unwrap_err();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("library_kokoro_cube3d.json"), "{errors:?}");
}

/// A cube the lock doesn't know and a lock entry no cube uses both fail;
/// an empty lock is not a pass.
#[test]
fn cube_lock_check_fails_on_unlocked_and_stale_entries() {
    let facts = |sha: &str| serde_json::json!({"sha256": sha.repeat(64), "bytes": 1});
    let mut locked = Map::new();
    locked.insert("a_cube3d.json".into(), facts("a"));
    locked.insert("gone_cube3d.json".into(), facts("b"));
    let mut actual = Map::new();
    actual.insert("a_cube3d.json".into(), facts("a"));
    actual.insert("new_cube3d.png".into(), facts("c"));
    let errors = verify_cube_lock(&locked, &actual).unwrap_err();
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(errors.iter().any(|e| e.contains("new_cube3d.png")) && errors.iter().any(|e| e.contains("gone_cube3d.json")), "{errors:?}");
    assert!(verify_cube_lock(&Map::new(), &actual).is_err());
    let mut bytes_only = actual.clone();
    bytes_only.remove("new_cube3d.png");
    bytes_only["a_cube3d.json"]["bytes"] = Value::from(2);
    locked.remove("gone_cube3d.json");
    assert!(verify_cube_lock(&locked, &bytes_only).is_err(), "a byte count change alone fails");
}

/// Item 2: a cube uid comes from the generator's content (generator_sha256,
/// layer_method in fields; params via the cube JSON bytes), never a commit.
#[test]
fn cube_uid_is_generator_content_and_a_commit_is_provenance_only() {
    let catalog = json(ASSETS);
    let cubes: Vec<&Value> = catalog["assets"].as_array().unwrap().iter().filter(|asset| asset["kind"] == "cube_ihdr").collect();
    assert_eq!(cubes.len(), 5);
    for cube in cubes {
        let id = &cube["legacy_id"];
        let sha = cube["fields"]["generator_sha256"].as_str().unwrap_or_default();
        assert!(gen_audio_core::asset::is_sha256_hex(sha), "{id}: fields.generator_sha256");
        assert_eq!(cube["provenance"]["generator_sha256"], sha, "{id}");
        assert_eq!(cube["fields"]["layer_method"], "library_r3", "{id}");
        assert!(cube["fields"].as_object().unwrap().keys().all(|key| !key.contains("commit")), "{id}: no commit in identity");
        let mut moved = cube.clone();
        moved["provenance"]["generator_commit"] = Value::from("0123456789abcdef0123456789abcdef01234567");
        assert_eq!(validate_envelope(&moved).unwrap().uid, cube["uid"].as_str().unwrap(), "{id}: a commit change keeps the uid");
        let mut edited = cube.clone();
        edited["fields"]["generator_sha256"] = Value::from("f".repeat(64));
        assert_ne!(gen_audio_core::asset::envelope_identity(&edited).unwrap(), gen_audio_core::asset::envelope_identity(cube).unwrap(), "{id}");
    }
}

/// E (Optimus, F5): status ok only for voice models Generate can produce in
/// this app. Every model with no in-app adapter is offline_only (clips were
/// rendered elsewhere, the VibeVoice honesty contract) or unavailable, with a
/// reason; and its clips' provenance says "offline run".
#[test]
fn every_engine_without_an_in_app_adapter_is_offline_only_or_unavailable() {
    let assets: Value = serde_json::from_str(ASSETS).unwrap();
    let assets = assets["assets"].as_array().unwrap();
    let models: Vec<&Value> = assets.iter().filter(|asset| asset["kind"] == "voice_model").collect();
    assert_eq!(models.len(), gen_audio_core::catalog::voice_models().len());
    for model in &models {
        let id = model["legacy_id"].as_str().unwrap();
        let adapter = model["body"]["synth_adapter"].as_bool().unwrap();
        let spec = gen_audio_core::catalog::voice_model(id).unwrap();
        assert_eq!(adapter, spec.synth_adapter, "{id}");
        if adapter {
            assert_eq!(model["status"], "ok", "{id} has an adapter");
            assert!(model["body"].get("availability").is_none(), "{id}");
            continue;
        }
        assert_eq!(model["status"], "unavailable", "{id}: no in-app adapter, so Generate cannot produce it");
        let availability = &model["body"]["availability"];
        let status = availability["status"].as_str().unwrap_or_else(|| panic!("{id}: no availability"));
        assert!(matches!(status, "offline_only" | "unavailable"), "{id}: {status}");
        assert!(!availability["reason"].as_str().unwrap_or("").is_empty(), "{id}: availability needs a reason");
        // A model whose clips exist here only as offline renders says so on each clip.
        for clip in assets.iter().filter(|asset| {
            asset["kind"] == "audio_clip" && asset["provenance"]["voice_model"] == model["uid"]
        }) {
            let generator = clip["provenance"]["generator"].as_str().unwrap_or("");
            assert!(generator.contains("offline run"), "{id}: clip {} provenance {generator}", clip["legacy_id"]);
        }
    }
    let status = |id: &str| models.iter().find(|model| model["legacy_id"] == id).unwrap()["body"]["availability"]["status"].clone();
    assert_eq!(status("kokoro_dayour"), "offline_only");
    assert_eq!(status("misaki"), "offline_only");
    assert_eq!(status("vibevoice"), "unavailable");
    // The engine list agrees: only the implemented engine is producible.
    for engine in gen_audio_core::engines::engines() {
        let implemented = engine.status == gen_audio_core::engines::EngineStatus::Implemented;
        if let Some(spec) = gen_audio_core::catalog::voice_model(engine.id) {
            assert_eq!(implemented, spec.synth_adapter, "{}", engine.id);
        }
    }
}

/// H (Optimus): a persona is config, not audio. No voice_profile and no
/// persona card may be bound_to a cube whose speakers do not include that
/// persona. Cubes carry no speakers list today, so no persona is bound to any
/// cube; the honest link is voice_profile.fields.voice_model, and the claim is
/// persona_config (never profile_preview).
#[test]
fn no_persona_is_bound_to_a_cube_it_does_not_speak_in() {
    let mut all = json(ASSETS)["assets"].as_array().unwrap().clone();
    all.extend(json(DEV_ASSETS)["assets"].as_array().unwrap().iter().cloned());
    let by_uid = |uid: &str| all.iter().find(|asset| asset["uid"] == uid).cloned();
    let speakers = |cube: &Value| -> Vec<String> {
        let list = cube.pointer("/body/speakers").or_else(|| cube.pointer("/fields/speakers"));
        list.and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().or_else(|| item["persona_id"].as_str()).or_else(|| item["id"].as_str()))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut personas = 0;
    let mut bad = Vec::new();
    for asset in &all {
        let persona = match asset["kind"].as_str() {
            Some("voice_profile") => asset["fields"]["persona_id"].as_str(),
            Some("card") if asset["fields"]["view"] == "VoiceProfile" => asset.pointer("/body/body/personaId").and_then(Value::as_str),
            _ => continue,
        };
        let persona = persona.unwrap_or_else(|| panic!("{} has no persona id", asset["uid"]));
        personas += 1;
        let claims = asset["honesty"]["claims"].as_array().unwrap();
        assert!(claims.iter().any(|claim| claim == "persona_config"), "{} claims {claims:?}", asset["uid"]);
        assert!(!claims.iter().any(|claim| claim == "profile_preview"), "{}", asset["uid"]);
        for target in asset.pointer("/relations/bound_to").and_then(Value::as_array).into_iter().flatten() {
            let target = by_uid(target.as_str().unwrap()).unwrap_or_else(|| panic!("{} bound_to dangles", asset["uid"]));
            if target["kind"] == "cube_ihdr" && !speakers(&target).iter().any(|id| id == persona) {
                bad.push(format!("{} ({persona}) -> {} ({})", asset["legacy_id"], target["uid"], target["legacy_id"]));
            }
        }
        if asset["kind"] == "voice_profile" {
            let model = asset["fields"]["voice_model"].as_str().unwrap();
            assert_eq!(by_uid(model).map(|m| m["kind"].clone()), Some(Value::from("voice_model")), "{persona}: voice_model");
        }
    }
    assert!(personas >= 10, "expected the 5 voice profiles and their 5 cards, saw {personas}");
    assert!(bad.is_empty(), "persona bound_to a cube it does not speak in:\n  {}", bad.join("\n  "));
    for card in json(RELEASE_VIEWPORT)["cards"].as_array().unwrap().iter().filter(|card| card["kind"] == "VoiceProfile") {
        assert!(card["body"].get("cubeJsonUrl").is_none(), "{}: persona card links a cube", card["id"]);
        assert_eq!(card["body"]["spectrogram3d"], "none", "{}", card["id"]);
    }
}

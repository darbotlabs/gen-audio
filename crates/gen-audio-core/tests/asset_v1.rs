//! Asset object model v1: golden vectors, schema parity and the library catalog.

use std::path::Path;

use gen_audio_core::asset::{
    check_media_path, glyph_from_uid, mint, normalize_nfc, parse_uid, validate_envelope, validate_set, verify_media,
    MediaCheck, MediaDigest,
};
use gen_audio_core::asset_migrate::migrate_to_v1;
use serde_json::Value;

const VECTORS: &str = include_str!("../../../schemas/asset-object/vectors/v1.json");
const FIXTURES: &str = include_str!("../../../schemas/asset-object/vectors/fixtures_v1.json");
const SCHEMA: &str = include_str!("../../../schemas/asset-object.schema.json");
const CARD_SCHEMA: &str = include_str!("../../../schemas/card-viewport.schema.json");
const PROFILE_SCHEMA: &str = include_str!("../../../schemas/voice_profile.schema.json");
const ASSETS: &str = include_str!("../../../apps/desktop/public/library/assets.json");
const VIEWPORT: &str = include_str!("../../../schemas/examples/viewport.example.json");

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
    assert_eq!(catalog["legacy_index"], fixtures["legacy_index"], "fixtures_v1.json is stale; rerun build_assets");
    // viewport.example.json keeps id and carries the card uid alongside.
    let index = fixtures["legacy_index"].as_object().unwrap();
    for card in json(VIEWPORT)["cards"].as_array().unwrap() {
        let id = card["id"].as_str().unwrap();
        assert_eq!(card["uid"], index[&format!("card:{id}")], "card {id} uid");
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

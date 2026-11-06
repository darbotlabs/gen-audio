//! Voice documents match the versioned JSON Schemas and the Rust checkers.

use std::fs;
use std::path::PathBuf;

use gen_audio_core::voices::{
    validate_cast_json, validate_catalog_json, validate_g2p_json, validate_voice_json,
};
use serde_json::Value;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read_json(path: &std::path::Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn validator(relative: &str) -> jsonschema::Validator {
    let schema = read_json(&root().join(relative));
    jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .build(&schema)
        .unwrap_or_else(|err| panic!("{relative} does not compile: {err}"))
}

fn assert_valid(validator: &jsonschema::Validator, instance: &Value, label: &str) {
    let errors: Vec<_> = validator
        .iter_errors(instance)
        .map(|err| err.to_string())
        .collect();
    assert!(errors.is_empty(), "{label}: {}", errors.join("; "));
}

#[test]
fn schemas_accept_the_shipped_examples_and_rust_agrees() {
    let root = root();
    let voice_schema = validator("schemas/voices/voice.v1.schema.json");
    let g2p_schema = validator("schemas/voices/g2p.v1.schema.json");
    let catalog_schema = validator("schemas/voices/catalog.v1.schema.json");
    let cast_schema = validator("schemas/voices/cast_map.v1.schema.json");
    let registry_schema = validator("schemas/voices/registry.v1.schema.json");
    let registry = read_json(&root.join("schemas/voices/registry.v1.json"));
    assert_valid(&registry_schema, &registry, "registry");

    let mut param_schemas = std::collections::HashMap::new();
    for row in registry["frameworks"].as_array().unwrap() {
        let id = row["id"].as_str().unwrap();
        let relative = format!("schemas/voices/{}", row["params_schema"].as_str().unwrap());
        param_schemas.insert(id.to_string(), validator(&relative));
    }

    let voices = root.join("voices");
    let catalog = read_json(&voices.join("catalog.example.json"));
    assert_valid(&catalog_schema, &catalog, "catalog");
    validate_catalog_json(&catalog).unwrap();

    let cast = read_json(&voices.join("cast_map.example.json"));
    assert_valid(&cast_schema, &cast, "cast");
    let parsed = validate_cast_json(&cast).unwrap();
    assert_eq!(parsed.engine, "kokoro_onnx");
    assert_eq!(parsed.speakers["1"].voice, "af_heart");
    assert_eq!(parsed.speakers["2"].voice, "am_michael");

    for relative in catalog["voices"].as_array().unwrap() {
        let name = relative.as_str().unwrap();
        let value = read_json(&voices.join(name));
        assert_valid(&voice_schema, &value, name);
        let framework = value["framework"].as_str().unwrap();
        assert_valid(
            param_schemas.get(framework).unwrap(),
            &value["params"],
            name,
        );
        let voice = validate_voice_json(&value).unwrap();
        assert_eq!(voice.id, value["id"].as_str().unwrap());
    }
    for relative in catalog["g2p"].as_array().unwrap() {
        let name = relative.as_str().unwrap();
        let value = read_json(&voices.join(name));
        assert_valid(&g2p_schema, &value, name);
        validate_g2p_json(&value).unwrap();
    }
}

#[test]
fn invalid_fixtures_match_python_error_codes() {
    let folder = root().join("tests/fixtures/voices/invalid");
    let cases: Value = read_json(&folder.join("cases.json"));
    for case in cases["cases"].as_array().unwrap() {
        let name = case["file"].as_str().unwrap();
        let code = case["code"].as_str().unwrap();
        let value = read_json(&folder.join(name));
        let error = match case["kind"].as_str().unwrap() {
            "voice" => validate_voice_json(&value).unwrap_err(),
            "g2p" => validate_g2p_json(&value).unwrap_err(),
            "cast" => validate_cast_json(&value).unwrap_err(),
            other => panic!("unknown kind {other}"),
        };
        assert!(error.starts_with(code), "{name}: {error}");
    }
}

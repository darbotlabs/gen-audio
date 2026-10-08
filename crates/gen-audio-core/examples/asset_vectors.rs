//! Regenerate the shared Rust/TS golden vectors.
//!
//! cargo run -p gen-audio-core --example asset_vectors
//!
//! Writes schemas/asset-object/vectors/v1.json. Both `cargo test` and the
//! desktop `npm test` assert the committed file, so any drift between the
//! Rust and TS implementations fails CI. Expected values come from the Rust
//! implementation; an independent Python recomputation agreed on every
//! library uid when this file was introduced.

use std::fs;
use std::path::{Path, PathBuf};

use gen_audio_core::asset::{
    bin_frames_inferred, build_envelope, check_media_path, glyph_dots, hue_class, mint, ms_from_frames, normalize_nfc, parse_uid, round_half_up,
    sha256_hex, validate_envelope, validate_set, AssetError, EnvelopeParts, MediaDigest,
};
use gen_audio_core::asset_migrate::migrate_to_v1;
use serde_json::{json, Map, Value};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("repo root")
}

fn digests(media: &Value) -> Vec<MediaDigest> {
    media
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| MediaDigest { role: item["role"].as_str().unwrap().into(), sha256: item["sha256"].as_str().unwrap().into() })
        .collect()
}

fn strings(value: &Value) -> Vec<String> {
    value.as_array().cloned().unwrap_or_default().iter().map(|item| item.as_str().unwrap().to_string()).collect()
}

fn mint_vector(name: &str, kind: &str, fields_json: &str, media: Value, src: Value, normalize: bool) -> Value {
    let mut fields: Value = serde_json::from_str(fields_json).expect("fields json");
    if normalize {
        fields = normalize_nfc(&fields);
    }
    let minted = mint(kind, &fields, &digests(&media), &strings(&src)).unwrap_or_else(|e| panic!("{name}: {e}"));
    let bytes = minted.digest_hex.as_bytes();
    let byte = |i: usize| u8::from_str_radix(std::str::from_utf8(&bytes[i * 2..i * 2 + 2]).unwrap(), 16).unwrap();
    json!({
        "name": name,
        "kind": kind,
        "fields_json": fields_json,
        "normalize_nfc": normalize,
        "media": media,
        "src": src,
        "expect": {
            "canonical": minted.canonical,
            "preimage_hex": minted.preimage_hex,
            "digest_hex": minted.digest_hex,
            "uid": minted.uid,
            "glyph": minted.glyph,
            "glyph_codepoints": [0x2800 + u32::from(byte(0)), 0x2800 + u32::from(byte(1))],
            "glyph_dots": [glyph_dots(byte(0)), glyph_dots(byte(1))],
            "hue_class": hue_class(kind),
        }
    })
}

fn reject_vector(name: &str, kind: &str, fields_json: &str) -> Value {
    let error = match serde_json::from_str::<Value>(fields_json) {
        Err(_) => "lone_surrogate".to_string(),
        Ok(fields) => match mint(kind, &fields, &[], &[]) {
            Err(AssetError { code, .. }) => code.to_string(),
            Ok(_) => panic!("{name} unexpectedly minted"),
        },
    };
    json!({"name": name, "kind": kind, "fields_json": fields_json, "error": error})
}

/// A real 16-bit mono PCM WAV of `frames` zero samples (44-byte header).
fn silent_wav(frames: u32, rate: u32) -> Vec<u8> {
    let data = frames * 2;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    out.resize(44 + data as usize, 0);
    out
}

fn code(result: Result<impl Sized, AssetError>) -> Value {
    match result {
        Ok(_) => Value::Null,
        Err(error) => json!(error.code),
    }
}

fn parts(kind: &'static str, title: &str, fields: Value) -> EnvelopeParts {
    EnvelopeParts {
        kind,
        legacy_id: None,
        title: title.into(),
        summary: None,
        status: "ok",
        fields,
        media: vec![],
        src: vec![],
        relations: Map::new(),
        honesty: json!({"synthesized_speech": false, "fixture": false, "claims": []}),
        provenance: Map::new(),
        body: json!({}),
    }
}

fn main() {
    let root = repo();
    let catalog: Value =
        serde_json::from_str(&fs::read_to_string(root.join("apps/desktop/public/library/assets.json")).unwrap()).unwrap();
    let assets = catalog["assets"].as_array().cloned().unwrap();
    let find = |legacy: &str, kind: &str| {
        assets.iter().find(|a| a["legacy_id"] == legacy && a["kind"] == kind).cloned().unwrap_or_else(|| panic!("{kind}:{legacy}"))
    };
    let clip = find("lib-misaki-kokoro", "audio_clip");
    let cube = find("lib-misaki-kokoro.cube", "cube_ihdr");
    let layer = find("lib-misaki-kokoro.cube.signal", "layer");
    let kokoro = find("kokoro_dayour", "voice_model");
    let kokoro_uid = kokoro["uid"].as_str().unwrap().to_string();
    let wav_sha = clip["media"][0]["sha256"].as_str().unwrap().to_string();

    // ---- positive mint vectors
    let cube_fields = serde_json::to_string(&cube["fields"]).unwrap();
    let cube_media: Vec<Value> = cube["media"].as_array().unwrap().iter().map(|m| json!({"role": m["role"], "sha256": m["sha256"]})).collect();
    let profile = |tone: &str| {
        format!(
            "{{\"persona_id\":\"cafe\",\"voice_model\":\"{kokoro_uid}\",\"tone\":\"{tone}\",\"purpose\":\"Golden vector\",\"domain\":\"Tests\",\"accent\":\"General American\",\"traits\":\"Golden vector persona.\",\"refs\":[\"persona:cafe\"]}}"
        )
    };
    let shuffled = format!(
        "{{\"refs\":[\"persona:cafe\"],\"traits\":\"Golden vector persona.\",\"accent\":\"General American\",\"domain\":\"Tests\",\"purpose\":\"Golden vector\",\"tone\":\"caf\\u00e9\",\"voice_model\":\"{kokoro_uid}\",\"persona_id\":\"cafe\"}}"
    );
    let mut zero_glyph = None;
    for n in 0..100_000u64 {
        let fields = format!("{{\"language\":\"en\",\"n_words\":{n}}}");
        let minted = mint("transcript", &serde_json::from_str(&fields).unwrap(), &[], &[]).unwrap();
        if minted.digest_hex.starts_with("00") {
            zero_glyph = Some(fields);
            break;
        }
    }
    let mut mint_vectors = vec![
        mint_vector("misaki_cube_real", "cube_ihdr", &cube_fields, json!(cube_media), cube["src"].clone(), false),
        mint_vector("nfc_cafe", "voice_profile", &profile("caf\\u00e9"), json!([]), json!([]), false),
        mint_vector("nfd_cafe_normalized_at_ingest", "voice_profile", &profile("cafe\\u0301"), json!([]), json!([]), true),
        mint_vector("key_order_shuffle", "voice_profile", &shuffled, json!([]), json!([]), false),
        mint_vector("emoji_astral", "voice_profile", &profile("loud \\ud83d\\ude00 \\ud834\\udd1e"), json!([]), json!([]), false),
        mint_vector("empty_relations", "voice_model", "{\"model_id\":\"golden\",\"waveform\":true}", json!([]), json!([]), false),
        mint_vector(
            "media_a",
            "audio_clip",
            "{\"engine\":\"kokoro_onnx\",\"sample_rate_hz\":24000,\"duration_ms\":1000}",
            json!([{"role": "wav", "sha256": wav_sha}]),
            json!([]),
            false,
        ),
        mint_vector(
            "media_b_changes_uid",
            "audio_clip",
            "{\"engine\":\"kokoro_onnx\",\"sample_rate_hz\":24000,\"duration_ms\":1000}",
            json!([{"role": "wav", "sha256": "0".repeat(64)}]),
            json!([]),
            false,
        ),
        mint_vector("glyph_zero_byte", "transcript", zero_glyph.as_deref().unwrap(), json!([]), json!([]), false),
        mint_vector("layer_with_parent", "layer", "{\"name\":\"signal\",\"index\":0}", json!([]), layer["src"].clone(), false),
        // B1: duration_ms derived with the normative half-up rule. 24008 frames
        // at 16 kHz = 1500.5 ms exactly (tie -> 1501); 33447 at 24 kHz =
        // 1393.625 ms (non-whole -> 1394).
        mint_vector(
            "clip_tie_half_ms_16k",
            "audio_clip",
            &format!("{{\"engine\":\"kokoro_onnx\",\"sample_rate_hz\":16000,\"duration_ms\":{}}}", ms_from_frames(24_008, 16_000)),
            json!([{"role": "wav", "sha256": "0".repeat(64)}]),
            json!([]),
            false,
        ),
        mint_vector(
            "clip_non_whole_ms_24k",
            "audio_clip",
            &format!("{{\"engine\":\"kokoro_onnx\",\"sample_rate_hz\":24000,\"duration_ms\":{}}}", ms_from_frames(33_447, 24_000)),
            json!([{"role": "wav", "sha256": "0".repeat(64)}]),
            json!([]),
            false,
        ),
    ];
    mint_vectors[10]["derived_from"] = json!({"frames": 24_008, "sample_rate_hz": 16_000, "rule": "ms_from_frames"});
    mint_vectors[11]["derived_from"] = json!({"frames": 33_447, "sample_rate_hz": 24_000, "rule": "ms_from_frames"});

    // ---- normative rounding table (B1)
    let mut rounding: Vec<Value> = Vec::new();
    for (frames, rate) in [(24_008u64, 16_000u64), (33_447, 24_000), (12, 24_000), (11, 24_000), (1, 48_000), (3_345_000, 24_000), (7, 44_100)] {
        rounding.push(json!({"op": "ms_from_frames", "frames": frames, "rate": rate, "expect": ms_from_frames(frames, rate)}));
    }
    for (value, scale) in [(139.04, 1000.0), (0.0125, 1000.0), (1.0005, 1000.0), (2.5, 1.0), (0.5, 1.0), (0.49999999999999994, 1.0), (1.2345675, 1_000_000.0), (0.0016666666666666668, 24_000.0)] {
        let product: f64 = value * scale;
        rounding.push(json!({"op": "round_half_up", "value": value, "scale": scale, "expect": round_half_up(product).unwrap()}));
    }
    // B1': inferred cube bin_frames, two binary64 ops (multiply, then divide).
    // 157.134 * 24000 = 3771215.9999999995, / 96 = 39283.49999999999 -> 39283.
    // Exact rational math gives 39283.5 -> 39284; lib-cube-explainer's cube uid
    // (ga:cube_ihdr:bcuw4m76pyqanslfiugnvlxnda) depends on the binary64 result.
    let near_tie = bin_frames_inferred(157.134, 24_000, 96).unwrap();
    assert_eq!(near_tie, 39_283, "B1' near-tie");
    rounding.push(json!({
        "op": "bin_frames_inferred", "duration_s": 157.134, "sample_rate_hz": 24_000, "time_bins": 96, "expect": near_tie,
        "note": "near-tie: fl(fl(157.134*24000)/96) = 39283.49999999999 -> 39283; exact rational 39283.5 would give 39284"
    }));
    for vector in &mut mint_vectors {
        vector["expect"]["canonical"] = vector["expect"]["canonical"].clone();
    }

    // ---- canonicalization rejects
    let rejects = vec![
        reject_vector("float_0_1", "transcript", "{\"language\":\"en\",\"n_words\":0.1}"),
        reject_vector("float_1e_minus_7", "transcript", "{\"language\":\"en\",\"n_words\":1e-7}"),
        reject_vector("float_139_375", "audio_clip", "{\"engine\":\"x\",\"sample_rate_hz\":24000,\"duration_ms\":139.375}"),
        reject_vector("integral_float_139375_0", "audio_clip", "{\"engine\":\"x\",\"sample_rate_hz\":24000,\"duration_ms\":139375.0}"),
        reject_vector("integral_float_exponent_1e3", "transcript", "{\"language\":\"en\",\"n_words\":1e3}"),
        reject_vector("negative_zero", "transcript", "{\"language\":\"en\",\"n_words\":-0}"),
        reject_vector("unsafe_integer", "transcript", "{\"language\":\"en\",\"n_words\":9007199254740993}"),
        reject_vector("null_value", "transcript", "{\"language\":null,\"n_words\":1}"),
        reject_vector("non_ascii_key", "transcript", "{\"langu\\u00e4ge\":\"en\",\"n_words\":1}"),
        reject_vector("control_char_u001f", "transcript", "{\"language\":\"e\\u001fn\",\"n_words\":1}"),
        reject_vector("non_nfc_text", "transcript", "{\"language\":\"cafe\\u0301\",\"n_words\":1}"),
        reject_vector("lone_surrogate", "transcript", "{\"language\":\"\\ud800\",\"n_words\":1}"),
    ];

    // ---- identity rejects (B2 duplicate role, D3 duplicate src / fan-out)
    let identity_reject = |name: &str, kind: &str, fields: Value, media: Value, src: Value| {
        let error = code(mint(kind, &fields, &digests(&media), &strings(&src)));
        assert!(!error.is_null(), "{name} unexpectedly minted");
        json!({"name": name, "kind": kind, "fields": fields, "media": media, "src": src, "error": error})
    };
    let cube_uid = cube["uid"].as_str().unwrap().to_string();
    let many: Vec<String> = (0..17u64)
        .map(|n| mint("transcript", &json!({"language": "en", "n_words": n}), &[], &[]).unwrap().uid)
        .collect();
    let identity_rejects = vec![
        identity_reject(
            "duplicate_media_role",
            "audio_clip",
            json!({"engine": "kokoro_onnx", "sample_rate_hz": 24000, "duration_ms": 1000}),
            json!([{"role": "wav", "sha256": wav_sha}, {"role": "wav", "sha256": "0".repeat(64)}]),
            json!([]),
        ),
        identity_reject("duplicate_src", "layer", json!({"name": "signal", "index": 0}), json!([]), json!([cube_uid, cube_uid])),
        identity_reject("src_fan_out_17", "card", json!({"card_id": "fan", "view": "LibraryClip"}), json!([]), json!(many)),
    ];

    // ---- uid grammar
    let good = cube["uid"].as_str().unwrap().to_string();
    let body = good.rsplit(':').next().unwrap().to_string();
    let mut padded = body.clone();
    padded.replace_range(25..26, "b");
    let uid_cases = vec![
        good.clone(),
        good.to_uppercase(),
        format!("ga:cube_ihdr:{}1", &body[..25]),
        format!("ga:cube_ihdr:{}", &body[..25]),
        format!("ga:cube_ihdr:{body}a"),
        format!("ga:cube_ihdr:{padded}"),
        format!("ga:widget:{body}"),
        format!("ga:agent/VoiceProfile:{body}"),
        format!("xx:cube_ihdr:{body}"),
    ];
    let uid_vectors: Vec<Value> = uid_cases.iter().map(|uid| json!({"uid": uid, "error": code(parse_uid(uid))})).collect();

    // ---- media paths
    let paths = [
        "genaid_full_misaki_kokoro.wav", "cubes/rev2/x_cube3d.json", "", "../secrets.wav", "a/../b.wav", "a..b.wav",
        "/etc/passwd", "\\\\server\\share\\x.wav", "//server/share/x.wav", "C:\\gen-audio\\x.wav", "D:/x.wav",
        "https://example.com/x.wav", "file:x.wav", "a\\b.wav", ".hidden.wav", "a b.wav",
    ];
    let path_vectors: Vec<Value> = paths.iter().map(|path| json!({"path": path, "error": code(check_media_path(path))})).collect();

    let card = |id: &str| {
        let mut p = parts("card", id, json!({"card_id": id, "view": "LibraryClip"}));
        p.body = json!({"id": id, "kind": "LibraryClip", "title": id, "body": {}});
        build_envelope(p).unwrap()
    };

    // ---- envelope invariants (one negative per invariant)
    let mut envelopes: Vec<Value> = Vec::new();
    let mut push = |name: &str, envelope: Value, schema_valid: bool| {
        let error = code(validate_envelope(&envelope));
        envelopes.push(json!({"name": name, "envelope": envelope, "schema_valid": schema_valid, "error": error}));
    };
    push("ok_misaki_clip", clip.clone(), true);
    push("ok_misaki_cube", cube.clone(), true);
    push("ok_misaki_layer", layer.clone(), true);
    push("float_view_in_body_ok", { let mut c = cube.clone(); c["body"]["duration_s"] = json!(139.375); c }, true);
    let mut retitled = cube.clone();
    retitled["display"]["title"] = json!("Renamed cube (title is display only)");
    push("title_change_keeps_uid", retitled, true);
    push("fixture_claims_speech", { let mut c = kokoro.clone(); c["honesty"]["fixture"] = json!(true); c["honesty"]["synthesized_speech"] = json!(true); c }, false);
    let profile_env = find("optimus", "voice_profile");
    push("profile_claims_speech", { let mut p = profile_env.clone(); p["honesty"]["synthesized_speech"] = json!(true); p }, false);
    push("profile_not_marked_not_podcast", { let mut p = profile_env.clone(); p["honesty"]["not_podcast"] = json!(false); p }, false);
    let clip_parts = |media: Vec<Value>, provenance: Map<String, Value>| {
        let mut p = parts("audio_clip", "clip", json!({"engine": "kokoro_onnx", "sample_rate_hz": 24000, "duration_ms": 1000}));
        p.media = media;
        p.provenance = provenance;
        p.honesty = json!({"synthesized_speech": true, "fixture": false, "claims": ["synthesized_speech"]});
        build_envelope(p).unwrap()
    };
    // The misaki WAV's real sha256 with its real byte count (bytes is unhashed).
    let wav = json!({"role": "wav", "path": "x.wav", "sha256": wav_sha, "bytes": clip["media"][0]["bytes"].clone(), "mime": "audio/wav"});
    let full_prov: Map<String, Value> = [("engine".to_string(), json!("kokoro_onnx")), ("voice_model".to_string(), json!(kokoro_uid))].into_iter().collect();
    push("clip_missing_wav", clip_parts(vec![], full_prov.clone()), false);
    push("clip_missing_engine", clip_parts(vec![wav.clone()], [("voice_model".to_string(), json!(kokoro_uid))].into_iter().collect()), false);
    push("clip_missing_voice_model", clip_parts(vec![wav.clone()], [("engine".to_string(), json!("kokoro_onnx"))].into_iter().collect()), false);
    let cube_with = |src: Vec<String>, covers: u64| {
        let mut fields = cube["fields"].clone();
        fields["covers_ms"] = json!(covers);
        let mut p = parts("cube_ihdr", "cube", fields);
        p.media = cube["media"].as_array().unwrap().clone();
        p.src = src;
        p.honesty = json!({"synthesized_speech": false, "fixture": false, "claims": ["library_cube"]});
        build_envelope(p).unwrap()
    };
    let clip_uid = clip["uid"].as_str().unwrap().to_string();
    let other_clip = find("lib-kokoro", "audio_clip")["uid"].as_str().unwrap().to_string();
    push("derived_needs_one_clip", cube_with(vec![clip_uid.clone(), other_clip.clone()], 139_040), false);
    push("covers_exceeds_duration", cube_with(vec![clip_uid.clone()], 139_376), true);
    let layer_with = |name: &str, layer_of: Option<&str>| {
        let mut p = parts("layer", "layer", json!({"name": name, "index": 0}));
        p.src = vec![cube["uid"].as_str().unwrap().to_string()];
        if let Some(target) = layer_of {
            p.relations.insert("layer_of".into(), json!(target));
        }
        build_envelope(p).unwrap()
    };
    push("layer_needs_cube", layer_with("signal", None), false);
    push("bad_layer_name", layer_with("bass", Some(cube["uid"].as_str().unwrap())), false);
    for (name, voice) in [("bad_voice_ref_connector", "copilot"), ("bad_voice_ref_pack", "kokoro-pack:af_heart")] {
        let mut fields = profile_env["fields"].clone();
        fields["voice_model"] = json!(voice);
        let mut p = parts("voice_profile", "Optimus", fields);
        p.honesty = profile_env["honesty"].clone();
        push(name, build_envelope(p).unwrap(), false);
    }
    let tool = |description: &str| {
        let mut p = parts("mcp_tool", "tool", json!({"name": "asset_glyph", "input_schema": {"type": "object", "required": ["uid"], "properties": {"uid": {"type": "string"}}}}));
        p.body = json!({"description": description});
        build_envelope(p).unwrap()
    };
    push("ok_mcp_tool", tool("Return the glyph for a uid."), true);
    push("mcp_tool_secret", tool("Call with key sk-abcdef1234567890"), true);
    push("mcp_tool_env", tool("OPENAI_API_KEY=abc123"), true);
    push("unknown_claim", { let mut c = kokoro.clone(); c["honesty"]["claims"] = json!(["studio_quality"]); c }, false);
    push("unknown_major", { let mut c = kokoro.clone(); c["schema_version"] = json!("2.0.0"); c }, false);
    push("uid_mismatch_tampered_field", { let mut c = cube.clone(); c["fields"]["n_points"] = json!(3601); c }, true);
    push("glyph_mismatch", { let mut c = cube.clone(); c["display"]["glyph"] = json!("\u{2800}\u{2800}"); c }, true);
    push("kind_mismatch", { let mut c = cube.clone(); c["kind"] = json!("spectrogram_2d"); c }, false);
    push("media_path_dotdot_inside_segment", { let mut p = clip.clone(); p["media"][0]["path"] = json!("a..b.wav"); p }, false);
    push("bad_extension_key", { let mut c = cube.clone(); c["extensions"] = json!({"vendor": 1}); c }, false);
    // D1: root/display/legacy_id rules enforced by the validator as well as the schema.
    push("unknown_root_key", { let mut c = cube.clone(); c["notes"] = json!("x"); c }, false);
    push("missing_glyph", { let mut c = cube.clone(); c["display"].as_object_mut().unwrap().remove("glyph"); c }, false);
    push("empty_title", { let mut c = cube.clone(); c["display"]["title"] = json!(""); c }, false);
    push("legacy_id_dotdot", { let mut c = cube.clone(); c["legacy_id"] = json!(".."); c }, false);
    push("legacy_id_traversal", { let mut c = cube.clone(); c["legacy_id"] = json!("../../etc/passwd"); c }, false);
    // C2: display_rev is a display-only integer; bumping it never changes the uid.
    let mut renamed = cube.clone();
    renamed["display"]["title"] = json!("Renamed cube");
    renamed["display"]["display_rev"] = json!(1);
    push("display_rev_bump_keeps_uid", renamed, true);
    push("display_rev_not_integer", { let mut c = cube.clone(); c["display"]["display_rev"] = json!("1"); c }, false);
    // B2: one entry per role; required roles per kind.
    push("media_duplicate_role", { let mut c = cube.clone(); let first = c["media"][0].clone(); c["media"].as_array_mut().unwrap().push(first); c }, false);
    let png_only = {
        let mut p = parts("cube_ihdr", "cube", cube["fields"].clone());
        p.media = cube["media"].as_array().unwrap().iter().filter(|m| m["role"] != "cube_json").cloned().collect();
        p.src = strings(&cube["src"]);
        p.honesty = cube["honesty"].clone();
        build_envelope(p).unwrap()
    };
    push("cube_missing_cube_json", png_only, false);
    // B2 ruling: a real cube (claims library_cube, not a fixture) needs cube_json
    // AND cube_png; a pending cube (no library_cube claim) may lack the png.
    let json_only = |honesty: Value| {
        let mut p = parts("cube_ihdr", "cube", cube["fields"].clone());
        p.media = cube["media"].as_array().unwrap().iter().filter(|m| m["role"] == "cube_json").cloned().collect();
        p.src = strings(&cube["src"]);
        p.honesty = honesty;
        build_envelope(p).unwrap()
    };
    push("cube_real_with_json_and_png", cube.clone(), true);
    push("cube_real_missing_png", json_only(cube["honesty"].clone()), false);
    push("cube_pending_missing_png", json_only(json!({"synthesized_speech": false, "fixture": false, "claims": []})), true);
    // D2: identity integers are integer tokens in the envelope too (Rust code
    // float_in_identity, same as mint). JSON Schema cannot see the token.
    push("envelope_integral_float_field", { let mut c = cube.clone(); c["fields"]["duration_ms"] = json!(139_375.0); c }, true);
    // C2: display_rev 3.0 is rejected by Rust and TS (raw token); Ajv cannot tell.
    push("display_rev_integral_float", { let mut c = cube.clone(); c["display"]["display_rev"] = json!(3.0); c }, true);
    // D1: honesty.note is capped at 400 chars everywhere.
    push("honesty_note_400_ok", { let mut c = cube.clone(); c["honesty"]["note"] = json!("n".repeat(400)); c }, true);
    push("honesty_note_401", { let mut c = cube.clone(); c["honesty"]["note"] = json!("n".repeat(401)); c }, false);
    // D3: channels 1..32, wav_url follows media_path rules, relation fan-out 8.
    let clip_with_channels = |channels: u64| {
        let mut fields = clip["fields"].clone();
        fields["channels"] = json!(channels);
        let mut p = parts("audio_clip", "clip", fields);
        p.media = clip["media"].as_array().unwrap().clone();
        p.provenance = clip["provenance"].as_object().unwrap().clone();
        p.honesty = clip["honesty"].clone();
        build_envelope(p).unwrap()
    };
    push("channels_zero", clip_with_channels(0), false);
    push("channels_33", clip_with_channels(33), false);
    push("wav_url_dotdot", { let mut c = clip.clone(); c["body"]["wav_url"] = json!("/library/../secrets.wav"); c }, false);
    push("wav_url_dotdot_inside_segment", { let mut c = clip.clone(); c["body"]["wav_url"] = json!("/library/a..b.wav"); c }, false);
    push("wav_url_nested_ok", { let mut c = clip.clone(); c["body"]["wav_url"] = json!("/library/clips/2026/x.wav"); c }, true);
    push("wav_url_not_library", { let mut c = clip.clone(); c["body"]["wav_url"] = json!("https://example.com/x.wav"); c }, false);
    push("relations_fan_out_9", { let mut c = card("fan-out"); c["relations"] = json!({"composes": many[..9]}); c }, false);

    // ---- set-level invariants
    let mut sets: Vec<Value> = Vec::new();
    let mut push_set = |name: &str, members: Vec<Value>| {
        let error = code(validate_set(&members));
        sets.push(json!({"name": name, "members": members, "error": error}));
    };
    let misaki_set: Vec<Value> = vec![kokoro.clone(), find("misaki", "voice_model"), clip.clone(), cube.clone(), layer.clone()];
    push_set("ok_misaki_set", misaki_set.clone());
    let mut wrong_sha = cube.clone();
    {
        let mut fields = cube["fields"].clone();
        fields["source_sha256"] = json!("1".repeat(64));
        let mut p = parts("cube_ihdr", "cube", fields);
        p.media = cube["media"].as_array().unwrap().clone();
        p.src = vec![clip_uid.clone()];
        wrong_sha = build_envelope(p).unwrap_or(wrong_sha);
    }
    push_set("source_sha_mismatch", vec![kokoro.clone(), find("misaki", "voice_model"), clip.clone(), wrong_sha]);
    let drift = {
        let mut fields = cube["fields"].clone();
        fields["duration_ms"] = json!(139_375 + 400);
        fields["covers_ms"] = json!(139_040);
        let mut p = parts("cube_ihdr", "cube", fields);
        p.media = cube["media"].as_array().unwrap().clone();
        p.src = vec![clip_uid.clone()];
        build_envelope(p).unwrap()
    };
    push_set("duration_drift", vec![kokoro.clone(), find("misaki", "voice_model"), clip.clone(), drift]);
    let (mut a, mut b) = (card("cycle-a"), card("cycle-b"));
    let (ua, ub) = (a["uid"].clone(), b["uid"].clone());
    a["relations"] = json!({"bound_to": [ub]});
    b["relations"] = json!({"bound_to": [ua]});
    push_set("link_cycle", vec![a, b]);
    push_set("layer_cube_missing", vec![layer.clone()]);
    let mut dangling = card("dangling");
    dangling["relations"] = json!({"bound_to": [many[0]]});
    push_set("dangling_relation", vec![dangling]);

    // ---- migration before/after. D6: the media entry is a real, synthetic
    // file: a 1.000 s, 24 kHz, mono, 16-bit silent PCM WAV (44 + 48000 =
    // 48044 bytes), so sha256 and bytes agree with each other.
    let golden_wav = silent_wav(24_000, 24_000);
    let golden_sha = sha256_hex(&golden_wav);
    let v0 = json!({
        "manifest": {"clips": [{
            "id": "lib-golden", "engineId": "kokoro_onnx", "title": "Golden clip", "status": "ok",
            "synthesizedSpeech": true, "sample_rate": 24000, "duration_s": 1.0,
            "wavUrl": "/library/golden.wav", "wav": "artifacts/library/golden.wav",
            "absWav": "D:\\gen-audio\\artifacts\\library\\golden.wav", "synth_wall_s": 0.42
        }, {"id": "lib-magpie", "engineId": "magpie", "title": "Magpie", "status": "unavailable", "synthesizedSpeech": false, "reason": "not built"}]},
        "voice_models": [
            {"id": "kokoro_onnx", "label": "kokoro-onnx", "waveform": true, "synthAdapter": true, "unavailable": false, "note": "golden"},
            {"id": "magpie", "label": "Magpie (unavailable)", "waveform": true, "synthAdapter": false, "unavailable": true, "note": "golden"}
        ],
        "cards": [], "profiles": [], "cube_docs": {}, "spectrograms": [],
        "media": {"golden.wav": {"sha256": golden_sha, "bytes": golden_wav.len(), "frames": 24000, "sample_rate": 24000, "channels": 1}}
    });
    let after = migrate_to_v1(&v0).expect("migration vector");
    let mut retitled_v0 = v0.clone();
    retitled_v0["manifest"]["clips"][0]["title"] = json!("Renamed golden clip");
    let retitled_after = migrate_to_v1(&retitled_v0).expect("retitled migration");
    // E4: a cube JSON that records the sha256 of a different WAV is refused.
    let mut foreign_cube = v0.clone();
    foreign_cube["manifest"]["clips"][0]["cube"] = json!({"jsonUrl": "/library/golden_cube3d.json", "pngUrl": "/library/golden_cube3d.png"});
    foreign_cube["cube_docs"] = json!({"golden_cube3d.json": {"source_sha256": "0".repeat(64), "cube_revision": 3, "sample_rate": 24000, "duration_s": 1.0}});
    let migrations = vec![
        json!({"name": "cube_made_from_another_wav", "before": foreign_cube, "after": null, "error": code(migrate_to_v1(&foreign_cube))}),
        json!({"name": "v0_manifest_clip_to_v1", "synthetic_media": "golden.wav = 1.000 s 24 kHz mono 16-bit silent PCM WAV (48044 bytes, all-zero samples)", "before": v0, "after": after, "error": null}),
        json!({"name": "v0_rename_keeps_uid", "before": retitled_v0, "after": retitled_after, "error": null}),
        json!({"name": "unknown_major_rejected", "before": {"schema_version": "2.0.0", "assets": []}, "after": null, "error": code(migrate_to_v1(&json!({"schema_version": "2.0.0", "assets": []})))}),
    ];

    let doc = json!({
        "schema_version": "1.0.0",
        "uid_scheme": "ga1",
        "domain_tag": "ga-asset-v1",
        "note": "Shared golden vectors. Regenerate with `cargo run -p gen-audio-core --example asset_vectors`; cargo test and `npm test` (apps/desktop) both assert this file.",
        "mint": mint_vectors,
        "canonical_reject": rejects,
        "identity_reject": identity_rejects,
        "rounding": rounding,
        "uid_parse": uid_vectors,
        "media_path": path_vectors,
        "envelopes": envelopes,
        "sets": sets,
        "migrations": migrations,
    });
    let out = root.join("schemas/asset-object/vectors/v1.json");
    fs::write(&out, serde_json::to_string_pretty(&doc).unwrap() + "\n").unwrap();
    println!("wrote {}", out.display());
}

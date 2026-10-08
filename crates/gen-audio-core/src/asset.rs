//! Unified asset object model, v1 draft.
//!
//! The hand-written JSON Schema `schemas/asset-object.schema.json` is the
//! contract. This module holds the parts a schema cannot express: the
//! canonical identity preimage, the content-addressed uid, the glyph, the
//! media-path rules, the honesty invariants and the v0 -> v1 migration.
//! `apps/desktop/src/asset.ts` mirrors it; both are pinned by
//! `schemas/asset-object/vectors/v1.json`.
//!
//! Identity (hashed):
//! ```text
//! identity = {"kind", "schema_major", "fields", "media":[{"role","sha256"}] (sorted by role, sha256),
//!             "src":[parent uids] (sorted; duplicates rejected)}
//! preimage = "ga-asset-v1" 0x00 JCS(identity)        (RFC 8785, integer-only subset)
//! digest   = SHA-256(preimage)
//! uid      = "ga:" kind ":" base32_lower_nopad(digest[0..16])   (26 chars, last 2 pad bits 0)
//! glyph    = U+2800+digest[0], U+2800+digest[1]                  (visual hint only)
//! ```

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use unicode_normalization::{is_nfc, UnicodeNormalization};

use crate::redact::redact_secrets;

pub const SCHEMA_MAJOR: u64 = 1;
pub const SCHEMA_VERSION: &str = "1.0.0";
pub const UID_SCHEME: &str = "ga1";
pub const DOMAIN_TAG: &str = "ga-asset-v1";
pub const MAX_MEDIA_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_LINK_DEPTH: usize = 16;
pub const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;
/// Max parents in `src` (schema maxItems).
pub const MAX_SRC: usize = 16;
/// Max targets per relation list (composes, bound_to, supersedes).
pub const MAX_FAN_OUT: usize = 8;
/// `honesty.note` cap, in Unicode scalar values (schema `maxLength: 400`).
pub const MAX_HONESTY_NOTE_CHARS: usize = 400;
/// Envelope root keys (schema `additionalProperties: false`).
pub const ROOT_KEYS: &[&str] = &[
    "schema_version", "uid_scheme", "kind", "uid", "legacy_id", "status", "fields", "media", "src", "relations", "honesty",
    "provenance", "display", "body", "extensions",
];
const DISPLAY_KEYS: &[&str] = &["title", "summary", "semantic_name", "face_name", "glyph", "display_rev"];

pub const KINDS: &[&str] = &[
    "voice_model",
    "voice_profile",
    "audio_clip",
    "spectrogram_2d",
    "cube_ihdr",
    "layer",
    "podcast_script",
    "transcript",
    "card",
    "mcp_tool",
];

pub const LAYER_NAMES: &[&str] = &["signal", "tonality", "confidence", "quality"];

/// Closed honesty vocabulary. Anything else is rejected.
pub const CLAIMS: &[&str] = &[
    "real_wav",
    "synthesized_speech",
    "library_cube",
    "library_spectrogram",
    "fixture_tone",
    "persona_config",
    "reference_only",
    "not_a_podcast_render",
    "engine_unavailable",
    "g2p_only",
    "status_only",
    "sample_content",
];

const BASE32: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetError {
    pub code: &'static str,
    pub detail: String,
}

impl std::fmt::Display for AssetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.detail)
    }
}

impl std::error::Error for AssetError {}

fn err<T>(code: &'static str, detail: impl Into<String>) -> Result<T, AssetError> {
    Err(AssetError { code, detail: detail.into() })
}

// ---------------------------------------------------------------- canonical JSON

/// RFC 8785 (JCS) serialization of the identity subset. Identity holds no
/// floats, no null, ASCII keys and NFC strings without control characters, so
/// JCS reduces to: sorted keys, no whitespace, integers in decimal, and only
/// `"` and `\` escaped.
pub fn canonicalize(value: &Value) -> Result<String, AssetError> {
    let mut out = String::new();
    write_canonical(value, &mut out, "$")?;
    Ok(out)
}

fn write_canonical(value: &Value, out: &mut String, path: &str) -> Result<(), AssetError> {
    match value {
        Value::Null => err("null_in_identity", format!("{path} is null; omit the key instead")),
        Value::Bool(flag) => {
            out.push_str(if *flag { "true" } else { "false" });
            Ok(())
        }
        Value::Number(number) => {
            out.push_str(&canonical_integer(number, path)?.to_string());
            Ok(())
        }
        Value::String(text) => {
            check_identity_string(text, path)?;
            write_string(text, out);
            Ok(())
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical(item, out, &format!("{path}[{index}]"))?;
            }
            out.push(']');
            Ok(())
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            for key in &keys {
                if key.is_empty() || !key.bytes().all(|byte| (0x20..=0x7e).contains(&byte)) {
                    return err("non_ascii_key", format!("{path} key {key:?} must be printable ASCII"));
                }
            }
            // ASCII keys: UTF-8 byte order equals the UTF-16 order JCS specifies.
            keys.sort();
            out.push('{');
            for (index, key) in keys.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_string(key, out);
                out.push(':');
                write_canonical(&map[*key], out, &format!("{path}.{key}"))?;
            }
            out.push('}');
            Ok(())
        }
    }
}

fn canonical_integer(number: &serde_json::Number, path: &str) -> Result<i64, AssetError> {
    if let Some(value) = number.as_i64() {
        if value.abs() > MAX_SAFE_INTEGER {
            return err("integer_out_of_range", format!("{path} is outside +/-(2^53-1)"));
        }
        return Ok(value);
    }
    if number.as_u64().is_some() {
        return err("integer_out_of_range", format!("{path} is outside +/-(2^53-1)"));
    }
    let float = number.as_f64().unwrap_or(f64::NAN);
    if !float.is_finite() {
        return err("non_finite_number", format!("{path} is NaN or infinite"));
    }
    if float == 0.0 && float.is_sign_negative() {
        return err("negative_zero", format!("{path} is -0"));
    }
    // Integral floats (139375.0, 1e3) are rejected too: identity integers are
    // JSON integer tokens, and the minter only ever writes integer tokens.
    if float.fract() != 0.0 {
        return err("float_in_identity", format!("{path} is not an integer; keep float views outside identity"));
    }
    err("float_in_identity", format!("{path} is an integral float ({float}); write it as an integer token"))
}

// ---------------------------------------------------------------- rounding (normative)

/// Whole milliseconds from a frame count: round half up, in exact integer
/// arithmetic. `duration_ms = (frames * 1000 + rate / 2) div rate`.
pub fn ms_from_frames(frames: u64, rate: u64) -> u64 {
    if rate == 0 {
        return 0;
    }
    (frames.saturating_mul(1000) + rate / 2) / rate
}

/// Round a non-negative IEEE-754 double half up (ties away from zero, which
/// is the same thing for x >= 0). Used for `covers_ms = round(cube_covers_s *
/// 1000)` and `inv_hdr_ppm = round(inv_hdr * 1e6)` (one IEEE-754 multiply),
/// and for the inferred cube `bin_frames = round(fl(fl(duration_s *
/// sample_rate_hz) / time_bins))` (two binary64 ops, multiply first); see
/// `asset_migrate` and docs/ASSET_OBJECT_MODEL.md (B1').
pub fn round_half_up(value: f64) -> Result<u64, AssetError> {
    if !value.is_finite() || value < 0.0 {
        return err("bad_rounding_input", format!("{value} must be finite and >= 0"));
    }
    let rounded = value.round();
    if rounded > MAX_SAFE_INTEGER as f64 {
        return err("integer_out_of_range", format!("{value} rounds outside 2^53-1"));
    }
    Ok(rounded as u64)
}

/// Inferred cube `bin_frames` for a cube JSON without `downsample_sf_st`:
/// `round_half_up(fl(fl(duration_s * sample_rate_hz) / time_bins))`, exactly
/// two binary64 operations, multiply first (B1'). Uses the cube JSON's float
/// `duration_s`, never frames: 157.134 s at 24 kHz over 96 bins is
/// 39283.49999999999 in binary64, so 39283 (exact rational math says 39283.5,
/// which would round to 39284 and re-mint lib-cube-explainer's cube).
pub fn bin_frames_inferred(duration_s: f64, sample_rate_hz: u64, time_bins: u64) -> Result<u64, AssetError> {
    let product = duration_s * sample_rate_hz as f64;
    round_half_up(product / time_bins.max(1) as f64)
}

fn check_identity_string(text: &str, path: &str) -> Result<(), AssetError> {
    if let Some(ch) = text.chars().find(|ch| (*ch as u32) < 0x20 || *ch as u32 == 0x7f) {
        return err("control_character", format!("{path} holds U+{:04X}", ch as u32));
    }
    if !is_nfc(text) {
        return err("non_nfc_string", format!("{path} is not NFC; normalize at ingest"));
    }
    Ok(())
}

fn write_string(text: &str, out: &mut String) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            _ => out.push(ch),
        }
    }
    out.push('"');
}

/// NFC-normalize every string (and key) in a JSON value. Used at ingest and
/// migration time; hashing itself never normalizes, it rejects non-NFC input.
pub fn normalize_nfc(value: &Value) -> Value {
    match value {
        Value::String(text) => Value::String(text.nfc().collect()),
        Value::Array(items) => Value::Array(items.iter().map(normalize_nfc).collect()),
        Value::Object(map) => {
            Value::Object(map.iter().map(|(key, item)| (key.nfc().collect(), normalize_nfc(item))).collect())
        }
        other => other.clone(),
    }
}

// ---------------------------------------------------------------- uid + glyph

pub fn is_kind(kind: &str) -> bool {
    KINDS.contains(&kind)
}

pub fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn base32_lower(bytes: &[u8]) -> String {
    let mut out = String::new();
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for byte in bytes {
        buffer = (buffer << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(BASE32[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(BASE32[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

fn base32_value(ch: u8) -> Option<u32> {
    BASE32.iter().position(|item| *item == ch).map(|index| index as u32)
}

/// Parse `ga:<kind>:<26 base32>`; returns (kind, body).
pub fn parse_uid(uid: &str) -> Result<(&str, &str), AssetError> {
    let Some(rest) = uid.strip_prefix("ga:") else {
        return err("bad_uid", format!("{uid:?} must start with ga:"));
    };
    let Some((kind, body)) = rest.split_once(':') else {
        return err("bad_uid", format!("{uid:?} must be ga:<kind>:<26 chars>"));
    };
    if kind.is_empty() || !kind.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_') {
        return err("bad_uid", format!("{uid:?} kind token must be snake_case"));
    }
    if body.len() != 26 {
        return err("bad_uid_length", format!("{uid:?} body must be 26 chars, got {}", body.len()));
    }
    if !body.bytes().all(|byte| base32_value(byte).is_some()) {
        return err("bad_uid_alphabet", format!("{uid:?} body must be lowercase RFC 4648 base32"));
    }
    let last = base32_value(body.as_bytes()[25]).unwrap_or(0);
    if last & 0b11 != 0 {
        return err("bad_uid_padding", format!("{uid:?} last char must have its 2 pad bits zero"));
    }
    if !is_kind(kind) {
        return err("unknown_kind", format!("{uid:?} kind {kind} is not in the v1 enum"));
    }
    Ok((kind, body))
}

pub fn glyph_from_bytes(first: u8, second: u8) -> String {
    [first, second]
        .iter()
        .map(|byte| char::from_u32(0x2800 + u32::from(*byte)).unwrap_or('\u{2800}'))
        .collect()
}

/// Recover digest bytes 0 and 1 from the uid text (its first 16 bits).
pub fn glyph_bytes_from_uid(uid: &str) -> Result<[u8; 2], AssetError> {
    let (_, body) = parse_uid(uid)?;
    let mut acc: u32 = 0;
    for byte in body.bytes().take(4) {
        acc = (acc << 5) | base32_value(byte).unwrap_or(0);
    }
    // 4 chars = 20 bits; the top 16 are digest[0..2].
    let top = acc >> 4;
    Ok([(top >> 8) as u8, (top & 0xff) as u8])
}

pub fn glyph_from_uid(uid: &str) -> Result<String, AssetError> {
    let [first, second] = glyph_bytes_from_uid(uid)?;
    Ok(glyph_from_bytes(first, second))
}

/// Lit braille dots (1..=8) for one glyph byte: bit k lights dot k+1.
pub fn glyph_dots(byte: u8) -> Vec<u8> {
    (0..8u8).filter(|bit| byte & (1 << bit) != 0).map(|bit| bit + 1).collect()
}

pub fn hue_class(kind: &str) -> String {
    if is_kind(kind) {
        format!("ga-kind-{kind}")
    } else {
        "ga-kind-unknown".into()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Minted {
    pub identity: Value,
    pub canonical: String,
    pub preimage_hex: String,
    pub digest_hex: String,
    pub uid: String,
    pub glyph: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct MediaDigest {
    pub role: String,
    pub sha256: String,
}

pub fn identity_value(kind: &str, fields: &Value, media: &[MediaDigest], src: &[String]) -> Result<Value, AssetError> {
    if !is_kind(kind) {
        return err("unknown_kind", format!("kind {kind:?} is not in the v1 enum"));
    }
    if !fields.is_object() {
        return err("bad_fields", "fields must be an object");
    }
    let mut media_sorted = media.to_vec();
    for item in &media_sorted {
        if !is_sha256_hex(&item.sha256) {
            return err("bad_sha256", format!("media {} sha256 must be 64 lowercase hex", item.role));
        }
        if item.role.is_empty() || !item.role.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_') {
            return err("bad_media_role", format!("media role {:?} must be snake_case", item.role));
        }
    }
    let mut roles = BTreeSet::new();
    for item in &media_sorted {
        if !roles.insert(item.role.as_str()) {
            return err("duplicate_media_role", format!("{kind} carries media role {:?} twice", item.role));
        }
    }
    media_sorted.sort();
    if src.len() > MAX_SRC {
        return err("fan_out_exceeded", format!("src has {} parents (max {MAX_SRC})", src.len()));
    }
    let mut parents: Vec<String> = Vec::new();
    for uid in src {
        parse_uid(uid)?;
        if parents.contains(uid) {
            return err("duplicate_src", format!("src lists {uid} twice"));
        }
        parents.push(uid.clone());
    }
    parents.sort();
    Ok(json!({
        "kind": kind,
        "schema_major": SCHEMA_MAJOR,
        "fields": fields,
        "media": media_sorted.iter().map(|item| json!({"role": item.role, "sha256": item.sha256})).collect::<Vec<_>>(),
        "src": parents,
    }))
}

pub fn mint(kind: &str, fields: &Value, media: &[MediaDigest], src: &[String]) -> Result<Minted, AssetError> {
    let identity = identity_value(kind, fields, media, src)?;
    mint_identity(&identity)
}

pub fn mint_identity(identity: &Value) -> Result<Minted, AssetError> {
    let kind = identity.get("kind").and_then(Value::as_str).unwrap_or_default().to_string();
    if !is_kind(&kind) {
        return err("unknown_kind", format!("kind {kind:?} is not in the v1 enum"));
    }
    let canonical = canonicalize(identity)?;
    let mut preimage = Vec::with_capacity(canonical.len() + 12);
    preimage.extend_from_slice(DOMAIN_TAG.as_bytes());
    preimage.push(0);
    preimage.extend_from_slice(canonical.as_bytes());
    let digest = Sha256::digest(&preimage);
    let uid = format!("ga:{kind}:{}", base32_lower(&digest[..16]));
    Ok(Minted {
        identity: identity.clone(),
        canonical,
        preimage_hex: hex(&preimage),
        digest_hex: hex(&digest),
        glyph: glyph_from_bytes(digest[0], digest[1]),
        uid,
    })
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

// ---------------------------------------------------------------- per-kind identity fields

#[derive(Clone, Copy, PartialEq, Eq)]
enum FieldType {
    Str,
    Int,
    NonNegInt,
    /// Integer in 1..=32 (audio channels).
    Channels,
    Bool,
    Uid(&'static str),
    StrList,
    Object,
    Sha,
}

struct FieldSpec {
    name: &'static str,
    ty: FieldType,
    required: bool,
}

const fn req(name: &'static str, ty: FieldType) -> FieldSpec {
    FieldSpec { name, ty, required: true }
}

const fn opt(name: &'static str, ty: FieldType) -> FieldSpec {
    FieldSpec { name, ty, required: false }
}

use FieldType::*;

/// The fixed identity allowlist per kind. Titles, status, notes, timestamps
/// and paths are never identity.
fn field_specs(kind: &str) -> &'static [FieldSpec] {
    match kind {
        "voice_model" => {
            const F: &[FieldSpec] = &[req("model_id", Str), req("waveform", Bool)];
            F
        },
        "voice_profile" => {
            const F: &[FieldSpec] = &[
            req("persona_id", Str),
            req("voice_model", Uid("voice_model")),
            req("tone", Str),
            req("purpose", Str),
            req("domain", Str),
            req("accent", Str),
            req("traits", Str),
            req("refs", StrList),
        ];
            F
        },
        "audio_clip" => {
            const F: &[FieldSpec] = &[
            req("engine", Str),
            req("sample_rate_hz", NonNegInt),
            req("duration_ms", NonNegInt),
            opt("channels", Channels),
        ];
            F
        },
        "spectrogram_2d" => {
            const F: &[FieldSpec] = &[
            req("source_sha256", Sha),
            req("sample_rate_hz", NonNegInt),
            req("n_fft", NonNegInt),
            req("hop_frames", NonNegInt),
            req("n_bands", NonNegInt),
            req("width_px", NonNegInt),
            req("height_px", NonNegInt),
            req("duration_ms", NonNegInt),
            req("covers_ms", NonNegInt),
            req("db_floor", Int),
            req("colormap", Str),
        ];
            F
        },
        "cube_ihdr" => {
            const F: &[FieldSpec] = &[
            req("source_sha256", Sha),
            req("sample_rate_hz", NonNegInt),
            req("bin_frames", NonNegInt),
            req("time_bins", NonNegInt),
            req("freq_bins", NonNegInt),
            req("duration_ms", NonNegInt),
            req("covers_ms", NonNegInt),
            req("inv_hdr_ppm", NonNegInt),
            req("cube_revision", NonNegInt),
            req("n_points", NonNegInt),
            opt("n_fft", NonNegInt),
            opt("hop_frames", NonNegInt),
            // Cube identity (item 2): the generator's CRLF-normalized source
            // sha256 and its layer method, never a commit SHA.
            opt("generator_sha256", Sha),
            opt("layer_method", Str),
        ];
            F
        },
        "layer" => {
            const F: &[FieldSpec] = &[req("name", Str), req("index", NonNegInt)];
            F
        },
        "podcast_script" => {
            const F: &[FieldSpec] = &[req("format", Str), req("n_turns", NonNegInt), req("n_words", NonNegInt)];
            F
        },
        "transcript" => {
            const F: &[FieldSpec] = &[req("language", Str), req("n_words", NonNegInt)];
            F
        },
        "card" => {
            const F: &[FieldSpec] = &[req("card_id", Str), req("view", Str)];
            F
        },
        "mcp_tool" => {
            const F: &[FieldSpec] = &[req("name", Str), req("input_schema", Object)];
            F
        },
        _ => &[],
    }
}

fn media_roles(kind: &str) -> &'static [&'static str] {
    match kind {
        "audio_clip" => &["wav"],
        "spectrogram_2d" => &["spectrogram_png"],
        "cube_ihdr" => &["cube_json", "cube_png"],
        "podcast_script" => &["script_txt"],
        "transcript" => &["transcript_json", "transcript_txt"],
        "voice_profile" => &["profile_json"],
        _ => &[],
    }
}

/// Media roles every envelope of `kind` must carry (exactly once).
/// A real cube_ihdr also needs cube_png; see `check_required_media` (B2 ruling).
fn required_media_roles(kind: &str) -> &'static [&'static str] {
    match kind {
        "audio_clip" => &["wav"],
        "spectrogram_2d" => &["spectrogram_png"],
        "cube_ihdr" => &["cube_json"],
        "podcast_script" => &["script_txt"],
        _ => &[],
    }
}

fn check_fields(kind: &str, fields: &Value) -> Result<(), AssetError> {
    let map = fields.as_object().ok_or(AssetError { code: "bad_fields", detail: "fields must be an object".into() })?;
    let specs = field_specs(kind);
    for key in map.keys() {
        if !specs.iter().any(|spec| spec.name == key) {
            return err("unknown_field", format!("{kind}.fields.{key} is not an identity field"));
        }
    }
    for spec in specs {
        let Some(value) = map.get(spec.name) else {
            if spec.required {
                return err("missing_field", format!("{kind}.fields.{} is required", spec.name));
            }
            continue;
        };
        // A non-integer number token (139375.0, 1.39375e5, 1.5) is the same
        // error the minter raises, whatever the field's declared type.
        if value.is_f64() {
            return err("float_in_identity", format!("{kind}.fields.{} is not an integer token; keep float views outside identity", spec.name));
        }
        let ok = match spec.ty {
            Str => value.as_str().is_some_and(|text| !text.is_empty()),
            Int => value.as_i64().is_some(),
            NonNegInt => value.as_u64().is_some(),
            Channels => value.as_u64().is_some_and(|count| (1..=32).contains(&count)),
            Bool => value.is_boolean(),
            Sha => value.as_str().is_some_and(is_sha256_hex),
            Object => value.is_object(),
            StrList => value.as_array().is_some_and(|items| items.iter().all(Value::is_string)),
            Uid(want) => match value.as_str() {
                Some(text) => match parse_uid(text) {
                    Ok((got, _)) if got == want => true,
                    _ => {
                        let code = if kind == "voice_profile" && spec.name == "voice_model" { "bad_voice_ref" } else { "bad_field_type" };
                        return err(code, format!("{kind}.fields.{} must be a ga:{want}: uid, got {text:?}", spec.name));
                    }
                },
                None => false,
            },
        };
        if !ok {
            return err("bad_field_type", format!("{kind}.fields.{} has the wrong type", spec.name));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- media paths

/// Media paths are relative to the library root: `name.ext` or `dir/name.ext`.
pub fn check_media_path(path: &str) -> Result<(), AssetError> {
    if path.is_empty() {
        return err("media_path_empty", "media path is empty");
    }
    if path.starts_with("\\\\") || path.starts_with("//") {
        return err("media_path_unc", format!("{path:?} is a UNC path"));
    }
    if path.starts_with('/') || path.starts_with('\\') {
        return err("media_path_absolute", format!("{path:?} is absolute"));
    }
    let bytes = path.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return err("media_path_drive", format!("{path:?} has a drive letter"));
    }
    if path.contains(':') {
        return err("media_path_scheme", format!("{path:?} has a URL scheme"));
    }
    if path.contains('\\') {
        return err("media_path_backslash", format!("{path:?} uses backslashes"));
    }
    for segment in path.split('/') {
        if segment == ".." || segment == "." || segment.contains("..") {
            return err("media_path_dotdot", format!("{path:?} walks out of the library root"));
        }
        let mut chars = segment.bytes();
        let first_ok = chars.next().is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
        let rest_ok = segment.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'));
        if !first_ok || !rest_ok || segment.len() > 128 {
            return err("media_path_chars", format!("{path:?} segment {segment:?} is not [A-Za-z0-9_-][A-Za-z0-9_.-]*"));
        }
    }
    if path.split('/').count() > 4 {
        return err("media_path_chars", format!("{path:?} is nested too deep"));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaCheck {
    Verified,
    Missing,
}

/// Verify one media ref against the library root: size cap, byte length and sha256.
pub fn verify_media(root: &Path, media: &Value) -> Result<MediaCheck, AssetError> {
    let path = media.get("path").and_then(Value::as_str).unwrap_or_default();
    check_media_path(path)?;
    let want_sha = media.get("sha256").and_then(Value::as_str).unwrap_or_default();
    let want_bytes = media.get("bytes").and_then(Value::as_u64).unwrap_or(0);
    let full = root.join(path);
    let Ok(meta) = std::fs::metadata(&full) else {
        return Ok(MediaCheck::Missing);
    };
    if meta.len() > MAX_MEDIA_BYTES {
        return err("media_too_large", format!("{path} is {} bytes (cap {MAX_MEDIA_BYTES})", meta.len()));
    }
    if meta.len() != want_bytes {
        return err("media_bytes_mismatch", format!("{path} is {} bytes, envelope says {want_bytes}", meta.len()));
    }
    let bytes = std::fs::read(&full).map_err(|error| AssetError { code: "media_unreadable", detail: format!("{path}: {error}") })?;
    let got = sha256_hex(&bytes);
    if got != want_sha {
        return err("media_sha_mismatch", format!("{path} sha256 {got} != {want_sha}"));
    }
    Ok(MediaCheck::Verified)
}

// ---------------------------------------------------------------- envelopes

pub fn parse_schema_major(version: &str) -> Result<u64, AssetError> {
    let core = version.split(['-', '+']).next().unwrap_or_default();
    let parts: Vec<&str> = core.split('.').collect();
    if parts.len() != 3 || parts.iter().any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit())) {
        return err("bad_schema_version", format!("schema_version {version:?} must be semver"));
    }
    parts[0].parse::<u64>().map_err(|_| AssetError { code: "bad_schema_version", detail: version.into() })
}

fn str_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut node = value;
    for key in path {
        node = node.get(*key)?;
    }
    node.as_str()
}

fn bool_at(value: &Value, path: &[&str]) -> Option<bool> {
    let mut node = value;
    for key in path {
        node = node.get(*key)?;
    }
    node.as_bool()
}

fn media_digests(envelope: &Value) -> Result<Vec<MediaDigest>, AssetError> {
    let kind = envelope.get("kind").and_then(Value::as_str).unwrap_or_default();
    let mut out = Vec::new();
    for item in envelope.get("media").and_then(Value::as_array).cloned().unwrap_or_default() {
        let role = item.get("role").and_then(Value::as_str).unwrap_or_default().to_string();
        if !media_roles(kind).contains(&role.as_str()) {
            return err("bad_media_role", format!("{kind} cannot carry media role {role:?}"));
        }
        let path = item.get("path").and_then(Value::as_str).unwrap_or_default();
        check_media_path(path)?;
        let sha256 = item.get("sha256").and_then(Value::as_str).unwrap_or_default().to_string();
        if !is_sha256_hex(&sha256) {
            return err("bad_sha256", format!("media {role} sha256 must be 64 lowercase hex"));
        }
        match item.get("bytes").and_then(Value::as_u64) {
            Some(bytes) if bytes <= MAX_MEDIA_BYTES => {}
            Some(_) => return err("media_too_large", format!("media {role} exceeds {MAX_MEDIA_BYTES} bytes")),
            None => return err("bad_media", format!("media {role} needs integer bytes")),
        }
        out.push(MediaDigest { role, sha256 });
    }
    Ok(out)
}

fn uid_list(value: Option<&Value>, field: &str) -> Result<Vec<String>, AssetError> {
    let mut out = Vec::new();
    for item in value.and_then(Value::as_array).cloned().unwrap_or_default() {
        let Some(text) = item.as_str() else {
            return err("bad_uid", format!("{field} entries must be uid strings"));
        };
        parse_uid(text)?;
        out.push(text.to_string());
    }
    Ok(out)
}

/// Recompute the identity projection of an envelope.
pub fn envelope_identity(envelope: &Value) -> Result<Value, AssetError> {
    let kind = envelope.get("kind").and_then(Value::as_str).unwrap_or_default();
    let fields = envelope.get("fields").cloned().unwrap_or(Value::Null);
    let media = media_digests(envelope)?;
    let src = uid_list(envelope.get("src"), "src")?;
    identity_value(kind, &fields, &media, &src)
}

/// Single-envelope checks: shape, identity, uid/glyph, media paths, honesty.
pub fn validate_envelope(envelope: &Value) -> Result<Minted, AssetError> {
    let Some(root) = envelope.as_object() else {
        return err("bad_envelope", "envelope must be an object");
    };
    if let Some(key) = root.keys().find(|key| !ROOT_KEYS.contains(&key.as_str())) {
        return err("unknown_root_key", format!("envelope key {key:?} is not in the v1 schema"));
    }
    let version = root.get("schema_version").and_then(Value::as_str).unwrap_or_default();
    let major = parse_schema_major(version)?;
    if major != SCHEMA_MAJOR {
        return err("unknown_major", format!("schema_version {version} has major {major}; only 1 is known"));
    }
    if root.get("uid_scheme").and_then(Value::as_str) != Some(UID_SCHEME) {
        return err("bad_uid_scheme", "uid_scheme must be \"ga1\"");
    }
    let kind = root.get("kind").and_then(Value::as_str).unwrap_or_default();
    if !is_kind(kind) {
        return err("unknown_kind", format!("kind {kind:?} is not in the v1 enum"));
    }
    let uid = root.get("uid").and_then(Value::as_str).unwrap_or_default();
    let (uid_kind, _) = parse_uid(uid)?;
    if uid_kind != kind {
        return err("kind_mismatch", format!("uid prefix {uid_kind} != kind {kind}"));
    }
    if let Some(id) = root.get("legacy_id") {
        let ok = id.as_str().is_some_and(|text| {
            (1..=96).contains(&text.len())
                && !text.contains("..")
                && text.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
        });
        if !ok {
            return err("bad_legacy_id", format!("legacy_id {id} must match [A-Za-z0-9_.-]{{1,96}} without '..'"));
        }
    }
    check_fields(kind, root.get("fields").unwrap_or(&Value::Null))?;
    let minted = mint_identity(&envelope_identity(envelope)?)?;
    if minted.uid != uid {
        return err("uid_mismatch", format!("uid {uid} != recomputed {}", minted.uid));
    }
    check_display(root.get("display"), &minted.glyph)?;
    if kind == "audio_clip" {
        if let Some(url) = root.get("body").and_then(|body| body.get("wav_url")) {
            let rel = url.as_str().and_then(|text| text.strip_prefix("/library/"));
            match rel {
                Some(rel) => check_media_path(rel).map_err(|error| AssetError { code: "bad_wav_url", detail: error.detail })?,
                None => return err("bad_wav_url", format!("wav_url {url} must be /library/<media path>")),
            }
        }
    }
    if let Some(extensions) = root.get("extensions").and_then(Value::as_object) {
        for key in extensions.keys() {
            let ok = key.starts_with("x-") && key.len() > 2 && key[2..].bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
            if !ok {
                return err("bad_extension_key", format!("extension key {key:?} must match x-[a-z0-9-]+"));
            }
        }
    }
    for field in ["composes", "bound_to", "supersedes"] {
        let targets = uid_list(envelope.get("relations").and_then(|item| item.get(field)), field)?;
        if targets.len() > MAX_FAN_OUT {
            return err("fan_out_exceeded", format!("relations.{field} has {} targets (max {MAX_FAN_OUT})", targets.len()));
        }
    }
    check_honesty(kind, envelope)?;
    check_required_media(kind, envelope)?;
    check_speech_facts(kind, envelope)?;
    Ok(minted)
}

/// Speaker facts live in the unhashed body. Absent is valid (older envelopes).
/// `"unresolved"` means the clip has no per-line script offsets and nothing was guessed.
fn check_speech_facts(kind: &str, envelope: &Value) -> Result<(), AssetError> {
    if kind != "audio_clip" && kind != "cube_ihdr" {
        return Ok(());
    }
    let Some(body) = envelope.get("body").and_then(Value::as_object) else {
        return Ok(());
    };
    let Some(speakers) = body.get("speakers") else {
        if body.get("segments").is_some() || body.get("speaker_idx").is_some() {
            return err("bad_speakers", "segments and speaker_idx require speakers");
        }
        return Ok(());
    };
    match speakers {
        Value::String(text) if text == "unresolved" => {
            let segments = body.get("segments").and_then(Value::as_array);
            let bins = body.get("speaker_idx").and_then(Value::as_array);
            if segments.is_some_and(|items| !items.is_empty())
                || bins.is_some_and(|items| !items.is_empty())
            {
                return err(
                    "speakers_unresolved",
                    "unresolved speakers cannot carry segments or speaker_idx",
                );
            }
            Ok(())
        }
        Value::Array(items) => {
            if items.is_empty() {
                return err(
                    "bad_speakers",
                    "speakers must be \"unresolved\" or a non-empty roster",
                );
            }
            if items.len() > 8 {
                return err(
                    "too_many_speakers",
                    format!("speakers has {} entries (max 8)", items.len()),
                );
            }
            let mut roster = BTreeSet::new();
            for (index, item) in items.iter().enumerate() {
                let Some(obj) = item.as_object() else {
                    return err(
                        "bad_speakers",
                        format!("speakers[{index}] must be an object"),
                    );
                };
                if obj
                    .keys()
                    .any(|key| !matches!(key.as_str(), "idx" | "persona" | "voice" | "engine"))
                {
                    return err(
                        "bad_speakers",
                        format!("speakers[{index}] has an unknown field"),
                    );
                }
                let Some(idx) = obj.get("idx").and_then(Value::as_u64) else {
                    return err(
                        "bad_speakers",
                        format!("speakers[{index}].idx must be an integer 0..7"),
                    );
                };
                if idx > 7 {
                    return err(
                        "bad_speakers",
                        format!("speakers[{index}].idx {idx} is outside 0..7"),
                    );
                }
                if !roster.insert(idx) {
                    return err("bad_speakers", format!("speaker idx {idx} is duplicated"));
                }
                for key in ["persona", "voice", "engine"] {
                    if !obj
                        .get(key)
                        .and_then(Value::as_str)
                        .is_some_and(|text| !text.trim().is_empty())
                    {
                        return err(
                            "bad_speakers",
                            format!("speakers[{index}].{key} must be a non-empty string"),
                        );
                    }
                }
            }
            let Some(segments) = body.get("segments").and_then(Value::as_array) else {
                return err(
                    "bad_segment",
                    "a resolved speaker roster needs a segments array",
                );
            };
            let duration = speech_duration_s(envelope).ok_or(AssetError {
                code: "segment_exceeds_duration",
                detail: "segments need body.duration_s or fields.duration_ms".into(),
            })?;
            let mut prev_start = f64::NEG_INFINITY;
            let mut prev_end = f64::NEG_INFINITY;
            for (index, item) in segments.iter().enumerate() {
                let Some(obj) = item.as_object() else {
                    return err(
                        "bad_segment",
                        format!("segments[{index}] must be an object"),
                    );
                };
                if obj
                    .keys()
                    .any(|key| !matches!(key.as_str(), "speaker_idx" | "start_s" | "end_s"))
                {
                    return err(
                        "bad_segment",
                        format!("segments[{index}] has an unknown field"),
                    );
                }
                let Some(speaker_idx) = obj.get("speaker_idx").and_then(Value::as_u64) else {
                    return err(
                        "bad_segment",
                        format!("segments[{index}].speaker_idx must be an integer 0..7"),
                    );
                };
                if !roster.contains(&speaker_idx) {
                    return err(
                        "bad_segment",
                        format!("segments[{index}].speaker_idx {speaker_idx} is not in the roster"),
                    );
                }
                let (Some(start), Some(end)) =
                    (finite_f64(obj.get("start_s")), finite_f64(obj.get("end_s")))
                else {
                    return err(
                        "bad_segment",
                        format!("segments[{index}] start_s and end_s must be finite numbers"),
                    );
                };
                if start < 0.0 || !(end > start) {
                    return err(
                        "bad_segment",
                        format!("segments[{index}] must have 0 <= start_s < end_s"),
                    );
                }
                if start + 1e-9 < prev_start {
                    return err(
                        "segments_unsorted",
                        format!("segments[{index}] starts before the previous segment"),
                    );
                }
                if start + 1e-9 < prev_end {
                    return err(
                        "segments_overlap",
                        format!("segments[{index}] overlaps the previous segment"),
                    );
                }
                if end > duration + 1e-6 {
                    return err(
                        "segment_exceeds_duration",
                        format!("segments[{index}].end_s {end} exceeds clip duration {duration}"),
                    );
                }
                prev_start = start;
                prev_end = end;
            }
            if kind == "cube_ihdr" {
                let Some(bins) = body.get("speaker_idx").and_then(Value::as_array) else {
                    return err(
                        "bad_speaker_idx",
                        "a resolved cube needs speaker_idx aligned to time_bins",
                    );
                };
                let time_bins = envelope
                    .pointer("/fields/time_bins")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as usize;
                if bins.len() != time_bins {
                    return err(
                        "bad_speaker_idx",
                        format!(
                            "speaker_idx has {} bins; time_bins is {time_bins}",
                            bins.len()
                        ),
                    );
                }
                for (index, bin) in bins.iter().enumerate() {
                    let Some(idx) = bin.as_u64() else {
                        return err(
                            "bad_speaker_idx",
                            format!("speaker_idx[{index}] must be an integer"),
                        );
                    };
                    if idx == 255 {
                        continue;
                    }
                    if !roster.contains(&idx) {
                        return err(
                            "bad_speaker_idx",
                            format!("speaker_idx[{index}] {idx} is not a roster speaker or 255"),
                        );
                    }
                }
            }
            Ok(())
        }
        _ => err(
            "bad_speakers",
            "speakers must be \"unresolved\" or an array of at most 8",
        ),
    }
}

fn speech_duration_s(envelope: &Value) -> Option<f64> {
    if let Some(duration) = finite_f64(envelope.pointer("/body/duration_s")) {
        if duration >= 0.0 {
            return Some(duration);
        }
    }
    envelope
        .pointer("/fields/duration_ms")
        .and_then(Value::as_u64)
        .map(|ms| ms as f64 / 1000.0)
}

fn finite_f64(value: Option<&Value>) -> Option<f64> {
    value
        .and_then(Value::as_f64)
        .filter(|number| number.is_finite())
}

fn check_display(display: Option<&Value>, glyph: &str) -> Result<(), AssetError> {
    let Some(display) = display.and_then(Value::as_object) else {
        return err("missing_glyph", "display with title and glyph is required");
    };
    if let Some(key) = display.keys().find(|key| !DISPLAY_KEYS.contains(&key.as_str())) {
        return err("unknown_display_key", format!("display.{key} is not in the v1 schema"));
    }
    match display.get("title").and_then(Value::as_str) {
        Some(title) if !title.is_empty() && title.chars().count() <= 160 => {}
        _ => return err("bad_title", "display.title must be a 1..160 char string"),
    }
    match display.get("glyph").and_then(Value::as_str) {
        None => return err("missing_glyph", "display.glyph is required"),
        Some(got) if got != glyph => return err("glyph_mismatch", format!("glyph {got} != {glyph}")),
        Some(_) => {}
    }
    if let Some(rev) = display.get("display_rev") {
        if !rev.as_u64().is_some_and(|value| value <= MAX_SAFE_INTEGER as u64) {
            return err("bad_display_rev", "display.display_rev must be a non-negative integer");
        }
    }
    Ok(())
}

fn check_required_media(kind: &str, envelope: &Value) -> Result<(), AssetError> {
    let media = envelope.get("media").and_then(Value::as_array).cloned().unwrap_or_default();
    for role in required_media_roles(kind) {
        if !media.iter().any(|item| item.get("role").and_then(Value::as_str) == Some(role)) {
            let code = if kind == "audio_clip" { "clip_missing_wav" } else { "missing_media_role" };
            return err(code, format!("{kind} needs a {role} media entry"));
        }
    }
    if kind == "transcript" && media.is_empty() {
        return err("missing_media_role", "transcript needs transcript_json or transcript_txt");
    }
    // B2 ruling: a real cube (analysis of a real clip: claims library_cube,
    // not a fixture) ships both its JSON and its PNG. A pending cube (no
    // library_cube claim yet) may carry the JSON alone.
    if kind == "cube_ihdr" && cube_is_real(envelope) && !media.iter().any(|item| item.get("role").and_then(Value::as_str) == Some("cube_png")) {
        return err("missing_media_role", "a real cube_ihdr (claims library_cube) needs a cube_png media entry");
    }
    Ok(())
}

/// Claims that mark dev/test-only assets: fixture tones, reference numbers,
/// and sample content (a sample cast script, unprobed placeholder endpoints).
pub const DEV_FIXTURE_CLAIMS: [&str; 3] = ["fixture_tone", "reference_only", "sample_content"];

/// True for an asset that ships only in dev/test builds (the cards
/// `spec-fixture`, `cube-fixture`, `bench-ref`, `cast-sample`, `serve-node`
/// and `serve-gateway` today): `honesty.fixture`
/// is true or a claim is in `DEV_FIXTURE_CLAIMS`. Release `assets.json` and
/// `viewport.release.json` leave these out (VITE_GEN_AUDIO_FIXTURES=1 brings
/// the example deck back in dev).
pub fn is_dev_fixture(envelope: &Value) -> bool {
    let claims_dev = envelope
        .pointer("/honesty/claims")
        .and_then(Value::as_array)
        .is_some_and(|claims| claims.iter().any(|claim| claim.as_str().is_some_and(|claim| DEV_FIXTURE_CLAIMS.contains(&claim))));
    claims_dev || bool_at(envelope, &["honesty", "fixture"]) == Some(true)
}

/// A cube_ihdr is "real" when it claims `library_cube` and is not a fixture.
pub fn cube_is_real(envelope: &Value) -> bool {
    let claims_cube = envelope
        .pointer("/honesty/claims")
        .and_then(Value::as_array)
        .is_some_and(|claims| claims.iter().any(|claim| claim.as_str() == Some("library_cube")));
    claims_cube && bool_at(envelope, &["honesty", "fixture"]) != Some(true)
}

fn check_honesty(kind: &str, envelope: &Value) -> Result<(), AssetError> {
    let synthesized = bool_at(envelope, &["honesty", "synthesized_speech"]).ok_or(AssetError {
        code: "bad_honesty",
        detail: "honesty.synthesized_speech must be a boolean".into(),
    })?;
    let fixture = bool_at(envelope, &["honesty", "fixture"]).ok_or(AssetError {
        code: "bad_honesty",
        detail: "honesty.fixture must be a boolean".into(),
    })?;
    for claim in envelope.pointer("/honesty/claims").and_then(Value::as_array).cloned().unwrap_or_default() {
        let text = claim.as_str().unwrap_or_default();
        if !CLAIMS.contains(&text) {
            return err("unknown_claim", format!("claim {text:?} is not in the closed vocabulary"));
        }
    }
    if fixture && synthesized {
        return err("fixture_claims_speech", "fixture:true requires synthesized_speech:false");
    }
    if let Some(note) = envelope.pointer("/honesty/note") {
        if !note.as_str().is_some_and(|text| text.chars().count() <= MAX_HONESTY_NOTE_CHARS) {
            return err("bad_honesty", format!("honesty.note must be a string of at most {MAX_HONESTY_NOTE_CHARS} chars"));
        }
    }
    let status = envelope.get("status").and_then(Value::as_str).unwrap_or("ok");
    match kind {
        "voice_profile" => {
            if synthesized {
                return err("profile_claims_speech", "a voice_profile never claims synthesized speech");
            }
            if bool_at(envelope, &["honesty", "not_podcast"]) != Some(true) {
                return err("profile_not_marked_not_podcast", "a voice_profile needs honesty.not_podcast:true");
            }
        }
        "audio_clip" => {
            if !matches!(status, "ok" | "missing") {
                return err("bad_status", format!("audio_clip status {status:?} must be ok or missing"));
            }
            if synthesized {
                let has_wav = envelope
                    .get("media")
                    .and_then(Value::as_array)
                    .is_some_and(|items| items.iter().any(|item| item.get("role").and_then(Value::as_str) == Some("wav")));
                if !has_wav {
                    return err("clip_missing_wav", "a synthesized audio_clip needs a wav media sha256");
                }
                if str_at(envelope, &["provenance", "engine"]).is_none_or(str::is_empty) {
                    return err("clip_missing_engine", "a synthesized audio_clip needs provenance.engine");
                }
                match str_at(envelope, &["provenance", "voice_model"]).map(parse_uid) {
                    Some(Ok(("voice_model", _))) => {}
                    _ => return err("clip_missing_voice_model", "a synthesized audio_clip needs a provenance.voice_model uid"),
                }
            }
        }
        "cube_ihdr" | "spectrogram_2d" => {
            let src = uid_list(envelope.get("src"), "src")?;
            if src.len() != 1 || parse_uid(&src[0])?.0 != "audio_clip" {
                return err("derived_needs_one_clip", format!("{kind} must derive from exactly one audio_clip"));
            }
            let duration = envelope.pointer("/fields/duration_ms").and_then(Value::as_u64).unwrap_or(0);
            let covers = envelope.pointer("/fields/covers_ms").and_then(Value::as_u64).unwrap_or(0);
            if covers > duration {
                return err("covers_exceeds_duration", format!("{kind} covers_ms {covers} > duration_ms {duration}"));
            }
        }
        "layer" => {
            let name = envelope.pointer("/fields/name").and_then(Value::as_str).unwrap_or_default();
            if !LAYER_NAMES.contains(&name) {
                return err("bad_layer_name", format!("layer name {name:?} must be signal/tonality/confidence/quality"));
            }
            let layer_of = str_at(envelope, &["relations", "layer_of"]).unwrap_or_default();
            match parse_uid(layer_of) {
                Ok(("cube_ihdr", _)) => {}
                _ => return err("layer_needs_cube", "a layer needs relations.layer_of = one cube_ihdr uid"),
            }
            let src = uid_list(envelope.get("src"), "src")?;
            if src != [layer_of.to_string()] {
                return err("layer_src_mismatch", "a layer's src must be exactly its layer_of cube");
            }
        }
        "mcp_tool" => check_tool_secrets(envelope)?,
        _ => {}
    }
    Ok(())
}

const SECRET_KEYS: &[&str] = &["env", "secret", "token", "password", "api_key", "apikey", "authorization", "credential"];

fn check_tool_secrets(envelope: &Value) -> Result<(), AssetError> {
    fn walk(value: &Value, path: &str) -> Result<(), AssetError> {
        match value {
            Value::String(text) => {
                if redact_secrets(text) != *text {
                    return err("mcp_tool_secret", format!("{path} holds a credential-shaped value"));
                }
                let mut parts = text.splitn(2, '=');
                let name = parts.next().unwrap_or_default();
                if parts.next().is_some_and(|rest| !rest.is_empty())
                    && name.len() > 1
                    && name.bytes().all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
                {
                    return err("mcp_tool_env", format!("{path} holds an environment assignment"));
                }
                Ok(())
            }
            Value::Array(items) => items.iter().enumerate().try_for_each(|(index, item)| walk(item, &format!("{path}[{index}]"))),
            Value::Object(map) => {
                for (key, item) in map {
                    let lower = key.to_ascii_lowercase();
                    if SECRET_KEYS.contains(&lower.as_str()) && item.as_str().is_some_and(|text| !text.is_empty()) {
                        return err("mcp_tool_secret", format!("{path}.{key} carries a secret or env value"));
                    }
                    walk(item, &format!("{path}.{key}"))?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
    for part in ["fields", "body", "display", "provenance"] {
        if let Some(value) = envelope.get(part) {
            walk(value, part)?;
        }
    }
    Ok(())
}

/// Whole-set checks: each envelope, unique uids, cross-asset invariants and
/// link cycles.
pub fn validate_set(envelopes: &[Value]) -> Result<BTreeMap<String, Value>, AssetError> {
    let mut by_uid: BTreeMap<String, Value> = BTreeMap::new();
    let mut legacy: BTreeSet<(String, String)> = BTreeSet::new();
    for envelope in envelopes {
        let minted = validate_envelope(envelope)?;
        if by_uid.insert(minted.uid.clone(), envelope.clone()).is_some() {
            return err("duplicate_uid", format!("{} appears twice", minted.uid));
        }
        if let Some(id) = envelope.get("legacy_id").and_then(Value::as_str) {
            let kind = envelope["kind"].as_str().unwrap_or_default().to_string();
            if !legacy.insert((kind.clone(), id.to_string())) {
                return err("duplicate_legacy_id", format!("{kind} legacy_id {id} appears twice"));
            }
        }
    }
    for (uid, envelope) in &by_uid {
        let kind = envelope["kind"].as_str().unwrap_or_default();
        match kind {
            "cube_ihdr" | "spectrogram_2d" => {
                let parent_uid = envelope["src"][0].as_str().unwrap_or_default();
                let Some(clip) = by_uid.get(parent_uid) else {
                    return err("derived_source_missing", format!("{uid} derives from {parent_uid}, which is not in the set"));
                };
                let clip_sha = clip
                    .get("media")
                    .and_then(Value::as_array)
                    .and_then(|items| items.iter().find(|item| item["role"] == "wav"))
                    .and_then(|item| item["sha256"].as_str())
                    .unwrap_or_default();
                if envelope.pointer("/fields/source_sha256").and_then(Value::as_str) != Some(clip_sha) {
                    return err("source_sha_mismatch", format!("{uid} source_sha256 does not match {parent_uid} wav"));
                }
                let sr = envelope.pointer("/fields/sample_rate_hz").and_then(Value::as_u64).unwrap_or(0).max(1);
                let frames_key = if kind == "cube_ihdr" { "/fields/bin_frames" } else { "/fields/hop_frames" };
                let frames = envelope.pointer(frames_key).and_then(Value::as_u64).unwrap_or(0);
                let bin_ms = (frames * 1000).div_ceil(sr);
                let duration = envelope.pointer("/fields/duration_ms").and_then(Value::as_u64).unwrap_or(0);
                let clip_duration = clip.pointer("/fields/duration_ms").and_then(Value::as_u64).unwrap_or(0);
                if duration.abs_diff(clip_duration) > bin_ms {
                    return err(
                        "duration_drift",
                        format!("{uid} duration_ms {duration} vs clip {clip_duration} differs by more than one bin ({bin_ms} ms)"),
                    );
                }
            }
            "layer" => {
                let cube = envelope.pointer("/relations/layer_of").and_then(Value::as_str).unwrap_or_default();
                if !by_uid.contains_key(cube) {
                    return err("layer_cube_missing", format!("{uid} layer_of {cube} is not in the set"));
                }
            }
            "voice_profile" => {
                let voice = envelope.pointer("/fields/voice_model").and_then(Value::as_str).unwrap_or_default();
                if !by_uid.contains_key(voice) {
                    return err("voice_model_missing", format!("{uid} voice_model {voice} is not in the set"));
                }
            }
            "audio_clip" => {
                if let Some(voice) = envelope.pointer("/provenance/voice_model").and_then(Value::as_str) {
                    if !by_uid.contains_key(voice) {
                        return err("voice_model_missing", format!("{uid} provenance.voice_model {voice} is not in the set"));
                    }
                }
            }
            _ => {}
        }
    }
    for (uid, envelope) in &by_uid {
        let Some(relations) = envelope.get("relations").and_then(Value::as_object) else { continue };
        for (field, targets) in relations {
            let targets: Vec<&str> = match targets {
                Value::String(one) => vec![one.as_str()],
                Value::Array(many) => many.iter().filter_map(Value::as_str).collect(),
                _ => vec![],
            };
            if let Some(missing) = targets.iter().find(|target| !by_uid.contains_key(**target)) {
                return err("dangling_relation", format!("{uid} relations.{field} -> {missing} is not in the set"));
            }
        }
    }
    check_link_cycles(&by_uid)?;
    Ok(by_uid)
}

fn check_link_cycles(by_uid: &BTreeMap<String, Value>) -> Result<(), AssetError> {
    let mut edges: HashMap<&str, Vec<String>> = HashMap::new();
    for (uid, envelope) in by_uid {
        let mut out = Vec::new();
        for field in ["composes", "bound_to", "supersedes"] {
            out.extend(uid_list(envelope.get("relations").and_then(|item| item.get(field)), field)?);
        }
        edges.insert(uid.as_str(), out);
    }
    fn visit<'a>(
        node: &'a str,
        edges: &'a HashMap<&str, Vec<String>>,
        stack: &mut Vec<&'a str>,
        done: &mut BTreeSet<&'a str>,
    ) -> Result<(), AssetError> {
        if stack.contains(&node) {
            return err("link_cycle", format!("relations cycle through {node}"));
        }
        if done.contains(node) {
            return Ok(());
        }
        if stack.len() >= MAX_LINK_DEPTH {
            return err("link_depth", format!("relations deeper than {MAX_LINK_DEPTH} at {node}"));
        }
        stack.push(node);
        if let Some(next) = edges.get(node) {
            for target in next {
                if let Some((key, _)) = edges.get_key_value(target.as_str()) {
                    visit(key, edges, stack, done)?;
                }
            }
        }
        stack.pop();
        done.insert(node);
        Ok(())
    }
    let mut done = BTreeSet::new();
    for node in edges.keys() {
        visit(node, &edges, &mut Vec::new(), &mut done)?;
    }
    Ok(())
}

// ---------------------------------------------------------------- envelope builder

/// Inputs for one envelope; `build_envelope` mints uid + glyph.
pub struct EnvelopeParts {
    pub kind: &'static str,
    pub legacy_id: Option<String>,
    pub title: String,
    pub summary: Option<String>,
    pub status: &'static str,
    pub fields: Value,
    pub media: Vec<Value>,
    pub src: Vec<String>,
    pub relations: Map<String, Value>,
    pub honesty: Value,
    pub provenance: Map<String, Value>,
    pub body: Value,
}

pub fn build_envelope(parts: EnvelopeParts) -> Result<Value, AssetError> {
    let fields = normalize_nfc(&parts.fields);
    let mut media = parts.media.clone();
    media.sort_by(|a, b| {
        (a["role"].as_str(), a["sha256"].as_str()).cmp(&(b["role"].as_str(), b["sha256"].as_str()))
    });
    let digests: Vec<MediaDigest> = media
        .iter()
        .map(|item| MediaDigest {
            role: item["role"].as_str().unwrap_or_default().into(),
            sha256: item["sha256"].as_str().unwrap_or_default().into(),
        })
        .collect();
    let mut src = parts.src.clone();
    src.sort();
    src.dedup();
    let minted = mint(parts.kind, &fields, &digests, &src)?;
    let mut display = Map::new();
    display.insert("title".into(), Value::String(parts.title));
    if let Some(summary) = parts.summary {
        display.insert("summary".into(), Value::String(summary));
    }
    display.insert("glyph".into(), Value::String(minted.glyph.clone()));
    let mut root = Map::new();
    root.insert("schema_version".into(), json!(SCHEMA_VERSION));
    root.insert("uid_scheme".into(), json!(UID_SCHEME));
    root.insert("kind".into(), json!(parts.kind));
    root.insert("uid".into(), json!(minted.uid));
    if let Some(id) = parts.legacy_id {
        root.insert("legacy_id".into(), json!(id));
    }
    root.insert("status".into(), json!(parts.status));
    root.insert("fields".into(), fields);
    root.insert("media".into(), Value::Array(media));
    root.insert("src".into(), json!(src));
    root.insert("relations".into(), Value::Object(parts.relations));
    root.insert("honesty".into(), parts.honesty);
    root.insert("provenance".into(), Value::Object(parts.provenance));
    root.insert("display".into(), Value::Object(display));
    root.insert("body".into(), parts.body);
    Ok(Value::Object(root))
}

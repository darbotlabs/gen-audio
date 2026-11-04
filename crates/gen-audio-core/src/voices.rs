//! Voice descriptors, G2P configs, catalogs, and cast maps.
//!
//! JSON Schema under `schemas/voices/` is the document contract. These types
//! parse that JSON and check the cross-field rules (id prefix, pack gender,
//! blend weights, library-relative paths). Adding a shipped framework means a
//! row in `registry.v1.json`, a params schema, a Python adapter, and a match
//! arm in [`validate_params`].

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;
use serde_json::{Map, Value};

const REGISTRY_JSON: &str = include_str!("../../../schemas/voices/registry.v1.json");

#[derive(Debug, Clone, Deserialize)]
pub struct FrameworkRow {
    pub id: String,
    pub kind: String,
    pub engine_id: Option<String>,
    pub params_schema: String,
    pub wired: bool,
    pub summary: String,
}

#[derive(Debug, Clone, Deserialize)]
struct RegistryFile {
    schema_version: String,
    frameworks: Vec<FrameworkRow>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VoiceAsset {
    pub role: String,
    pub path: String,
    pub format: String,
    #[serde(default)]
    pub member: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VoiceSource {
    pub name: String,
    pub url: String,
    pub license: String,
    #[serde(default)]
    pub attribution: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VoiceDescriptor {
    pub schema_version: String,
    pub id: String,
    pub display_name: String,
    pub language: String,
    pub gender: String,
    pub framework: String,
    pub assets: Vec<VoiceAsset>,
    pub source: VoiceSource,
    pub tags: Vec<String>,
    #[serde(default)]
    pub g2p: Option<String>,
    pub params: Map<String, Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct G2pConfig {
    pub schema_version: String,
    pub id: String,
    pub framework: String,
    pub language: String,
    pub backend: String,
    #[serde(default)]
    pub variant: Option<String>,
    #[serde(default)]
    pub british: Option<bool>,
    #[serde(default)]
    pub transformer: Option<bool>,
    #[serde(default)]
    pub espeak_fallback: Option<bool>,
    #[serde(default)]
    pub espeak_language: Option<String>,
    #[serde(default)]
    pub lexicon: Option<String>,
    #[serde(default)]
    pub custom_entries: BTreeMap<String, String>,
    #[serde(default)]
    pub unk: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CatalogIndex {
    pub schema_version: String,
    pub id: String,
    pub catalog_source: String,
    #[serde(default)]
    pub note: Option<String>,
    pub library: CatalogLibrary,
    pub voices: Vec<String>,
    pub g2p: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CatalogLibrary {
    pub env: String,
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CastDocument {
    #[serde(default)]
    pub schema_version: Option<String>,
    pub engine: String,
    #[serde(default)]
    pub default_lang: Option<String>,
    #[serde(default)]
    pub default_speed: Option<f64>,
    #[serde(default)]
    pub speakers: BTreeMap<String, CastSpeaker>,
    #[serde(default)]
    pub characters: BTreeMap<String, CastCharacter>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CastSpeaker {
    pub name: String,
    pub voice: String,
    #[serde(default)]
    pub lang: Option<String>,
    #[serde(default)]
    pub speed: Option<f64>,
    #[serde(default)]
    pub framework: Option<String>,
    #[serde(default)]
    pub voice_id: Option<String>,
    #[serde(default)]
    pub character: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CastCharacter {
    pub display_name: String,
    pub voices: BTreeMap<String, String>,
}

pub fn frameworks() -> &'static [FrameworkRow] {
    registry().frameworks.as_slice()
}

pub fn framework(id: &str) -> Option<&'static FrameworkRow> {
    frameworks().iter().find(|row| row.id == id)
}

pub fn validate_voice_json(value: &Value) -> Result<VoiceDescriptor, String> {
    let voice: VoiceDescriptor =
        serde_json::from_value(value.clone()).map_err(|err| err.to_string())?;
    if voice.schema_version != "1.0.0" {
        return Err("schema_version must be 1.0.0".into());
    }
    if !voice_id_ok(&voice.id) || !voice.id.starts_with(&(voice.framework.clone() + ".")) {
        return Err(format!(
            "voice id {} must start with {}.",
            voice.id, voice.framework
        ));
    }
    if framework(&voice.framework).is_none() {
        return Err(format!("unknown framework {}", voice.framework));
    }
    if !matches!(
        voice.gender.as_str(),
        "female" | "male" | "neutral" | "unspecified"
    ) {
        return Err("gender is not a known value".into());
    }
    if !lang_ok(&voice.language) {
        return Err("language tag is invalid".into());
    }
    if voice.assets.is_empty() || voice.assets.len() > 16 {
        return Err("assets must be 1 to 16".into());
    }
    let mut seen = Vec::new();
    for asset in &voice.assets {
        if !path_ok(&asset.path) {
            return Err(format!(
                "asset path must stay inside the voice library: {}",
                asset.path
            ));
        }
        if seen.contains(&asset.path) {
            return Err(format!("repeated asset path {}", asset.path));
        }
        seen.push(asset.path.clone());
        if !matches!(
            asset.format.as_str(),
            "pt" | "npz" | "onnx" | "wav" | "bin" | "json"
        ) {
            return Err(format!("unsupported asset format {}", asset.format));
        }
    }
    if !voice.source.url.starts_with("https://") {
        return Err("source url must be https".into());
    }
    let kokoro = voice.framework == "kokoro" || voice.framework == "kokoro_onnx";
    match &voice.g2p {
        Some(g2p) if kokoro && g2p.starts_with("misaki.") => {}
        None if !kokoro => {}
        _ => return Err("g2p is required for Kokoro voices and omitted otherwise".into()),
    }
    validate_params(&voice)?;
    Ok(voice)
}

pub fn validate_g2p_json(value: &Value) -> Result<G2pConfig, String> {
    let config: G2pConfig = serde_json::from_value(value.clone()).map_err(|err| err.to_string())?;
    if config.schema_version != "1.0.0"
        || config.framework != "misaki"
        || !config.id.starts_with("misaki.")
    {
        return Err("g2p id must be a misaki config".into());
    }
    if let Some(path) = &config.lexicon {
        if !path_ok(path) {
            return Err("lexicon path must stay inside the voice library".into());
        }
    }
    match config.backend.as_str() {
        "misaki-en" => {
            if config.language != "en"
                || !matches!(config.variant.as_deref(), Some("en-us" | "en-gb"))
            {
                return Err("English Misaki needs variant en-us or en-gb".into());
            }
            let british = config.british.ok_or("English Misaki needs british")?;
            if british != (config.variant.as_deref() == Some("en-gb")) {
                return Err("british must follow the en-gb variant".into());
            }
        }
        "misaki-ja" if config.language == "ja" => {}
        "misaki-zh" if config.language == "zh" => {}
        "espeak" => {
            let espeak = config.espeak_language.as_deref().unwrap_or("");
            if config.espeak_fallback != Some(true) || !lang_ok(espeak) {
                return Err("espeak backend needs espeak_language".into());
            }
        }
        _ => return Err(format!("unsupported g2p backend {}", config.backend)),
    }
    Ok(config)
}

pub fn validate_catalog_json(value: &Value) -> Result<CatalogIndex, String> {
    let catalog: CatalogIndex =
        serde_json::from_value(value.clone()).map_err(|err| err.to_string())?;
    if catalog.schema_version != "1.0.0" || catalog.library.env != "GENAID_VOICES_DIR" {
        return Err("catalog must be schema 1.0.0 and use GENAID_VOICES_DIR".into());
    }
    if !catalog_source_ok(&catalog.catalog_source) {
        return Err("catalog_source is unknown".into());
    }
    if catalog.voices.is_empty() {
        return Err("catalog needs at least one voice".into());
    }
    for relative in catalog.voices.iter().chain(catalog.g2p.iter()) {
        if !path_ok(relative) || !relative.ends_with(".json") {
            return Err(format!(
                "catalog entry is not a relative JSON path: {relative}"
            ));
        }
    }
    Ok(catalog)
}

pub fn validate_cast_json(value: &Value) -> Result<CastDocument, String> {
    let cast: CastDocument =
        serde_json::from_value(value.clone()).map_err(|err| err.to_string())?;
    if cast.speakers.is_empty() || cast.speakers.len() > 8 {
        return Err("cast map needs 1 to 8 speakers".into());
    }
    if let Some(version) = &cast.schema_version {
        if version != "1.0.0" {
            return Err("cast map schema_version must be 1.0.0".into());
        }
    }
    for (id, speaker) in &cast.speakers {
        if id.parse::<u32>().is_err() {
            return Err(format!("speaker id {id} must be an integer"));
        }
        let framework = speaker.framework.as_deref().unwrap_or(cast.engine.as_str());
        if let Some(voice_id) = &speaker.voice_id {
            if !voice_id.starts_with(&(framework.to_string() + ".")) {
                return Err(format!(
                    "speaker {id} voice_id must start with {framework}."
                ));
            }
        }
        if let Some(character) = &speaker.character {
            let Some(found) = cast.characters.get(character) else {
                return Err(format!("speaker {id} character {character} is missing"));
            };
            if let Some(voice_id) = &speaker.voice_id {
                if found.voices.get(framework) != Some(voice_id) {
                    return Err(format!("speaker {id} does not match character {character}"));
                }
            }
        }
        if let Some(speed) = speaker.speed {
            check_speed(framework, speed)?;
        }
    }
    for (character_id, character) in &cast.characters {
        if character.voices.is_empty() {
            return Err(format!("character {character_id} has no voices"));
        }
        for (framework, voice_id) in &character.voices {
            if !voice_id.starts_with(&(framework.clone() + ".")) {
                return Err(format!(
                    "character {character_id} {framework} voice id does not match"
                ));
            }
        }
    }
    Ok(cast)
}

pub fn path_ok(path: &str) -> bool {
    if path.is_empty() || path.len() > 220 || path.starts_with('/') || path.contains('\\') {
        return false;
    }
    path.split('/')
        .all(|part| !part.is_empty() && part != "." && part != "..")
        && path
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '/' | '+' | '-'))
}

fn registry() -> &'static RegistryFile {
    static ONCE: OnceLock<RegistryFile> = OnceLock::new();
    ONCE.get_or_init(|| {
        let file: RegistryFile = serde_json::from_str(REGISTRY_JSON).expect("registry.v1.json");
        assert_eq!(file.schema_version, "1.0.0");
        file
    })
}

fn validate_params(voice: &VoiceDescriptor) -> Result<(), String> {
    match voice.framework.as_str() {
        "kokoro" => validate_kokoro(voice, true),
        "kokoro_onnx" => validate_kokoro(voice, false),
        "vibevoice" => validate_vibevoice(voice),
        "livevoice" => validate_livevoice(voice),
        "personaplex" => validate_personaplex(voice),
        other => Err(format!("no voice param checker for {other}")),
    }
}

fn validate_kokoro(voice: &VoiceDescriptor, torch: bool) -> Result<(), String> {
    let lang = voice.language.as_str();
    let speed = number(voice, "speed")?;
    if !(0.5..=2.0).contains(&speed) {
        return Err("kokoro speed must be 0.5..2.0".into());
    }
    let ids = kokoro_ids(voice)?;
    if torch {
        let code = text(voice, "lang_code")?;
        let expected = kokoro_lang(code).ok_or("lang_code is not a Kokoro pipeline code")?;
        if text(voice, "lang")? != expected || expected != lang {
            return Err("kokoro lang_code does not match language".into());
        }
        if ids.iter().any(|id| !id.starts_with(code)) {
            return Err("kokoro pack id does not use lang_code".into());
        }
    } else if text(voice, "lang")? != lang {
        return Err("kokoro_onnx lang does not match language".into());
    }
    let letters: Vec<char> = ids
        .iter()
        .map(|id| id.chars().nth(1).unwrap_or(' '))
        .collect();
    let same = letters.iter().all(|letter| *letter == letters[0]);
    if same {
        let expected = if letters[0] == 'f' { "female" } else { "male" };
        if voice.gender != expected {
            return Err(format!("kokoro gender must be {expected}"));
        }
    } else if voice.gender != "unspecified" {
        return Err("mixed-gender blend must set gender to unspecified".into());
    }
    if torch {
        let members: Vec<_> = voice
            .assets
            .iter()
            .filter_map(|asset| asset.member.clone())
            .collect();
        let mut left = members.clone();
        let mut right = ids.clone();
        left.sort();
        right.sort();
        if left != right {
            return Err("kokoro assets must list each pack once".into());
        }
    }
    Ok(())
}

fn validate_vibevoice(voice: &VoiceDescriptor) -> Result<(), String> {
    let stem = text(voice, "file_stem")?;
    let (stem_lang, gender, bgm) = split_stem(stem, true)?;
    if stem_lang != voice.language || gender != voice.gender {
        return Err("vibevoice file_stem does not match language or gender".into());
    }
    if bool_param(voice, "background_music")? != bgm {
        return Err("vibevoice background_music must follow _bgm".into());
    }
    if voice.params.contains_key("model_size") || voice.params.contains_key("model_id") {
        let size = text(voice, "model_size")?;
        let model = text(voice, "model_id")?;
        let expected = match size {
            "1.5B" => "microsoft/VibeVoice-1.5B",
            "7B" => "microsoft/VibeVoice-Large",
            _ => return Err("vibevoice model_size must be 1.5B or 7B".into()),
        };
        if model != expected {
            return Err("vibevoice model_id does not match model_size".into());
        }
    }
    if voice.assets.len() != 1 || !voice.assets[0].path.ends_with(&format!("{stem}.wav")) {
        return Err("vibevoice asset must be the reference wav".into());
    }
    Ok(())
}

fn validate_livevoice(voice: &VoiceDescriptor) -> Result<(), String> {
    let stem = text(voice, "file_stem")?;
    let (stem_lang, gender, bgm) = split_stem(stem, false)?;
    if bgm || stem_lang != voice.language || gender != voice.gender {
        return Err("livevoice file_stem does not match the descriptor".into());
    }
    if text(voice, "model_id")? != "microsoft/VibeVoice-Realtime-0.5B"
        || text(voice, "preset_kind")? != "kv_cache"
    {
        return Err("livevoice model or preset_kind is wrong".into());
    }
    let experimental = bool_param(voice, "experimental")?;
    let path = &voice.assets[0].path;
    if experimental != path.contains("experimental_voices/")
        || !path.ends_with(&format!("{stem}.pt"))
    {
        return Err(
            "livevoice experimental must follow experimental_voices/ and the file stem".into(),
        );
    }
    Ok(())
}

fn validate_personaplex(voice: &VoiceDescriptor) -> Result<(), String> {
    let code = text(voice, "voice_code")?;
    if !persona_code_ok(code) {
        return Err("personaplex voice_code is not in the stock list".into());
    }
    let family = if code.starts_with("NAT") {
        "natural"
    } else {
        "variety"
    };
    if text(voice, "voice_family")? != family
        || text(voice, "voice_prompt")? != format!("{code}.pt")
    {
        return Err("personaplex voice family or prompt file does not match the code".into());
    }
    let gender = if code.chars().nth(3) == Some('F') {
        "female"
    } else {
        "male"
    };
    if voice.gender != gender || voice.language != "en" || bool_param(voice, "gated")? != true {
        return Err("personaplex gender, language, or gated flag is wrong".into());
    }
    if text(voice, "model_id")? != "nvidia/personaplex-7b-v1" {
        return Err("personaplex model_id must be nvidia/personaplex-7b-v1".into());
    }
    if voice.assets.len() != 1 || !voice.assets[0].path.ends_with(&format!("{code}.pt")) {
        return Err("personaplex asset must be the voice prompt".into());
    }
    Ok(())
}

fn kokoro_ids(voice: &VoiceDescriptor) -> Result<Vec<String>, String> {
    let single = voice.params.get("voice_id");
    let blend = voice.params.get("blend");
    match (single, blend) {
        (Some(Value::String(id)), None) => {
            if !pack_ok(id) {
                return Err("voice_id is not a Kokoro pack id".into());
            }
            Ok(vec![id.clone()])
        }
        (None, Some(Value::Array(parts))) if (2..=8).contains(&parts.len()) => {
            let mut ids = Vec::new();
            let mut total = 0.0;
            for part in parts {
                let obj = part.as_object().ok_or("blend part must be an object")?;
                let id = obj
                    .get("voice_id")
                    .and_then(Value::as_str)
                    .ok_or("blend voice_id")?;
                if !pack_ok(id) || ids.iter().any(|seen| seen == id) {
                    return Err("blend voice_id is invalid or repeated".into());
                }
                let weight = obj
                    .get("weight")
                    .and_then(Value::as_f64)
                    .ok_or("blend weight")?;
                if weight <= 0.0 || weight > 1.0 {
                    return Err("blend weight must be in (0, 1]".into());
                }
                total += weight;
                ids.push(id.to_string());
            }
            if (total - 1.0).abs() > 1e-6 {
                return Err("blend weights must sum to 1".into());
            }
            Ok(ids)
        }
        _ => Err("kokoro params need voice_id or blend, not both".into()),
    }
}

fn split_stem(stem: &str, allow_bgm: bool) -> Result<(&str, &str, bool), String> {
    let (body, bgm) = if allow_bgm {
        if let Some(stripped) = stem.strip_suffix("_bgm") {
            (stripped, true)
        } else {
            (stem, false)
        }
    } else if stem.contains("_bgm") {
        return Err("this file stem does not allow _bgm".into());
    } else {
        (stem, false)
    };
    let (lang, rest) = body
        .split_once('-')
        .ok_or("file stem needs a language prefix")?;
    let (name, gender) = rest
        .rsplit_once('_')
        .ok_or("file stem needs a gender suffix")?;
    if lang.len() < 2 || name.is_empty() || !matches!(gender, "man" | "woman" | "neutral") {
        return Err("file stem has the wrong shape".into());
    }
    let mapped = match gender {
        "woman" => "female",
        "man" => "male",
        _ => "neutral",
    };
    Ok((lang, mapped, bgm))
}

fn persona_code_ok(code: &str) -> bool {
    let bytes = code.as_bytes();
    if bytes.len() != 5 {
        return false;
    }
    let family = &code[..3];
    let gender = bytes[3] as char;
    let index = bytes[4] as char;
    let natural = family == "NAT" && matches!(gender, 'F' | 'M') && matches!(index, '0'..='3');
    let variety = family == "VAR" && matches!(gender, 'F' | 'M') && matches!(index, '0'..='4');
    natural || variety
}

fn pack_ok(id: &str) -> bool {
    let mut chars = id.chars();
    let Some(lang) = chars.next() else {
        return false;
    };
    let Some(gender) = chars.next() else {
        return false;
    };
    let Some('_') = chars.next() else {
        return false;
    };
    "abefhijpz".contains(lang)
        && matches!(gender, 'f' | 'm')
        && chars.clone().next().is_some()
        && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
}

fn kokoro_lang(code: &str) -> Option<&'static str> {
    Some(match code {
        "a" => "en-us",
        "b" => "en-gb",
        "e" => "es",
        "f" => "fr-fr",
        "h" => "hi",
        "i" => "it",
        "j" => "ja",
        "p" => "pt-br",
        "z" => "zh",
        _ => return None,
    })
}

fn text<'a>(voice: &'a VoiceDescriptor, key: &str) -> Result<&'a str, String> {
    voice
        .params
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("params.{key} must be a string"))
}

fn number(voice: &VoiceDescriptor, key: &str) -> Result<f64, String> {
    voice
        .params
        .get(key)
        .and_then(Value::as_f64)
        .ok_or_else(|| format!("params.{key} must be a number"))
}

fn bool_param(voice: &VoiceDescriptor, key: &str) -> Result<bool, String> {
    voice
        .params
        .get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("params.{key} must be a boolean"))
}

fn catalog_source_ok(value: &str) -> bool {
    if matches!(value, "naming-conventions" | "library-scan" | "handwritten") {
        return true;
    }
    let Some(rest) = value.strip_prefix("manifest:") else {
        return false;
    };
    let Some((repo, sha)) = rest.split_once('@') else {
        return false;
    };
    let Some((owner, name)) = repo.split_once('/') else {
        return false;
    };
    !owner.is_empty()
        && !name.is_empty()
        && (7..=40).contains(&sha.len())
        && sha.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn check_speed(framework: &str, speed: f64) -> Result<(), String> {
    if framework == "kokoro" || framework == "kokoro_onnx" {
        if !(0.5..=2.0).contains(&speed) {
            return Err("kokoro speed must be 0.5..2.0".into());
        }
    } else if !speed.is_finite() || speed <= 0.0 || speed > 4.0 {
        return Err("speed must be greater than 0 and at most 4".into());
    }
    Ok(())
}

fn voice_id_ok(id: &str) -> bool {
    let Some((framework, local)) = id.split_once('.') else {
        return false;
    };
    slug_ok(framework)
        && !local.is_empty()
        && local.len() <= 64
        && local
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphanumeric())
        && local
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-'))
}

fn slug_ok(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_lowercase())
        && value
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
}

fn lang_ok(value: &str) -> bool {
    let mut parts = value.split('-');
    let Some(primary) = parts.next() else {
        return false;
    };
    if !(2..=3).contains(&primary.len()) || !primary.chars().all(|ch| ch.is_ascii_lowercase()) {
        return false;
    }
    match parts.next() {
        None => parts.next().is_none(),
        Some(rest) => {
            (2..=8).contains(&rest.len())
                && rest
                    .chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
                && parts.next().is_none()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn registry_lists_the_shipped_frameworks() {
        let ids: Vec<_> = frameworks().iter().map(|row| row.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "kokoro",
                "kokoro_onnx",
                "misaki",
                "vibevoice",
                "livevoice",
                "personaplex"
            ]
        );
        assert!(framework("kokoro_onnx").unwrap().wired);
        assert!(!framework("personaplex").unwrap().wired);
        assert!(framework("livevoice").unwrap().engine_id.is_none());
    }

    #[test]
    fn paths_reject_escape_and_accept_library_relatives() {
        assert!(path_ok("en-Alice_woman.wav"));
        assert!(path_ok(
            "_hf_vibevoice_samples/realtime_model/voices/streaming_model/en-Carter_man.pt"
        ));
        assert!(!path_ok("../secrets.pt"));
        assert!(!path_ok("/tmp/voice.pt"));
        assert!(!path_ok("voices\\af_heart.pt"));
    }

    #[test]
    fn blend_weights_must_sum_to_one() {
        let value = json!({
            "schema_version": "1.0.0",
            "id": "kokoro.blend_bad",
            "display_name": "Bad",
            "language": "en-us",
            "gender": "female",
            "framework": "kokoro",
            "assets": [
                {"role": "voicepack", "path": "kokoro/voices/af_heart.pt", "format": "pt", "member": "af_heart"},
                {"role": "voicepack", "path": "kokoro/voices/af_bella.pt", "format": "pt", "member": "af_bella"}
            ],
            "source": {"name": "hexgrad/Kokoro-82M", "url": "https://huggingface.co/hexgrad/Kokoro-82M", "license": "Apache-2.0"},
            "tags": ["en-us"],
            "g2p": "misaki.en-us",
            "params": {
                "lang_code": "a",
                "lang": "en-us",
                "speed": 1.0,
                "blend": [
                    {"voice_id": "af_heart", "weight": 0.9},
                    {"voice_id": "af_bella", "weight": 0.9}
                ]
            }
        });
        let error = validate_voice_json(&value).unwrap_err();
        assert!(error.contains("sum to 1"), "{error}");
    }
}

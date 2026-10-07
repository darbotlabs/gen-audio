//! Personas, TTS voice models, and the library clip catalog.
//!
//! Agent ids here are voice personas. Connector ids (copilot, claude, gpt,
//! gemini) are not personas. Voice ids are TTS / G2P models. Kokoro pack ids
//! such as `af_heart` are references on a persona, not Voice selector values.

use serde::Serialize;
use serde_json::{json, Value};

pub const MAX_AGENTS_PER_TRACK: usize = 8;

pub const SLIDES: &[&str] = &[
    "models",
    "profiles",
    "studio",
    "library",
    "video",
    "spatial",
    "connectors",
    "pipeline",
];

const CONNECTOR_IDS: &[&str] = &["mcp", "acp", "harness", "copilot", "claude", "gpt", "gemini", "local"];

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Persona {
    pub id: &'static str,
    pub name: &'static str,
    pub tone: &'static str,
    pub purpose: &'static str,
    pub domain: &'static str,
    pub accent: &'static str,
    pub traits: &'static str,
    pub refs: &'static [&'static str],
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct VoiceModel {
    pub id: &'static str,
    pub label: &'static str,
    pub waveform: bool,
    pub synth_adapter: bool,
    pub unavailable: bool,
    pub note: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct LibraryClipMeta {
    pub id: &'static str,
    pub title: &'static str,
    pub engine_id: &'static str,
    pub status: &'static str,
    pub synthesized_speech: bool,
    pub wav_url: Option<&'static str>,
    pub cube_json_url: Option<&'static str>,
    pub sidecar_url: Option<&'static str>,
    pub summary: &'static str,
}

const PERSONAS: &[Persona] = &[
    Persona {
        id: "anton",
        name: "Anton",
        tone: "measured",
        purpose: "Host a Gen-Audio briefing",
        domain: "Desktop app and SDK layout",
        accent: "General American",
        traits: "Low-mid pitch, even pace, short pauses. Not a connector model.",
        refs: &["persona:anton"],
    },
    Persona {
        id: "alice",
        name: "Alice",
        tone: "warm",
        purpose: "Walk through a labeled example",
        domain: "SDK onboarding",
        accent: "General American",
        traits: "Persona Alice is the agent name. Kokoro pack id af_heart is a reference on this profile, not the Voice selector.",
        refs: &["kokoro-pack:af_heart", "persona:alice"],
    },
    Persona {
        id: "khortana",
        name: "Khortana",
        tone: "bright",
        purpose: "Offer a contrasting take",
        domain: "Product critique",
        accent: "General American",
        traits: "Higher placement and a quicker glide. Not a TTS model id.",
        refs: &["persona:khortana"],
    },
    Persona {
        id: "rocky",
        name: "Rocky",
        tone: "gravel",
        purpose: "Short reactions between turns",
        domain: "Studio banter",
        accent: "General American",
        traits: "Low pitch and a narrow band. Not a TTS model id.",
        refs: &["persona:rocky"],
    },
    Persona {
        id: "optimus",
        name: "Optimus Timelarp",
        tone: "assertive",
        purpose: "Gate and harden product stamps",
        domain: "Product / A-E stamps",
        accent: "General American",
        traits: "Decisive, honest, screenshot-proof. AP-7 hard kill. Not a TTS model id.",
        refs: &[
            "persona:optimus",
            "tts:kokoro_onnx",
            "cube:library_cube_explainer",
            "clip:lib-cube-explainer",
        ],
    },
    Persona {
        id: "frank",
        name: "Frank",
        tone: "plain",
        purpose: "Second chair on a briefing",
        domain: "Multi-speaker scripts",
        accent: "General American",
        traits: "Persona Frank is the agent name. Kokoro pack id am_michael is a reference, not the Voice selector.",
        refs: &["kokoro-pack:am_michael", "persona:frank"],
    },
    Persona {
        id: "nova",
        name: "Nova",
        tone: "clear",
        purpose: "Read a status line",
        domain: "Readiness and health",
        accent: "General American",
        traits: "Mid pitch, little glide. Not a connector.",
        refs: &["persona:nova"],
    },
    Persona {
        id: "ivo",
        name: "Ivo",
        tone: "dry",
        purpose: "Name what is stubbed",
        domain: "Engine honesty",
        accent: "General American",
        traits: "Flat delivery. Magpie, VibeVoice, and Pocket stay unavailable when this persona is selected.",
        refs: &["persona:ivo"],
    },
    Persona {
        id: "sable",
        name: "Sable",
        tone: "soft",
        purpose: "Close a segment",
        domain: "Clip outros",
        accent: "General American",
        traits: "Softer high end. Does not imply a video track.",
        refs: &["persona:sable"],
    },
];

const VOICE_MODELS: &[VoiceModel] = &[
    VoiceModel {
        id: "kokoro_onnx",
        label: "kokoro-onnx",
        waveform: true,
        synth_adapter: true,
        unavailable: false,
        note: "Python kokoro-onnx path. Needs local ONNX and voices files. Weights are not in this repo.",
    },
    VoiceModel {
        id: "kokoro_dayour",
        label: "dayour/kokoro",
        waveform: true,
        synth_adapter: false,
        unavailable: false,
        note: "dayour/kokoro torch runtime. Not vendored here. A library WAV may exist; this repo has no synth adapter.",
    },
    VoiceModel {
        id: "misaki",
        label: "misaki (G2P, not a waveform)",
        waveform: false,
        synth_adapter: false,
        unavailable: false,
        note: "Grapheme-to-phoneme for Kokoro. It does not emit a waveform by itself.",
    },
    VoiceModel {
        id: "vibevoice",
        label: "VibeVoice (unavailable)",
        waveform: true,
        synth_adapter: false,
        unavailable: true,
        note: "No adapter and no verified synth in this app.",
    },
    VoiceModel {
        id: "magpie",
        label: "Magpie (unavailable)",
        waveform: true,
        synth_adapter: false,
        unavailable: true,
        note: "GGUF may exist on a machine; magpie-tts.cpp is not built here. No fake audio.",
    },
    VoiceModel {
        id: "pocket_tts",
        label: "Pocket TTS (unavailable)",
        waveform: true,
        synth_adapter: false,
        unavailable: true,
        note: "pocket_tts is not installed in this app. No teaser audio is reused.",
    },
];

const LIBRARY: &[LibraryClipMeta] = &[
    LibraryClipMeta {
        id: "lib-kokoro-onnx",
        title: "kokoro-onnx",
        engine_id: "kokoro_onnx",
        status: "ok",
        synthesized_speech: true,
        wav_url: Some("/library/library_kokoro_onnx.wav"),
        cube_json_url: Some("/library/library_kokoro_onnx_cube3d.json"),
        sidecar_url: Some("/library/library_kokoro_onnx.synth.json"),
        summary: "Catalog says this clip is a real kokoro-onnx briefing. This process does not open the WAV.",
    },
    LibraryClipMeta {
        id: "lib-kokoro",
        title: "dayour/kokoro",
        engine_id: "kokoro_dayour",
        status: "ok",
        synthesized_speech: true,
        wav_url: Some("/library/library_kokoro.wav"),
        cube_json_url: None,
        sidecar_url: None,
        summary: "Catalog says this clip is a real dayour/kokoro briefing. This process does not open the WAV. No synth adapter in this repo.",
    },
    LibraryClipMeta {
        id: "lib-misaki-kokoro",
        title: "misaki→kokoro",
        engine_id: "misaki_kokoro",
        status: "running",
        synthesized_speech: false,
        wav_url: None,
        cube_json_url: None,
        sidecar_url: None,
        summary: "Misaki→Kokoro is not finished. No WAV until a real file exists.",
    },
    LibraryClipMeta {
        id: "lib-magpie",
        title: "Magpie",
        engine_id: "magpie",
        status: "unavailable",
        synthesized_speech: false,
        wav_url: None,
        cube_json_url: None,
        sidecar_url: None,
        summary: "Magpie stays unavailable. No fake audio.",
    },
    LibraryClipMeta {
        id: "lib-vibevoice",
        title: "VibeVoice",
        engine_id: "vibevoice",
        status: "unavailable",
        synthesized_speech: false,
        wav_url: None,
        cube_json_url: None,
        sidecar_url: None,
        summary: "VibeVoice stays unavailable. No fake audio.",
    },
    LibraryClipMeta {
        id: "lib-pocket",
        title: "Pocket TTS",
        engine_id: "pocket_tts",
        status: "unavailable",
        synthesized_speech: false,
        wav_url: None,
        cube_json_url: None,
        sidecar_url: None,
        summary: "Pocket TTS stays unavailable. No fake audio.",
    },
];

pub fn personas() -> &'static [Persona] {
    PERSONAS
}

pub fn persona(id: &str) -> Option<&'static Persona> {
    PERSONAS.iter().find(|item| item.id == id)
}

pub fn voice_models() -> &'static [VoiceModel] {
    VOICE_MODELS
}

pub fn voice_model(id: &str) -> Option<&'static VoiceModel> {
    VOICE_MODELS.iter().find(|item| item.id == id)
}

pub fn library_clips() -> &'static [LibraryClipMeta] {
    LIBRARY
}

pub fn library_clip(id: &str) -> Option<&'static LibraryClipMeta> {
    LIBRARY.iter().find(|item| item.id == id)
}

pub fn is_connector_id(id: &str) -> bool {
    CONNECTOR_IDS.contains(&id)
}

pub fn voice_profile_value(id: &str) -> Option<Value> {
    let person = persona(id)?;
    let voice = voice_models().iter().find(|item| item.id == "kokoro_onnx");
    let voice_label = voice.map(|item| item.label).unwrap_or("kokoro-onnx");
    Some(json!({
        "agentName": person.name,
        "personaId": person.id,
        "voiceModel": "kokoro_onnx",
        "voiceLabel": voice_label,
        "tone": person.tone,
        "purpose": person.purpose,
        "domain": person.domain,
        "accent": person.accent,
        "traits": person.traits,
        "refs": person.refs,
        "spectrogram2d": "browser-profile-map",
        "spectrogram3d": if matches!(person.id, "alice" | "optimus") { "library-cube-hook" } else { "none" },
        "cubeJsonUrl": match person.id {
            "alice" => Value::String("/library/library_kokoro_onnx_cube3d.json".into()),
            "optimus" => Value::String("/library/library_cube_explainer_kokoro_onnx_cube3d.json".into()),
            _ => Value::Null,
        },
        "notPodcast": true,
        "synthesizedSpeech": false,
        "disclaimer": format!(
            "Profile for persona {}. Voice model is {}. Not a podcast render and not a Python spectrogram.",
            person.name, voice_label
        )
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optimus_is_a_catalog_persona_without_a_wav_claim() {
        let person = persona("optimus").expect("optimus");
        assert_eq!(person.name, "Optimus Timelarp");
        let profile = voice_profile_value("optimus").expect("profile");
        assert_eq!(profile["notPodcast"], true);
        assert_eq!(profile["synthesizedSpeech"], false);
        assert!(profile.get("wavUrl").is_none());
        assert_eq!(profile["spectrogram3d"], "library-cube-hook");
        assert_eq!(
            profile["cubeJsonUrl"],
            "/library/library_cube_explainer_kokoro_onnx_cube3d.json"
        );
    }
}

//! Compare-list engines. Only `kokoro_onnx` has a synthesizer, and only in the Python SDK.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineKind {
    Synth,
    G2p,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineStatus {
    Implemented,
    External,
    Library,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct EngineSpec {
    pub id: &'static str,
    pub kind: EngineKind,
    pub status: EngineStatus,
    pub weights_bundled: bool,
    pub summary: &'static str,
}

const ENGINES: &[EngineSpec] = &[
    EngineSpec {
        id: "kokoro_onnx",
        kind: EngineKind::Synth,
        status: EngineStatus::Implemented,
        weights_bundled: false,
        summary: "Python kokoro-onnx path. Needs local ONNX and voices files. Weights are not in this repo.",
    },
    EngineSpec {
        id: "kokoro_dayour",
        kind: EngineKind::Synth,
        status: EngineStatus::External,
        weights_bundled: false,
        summary: "dayour/kokoro torch runtime. Not vendored here.",
    },
    EngineSpec {
        id: "misaki",
        kind: EngineKind::G2p,
        status: EngineStatus::Library,
        weights_bundled: false,
        summary: "dayour/misaki is G2P for Kokoro. It does not emit a waveform by itself.",
    },
    EngineSpec {
        id: "vibevoice",
        kind: EngineKind::Synth,
        status: EngineStatus::External,
        weights_bundled: false,
        summary: "VibeVoice 1.5B compare slot (Alice/Frank in the 2026-10-06 notes). No adapter and no weights here.",
    },
    EngineSpec {
        id: "magpie",
        kind: EngineKind::Synth,
        status: EngineStatus::External,
        weights_bundled: false,
        summary: "Magpie TTS 357M compare slot (Sofia/Jason in the 2026-10-06 notes). No adapter and no weights here.",
    },
    EngineSpec {
        id: "pocket_tts",
        kind: EngineKind::Synth,
        status: EngineStatus::External,
        weights_bundled: false,
        summary: "Kyutai Pocket TTS, the CPU stand-in used when PersonaPlex weights are unavailable. No adapter here.",
    },
];

pub fn engines() -> &'static [EngineSpec] {
    ENGINES
}

pub fn engine(id: &str) -> Option<&'static EngineSpec> {
    ENGINES.iter().find(|spec| spec.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_kokoro_onnx_is_implemented() {
        let implemented: Vec<_> = engines()
            .iter()
            .filter(|spec| spec.status == EngineStatus::Implemented)
            .map(|spec| spec.id)
            .collect();
        assert_eq!(implemented, vec!["kokoro_onnx"]);
        assert_eq!(engine("misaki").unwrap().kind, EngineKind::G2p);
        assert!(engine("vibevoice").unwrap().summary.contains("No adapter"));
        assert!(engines().iter().all(|spec| !spec.weights_bundled));
    }
}

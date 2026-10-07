//! Figures copied from the 2026-10-06 GenAID compare notes.
//!
//! They are reference text for the BenchmarkCompare card. This crate did not
//! remeasure them and does not treat inverse-HDR as a publish ranking.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct BenchmarkRow {
    pub engine: &'static str,
    pub metric: &'static str,
    pub value: &'static str,
    pub unit: &'static str,
    pub note: &'static str,
}

pub const SOURCE_NOTE: &str = "Reference figures from the 2026-10-06 GenAID podcast compare notes. Not remeasured in this repository. Do not rank a publish WAV on inverse-HDR alone.";

pub fn reference_rows() -> &'static [BenchmarkRow] {
    &[
        BenchmarkRow {
            engine: "pocket_tts",
            metric: "teaser_wer",
            value: "22.22%",
            unit: "WER",
            note: "Nemotron ASR on the short teaser. PersonaPlex-family stand-in.",
        },
        BenchmarkRow {
            engine: "vibevoice",
            metric: "teaser_wer",
            value: "30.00%",
            unit: "WER",
            note: "Short teaser. Full improved WAV in those notes was the preferred multi-speaker publish.",
        },
        BenchmarkRow {
            engine: "magpie",
            metric: "teaser_wer",
            value: "42.22%",
            unit: "WER",
            note: "Short teaser truncated relative to the reference word count.",
        },
        BenchmarkRow {
            engine: "vibevoice",
            metric: "bw95_hz",
            value: "2670.5",
            unit: "Hz",
            note: "Full improved final in the 2026-10-06 notes, not a new measurement.",
        },
        BenchmarkRow {
            engine: "magpie",
            metric: "bw95_hz",
            value: "1997.5",
            unit: "Hz",
            note: "Full improved final in the 2026-10-06 notes.",
        },
        BenchmarkRow {
            engine: "kokoro_onnx",
            metric: "bw95_hz_raw",
            value: "826.9",
            unit: "Hz",
            note: "Raw kokoro-onnx in those notes. Rev3 rose to about 1398 Hz and was still below VibeVoice.",
        },
        BenchmarkRow {
            engine: "vibevoice",
            metric: "inv_hdr_rev3",
            value: "0.2165",
            unit: "inv-HDR",
            note: "Near no-op versus rev0. A higher Kokoro inv-HDR did not make it the publish pick.",
        },
    ]
}

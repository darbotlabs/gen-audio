//! Deterministic sine tone used as a UI and pipeline fixture.
//!
//! The WAV is not speech and not a podcast render. A sidecar JSON says so.

use std::fs;
use std::path::Path;

use serde_json::json;

use crate::paths::write_name;

pub const FIXTURE_WAV_NAME: &str = "fixture-tone.wav";
pub const FIXTURE_SIDECAR_NAME: &str = "fixture-tone.fixture.json";

pub struct FixtureTone {
    pub wav: std::path::PathBuf,
    pub sidecar: std::path::PathBuf,
    pub sample_rate: u32,
    pub samples: usize,
}

pub fn write_fixture_tone(work: &Path) -> Result<FixtureTone, String> {
    let wav_path = write_name(work, FIXTURE_WAV_NAME)?;
    let sidecar_path = write_name(work, FIXTURE_SIDECAR_NAME)?;
    let sample_rate: u32 = 16_000;
    let edge: usize = (0.08 * sample_rate as f32) as usize;
    let body: usize = (0.50 * sample_rate as f32) as usize;
    let mut samples = Vec::with_capacity(edge * 2 + body);
    samples.extend(std::iter::repeat(0.0f32).take(edge));
    for index in 0..body {
        let t = index as f32 / sample_rate as f32;
        let rumble = 0.25 * (2.0 * std::f32::consts::PI * 40.0 * t).sin();
        let voice = 0.35 * (2.0 * std::f32::consts::PI * 440.0 * t).sin();
        samples.push(rumble + voice);
    }
    samples.extend(std::iter::repeat(0.0f32).take(edge));
    let bytes = encode_wav(&samples, sample_rate);
    fs::write(&wav_path, bytes).map_err(|err| err.to_string())?;
    let meta = json!({
        "kind": "fixture-tone",
        "notPodcast": true,
        "notSpeech": true,
        "sampleRate": sample_rate,
        "samples": samples.len(),
        "description": "Sine fixture (40 Hz plus 440 Hz) with silent edges. Not a podcast render."
    });
    fs::write(&sidecar_path, serde_json::to_vec_pretty(&meta).unwrap())
        .map_err(|err| err.to_string())?;
    Ok(FixtureTone {
        wav: wav_path,
        sidecar: sidecar_path,
        sample_rate,
        samples: samples.len(),
    })
}

fn encode_wav(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    let mut pcm = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        let clamped = sample.clamp(-1.0, 1.0);
        let value = (clamped * 32767.0).round() as i16;
        pcm.extend_from_slice(&value.to_le_bytes());
    }
    let data_len = pcm.len() as u32;
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    let byte_rate = sample_rate * 2;
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend(pcm);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::make_work_dir;

    #[test]
    fn fixture_is_labeled_and_not_empty() {
        let work = make_work_dir().unwrap();
        let tone = write_fixture_tone(&work).unwrap();
        assert!(tone.samples > 1000);
        let meta: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&tone.sidecar).unwrap()).unwrap();
        assert_eq!(meta["notPodcast"], true);
        assert_eq!(meta["kind"], "fixture-tone");
        let header = std::fs::read(&tone.wav).unwrap();
        assert_eq!(&header[0..4], b"RIFF");
        let _ = std::fs::remove_dir_all(&work);
    }
}

//! Fixed-profile mastering of issued audio, not a speech-synthesis adapter.
//!
//! `master(&scratch, &json!({"input": "issued.wav", "output": "master.wav"}))`
//! accepts only scratch basenames; `output` is optional and must be unused.
//! Requires FFmpeg with highpass, lowpass, acompressor and loudnorm filters.
//! `GEN_AUDIO_FFMPEG` may select a trusted absolute executable or PATH name.
//! No Python, ffprobe, network access, caller filters or new dependencies are used.
//!
//! The profile retains the prototype's compressor makeup gain of 2 (linear).
//! Measurements describe the actual raw/final files, not exact speech timing.

use std::ffi::{OsStr, OsString};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use crate::asset::sha256_hex;
use crate::bridge::wav_metadata;
use crate::paths::{self, Scratch, SpeechReservation};
use crate::subprocess::{self, Limits};

const SAMPLE_RATE: u32 = 24_000;
const MAX_SAMPLES: u64 = SAMPLE_RATE as u64 * 600;
const MAX_WAV_BYTES: usize = MAX_SAMPLES as usize * 2 + 65_536;
const MAX_REPORT_BYTES: usize = 64 * 1024;
const DEADLINE: Duration = Duration::from_secs(120);
const TARGET_I: f64 = -16.0;
const TARGET_TP: f64 = -1.5;
const TARGET_LRA: f64 = 4.0;
const I_TOLERANCE: f64 = 1.0;
const TP_TOLERANCE: f64 = 0.2;
const LRA_TOLERANCE: f64 = 1.0;
const CONDITIONING: &str = "highpass=f=60,lowpass=f=11000,\
    acompressor=threshold=-24dB:ratio=2.5:attack=15:release=180:makeup=2";
const ANALYSIS: &str = "loudnorm=I=-16:TP=-1.5:LRA=4:print_format=json";

#[derive(Debug, Clone, Serialize)]
struct Measurement {
    integrated_lufs: f64,
    true_peak_dbtp: f64,
    loudness_range_lu: f64,
    threshold_lufs: f64,
    target_offset_lu: f64,
}

#[derive(Debug)]
struct Loudnorm {
    input: Measurement,
    normalization_type: String,
}

struct Request {
    input: String,
    output: String,
}

impl Request {
    fn parse(scratch: &Scratch, args: &Value) -> Result<Self, String> {
        let object = args
            .as_object()
            .ok_or("mastering arguments must be an object")?;
        if object
            .keys()
            .any(|key| !matches!(key.as_str(), "input" | "output"))
        {
            return Err("mastering accepts only input and optional output WAV names".into());
        }
        let input = object
            .get("input")
            .and_then(Value::as_str)
            .ok_or("mastering input must be an issued WAV filename")?;
        let output = match object.get("output") {
            None => format!("master-{}", scratch.unique_name(".wav")),
            Some(Value::String(name)) => name.clone(),
            _ => return Err("mastering output must be a WAV filename".into()),
        };
        for name in [input, output.as_str()] {
            paths::write_name(&scratch.dir, name)?;
            if !name.ends_with(".wav") {
                return Err("mastering input and output must end in .wav".into());
            }
        }
        if input.eq_ignore_ascii_case(&output) {
            return Err("mastering never overwrites its input; choose a unique output".into());
        }
        Ok(Self {
            input: input.into(),
            output,
        })
    }
}

/// Master one non-silent mono 24 kHz PCM16 WAV, at most 600 seconds long.
///
/// Success issues a new WAV and versioned JSON report. It means verified audio
/// processing, never proof of synthesized speech. All subprocesses share one
/// 120-second budget. Errors leave no downloadable partial result.
pub fn master(scratch: &Scratch, args: &Value) -> Result<Value, String> {
    master_with_program(scratch, args, &ffmpeg_program()?)
}

fn ffmpeg_program() -> Result<OsString, String> {
    match std::env::var_os("GEN_AUDIO_FFMPEG") {
        None => Ok("ffmpeg".into()),
        Some(program) => {
            let path = Path::new(&program);
            if program.is_empty()
                || (!path.is_absolute()
                    && (path.components().count() != 1
                        || program.to_string_lossy().contains(['/', '\\', ':'])))
            {
                return Err(
                    "GEN_AUDIO_FFMPEG must be an absolute executable path or PATH name".into(),
                );
            }
            Ok(program)
        }
    }
}

fn master_with_program(scratch: &Scratch, args: &Value, program: &OsStr) -> Result<Value, String> {
    let start = Instant::now();
    let request = Request::parse(scratch, args)?;
    let source = scratch.open_input(&request.input)?;
    let source_bytes = paths::read_bounded(&source, MAX_WAV_BYTES)?;
    let (rate, samples) = validate_wav(&source_bytes)?;
    let source_hash = sha256_hex(&source_bytes);
    let destination = scratch.reserve_speech_guard(&request.output)?;
    let snapshot = scratch.reserve_speech_guard(&scratch.unique_name(".wav"))?;
    let staged = scratch.reserve_speech_guard(&scratch.unique_name(".wav"))?;

    // Repeated passes consume one immutable copy, not a caller-replaceable input.
    write_new(&snapshot.output, &source_bytes)?;
    let raw = measure(
        program,
        &snapshot.output,
        ANALYSIS,
        start,
        "raw measurement",
    )?;
    let first_filter = format!("{CONDITIONING},{ANALYSIS}");
    let conditioned = measure(
        program,
        &snapshot.output,
        &first_filter,
        start,
        "conditioned first pass",
    )?;
    let second_filter = second_pass_filter(&conditioned.input);
    let mut render = command(program, &snapshot.output, &second_filter);
    render
        .args([
            "-ar",
            "24000",
            "-ac",
            "1",
            "-c:a",
            "pcm_s16le",
            "-f",
            "wav",
            "-fs",
        ])
        .arg(MAX_WAV_BYTES.to_string())
        .arg(&staged.output);
    let rendered = parse_loudnorm(&execute(&mut render, start, "measured second pass")?)
        .map_err(|error| format!("mastering second pass: {error}"))?;

    let mut final_bytes = paths::read_bounded(&staged.output, MAX_WAV_BYTES)?;
    let artist = crate::audio_metadata::read_artist(&source_bytes)?;
    let mut metadata = json!({});
    if let Some(artist) = artist {
        final_bytes = crate::audio_metadata::with_artist(&final_bytes, &artist)?;
        if final_bytes.len() > MAX_WAV_BYTES { return Err("Tagged master exceeds byte limit".into()); }
        fs::write(&staged.output, &final_bytes).map_err(|_| "Failed to preserve master Artist metadata")?;
        metadata["artist"] = json!(artist);
        for suffix in [".json", ".synth.json"] {
            let name = format!("{}{}", request.input.trim_end_matches(".wav"), suffix);
            if let Ok(path) = scratch.open_input(&name) {
                if let Ok(report) = paths::read_bounded(&path, 512 * 1024) {
                    if let Ok(value) = serde_json::from_slice::<Value>(&report) {
                        if let Ok(names) = serde_json::from_value::<Vec<String>>(value["metadata"]["agentNames"].clone()) {
                            if crate::audio_metadata::for_agents(&names).ok().as_ref() == Some(&value["metadata"])
                                && value["metadata"]["artist"] == metadata["artist"] {
                                metadata = value["metadata"].clone(); break;
                            }
                        }
                    }
                }
            }
        }
    }
    validate_rendered(&final_bytes, samples)?;
    // loudnorm's output_* values precede PCM encoding/resampling; do not use them
    // as evidence about the final deliverable or confuse sample peak with dBTP.
    let final_pass = measure(
        program,
        &staged.output,
        ANALYSIS,
        start,
        "final independent measurement",
    )?;
    validate_loudness(&final_pass.input)?;
    if paths::read_bounded(&staged.output, MAX_WAV_BYTES)? != final_bytes {
        return Err("mastering output changed during loudness verification".into());
    }
    if paths::read_bounded(&scratch.open_input(&request.input)?, MAX_WAV_BYTES)? != source_bytes
        || paths::read_bounded(&snapshot.output, MAX_WAV_BYTES)? != source_bytes
    {
        return Err("mastering input changed during processing; no output published".into());
    }
    remaining(start)?;

    let manifest_name = basename(&destination.manifest)?;
    let result = json!({
        "ok": true,
        "processedAudio": true,
        "verifiedAudio": true,
        "synthesizedSpeech": false,
        "wav": request.output,
        "manifest": manifest_name,
        "sample_rate": rate,
        "samples": samples,
        "duration_s": samples as f64 / rate as f64,
        "sha256": sha256_hex(&final_bytes),
        "sourceSha256": source_hash,
        "metadata": metadata,
        "report": {
            "profile": "prototype-master-v1",
            "target": {
                "integrated_lufs": TARGET_I,
                "true_peak_dbtp": TARGET_TP,
                "loudness_range_lu": TARGET_LRA
            },
            "raw": raw.input,
            "conditioned": conditioned.input,
            "final": final_pass.input,
            "normalization_type": rendered.normalization_type,
            "linear_requested": true,
            "chain": [
                {"filter": "highpass", "frequency_hz": 60},
                {"filter": "lowpass", "frequency_hz": 11000},
                {"filter": "acompressor", "threshold_db": -24, "ratio": 2.5,
                 "attack_ms": 15, "release_ms": 180, "makeup_linear": 2},
                {"filter": "loudnorm", "passes": 2}
            ],
            "validation": {
                "independent_final_measurement": true,
                "integrated_lufs_tolerance_lu": I_TOLERANCE,
                "true_peak_max_dbtp": TARGET_TP + TP_TOLERANCE,
                "loudness_range_max_lu": TARGET_LRA + LRA_TOLERANCE,
                "sample_count_preserved": true,
                "speech_alignment_verified": false
            }
        }
    });
    let mut manifest = result.clone();
    manifest["schema_version"] = json!("1.0");
    manifest["kind"] = json!("mastered-audio");
    manifest["source"] = json!({
        "wav": request.input,
        "sha256": source_hash,
        "sample_rate": rate,
        "samples": samples
    });
    let report_bytes =
        serde_json::to_vec_pretty(&manifest).map_err(|_| "mastering report encoding failed")?;
    if report_bytes.len() > MAX_REPORT_BYTES {
        return Err("mastering report exceeds byte limit".into());
    }
    write_new(&staged.manifest, &report_bytes)?;
    let persisted = paths::read_bounded(&staged.manifest, MAX_REPORT_BYTES)?;
    if persisted != report_bytes
        || serde_json::from_slice::<Value>(&persisted).ok().as_ref() != Some(&manifest)
    {
        return Err("mastering report failed persistence verification".into());
    }
    remaining(start)?;
    publish(scratch, &staged, &destination)?;
    Ok(result)
}

fn basename(path: &Path) -> Result<&str, String> {
    path.file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| "invalid mastering artifact name".into())
}

fn validate_wav(bytes: &[u8]) -> Result<(u32, u64), String> {
    let (rate, samples) = wav_metadata(bytes).map_err(|_| {
        "mastering requires a non-silent mono 24000 Hz PCM16 WAV (not empty or malformed)"
            .to_string()
    })?;
    if bytes.len() > MAX_WAV_BYTES || rate != SAMPLE_RATE || samples == 0 || samples > MAX_SAMPLES {
        return Err("mastering WAV exceeds the 600-second mono 24000 Hz PCM16 limit".into());
    }
    Ok((rate, samples))
}

fn validate_rendered(bytes: &[u8], source_samples: u64) -> Result<(), String> {
    let (_, samples) = validate_wav(bytes)?;
    if samples != source_samples {
        return Err("mastering changed the sample count or produced truncated audio".into());
    }
    Ok(())
}

fn validate_loudness(measured: &Measurement) -> Result<(), String> {
    if ![
        measured.integrated_lufs,
        measured.true_peak_dbtp,
        measured.loudness_range_lu,
        measured.threshold_lufs,
        measured.target_offset_lu,
    ]
    .iter()
    .all(|value| value.is_finite())
    {
        return Err("mastering loudness is non-finite; silence cannot be normalized".into());
    }
    if (measured.integrated_lufs - TARGET_I).abs() > I_TOLERANCE
        || measured.true_peak_dbtp > TARGET_TP + TP_TOLERANCE
        || measured.loudness_range_lu < 0.0
        || measured.loudness_range_lu > TARGET_LRA + LRA_TOLERANCE
    {
        return Err(format!(
            "mastering final loudness failed verification: {:.2} LUFS, {:.2} dBTP, {:.2} LU LRA",
            measured.integrated_lufs, measured.true_peak_dbtp, measured.loudness_range_lu
        ));
    }
    Ok(())
}

fn second_pass_filter(measured: &Measurement) -> String {
    format!(
        "{CONDITIONING},loudnorm=I=-16:TP=-1.5:LRA=4:measured_I={}:measured_TP={}:\
         measured_LRA={}:measured_thresh={}:offset={}:linear=true:print_format=json",
        measured.integrated_lufs,
        measured.true_peak_dbtp,
        measured.loudness_range_lu,
        measured.threshold_lufs,
        measured.target_offset_lu
    )
}

fn command(program: &OsStr, input: &Path, filter: &str) -> Command {
    let mut command = Command::new(program);
    command.args([
        "-hide_banner",
        "-nostdin",
        "-nostats",
        "-loglevel",
        "info",
        "-n",
        "-filter_threads",
        "1",
        "-threads",
        "1",
        "-protocol_whitelist",
        "file,pipe",
        "-f",
        "wav",
        "-i",
    ]);
    command.arg(input).args([
        "-map",
        "0:a:0",
        "-vn",
        "-sn",
        "-dn",
        "-map_metadata",
        "-1",
        "-threads",
        "1",
        "-af",
        filter,
    ]);
    command
}

fn remaining(start: Instant) -> Result<Duration, String> {
    DEADLINE
        .checked_sub(start.elapsed())
        .filter(|time| !time.is_zero())
        .ok_or_else(|| "mastering exceeded its total 120-second deadline".into())
}

fn execute(command: &mut Command, start: Instant, phase: &str) -> Result<Vec<u8>, String> {
    let output = subprocess::run(
        command,
        &[],
        Limits {
            deadline: remaining(start)?,
            stdout: 1024,
            stderr: 64 * 1024,
            stdin: 0,
        },
    )
    .map_err(|error| format!("mastering {phase}: FFmpeg {error}"))?;
    if !output.status.success() {
        // FFmpeg stderr can include input metadata or absolute local paths.
        return Err(format!(
            "mastering {phase}: FFmpeg failed (exit {:?}); check GEN_AUDIO_FFMPEG and support \
             for highpass, lowpass, acompressor and loudnorm",
            output.status.code()
        ));
    }
    Ok(output.stderr)
}

fn measure(
    program: &OsStr,
    path: &Path,
    filter: &str,
    start: Instant,
    phase: &str,
) -> Result<Loudnorm, String> {
    let mut command = command(program, path, filter);
    command.args(["-f", "null", "-"]);
    parse_loudnorm(&execute(&mut command, start, phase)?)
        .map_err(|error| format!("mastering {phase}: {error}"))
}

fn parse_loudnorm(stderr: &[u8]) -> Result<Loudnorm, String> {
    let start = stderr
        .iter()
        .rposition(|byte| *byte == b'{')
        .ok_or("FFmpeg loudnorm did not return measurement JSON")?;
    let end = stderr[start..]
        .iter()
        .position(|byte| *byte == b'}')
        .ok_or("FFmpeg loudnorm returned incomplete measurement JSON")?
        + start;
    let report: Value = serde_json::from_slice(&stderr[start..=end])
        .map_err(|_| "FFmpeg loudnorm returned invalid measurement JSON")?;
    let number = |field: &str, min: f64, max: f64| -> Result<f64, String> {
        let value = match report.get(field) {
            Some(Value::String(text)) => text.parse::<f64>().ok(),
            Some(Value::Number(number)) => number.as_f64(),
            _ => None,
        }
        .ok_or_else(|| format!("FFmpeg loudnorm missing or invalid {field}"))?;
        if !value.is_finite() {
            return Err(format!(
                "FFmpeg loudnorm {field} is non-finite; silence cannot be normalized"
            ));
        }
        if !(min..=max).contains(&value) {
            return Err(format!(
                "FFmpeg loudnorm {field} is outside its supported range"
            ));
        }
        Ok(value)
    };
    let input = Measurement {
        integrated_lufs: number("input_i", -99.0, 0.0)?,
        true_peak_dbtp: number("input_tp", -99.0, 99.0)?,
        loudness_range_lu: number("input_lra", 0.0, 99.0)?,
        threshold_lufs: number("input_thresh", -99.0, 0.0)?,
        target_offset_lu: number("target_offset", -99.0, 99.0)?,
    };
    for (field, min, max) in [
        ("output_i", -99.0, 0.0),
        ("output_tp", -99.0, 99.0),
        ("output_lra", 0.0, 99.0),
        ("output_thresh", -99.0, 0.0),
    ] {
        number(field, min, max)?;
    }
    let normalization_type = report["normalization_type"]
        .as_str()
        .filter(|value| matches!(*value, "linear" | "dynamic"))
        .ok_or("FFmpeg loudnorm returned an unsupported normalization_type")?
        .to_string();
    Ok(Loudnorm {
        input,
        normalization_type,
    })
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| "mastering could not create a reserved artifact")?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| "mastering could not persist a reserved artifact".into())
}

struct Publication {
    created: Vec<PathBuf>,
    committed: bool,
}

impl Drop for Publication {
    fn drop(&mut self) {
        if !self.committed {
            for path in &self.created {
                if let Err(error) = fs::remove_file(path) {
                    if error.kind() != std::io::ErrorKind::NotFound {
                        eprintln!("mastering publication cleanup failed: {}", error.kind());
                    }
                }
            }
        }
    }
}

fn publish(
    scratch: &Scratch,
    staged: &SpeechReservation<'_>,
    destination: &SpeechReservation<'_>,
) -> Result<(), String> {
    let mut publication = Publication {
        created: Vec::new(),
        committed: false,
    };
    for (from, to) in [
        (&staged.output, &destination.output),
        (&staged.manifest, &destination.manifest),
    ] {
        // A same-directory hard link publishes a whole file without clobbering an
        // existing name (rename would replace it on Unix). Both stay reserved.
        fs::hard_link(from, to)
            .map_err(|_| "mastering could not publish its reserved artifacts")?;
        publication.created.push(to.clone());
    }
    // No measurement or fallible report construction follows capability issuance.
    // If either adoption fails, the publication guard removes both linked files.
    scratch.adopt_file(basename(&destination.manifest)?)?;
    scratch.adopt_file(basename(&destination.output)?)?;
    publication.committed = true;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mastering_report_preserves_fractional_pcm_durations_through_json() {
        // Sample-aligned durations are often repeating decimals. Report
        // verification must not reject intact bytes after a JSON round trip.
        for samples in 240_000..241_000 {
            let manifest = json!({"sample_rate":24_000,"samples":samples,"duration_s":samples as f64 / 24_000.0});
            let bytes = serde_json::to_vec_pretty(&manifest).unwrap();
            assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap(), manifest, "sample count {samples}");
        }
    }

    struct Work(Scratch);

    impl Work {
        fn new() -> Self {
            Self(Scratch::create().unwrap())
        }
    }

    impl Drop for Work {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0.dir);
        }
    }

    // Synthetic DSP fixture only: this is not speech and proves no synthesis.
    fn non_speech_tone(seconds: u32, amplitude: f64) -> Vec<u8> {
        let samples = SAMPLE_RATE * seconds;
        let data_len = samples * 2;
        let mut wav = Vec::with_capacity(data_len as usize + 44);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
        wav.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        for index in 0..samples {
            let time = index as f64 / SAMPLE_RATE as f64;
            let sample =
                (amplitude * (time * 440.0 * std::f64::consts::TAU).sin() * 32767.0).round() as i16;
            wav.extend_from_slice(&sample.to_le_bytes());
        }
        wav
    }

    fn issued_tone(scratch: &Scratch) -> Vec<u8> {
        let bytes = non_speech_tone(8, 0.1);
        let reservation = scratch.reserve_speech_guard("non-speech.wav").unwrap();
        write_new(&reservation.output, &bytes).unwrap();
        scratch.adopt_file("non-speech.wav").unwrap();
        bytes
    }

    fn loudnorm_json() -> Value {
        json!({
            "input_i": "-26.00", "input_tp": "-20.00", "input_lra": "2.50",
            "input_thresh": "-36.00", "output_i": "-16.00", "output_tp": "-10.00",
            "output_lra": "2.50", "output_thresh": "-26.00",
            "normalization_type": "linear", "target_offset": "0.10"
        })
    }

    fn parsed(value: &Value) -> Result<Loudnorm, String> {
        parse_loudnorm(&serde_json::to_vec(value).unwrap())
    }

    #[test]
    fn arguments_are_strict_scratch_wav_names() {
        let work = Work::new();
        for args in [
            json!(null),
            json!([]),
            json!({}),
            json!({"input": 2}),
            json!({"input":"x.wav", "output":null}),
            json!({"input":"x.wav", "filter":"anull"}),
            json!({"input":"x.wav", "ffmpeg":"cmd.exe"}),
            json!({"input":"x.wav", "profile":"custom"}),
            json!({"input":"x.json"}),
            json!({"input":"x.wav", "output":"x.wav"}),
            json!({"input":"x.wav", "output":"X.wav"}),
        ] {
            assert!(Request::parse(&work.0, &args).is_err(), "{args}");
        }
        for name in [
            "",
            "../x.wav",
            r"..\x.wav",
            "/x.wav",
            r"C:\x.wav",
            r"\\server\x.wav",
            "https://host/x.wav",
            "x.wav:stream",
            ".hidden.wav",
            "NUL.wav",
            "x;whoami.wav",
        ] {
            assert!(
                Request::parse(&work.0, &json!({"input":name})).is_err(),
                "{name}"
            );
            assert!(
                Request::parse(&work.0, &json!({"input":"x.wav","output":name})).is_err(),
                "{name}"
            );
        }
        let a = Request::parse(&work.0, &json!({"input":"x.wav"})).unwrap();
        let b = Request::parse(&work.0, &json!({"input":"x.wav"})).unwrap();
        assert_ne!(a.output, b.output);
        assert!(a.output.ends_with(".wav"));
    }

    #[test]
    fn nonfinite_missing_and_invalid_measurements_fail_closed() {
        for field in [
            "input_i",
            "input_tp",
            "input_lra",
            "input_thresh",
            "target_offset",
            "output_i",
            "output_tp",
            "output_lra",
            "output_thresh",
        ] {
            for invalid in [
                json!("NaN"),
                json!("inf"),
                json!("-inf"),
                json!(null),
                json!({}),
                json!(true),
                json!("1:evil"),
            ] {
                let mut report = loudnorm_json();
                report[field] = invalid;
                assert!(parsed(&report).is_err(), "{field}");
            }
            let mut report = loudnorm_json();
            report.as_object_mut().unwrap().remove(field);
            assert!(parsed(&report).is_err());
        }
        for stderr in [b"no json".as_slice(), b"{", b"{bad}"] {
            assert!(parse_loudnorm(stderr).is_err());
        }
        let mut report = loudnorm_json();
        report["input_lra"] = json!(-1);
        assert!(parsed(&report).is_err());
        report = loudnorm_json();
        report["normalization_type"] = json!("unknown");
        assert!(parsed(&report).is_err());
    }

    #[test]
    fn measurements_drive_second_pass_and_preserve_dynamic_fallback() {
        let pass = parsed(&loudnorm_json()).unwrap();
        let filter = second_pass_filter(&pass.input);
        for part in [
            CONDITIONING,
            "measured_I=-26",
            "measured_TP=-20",
            "measured_LRA=2.5",
            "measured_thresh=-36",
            "offset=0.1",
            "linear=true",
            "print_format=json",
        ] {
            assert!(filter.contains(part), "{filter}");
        }
        let mut report = loudnorm_json();
        report["normalization_type"] = json!("dynamic");
        report["input_lra"] = json!(8.0);
        let parsed = parsed(&report).unwrap();
        assert_eq!(parsed.normalization_type, "dynamic");
        assert_eq!(parsed.input.loudness_range_lu, 8.0);
        let logged = format!("metadata with {{braces}}\n[Parsed_loudnorm_0]\n{report}\n");
        assert_eq!(
            parse_loudnorm(logged.as_bytes())
                .unwrap()
                .normalization_type,
            "dynamic"
        );
    }

    #[test]
    fn independent_quality_gate_checks_true_peak_lufs_and_lra() {
        let mut measured = parsed(&loudnorm_json()).unwrap().input;
        measured.integrated_lufs = -16.0;
        measured.true_peak_dbtp = -1.5;
        assert!(validate_loudness(&measured).is_ok());
        for i in [-17.01, -14.99, f64::NAN] {
            let mut invalid = measured.clone();
            invalid.integrated_lufs = i;
            assert!(validate_loudness(&invalid).is_err());
        }
        for tp in [-1.29, 0.0, f64::INFINITY] {
            let mut invalid = measured.clone();
            invalid.true_peak_dbtp = tp;
            assert!(validate_loudness(&invalid).is_err());
        }
        for lra in [-1.0, 5.01, f64::NAN] {
            let mut invalid = measured.clone();
            invalid.loudness_range_lu = lra;
            assert!(validate_loudness(&invalid).is_err());
        }
    }

    #[test]
    fn pcm_contract_rejects_silence_wrong_format_and_truncation() {
        let wav = non_speech_tone(1, 0.1);
        assert_eq!(validate_wav(&wav).unwrap(), (24000, 24000));
        assert!(validate_rendered(&wav, 23999).is_err());
        assert!(validate_wav(&wav[..wav.len() - 2]).is_err());
        assert!(validate_wav(&non_speech_tone(1, 0.0))
            .unwrap_err()
            .contains("non-silent"));
        for (index, value) in [(20, 3), (22, 2), (24, 0), (34, 32)] {
            let mut invalid = wav.clone();
            invalid[index] = value;
            assert!(validate_wav(&invalid).is_err());
        }
        assert!(validate_wav(&non_speech_tone(121, 0.1)).is_ok());
        assert!(validate_wav(&non_speech_tone(601, 0.1)).is_err());
    }

    #[test]
    fn input_must_be_issued_and_outputs_must_be_unused() {
        let work = Work::new();
        write_new(&work.0.dir.join("unissued.wav"), &non_speech_tone(1, 0.1)).unwrap();
        assert!(master(&work.0, &json!({"input":"unissued.wav"})).is_err());
        issued_tone(&work.0);
        let reservation = work.0.reserve_speech_guard("existing.wav").unwrap();
        assert!(master(
            &work.0,
            &json!({"input":"non-speech.wav","output":"existing.wav"})
        )
        .is_err());
        drop(reservation);
        let existing = work.0.prepare_output("existing.json").unwrap();
        fs::write(&existing, b"keep this").unwrap();
        assert!(master(
            &work.0,
            &json!({"input":"non-speech.wav","output":"existing.wav"})
        )
        .is_err());
        assert_eq!(fs::read(existing).unwrap(), b"keep this");
    }

    #[test]
    fn subprocess_failure_releases_every_reservation_without_issuing_outputs() {
        let work = Work::new();
        let source = issued_tone(&work.0);
        let missing = work.0.dir.join("ffmpeg-does-not-exist");
        let error = master_with_program(
            &work.0,
            &json!({"input":"non-speech.wav","output":"retry.wav"}),
            missing.as_os_str(),
        )
        .unwrap_err();
        assert!(error.contains("FFmpeg"), "{error}");
        assert_eq!(
            fs::read(work.0.open_input("non-speech.wav").unwrap()).unwrap(),
            source
        );
        work.0.adopt_new_files().unwrap();
        assert!(work.0.open_input("retry.wav").is_err());
        assert!(work.0.open_input("retry.json").is_err());
        assert_eq!(fs::read_dir(&work.0.dir).unwrap().count(), 1);
        assert!(work.0.reserve_speech_guard("retry.wav").is_ok());
    }

    #[test]
    fn publication_failure_removes_unissued_link_and_allows_retry() {
        let work = Work::new();
        let destination = work.0.reserve_speech_guard("retry.wav").unwrap();
        let staged = work.0.reserve_speech_guard("stage.wav").unwrap();
        write_new(&staged.output, &non_speech_tone(1, 0.1)).unwrap();
        // A missing staged report fails after linking the WAV, before issuance.
        assert!(publish(&work.0, &staged, &destination).is_err());
        assert!(!destination.output.exists());
        assert!(work.0.open_input("retry.wav").is_err());
        drop(staged);
        drop(destination);
        work.0.adopt_new_files().unwrap();
        assert!(work.0.open_input("retry.wav").is_err());
        assert_eq!(fs::read_dir(&work.0.dir).unwrap().count(), 0);
        assert!(work.0.reserve_speech_guard("retry.wav").is_ok());
    }

    #[test]
    fn command_is_shell_free_read_only_input_and_bounded() {
        let command = command(OsStr::new("ffmpeg"), Path::new("issued.wav"), ANALYSIS);
        assert_eq!(command.get_program(), "ffmpeg");
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy())
            .collect();
        assert!(args.contains(&"-n".into()));
        assert!(!args.contains(&"-y".into()));
        assert!(args.windows(2).any(|pair| pair == ["-i", "issued.wav"]));
        assert!(args.windows(2).any(|pair| pair == ["-af", ANALYSIS]));
        assert!(remaining(Instant::now()).unwrap() <= Duration::from_secs(120));
        assert!(remaining(Instant::now() - Duration::from_secs(121)).is_err());
    }

    #[test]
    fn ffmpeg_integration_masters_non_speech_tone_and_rejects_unmeasurable_audio() {
        let program = ffmpeg_program().unwrap();
        let mut probe = Command::new(&program);
        probe.args(["-hide_banner", "-version"]);
        let probe = subprocess::run(
            &mut probe,
            &[],
            Limits {
                deadline: Duration::from_secs(10),
                stdout: 64 * 1024,
                stderr: 64 * 1024,
                stdin: 0,
            },
        );
        if let Err(error) = &probe {
            if std::env::var_os("GEN_AUDIO_FFMPEG").is_none()
                && error.starts_with("subprocess failed to start:")
            {
                eprintln!("skipping mastering integration: FFmpeg is not on PATH");
                return;
            }
        }
        assert!(probe.expect("configured FFmpeg must run").status.success());
        let work = Work::new();
        let source = issued_tone(&work.0);
        let result = master_with_program(
            &work.0,
            &json!({"input":"non-speech.wav","output":"master.wav"}),
            &program,
        )
        .unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["processedAudio"], true);
        assert_eq!(result["verifiedAudio"], true);
        assert_eq!(result["synthesizedSpeech"], false);
        assert_eq!(result["sample_rate"], SAMPLE_RATE);
        assert_eq!(result["samples"], 8 * SAMPLE_RATE);
        assert_eq!(result["duration_s"], 8.0);
        assert_eq!(result["sourceSha256"], sha256_hex(&source));
        assert_eq!(
            fs::read(work.0.open_input("non-speech.wav").unwrap()).unwrap(),
            source
        );
        let final_bytes = fs::read(work.0.open_input("master.wav").unwrap()).unwrap();
        assert_eq!(result["sha256"], sha256_hex(&final_bytes));
        assert_ne!(result["sha256"], result["sourceSha256"]);
        assert_eq!(
            wav_metadata(&final_bytes).unwrap(),
            (SAMPLE_RATE, 8 * SAMPLE_RATE as u64)
        );
        let final_measurement = measure(
            &program,
            &work.0.open_input("master.wav").unwrap(),
            ANALYSIS,
            Instant::now(),
            "integration independent measurement",
        )
        .unwrap();
        validate_loudness(&final_measurement.input).unwrap();
        assert_eq!(
            result["report"]["final"]["true_peak_dbtp"],
            final_measurement.input.true_peak_dbtp
        );
        let report = fs::read(work.0.open_input("master.json").unwrap()).unwrap();
        let manifest: Value = serde_json::from_slice(&report).unwrap();
        assert_eq!(manifest["schema_version"], "1.0");
        assert_eq!(manifest["kind"], "mastered-audio");
        assert_eq!(manifest["source"]["wav"], "non-speech.wav");
        assert_eq!(manifest["sha256"], result["sha256"]);
        assert_eq!(manifest["report"], result["report"]);
        assert!(!String::from_utf8(report)
            .unwrap()
            .contains(&work.0.dir.to_string_lossy().to_string()));
        assert_eq!(fs::read_dir(&work.0.dir).unwrap().count(), 3);
        assert!(master_with_program(
            &work.0,
            &json!({"input":"non-speech.wav","output":"master.wav"}),
            &program
        )
        .is_err());

        // PCM is nonzero but below the EBU gate: never fabricate finite LUFS.
        let quiet = non_speech_tone(8, 0.0001);
        let quiet_reservation = work.0.reserve_speech_guard("quiet.wav").unwrap();
        write_new(&quiet_reservation.output, &quiet).unwrap();
        work.0.adopt_file("quiet.wav").unwrap();
        let error = master_with_program(
            &work.0,
            &json!({"input":"quiet.wav","output":"retry.wav"}),
            &program,
        )
        .unwrap_err();
        assert!(error.contains("non-finite"), "{error}");
        assert_eq!(
            fs::read(work.0.open_input("quiet.wav").unwrap()).unwrap(),
            quiet
        );
        work.0.adopt_new_files().unwrap();
        assert!(work.0.open_input("retry.wav").is_err());
        assert!(work.0.open_input("retry.json").is_err());
        assert_eq!(fs::read_dir(&work.0.dir).unwrap().count(), 4);
        assert!(work.0.reserve_speech_guard("retry.wav").is_ok());
    }
}

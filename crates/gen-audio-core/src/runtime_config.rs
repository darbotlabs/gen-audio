//! Local installation settings. RPC arguments never select runtime paths.
use serde_json::Value;
use std::{ffi::OsString, fs, path::PathBuf};

pub fn setting(key: &str) -> Option<OsString> {
    if let Some(value) = std::env::var_os(key).filter(|v| !v.is_empty()) {
        return Some(value);
    }
    if !key.starts_with("GEN_AUDIO_") && key != "GENAID_MODEL_BASE" {
        return None;
    }
    let file = std::env::var_os("GEN_AUDIO_CONFIG")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("LOCALAPPDATA")
                .map(|base| PathBuf::from(base).join("Gen-Audio/runtime.json"))
        })?;
    let metadata = fs::symlink_metadata(&file).ok()?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 65536 {
        return None;
    }
    let doc: Value = serde_json::from_slice(&fs::read(file).ok()?).ok()?;
    doc.get(key)?
        .as_str()
        .filter(|s| !s.is_empty())
        .map(OsString::from)
}

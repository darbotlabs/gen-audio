//! Path rules for untrusted tool arguments.
//!
//! Writes stay as a single file name inside the process work directory.
//! Reads of repo files are relative, reject `..`, and must canonicalize
//! back inside the repo. Symlinks that escape are rejected.

use std::fs;
use std::path::{Component, Path, PathBuf};

const WRITE_SUFFIXES: &[&str] = &[".wav", ".json", ".png"];

pub fn make_work_dir() -> std::io::Result<PathBuf> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("gen-audio-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&dir)?;
    Ok(dir.canonicalize()?)
}

pub fn find_repo_root() -> Option<PathBuf> {
    if let Ok(raw) = std::env::var("GEN_AUDIO_REPO") {
        let path = PathBuf::from(raw);
        if is_repo(&path) {
            return path.canonicalize().ok();
        }
    }
    let mut cur = std::env::current_dir().ok()?;
    loop {
        if is_repo(&cur) {
            return cur.canonicalize().ok();
        }
        if !cur.pop() {
            return None;
        }
    }
}

fn is_repo(path: &Path) -> bool {
    path.join("pyproject.toml").is_file() && path.join("src").join("gen_audio").is_dir()
}

pub fn write_name(work: &Path, name: &str) -> Result<PathBuf, String> {
    let file_name = single_component(name)?;
    if !WRITE_SUFFIXES.iter().any(|suffix| file_name.ends_with(suffix)) {
        return Err("output name must end in .wav, .json, or .png".into());
    }
    let root = work
        .canonicalize()
        .map_err(|err| format!("work directory: {err}"))?;
    let path = root.join(&file_name);
    if path.parent().and_then(|p| p.canonicalize().ok()).as_deref() != Some(root.as_path()) {
        return Err("output escaped the work directory".into());
    }
    Ok(path)
}

pub fn read_name_in_work(work: &Path, name: &str) -> Result<PathBuf, String> {
    let path = write_name(work, name)?;
    if !path.is_file() {
        return Err(format!("file is not in the work directory: {name}"));
    }
    Ok(path)
}

pub fn read_repo_relative(repo: &Path, raw: &str) -> Result<PathBuf, String> {
    let root = repo
        .canonicalize()
        .map_err(|err| format!("repo: {err}"))?;
    let joined = push_relative(&root, raw)?;
    let canon = joined
        .canonicalize()
        .map_err(|_| format!("repo path does not exist: {raw}"))?;
    if !canon.starts_with(&root) {
        return Err("repo path escapes the repository".into());
    }
    if !canon.is_file() {
        return Err(format!("not a file: {raw}"));
    }
    Ok(canon)
}

fn push_relative(base: &Path, raw: &str) -> Result<PathBuf, String> {
    if raw.is_empty() || raw.contains('\0') {
        return Err("path is empty".into());
    }
    let rel = Path::new(raw);
    if rel.is_absolute() {
        return Err("absolute paths are not accepted from tool arguments".into());
    }
    let mut out = base.to_path_buf();
    for component in rel.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir => return Err("path must not contain ..".into()),
            _ => return Err("unsupported path component".into()),
        }
    }
    Ok(out)
}

fn single_component(name: &str) -> Result<String, String> {
    if name.is_empty()
        || name.contains('\0')
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
    {
        return Err("file name must be a single path component".into());
    }
    if name.starts_with('.') {
        return Err("file name must not be hidden or relative".into());
    }
    if !name
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.')
    {
        return Err("file name has unsupported characters".into());
    }
    Ok(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_traversal_and_absolute_paths() {
        let work = make_work_dir().unwrap();
        assert!(write_name(&work, "../x.wav").is_err());
        assert!(write_name(&work, "a/b.wav").is_err());
        assert!(write_name(&work, "ok.wav").is_ok());
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let repo = repo.canonicalize().unwrap();
        assert!(read_repo_relative(&repo, "examples/podcast_script_sample.txt").is_ok());
        assert!(read_repo_relative(&repo, "../Cargo.toml").is_err());
        assert!(read_repo_relative(&repo, "/etc/passwd").is_err());
        let _ = fs::remove_dir_all(&work);
    }
}

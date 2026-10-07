//! `Speaker N` parser used by the harness for planning.
//!
//! The Python parser in `gen_audio.cast` is what synthesis uses. This copy
//! only plans traces. Keep the header rule aligned with that module.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Turn {
    pub speaker_id: String,
    pub text: String,
    pub script_name: Option<String>,
    pub line: usize,
}

pub fn parse_script(text: &str) -> Result<Vec<Turn>, String> {
    let mut turns = Vec::new();
    let mut speaker_id: Option<String> = None;
    let mut script_name: Option<String> = None;
    let mut start_line = 0usize;
    let mut parts: Vec<String> = Vec::new();

    let flush = |turns: &mut Vec<Turn>,
                 speaker_id: &mut Option<String>,
                 script_name: &mut Option<String>,
                 start_line: &mut usize,
                 parts: &mut Vec<String>|
     -> Result<(), String> {
        let Some(id) = speaker_id.take() else {
            return Ok(());
        };
        let body = parts
            .iter()
            .map(|part| part.trim())
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let body = body.split_whitespace().collect::<Vec<_>>().join(" ");
        if body.is_empty() {
            return Err(format!("Speaker {id} at line {start_line} has no text"));
        }
        turns.push(Turn {
            speaker_id: id,
            text: body,
            script_name: script_name.take(),
            line: *start_line,
        });
        *start_line = 0;
        parts.clear();
        Ok(())
    };

    for (offset, raw) in text.lines().enumerate() {
        let line_no = offset + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(header) = parse_header(line) {
            flush(
                &mut turns,
                &mut speaker_id,
                &mut script_name,
                &mut start_line,
                &mut parts,
            )?;
            speaker_id = Some(header.0);
            script_name = header.1;
            start_line = line_no;
            if let Some(inline) = header.2 {
                parts.push(inline);
            }
            continue;
        }
        if speaker_id.is_none() {
            return Err(format!("line {line_no} is outside a Speaker turn: {line}"));
        }
        parts.push(line.to_string());
    }
    flush(
        &mut turns,
        &mut speaker_id,
        &mut script_name,
        &mut start_line,
        &mut parts,
    )?;
    if turns.is_empty() {
        return Err("script contains no Speaker turns".into());
    }
    Ok(turns)
}

fn parse_header(line: &str) -> Option<(String, Option<String>, Option<String>)> {
    let rest = line.strip_prefix("Speaker ").or_else(|| line.strip_prefix("speaker "))?;
    let id_end = rest.find(|ch: char| !ch.is_ascii_digit()).unwrap_or(rest.len());
    if id_end == 0 {
        return None;
    }
    let id = rest[..id_end].parse::<u32>().ok()?.to_string();
    let after_id = rest[id_end..].trim_start();
    let (name, after_name) = if let Some(stripped) = after_id.strip_prefix('(') {
        let end = stripped.find(')')?;
        let name = stripped[..end].trim();
        let name = if name.is_empty() {
            None
        } else {
            Some(name.to_string())
        };
        (name, stripped[end + 1..].trim_start())
    } else {
        (None, after_id)
    };
    let after_name = after_name.strip_prefix(':')?.trim();
    let inline = if after_name.is_empty() {
        None
    } else {
        Some(after_name.to_string())
    };
    Some((id, name, inline))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sample_shape() {
        let text = "# SAMPLE\nSpeaker 1: Hello there.\nSpeaker 2 (Frank):\nSecond line.\n";
        let turns = parse_script(text).unwrap();
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].text, "Hello there.");
        assert_eq!(turns[1].script_name.as_deref(), Some("Frank"));
        assert_eq!(turns[1].text, "Second line.");
    }
}

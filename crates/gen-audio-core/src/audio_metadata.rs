//! Artist attribution is checked against the WAV, not just a JSON claim.
use serde_json::{json, Value};

pub fn for_agents(names: &[String]) -> Result<Value, String> {
    if names.len() > 8 { return Err("At most eight Artist agent names are supported".into()); }
    let mut artists = Vec::new();
    for name in names {
        if name.trim().is_empty() || name.trim() != name || name.chars().count() > 128
            || name.len() > 512 || name.chars().any(char::is_control) {
            return Err("Invalid Artist agent name".into());
        }
        if !artists.contains(name) { artists.push(name.clone()); }
    }
    if artists.join("; ").len() > 4095 { return Err("Combined Artist exceeds byte limit".into()); }
    Ok(if artists.is_empty() { json!({}) } else {
        json!({"artist":artists.join("; "),"artists":artists,"agentNames":artists})
    })
}

fn chunks(bytes: &[u8]) -> Result<Vec<(&[u8], &[u8], &[u8])>, String> {
    let mut result = Vec::new();
    let mut at = 0usize;
    while at < bytes.len() {
        if bytes.len() - at < 8 { return Err("Truncated RIFF chunk".into()); }
        let size = u32::from_le_bytes(bytes[at+4..at+8].try_into().unwrap()) as usize;
        let end = at.checked_add(8).and_then(|n| n.checked_add(size)).ok_or("RIFF size overflow")?;
        let padded = end.checked_add(size & 1).ok_or("RIFF size overflow")?;
        if padded > bytes.len() { return Err("RIFF chunk exceeds container".into()); }
        result.push((&bytes[at..at+4], &bytes[at+8..end], &bytes[at..padded]));
        at = padded;
    }
    Ok(result)
}

fn wave(bytes: &[u8]) -> Result<&[u8], String> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE"
        || u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as u64 + 8 != bytes.len() as u64 {
        return Err("Artist metadata requires a complete RIFF WAV".into());
    }
    Ok(&bytes[12..])
}

pub fn read_artist(bytes: &[u8]) -> Result<Option<String>, String> {
    let mut artist = None;
    for (kind, data, _) in chunks(wave(bytes)?)? {
        if kind == b"LIST" && data.starts_with(b"INFO") {
            for (tag, value, _) in chunks(&data[4..])? {
                if tag == b"IART" {
                    if artist.is_some() || value.len() > 4096 || value.last() != Some(&0) {
                        return Err("Invalid or ambiguous WAV Artist metadata".into());
                    }
                    let text = std::str::from_utf8(&value[..value.len()-1]).map_err(|_| "Artist must be UTF-8")?;
                    if text.is_empty() || text.chars().any(char::is_control) { return Err("Invalid WAV Artist".into()); }
                    artist = Some(text.to_string());
                }
            }
        }
    }
    Ok(artist)
}

pub fn verify(bytes: &[u8], metadata: &Value, names: &[String]) -> Result<(), String> {
    let expected = for_agents(names)?;
    if metadata != &expected || read_artist(bytes)?.as_deref() != expected["artist"].as_str() {
        return Err("WAV Artist attribution disagrees with the requested agents".into());
    }
    Ok(())
}

pub fn with_artist(bytes: &[u8], artist: &str) -> Result<Vec<u8>, String> {
    if artist.is_empty() || artist.len() > 4095 || artist.chars().any(char::is_control) { return Err("Invalid WAV Artist".into()); }
    let mut body = b"WAVE".to_vec();
    for (kind, data, encoded) in chunks(wave(bytes)?)? {
        if kind == b"LIST" && data.starts_with(b"INFO") {
            let mut other = b"INFO".to_vec();
            for (tag, _, chunk) in chunks(&data[4..])? { if tag != b"IART" { other.extend_from_slice(chunk); } }
            if other.len() > 4 { body.extend_from_slice(b"LIST"); body.extend_from_slice(&(other.len() as u32).to_le_bytes()); body.extend(other); }
        } else { body.extend_from_slice(encoded); }
    }
    let value = [artist.as_bytes(), &[0]].concat();
    let mut info = b"INFOIART".to_vec();
    info.extend_from_slice(&(value.len() as u32).to_le_bytes()); info.extend(value);
    if info.len() % 2 != 0 { info.push(0); }
    body.extend_from_slice(b"LIST"); body.extend_from_slice(&(info.len() as u32).to_le_bytes()); body.extend(info);
    let size = u32::try_from(body.len()).map_err(|_| "Tagged WAV exceeds RIFF limit")?;
    let mut result = b"RIFF".to_vec(); result.extend_from_slice(&size.to_le_bytes()); result.extend(body);
    if read_artist(&result)?.as_deref() != Some(artist) { return Err("Artist verification failed".into()); }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tagging_preserves_samples_and_rejects_ambiguity() {
        let raw = include_bytes!("../../../voices/references/en-Alice_woman.wav");
        let tagged = with_artist(raw, "Alice; Frank").unwrap();
        assert_eq!(crate::bridge::wav_metadata(raw).unwrap(), crate::bridge::wav_metadata(&tagged).unwrap());
        let data = |wav: &[u8]| chunks(wave(wav).unwrap()).unwrap().into_iter().find(|(kind,_,_)| *kind == b"data").unwrap().1.to_vec();
        assert_eq!(data(raw), data(&tagged));
        let names = vec!["Alice".into(), "Frank".into()];
        verify(&tagged, &for_agents(&names).unwrap(), &names).unwrap();
        assert!(verify(&tagged, &json!({"artist":"Kokoro"}), &names).is_err());
        assert_eq!(with_artist(&tagged, "Nova").unwrap().len(), with_artist(raw, "Nova").unwrap().len());
        assert!(read_artist(&tagged[..tagged.len()-1]).is_err());
    }
}

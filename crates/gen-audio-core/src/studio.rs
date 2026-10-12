//! Desktop VibeVoice request adapter: persona order, managed acoustic references
//! and bounded worker requests share one contract across GUI, MCP and ACP.
use crate::{catalog, model_worker, paths::Scratch, voice_library};
use serde_json::{json, Value};
use std::{collections::HashSet, path::Path};

pub fn request(repo: &Path, scratch: &Scratch, args: &Value) -> Result<Value, String> {
    if args["voice"] != "vibevoice" {
        return Err("This studio adapter requires VibeVoice script TTS".into());
    }
    let agents = args["agents"].as_array().ok_or("agents must be an array")?;
    if agents.is_empty() || agents.len() > 4 {
        return Err("VibeVoice supports 1–4 selected personas".into());
    }
    let mut seen = HashSet::new();
    let explicit = args
        .get("referenceIds")
        .map(|v| v.as_array().ok_or("referenceIds must be an array"))
        .transpose()?;
    if explicit.is_some_and(|refs| refs.len() != agents.len()) {
        return Err("Choose one managed reference per persona".into());
    }
    let mut refs = Vec::new();
    for (i, person) in agents.iter().enumerate() {
        let person = person.as_str().ok_or("persona id must be a string")?;
        if catalog::persona(person).is_none() || !seen.insert(person) {
            return Err("Choose distinct known personas".into());
        }
        let reference = explicit
            .and_then(|r| r[i].as_str())
            .or_else(|| voice_library::reference_for(person, "vibevoice"))
            .ok_or_else(|| format!("Choose a VibeVoice reference for {person}"))?;
        if explicit.is_some_and(|r| !r[i].is_string()) {
            return Err("referenceIds must contain strings".into());
        }
        refs.push(voice_library::prepare(repo, scratch, reference)?["wav"].clone());
    }
    let text = args["scriptText"].as_str().ok_or("Supply scriptText")?;
    if text.chars().count() > 8192 || text.len() > 32768 {
        return Err("VibeVoice scripts are limited to 8192 characters / 32768 UTF-8 bytes; nothing was truncated".into());
    }
    let has_turn = text
        .lines()
        .any(|line| line.trim().to_ascii_lowercase().starts_with("speaker "));
    let normalized = if !has_turn && agents.len() == 1 {
        format!(
            "Speaker 1: {}",
            text.split_whitespace().collect::<Vec<_>>().join(" ")
        )
    } else {
        crate::script::parse_script(text)?
            .into_iter()
            .map(|turn| format!("Speaker {}: {}", turn.speaker_id, turn.text))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let names = agents.iter().map(|agent| catalog::persona(agent.as_str().unwrap()).unwrap().name).collect::<Vec<_>>();
    let mut request = json!({"engine":"vibevoice","scriptText":normalized,"voiceRefs":refs,"agentNames":names,
        "maxSeconds":args.get("maxSeconds").cloned().unwrap_or(json!(30.0)),
        "maxNewTokens":args.get("maxNewTokens").cloned().unwrap_or(json!(512)),
        "seed":args.get("seed").cloned().unwrap_or(json!(0))});
    if let Some(output) = args.get("output") {
        request["output"] = output.clone();
    }
    if let Some(execution) = args.get("execution") {
        crate::execution::Execution::parse(execution)?;
        request["execution"] = execution.clone();
    }
    Ok(request)
}
pub fn preflight(repo: &Path, scratch: &Scratch, args: &Value) -> Result<Value, String> {
    let mut req = request(repo, scratch, args)?;
    req.as_object_mut().unwrap().remove("output");
    model_worker::preflight(Some(repo), scratch, &req)
}
pub fn generate(repo: &Path, scratch: &Scratch, args: &Value) -> Result<Value, String> {
    let req = request(repo, scratch, args)?;
    let mut result = model_worker::generate(Some(repo), scratch, &req)?;
    result["personas"] = args["agents"].clone();
    result["profileVersion"] = json!("2.0");
    result["referenceIds"] = args.get("referenceIds").cloned().unwrap_or_else(|| {
        json!(args["agents"]
            .as_array()
            .unwrap()
            .iter()
            .map(|agent| voice_library::reference_for(agent.as_str().unwrap(), "vibevoice"))
            .collect::<Vec<_>>())
    });
    result["sourceTextSha256"] = json!(crate::asset::sha256_hex(
        args["scriptText"].as_str().unwrap_or("").as_bytes()
    ));
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_mapping_and_library_hashes_are_required() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let work = Scratch::create().unwrap();
        let req = request(
            &root,
            &work,
            &json!({"voice":"vibevoice","agents":["alice","frank"],
            "scriptText":"Speaker 1 (Alice): Hello.\ncontinued words\nSpeaker 2: Hi."}),
        )
        .unwrap();
        assert_eq!(
            req["scriptText"],
            "Speaker 1: Hello. continued words\nSpeaker 2: Hi."
        );
        assert_eq!(req["voiceRefs"].as_array().unwrap().len(), 2);
        assert!(request(
            &root,
            &work,
            &json!({"voice":"vibevoice","agents":["nova"],"scriptText":"Hello"})
        )
        .is_err());
        assert!(request(&root,&work,&json!({"voice":"vibevoice","agents":["alice"],"scriptText":"Hello","referenceIds":["../private"]})).is_err());
        let req=request(&root,&work,&json!({"voice":"vibevoice","agents":["nova"],"scriptText":"Hello","referenceIds":["en-Carter_man"]})).unwrap();
        assert_eq!(req["scriptText"], "Speaker 1: Hello");
        for entry in std::fs::read_dir(&work.dir).unwrap().flatten() {
            std::fs::remove_file(entry.path()).unwrap();
        }
        std::fs::remove_dir(&work.dir).unwrap();
    }
}

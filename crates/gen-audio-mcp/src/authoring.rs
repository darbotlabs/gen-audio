//! Local authoring plus explicitly confirmed vendor drafting. No speech is generated here.

use gen_audio_connectors::Completion;
use gen_audio_core::authoring::{self, DraftRequest};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Vendor { Gpt, Claude, Copilot, Gemini }

impl Vendor {
    fn id(&self) -> &'static str {
        match self { Self::Gpt => "gpt", Self::Claude => "claude", Self::Copilot => "copilot", Self::Gemini => "gemini" }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Preview {
    request: DraftRequest,
    connector: Vendor,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Generate {
    request: DraftRequest,
    connector: Vendor,
    prompt_sha256: String,
    confirmed: bool,
}

pub fn handles(name: &str) -> bool {
    matches!(name, "script_analyze" | "draft_plan" | "draft_generate")
}

pub fn call(name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "script_analyze" => authoring::analyze(args),
        "draft_plan" => {
            let request: Preview = serde_json::from_value(args.clone()).map_err(|e| format!("invalid draft preview: {e}"))?;
            let mut result = serde_json::to_value(authoring::plan_draft(&request.request)?).map_err(|e| e.to_string())?;
            result["connector"] = json!(request.connector.id());
            result["externalRequestSent"] = json!(false);
            result["synthesizedSpeech"] = json!(false);
            Ok(result)
        }
        "draft_generate" => generate_with(args, |id, prompt, tokens| {
            gen_audio_connectors::complete_one_with_limit(id, prompt, tokens).map_err(|e| e.to_string())
        }),
        _ => Err("unknown authoring operation".into()),
    }
}

fn generate_with(
    args: &Value,
    complete: impl FnOnce(&str, &str, u32) -> Result<Completion, String>,
) -> Result<Value, String> {
    let request: Generate = serde_json::from_value(args.clone()).map_err(|e| format!("invalid draft generation: {e}"))?;
    let plan = authoring::plan_draft(&request.request)?;
    if !request.confirmed || request.prompt_sha256 != plan.prompt_sha256 {
        return Err("Preview and explicitly confirm the exact current draft prompt before sending.".into());
    }
    let result = complete(request.connector.id(), &plan.prompt, plan.max_output_tokens)?;
    if result.mock || result.connector_id != request.connector.id() {
        return Err("Drafting requires a configured live provider; mock/local output was not accepted.".into());
    }
    let draft = authoring::validate_draft(&request.request, &result.text)?;
    let mut output = serde_json::to_value(draft).map_err(|e| e.to_string())?;
    output["ok"] = json!(true);
    output["connector"] = json!(result.connector_id);
    output["model"] = json!(result.model);
    output["needsReview"] = json!(true);
    output["synthesizedSpeech"] = json!(false);
    Ok(output)
}

pub fn definitions() -> Vec<Value> {
    let agents = json!({"type":"array","items":{"type":"string","maxLength":64},"minItems":1,"maxItems":8,"uniqueItems":true});
    let duration = json!({"type":"integer","minimum":1,"maximum":15});
    let wpm = json!({"type":"integer","minimum":1,"maximum":600});
    let request = json!({
        "type":"object","additionalProperties":false,"required":["topic","agents","durationMinutes"],
        "properties":{
            "topic":{"type":"string","minLength":1,"maxLength":1000},
            "agents":agents,"durationMinutes":duration,"wordsPerMinute":wpm,
            "language":{"enum":["en"],"default":"en"},
            "style":{"enum":["conversational","interview","explainer"],"default":"conversational"},
            "existingScript":{"type":"string","minLength":1,"maxLength":4000}
        }
    });
    let vendor = json!({"type":"string","enum":["gpt","claude","copilot","gemini"]});
    vec![
        json!({
            "name":"script_analyze",
            "description":"Normalize and analyze supplied dialogue locally. Explicit one_based, zero_based or alternating import; ordered persona mapping; separate warnings and word-estimated duration. No model or network request.",
            "annotations":{"readOnlyHint":true},
            "inputSchema":{"type":"object","additionalProperties":false,"required":["scriptText","agents"],
                "properties":{"scriptText":{"type":"string","maxLength":262144},"agents":agents,
                    "indexing":{"enum":["one_based","zero_based","alternating"],"default":"one_based"},
                    "durationMinutes":duration,"wordsPerMinute":wpm}}
        }),
        json!({
            "name":"draft_plan","description":"Build the exact English draft/rewrite provider prompt locally. Show prompt, connector and promptSha256 to the user; no external request is sent.",
            "annotations":{"readOnlyHint":true},
            "inputSchema":{"type":"object","additionalProperties":false,"required":["request","connector"],
                "properties":{"request":request,"connector":vendor}}
        }),
        json!({
            "name":"draft_generate","description":"After explicit user review and confirmation, send the current draft_plan prompt to the selected configured live provider. Requires matching promptSha256 and confirmed=true. Rejects mocks and malformed dialogue; user must review before applying. Does not synthesize.",
            "annotations":{"readOnlyHint":false,"openWorldHint":true},
            "inputSchema":{"type":"object","additionalProperties":false,"required":["request","connector","promptSha256","confirmed"],
                "properties":{"request":request,"connector":vendor,"promptSha256":{"type":"string","pattern":"^[a-f0-9]{64}$"},"confirmed":{"const":true}}}
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> Value {
        json!({"topic":"Audio signals","agents":["alice","frank"],"durationMinutes":1,"style":"explainer"})
    }

    #[test]
    fn confirmation_is_bound_to_the_exact_prompt_before_any_call() {
        let preview = call("draft_plan", &json!({"request":request(),"connector":"gpt"})).unwrap();
        let mut args = json!({"request":request(),"connector":"gpt","promptSha256":preview["promptSha256"],"confirmed":true});
        args["confirmed"] = json!(false);
        assert!(generate_with(&args, |_,_,_| panic!("must not send")).is_err());
        args["confirmed"] = json!(true);
        args["request"]["topic"] = json!("Different topic after review");
        assert!(generate_with(&args, |_,_,_| panic!("must not send")).is_err());
        assert_eq!(preview["externalRequestSent"], false);
    }

    #[test]
    fn mock_and_invalid_dialogue_never_become_drafts() {
        let preview = call("draft_plan", &json!({"request":request(),"connector":"gpt"})).unwrap();
        let args = json!({"request":request(),"connector":"gpt","promptSha256":preview["promptSha256"],"confirmed":true});
        for (mock, text) in [(true,"Speaker 1: Hello.\nSpeaker 2: Hi."), (false,"Success! Here is your script.")] {
            assert!(generate_with(&args, |id,_,_| Ok(Completion {
                connector_id:id.into(),model:"test-only".into(),mock,text:text.into(),
            })).is_err());
        }
        let result = generate_with(&args, |id,prompt,tokens| {
            assert_eq!(prompt, preview["prompt"].as_str().unwrap());
            assert!((256..=4096).contains(&tokens));
            Ok(Completion { connector_id:id.into(),model:"test-only".into(),mock:false,text:"Speaker 1: Sound moves through air.\nSpeaker 2: A microphone records that sound.".into() })
        }).unwrap();
        assert_eq!(result["synthesizedSpeech"], false);
        assert_eq!(result["needsReview"], true);
        assert!(result["scriptText"].as_str().unwrap().starts_with("Speaker 1:"));
    }
}

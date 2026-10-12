//! Stateless Model Context Protocol server.
//!
//! `initialize` stores nothing. `tools/call` does not read a session map.
//! HTTP responses do not set `Mcp-Session-Id`. Stdio is newline-delimited JSON-RPC.

use std::io::{Read, Write};
use std::path::PathBuf;

use gen_audio_connectors::health_all;
use gen_audio_core::benchmark::{reference_rows, SOURCE_NOTE};
use gen_audio_core::bridge::{self, PythonTool};
use gen_audio_core::engines;
use gen_audio_core::fixture::{self, write_fixture_tone};
use gen_audio_core::model_worker;
use gen_audio_core::{generation_store,studio,voice_library};
use gen_audio_core::paths::{self, find_repo_root, Scratch};
use gen_audio_core::redact::redact_secrets;
use gen_audio_core::serve::{self, health_url, node_base_url, ready_url};
use serde_json::{json, Value};

#[derive(Clone, Debug)]
pub struct ToolError {
    pub code: i32,
    pub message: String,
    pub data: Option<Value>,
}

impl From<(i32, String)> for ToolError {
    fn from((code, message): (i32, String)) -> Self {
        Self {
            code,
            message,
            data: None,
        }
    }
}
mod authoring;

pub struct Server {
    pub scratch: Scratch,
    pub repo: Option<PathBuf>,
    /// HTTP rejections seen by this server's listener (served on /health).
    pub rejections: std::sync::Arc<http::RejectionStats>,
    /// Monotonic identity. A raw address can be recycled after the previous
    /// Server is dropped, so the local accept_loop pin compares this. Test
    /// builds only; the release binary does not carry the counter.
    #[cfg(test)]
    pub(crate) instance_id: u64,
}

#[cfg(test)]
static NEXT_SERVER_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

#[cfg(test)]
fn next_server_id() -> u64 {
    NEXT_SERVER_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl Server {
    pub fn boot() -> Self {
        let scratch = if let Ok(raw) = std::env::var("GEN_AUDIO_WORK_DIR") {
            Scratch::new(PathBuf::from(raw))
                .expect("GEN_AUDIO_WORK_DIR must be a dedicated directory outside the repository")
        } else {
            Scratch::create().expect("work directory")
        };
        Self {
            scratch,
            repo: find_repo_root(),
            rejections: Default::default(),
            #[cfg(test)]
            instance_id: next_server_id(),
        }
    }

    /// Fresh scratch for one remote HTTP request. Not shared with other clients.
    pub fn isolated() -> Self {
        Self {
            scratch: Scratch::create().expect("isolated work directory"),
            repo: find_repo_root(),
            rejections: Default::default(),
            #[cfg(test)]
            instance_id: next_server_id(),
        }
    }

    /// Fallible construction for bounded ACP sessions and transport startup.
    pub fn try_isolated() -> Result<Self, String> {
        Ok(Self {
            scratch: Scratch::create()?,
            repo: find_repo_root(),
            rejections: Default::default(),
            #[cfg(test)]
            instance_id: next_server_id(),
        })
    }
}

pub fn smoke() -> Result<String, String> {
    let server = Server::boot();
    let init = handle(
        &server,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
    )?
    .ok_or("initialize did not answer")?;
    if init.get("id").is_none() {
        return Err("initialize did not answer".into());
    }
    let listed = handle(
        &server,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    )?
    .ok_or("tools/list did not answer")?;
    let tools = listed["result"]["tools"]
        .as_array()
        .ok_or("tools/list missing tools")?;
    let names: Vec<_> = tools
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    for required in [
        "synth",
        "model_preflight",
        "model_generate",
        "improve",
        "spectrogram",
        "cube_revision",
        "serve_health",
        "connector_health",
        "fixture_tone",
        "list_connectors",
        "ui_navigate",
        "ui_select_tile",
        "ui_playback",
        "ui_seek_report",
        "ui_set_sidepane",
        "ui_generate",
        "library_list",
        "library_rename",
        "library_harvest",
        "voice_profile_get",
        "voice_profile_list",
        "ui_flip",
        "ui_compare",
        "viewport_get",
        "card_export",
        "cube_layers",
        "asset_resolve",
        "asset_list",
        "asset_glyph",
    ] {
        if !names.contains(&required) {
            return Err(format!("missing tool {required}"));
        }
    }
    let tone = handle(
        &server,
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"fixture_tone","arguments":{}}}),
    )?
    .ok_or("fixture tool did not answer")?;
    let text = tone["result"]["content"][0]["text"]
        .as_str()
        .ok_or("fixture tool returned no text")?;
    if !text.contains("notPodcast") {
        return Err("fixture tool did not label the tone".into());
    }
    Ok(format!("mcp smoke ok ({} tools)", names.len()))
}

pub fn handle(server: &Server, message: Value) -> Result<Option<Value>, String> {
    let Some(obj) = message.as_object() else {
        return Ok(Some(error_response(None, -32600, "request must be an object")));
    };
    if obj.get("id").is_some_and(|id| !gen_audio_core::subprocess::valid_rpc_id(id)) {
        return Ok(Some(error_response(None, -32600, "invalid request id")));
    }
    if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Ok(Some(error_response(
            obj.get("id").cloned(),
            -32600,
            "jsonrpc must be 2.0",
        )));
    }
    let id = obj.get("id").cloned();
    let notification = id.is_none();
    let Some(method) = obj.get("method").and_then(Value::as_str).filter(|s| !s.is_empty() && s.len() <= 128) else {
        return Ok(Some(error_response(id, -32600, "method must be a bounded string")));
    };
    let params = obj.get("params").cloned().unwrap_or(json!({}));
    if !params.is_object() {
        return Ok((!notification).then(|| error_response(id, -32602, "params must be an object")));
    }
    let result: Result<Value, ToolError> = match method {
        "initialize" => initialize_result(&params).map_err(ToolError::from),
        "notifications/initialized" | "initialized" => {
            return Ok(None);
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tool_defs()})),
        "tools/call" => call_tool(server, &params),
        _ => Err(ToolError::from((
            -32601,
            format!("method not found: {method}"),
        ))),
    };
    if notification {
        return Ok(None);
    }
    match result {
        Ok(value) => Ok(Some(json!({"jsonrpc":"2.0","id": id, "result": value}))),
        Err(err) => Ok(Some(error_response_data(
            id,
            err.code,
            &err.message,
            err.data.as_ref(),
        ))),
    }
}

fn initialize_result(params: &Value) -> Result<Value, (i32, String)> {
    const SUPPORTED: &[&str] = &["2024-11-05", "2025-03-26"];
    let requested = match params.get("protocolVersion") {
        None => "2024-11-05",
        Some(version) => version.as_str().ok_or((-32602, "protocolVersion must be a string".into()))?,
    };
    if !SUPPORTED.contains(&requested) {
        return Err((-32602, format!("unsupported protocolVersion {requested}")));
    }
    Ok(json!({
        "protocolVersion": requested,
        "capabilities": {"tools": {"listChanged": false}},
        "serverInfo": {"name": "gen-audio", "version": env!("CARGO_PKG_VERSION")},
        "instructions": "Stateless Gen-Audio MCP. initialize stores no session. UI tools queue a loopback command and do not write speech. Agent ids are personas (anton, alice, khortana, rocky, and the rest of the catalog), not connector ids. Voice ids are TTS models (kokoro_onnx, kokoro_dayour, misaki, vibevoice, magpie, pocket_tts). af_heart is a Kokoro pack ref, not a Voice id. GET /health is liveness. GET /ready means this listener is up, not that a model is loaded. GET /control/stream is a short stateless SSE snapshot."
    }))
}

fn tool_defs() -> Vec<Value> {
    let mut tools = vec![
        tool("synth", "Synthesize the supplied script. Kokoro uses castMap or mapped personas; VibeVoice uses 1–4 personas and managed referenceIds through the isolated worker. No example fallback. Verified WAVs and manifests are saved as local ga1 assets."),
        tool("studio_preflight", "Check the complete VibeVoice desktop request, selected personas and managed voice references without loading model weights. Returns actual runtime/tokenizer/checkpoint/memory blockers."),
        tool("engine_load", "Load actual configured model weights into an owned resident Python process. Kokoro, VibeVoice and PersonaPlex are isolated runtimes; local paths come only from server configuration. Replaces the previous resident engine. loaded:true requires a live worker load receipt; it is not an inference claim."),
        tool("engine_status", "Inspect the resident model process and its verified load metadata; exited workers cannot remain loaded."),
        tool("engine_unload", "Terminate the owned model worker and release its resident memory. Busy generation must be cancelled first."),
        tool("voice_reference_list", "List migrated hash-pinned managed reference voices with PCM metadata and provenance; not generated speech."),
        tool("voice_reference_prepare", "Prepare a declared referenceId as a validated issued scratch WAV for model_generate; arbitrary paths are never accepted."),
        tool("generated_library_list", "List durable locally generated ga1 audio assets, including incomplete renders with explicit status."),
        tool("generated_library_open", "Verify and reopen a saved generation by full ga1 UID into this process scratch for authenticated playback/download."),
        tool("model_preflight", "Inspect isolated VibeVoice script-TTS or PersonaPlex speech-to-speech readiness without loading weights. Runtime/model/tokenizer paths come only from server configuration. Optional request fields are checked; omissions remain explicit blockers, never pretend-ready. Audio names must already be issued in this process scratch."),
        tool("model_generate", "Run an isolated, offline model worker with an optional execution policy (CPU, CUDA or ROCm); availability is verified in the selected runtime. VibeVoice requires scriptText and 1..4 voiceRefs; PersonaPlex requires inputAudio, persona and one voiceRef (conversation response, not verbatim TTS). All audio inputs are issued scratch WAV names, e.g. stock Kokoro synth outputs; never paths. Returns verified WAV/.synth.json names, hashes and metadata; transcript_verified is false and capped output is incomplete."),
        tool("improve", "Run the Python publish chain on a WAV already in the work directory."),
        tool("master_audio", "Create a separate measured podcast master from an issued mono 24 kHz PCM16 WAV (up to 600 seconds). Rust runs bounded FFmpeg two-pass normalization to -16 LUFS/-1.5 dBTP with a report. Original audio stays unchanged; this is processing, not speech synthesis."),
        tool("spectrogram", "Run the Python spectrogram helper on work-directory WAVs."),
        tool("cube_revision", "Run the Python inverse-HDR cube revision sketch on a work-directory WAV."),
        tool("semantic_schema_library", "Get versioned semantic cube and record JSON schemas, domain examples and resource limits for agent sessions, storyline, camera, stocks, crypto and custom metrics."),
        tool("semantic_cube_project", "Validate supplied semantic records and layer mappings natively and return normalized spatial points. Preserves missing values; reports clipping; rejects invalid ranges, references and OHLC candles. No network or market feed."),
        tool("serve_health", "Probe a configured genaid-audio host. With no host the result is unconfigured and is not a live probe."),
        tool("connector_health", "Health for one connector id or the full roster of seven."),
        tool("list_connectors", "List connector ids and modes."),
        tool("list_engines", "List compare-list engines and whether this repo can synthesize with them."),
        tool("fixture_tone", "Write a labeled sine WAV. Not speech and not a podcast."),
        tool("benchmark_reference", "Return 2026-10-06 compare figures. measuredHere is false."),
        tool("harness_plan", "Parse the sample script and return a harness-style plan. Does not synthesize."),
        tool("ui_navigate", "Queue a viewport slide change for the local Gen-Audio window. Does not render audio."),
        tool("ui_select_tile", "Queue selection of a card id. Loads a 3D cube only when that tile has cube JSON."),
        tool("ui_flip", "Queue a flipcard. next:true advances one face and wraps. face selects an applicable id, or the front/back aliases. A bare call still means back. Optional personaId attaches the voice-profile payload. Does not render audio."),
        tool("ui_playback", "Queue play, pause, or seek for a library tile. A seek waits (waitMs, default 2000) for the window and returns {requested_t, landed_t, ok, reason}. Does not open the WAV in this process."),
        tool("ui_seek_report", "Desktop window only: report where a queued ui_playback seek (bus seq) landed. Records the result ui_playback returns."),
        tool("ui_set_sidepane", "Queue Agent personas (max 8) and a Voice TTS model. Connector ids are rejected."),
        tool("ui_generate", "Start generation and return a job id. The clip lands in the library when the job reaches done. Unset model env vars refuse and invent no speech. cancel=true stops a running job."),
        tool("ui_compare", "Set the A/B compare list (max 2). Optional select is a user Play: focus and clock.source follow that side."),
        tool("ui_cube", "Set single/compare Cube tab mode for matching library/pipeline cubes of one WAV. Reads back through viewport_get; never changes the playback clock."),
        tool("viewport_get", "Return the reduced viewport: slides, views, focus, clock source, jobs. No webview."),
        tool("card_export", "Export facts() as an Adaptive Card 1.5 JSON document. The webview does not template cards."),
        tool("library_list", "List the library catalog. Does not open WAV bytes. Unavailable clips stay unavailable."),
        tool("library_rename", "Persist a semantic name and/or face name into the library catalog. Does not rewrite the WAV."),
        tool("library_harvest", "Propose a semantic name and face name from the filename and sidecar counts. apply writes a clip ref onto a persona. Does not decode the WAV."),
        tool("voice_profile_get", "Return one persona profile: tone, purpose, domain, accent, traits, refs. Not audio."),
        tool("voice_profile_list", "List personas and TTS voice models. LLM ids are connectors, not agents."),
        tool("cube_layers", "Read signal, tonality, confidence, and quality summaries from an allowlisted library cube JSON. Omits point clouds and absolute paths."),
        tool("asset_resolve", "Resolve a ga1 asset uid (or a ga:<kind>: prefix of 8+ chars) from the library catalog. Media come back as /library URLs plus sha256."),
        tool("asset_list", "Page through library assets in uid order. Optional kind; limit 1 to 100 (default 50); cursor is the last uid returned."),
        tool("asset_glyph", "Braille glyph, dot pattern, kind hue class and aria label for a ga1 uid. Visual hint only; resolve by uid."),
    ];
    tools.extend(authoring::definitions());
    tools
}

static UID_PROP: std::sync::LazyLock<Value> = std::sync::LazyLock::new(
    || json!({"type": "string", "description": "ga1 asset uid, ga:<kind>:<26 base32>", "maxLength": 44}),
);

fn tool(name: &str, description: &str) -> Value {
    let (mut properties, required) = match name {
        "semantic_cube_project" => (json!({"document":{"type":"object"},"offset":{"type":"integer","minimum":0,"maximum":32768},"limit":{"type":"integer","minimum":1,"maximum":2048,"default":2048}}), json!(["document"])),
        "synth" => (json!({
            "script":{"type":"string"}, "scriptText":{"type":"string","maxLength":262144},
            "castMap":{"type":"string"}, "agents":{"type":"array","items":{"type":"string"},"minItems":1,"maxItems":8},
            "voice":{"type":"string"}, "output":{"type":"string","maxLength":180},
            "referenceIds":{"type":"array","items":{"type":"string","maxLength":80},"minItems":1,"maxItems":4},
            "maxSeconds":{"type":"number","minimum":1,"maximum":60},
            "maxNewTokens":{"type":"integer","minimum":1,"maximum":1024},
            "seed":{"type":"integer","minimum":0,"maximum":4294967295u64}
        }), json!([])),
        "engine_load" => (json!({"engine":{"type":"string","enum":["kokoro_onnx","vibevoice","personaplex"]}}), json!(["engine"])),
        "engine_status" | "engine_unload" => (json!({}), json!([])),
        "studio_preflight" => (json!({
            "voice":{"const":"vibevoice"},"agents":{"type":"array","items":{"type":"string"},"minItems":1,"maxItems":4},
            "scriptText":{"type":"string","minLength":1,"maxLength":8192},
            "referenceIds":{"type":"array","items":{"type":"string","maxLength":80},"minItems":1,"maxItems":4},
            "maxSeconds":{"type":"number","minimum":1,"maximum":60},"maxNewTokens":{"type":"integer","minimum":1,"maximum":1024},
            "seed":{"type":"integer","minimum":0,"maximum":4294967295u64}
        }),json!(["voice","agents","scriptText"])),
        "voice_reference_prepare" => (json!({"referenceId":{"type":"string","maxLength":80}}),json!(["referenceId"])),
        "generated_library_open" => (json!({"uid":UID_PROP.clone()}),json!(["uid"])),
        "model_preflight" | "model_generate" => {
            let wav_name = json!({"type":"string","minLength":5,"maxLength":180,
                "pattern":"^[A-Za-z0-9][A-Za-z0-9_.-]*\\.wav$",
                "description":"Single issued scratch WAV name; no absolute paths, directories, traversal, or unissued files."});
            let mut properties = json!({
                "engine":{"type":"string","enum":["vibevoice","personaplex"]},
                "scriptText":{"type":"string","minLength":1,"maxLength":8192,
                    "description":"VibeVoice only: plain text with one reference, or contiguous Speaker 1..N lines matching voiceRefs order. At most 32768 UTF-8 bytes."},
                "inputAudio":wav_name.clone(),
                "persona":{"type":"string","minLength":1,"maxLength":8192,
                    "description":"PersonaPlex conversation persona, not words to read verbatim; at most 32768 UTF-8 bytes."},
                "voiceRefs":{"type":"array","items":wav_name,"minItems":1,"maxItems":4,
                    "description":"Mono 24 kHz PCM16 issued WAV names, at most 30 seconds each. PersonaPlex needs exactly one reference of at least 0.5 seconds."},
                "maxNewTokens":{"type":"integer","minimum":1,"maximum":1024,"default":256},
                "maxSeconds":{"type":"number","minimum":1,"maximum":60,"default":30,
                    "description":"Output duration cap, not a requested duration."},
                "seed":{"type":"integer","minimum":0,"maximum":4294967295u64,"default":0},
                "agentNames":{"type":"array","maxItems":8,"items":{"type":"string","minLength":1,"maxLength":128},
                    "description":"Agent display names recorded as Artist metadata in generated WAV and JSON."}
            });
            if name == "model_generate" {
                properties["output"] = json!({"type":"string","minLength":5,"maxLength":173,
                    "pattern":"^[A-Za-z0-9][A-Za-z0-9_.-]*\\.wav$",
                    "description":"Optional unique unused scratch WAV name; its .synth.json manifest is reserved with it."});
            }
            (properties, json!(["engine"]))
        },
        "improve" => (json!({"input": {"type": "string"}, "output": {"type": "string"}}), json!(["input", "output"])),
        "master_audio" => (json!({"input":{"type":"string","minLength":5,"maxLength":180},"output":{"type":"string","minLength":5,"maxLength":180}}),json!(["input"])),
        "cube_revision" => (json!({"input": {"type": "string"}, "output": {"type": "string"}, "maxSteps": {"type": "integer"}}), json!(["input", "output"])),
        "spectrogram" => (json!({"before": {"type": "string"}, "after": {"type": "string"}}), json!(["before"])),
        "serve_health" => (json!({"host": {"type": "string"}, "port": {"type": "integer", "minimum":1,"maximum":65535}, "probe": {"type": "boolean"}}), json!([])),
        "connector_health" => (json!({"id": {"type": "string"}}), json!([])),
        "ui_navigate" => (
            json!({
                "slide": {"type": "string", "description": "slide:<slug> (e.g. slide:library). A bare slug is a deprecated alias: it resolves and the result carries a deprecation note."},
                "tileId": {"type": "string"},
                "uid": UID_PROP.clone()
            }),
            json!(["slide"]),
        ),
        "ui_select_tile" => (json!({"tileId": {"type": "string"}, "uid": UID_PROP.clone()}), json!([])),
        "ui_flip" => (json!({"tileId": {"type": "string"}, "uid": UID_PROP.clone(), "view": {"type": "string"}, "next": {"type": "boolean"}, "flipped": {"type": "boolean"}, "face": {"type": "string"}, "section": {"type": "string"}, "personaId": {"type": "string"}}), json!([])),
        "ui_playback" => (json!({"tileId": {"type": "string"}, "uid": UID_PROP.clone(), "action": {"type": "string"}, "seconds": {"type": "number"}, "origin": {"type": "string", "enum": ["user", "auto"], "description": "play only; default user"}, "clientRequestId":{"type":"string","minLength":1,"maxLength":64,"pattern":"^[A-Za-z0-9-]+$","description":"Optional local playback correlation id; echoed in resulting control events"}, "waitMs": {"type": "integer", "minimum": 0, "maximum": 10000, "description": "seek only; default 2000"}}), json!(["action"])),
        "ui_seek_report" => (
            json!({"seq": {"type": "integer", "minimum": 1}, "requested_t": {"type": "number"}, "landed_t": {"type": ["number", "null"]}, "ok": {"type": "boolean"}, "reason": {"type": "string", "maxLength": 240}}),
            json!(["seq", "requested_t", "landed_t", "ok", "reason"]),
        ),
        "asset_resolve" | "asset_glyph" => (json!({"uid": UID_PROP.clone()}), json!(["uid"])),
        "asset_list" => (
            json!({"kind": {"type": "string"}, "cursor": {"type": "string"}, "limit": {"type": "integer", "minimum": 1, "maximum": 100}}),
            json!([]),
        ),
        "ui_set_sidepane" => (
            json!({"agents": {"type": "array"}, "voice": {"type": "string"}, "durationMin": {"type": "integer"}, "promptNote": {"type": "string"}}),
            json!(["agents", "voice"]),
        ),
        "ui_generate" => (
            json!({"agents": {"type": "array"}, "voice": {"type": "string"}, "durationMin": {"type": "integer"}, "duration_s": {"type": "number"}, "prompt": {"type": "string"}, "promptNote": {"type": "string"}, "prompt_ref": {"type": "string"}, "focus": {"type": "boolean"}, "cancel": {"type": "boolean"}, "job": {"type": "string"}}),
            json!(["agents", "voice"]),
        ),
        "ui_compare" => (
            json!({"uids": {"type": "array"}, "select": {"type": "string"}}),
            json!(["uids"]),
        ),
        "ui_cube" => (json!({"mode":{"type":"string","enum":["single","compare"]},"tileId":{"type":"string"},"uid":UID_PROP.clone()}),json!(["mode"])),
        "card_export" => (
            json!({"uid": {"type": "string"}, "format": {"type": "string"}}),
            json!(["uid"]),
        ),
        "library_rename" => (
            json!({"clipId": {"type": "string"}, "semanticName": {"type": "string"}, "faceName": {"type": "string"}}),
            json!(["clipId"]),
        ),
        "library_harvest" => (
            json!({"clipId": {"type": "string"}, "personaId": {"type": "string"}, "apply": {"type": "boolean"}}),
            json!(["clipId"]),
        ),
        "cube_layers" => (json!({"clipId": {"type": "string"}}), json!(["clipId"])),
        "voice_profile_get" => (
            json!({"personaId": {"type": "string"}, "agentName": {"type": "string"}}),
            json!(["personaId"]),
        ),
        _ => (json!({}), json!([])),
    };
    if matches!(name, "synth" | "studio_preflight" | "model_preflight" | "model_generate" | "engine_load") {
        properties["execution"] = gen_audio_core::execution::schema();
    }
    let mut definition = json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "additionalProperties": false,
            "properties": properties,
            "required": required
        }
    });
    if name == "synth" {
        definition["inputSchema"]["allOf"] = json!([
            {"oneOf":[{"required":["script"],"not":{"required":["scriptText"]}},{"required":["scriptText"],"not":{"required":["script"]}}]},
            {"oneOf":[{"required":["castMap"],"not":{"anyOf":[{"required":["agents"]},{"required":["voice"]}]}},
                {"required":["agents","voice"],"not":{"required":["castMap"]}}]}
        ]);
    }
    if matches!(name, "model_preflight" | "model_generate") {
        let mut vibevoice = json!({"properties":{"engine":{"const":"vibevoice"}},
            "not":{"anyOf":[{"required":["inputAudio"]},{"required":["persona"]}]}});
        let mut personaplex = json!({"properties":{"engine":{"const":"personaplex"},"voiceRefs":{"maxItems":1}},
            "not":{"required":["scriptText"]}});
        if name == "model_generate" {
            vibevoice["required"] = json!(["scriptText", "voiceRefs"]);
            personaplex["required"] = json!(["inputAudio", "persona", "voiceRefs"]);
        }
        definition["inputSchema"]["oneOf"] = json!([vibevoice, personaplex]);
    }
    definition
}

pub fn call_tool(server: &Server, params: &Value) -> Result<Value, ToolError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or((-32602, "tools/call needs a name".into()))?;
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    if authoring::handles(name) {
        let payload = authoring::call(name, &args).map_err(|error| (-32602,error))?;
        return Ok(json!({"content":[{"type":"text","text":payload.to_string()}],"isError":false}));
    }
    known_arguments(name, &args)?;
    let payload = match name {
        "semantic_schema_library" => gen_audio_core::semantic_cube::library(),
        "semantic_cube_project" => gen_audio_core::semantic_cube::project_page(&args["document"],args["offset"].as_u64().unwrap_or(0).min(32769) as usize,args["limit"].as_u64().unwrap_or(2048).min(2049) as usize).map_err(|e|(-32602,e))?,
        "engine_load" => gen_audio_core::runtime_owner::load_with_execution(server.repo.as_deref().ok_or((-32603,"Repository model adapters are not configured".into()))?, args["engine"].as_str().unwrap_or(""), args.get("execution")).map_err(|error|(-32602,error))?,
        "engine_status" => gen_audio_core::runtime_owner::status(),
        "engine_unload" => gen_audio_core::runtime_owner::unload().map_err(|error|(-32602,error))?,
        "synth" => synth(server, &args)?,
        "model_preflight" => model_worker::preflight(server.repo.as_deref(), &server.scratch, &args).map_err(|e| (-32602, e))?,
        "model_generate" => model_worker::generate(server.repo.as_deref(), &server.scratch, &args).map_err(|e| (-32602, e))?,
        "improve" => python_tool(server, PythonTool::Improve, &args)?,
        "master_audio" => gen_audio_core::mastering::master(&server.scratch, &args).map_err(|error|(-32602,error))?,
        "spectrogram" => python_tool(server, PythonTool::Spectrogram, &args)?,
        "cube_revision" => python_tool(server, PythonTool::Cube, &args)?,
        "serve_health" => serve_health(&args)?,
        "connector_health" => connector_health(&args)?,
        "list_connectors" => json!({"connectors": health_all()}),
        "list_engines" => {
            json!({"liveSynth": false, "weightsBundled": false, "engines": engines::engines()})
        }
        "fixture_tone" => fixture(server)?,
        "benchmark_reference" => {
            json!({"measuredHere": false, "sourceNote": SOURCE_NOTE, "rows": reference_rows()})
        }
        "harness_plan" => harness_plan(server)?,
        "ui_navigate" => control::ui_navigate(&args)?,
        "ui_select_tile" => control::ui_select_tile(&args)?,
        "ui_flip" => match control::ui_flip(&args) {
            Ok(value) => value,
            Err((code, message, data)) => return Err(ToolError { code, message, data }),
        },
        "ui_playback" => control::ui_playback(&args)?,
        "ui_seek_report" => control::ui_seek_report(&args)?,
        "ui_set_sidepane" => control::ui_set_sidepane(&args)?,
        "ui_generate" => control::ui_generate(&args)?,
        "ui_compare" => control::ui_compare(&args)?,
        "ui_cube" => control::ui_cube(&args)?,
        "viewport_get" => control::viewport_get(),
        "card_export" => control::card_export(&args)?,
        "library_list" => control::library_list(),
        "library_rename" => control::library_rename(&args)?,
        "library_harvest" => control::library_harvest(server.repo.as_deref(), &args)?,
        "cube_layers" => control::cube_layers(server.repo.as_deref(), &args)?,
        "voice_profile_get" => control::voice_profile_get(&args)?,
        "voice_profile_list" => control::voice_profile_list(),
        "studio_preflight" => {
            let repo=server.repo.as_deref().ok_or((-32603,"repository root not found".into()))?;
            studio::preflight(repo,&server.scratch,&args).map_err(|e|(-32602,e))?
        },
        "voice_reference_list" => voice_library::list(),
        "voice_reference_prepare" => {
            let repo=server.repo.as_deref().ok_or((-32603,"repository root not found".into()))?;
            voice_library::prepare(repo,&server.scratch,args["referenceId"].as_str().unwrap_or("")).map_err(|e|(-32602,e))?
        },
        "generated_library_list" => generation_store::list().map_err(|e|(-32603,e))?,
        "generated_library_open" => generation_store::reopen(&server.scratch,args["uid"].as_str().unwrap_or("")).map_err(|e|(-32602,e))?,
        "asset_resolve" => assets::asset_resolve(&args)?,
        "asset_list" => assets::asset_list(&args)?,
        "asset_glyph" => assets::asset_glyph(&args)?,
        _ => return Err(ToolError::from((-32602, format!("unknown tool {name}")))),
    };
    Ok(json!({
        "content": [{"type": "text", "text": payload.to_string()}],
        "isError": payload.get("ok").and_then(Value::as_bool) == Some(false)
    }))
}

/// Argument names each tool accepts. Must equal the tool's advertised
/// inputSchema properties (tested): the UI and agents speak one contract.
fn allowed_arguments(name: &str) -> &'static [&'static str] {
    match name {
        "semantic_cube_project" => &["document", "offset", "limit"],
        "synth" => &["script", "scriptText", "castMap", "agents", "voice", "output", "referenceIds", "maxSeconds", "maxNewTokens", "seed", "execution"],
        "studio_preflight" => &["scriptText","agents","voice","referenceIds","maxSeconds","maxNewTokens","seed","execution"],
        "voice_reference_prepare" => &["referenceId"],
        "generated_library_open" => &["uid"],
        "model_preflight" => &["engine", "scriptText", "inputAudio", "persona", "voiceRefs", "maxNewTokens", "maxSeconds", "seed", "agentNames", "execution"],
        "model_generate" => &["engine", "scriptText", "inputAudio", "persona", "voiceRefs", "maxNewTokens", "maxSeconds", "seed", "output", "agentNames", "execution"],
        "improve" => &["input", "output"],
        "master_audio" => &["input", "output"],
        "spectrogram" => &["before", "after"],
        "cube_revision" => &["input", "output", "maxSteps"],
        "serve_health" => &["host", "port", "probe"],
        "connector_health" => &["id"],
        "engine_load" => &["engine", "execution"],
        "engine_status" | "engine_unload" | "list_connectors" | "list_engines" | "fixture_tone" | "benchmark_reference" | "harness_plan" | "library_list" | "voice_profile_list" | "voice_reference_list" | "generated_library_list" => &[],
        "ui_navigate" => &["slide", "tileId", "uid"],
        "ui_select_tile" => &["tileId", "uid"],
        "ui_flip" => &["tileId", "uid", "view", "next", "flipped", "face", "section", "personaId"],
        "ui_playback" => &["tileId", "uid", "action", "seconds", "origin", "waitMs", "clientRequestId"],
        "ui_seek_report" => &["seq", "requested_t", "landed_t", "ok", "reason"],
        "asset_resolve" | "asset_glyph" => &["uid"],
        "asset_list" => &["kind", "cursor", "limit"],
        "ui_set_sidepane" => &["agents", "voice", "durationMin", "promptNote"],
        "ui_generate" => &[
            "agents",
            "voice",
            "durationMin",
            "duration_s",
            "prompt",
            "promptNote",
            "prompt_ref",
            "focus",
            "cancel",
            "job",
        ],
        "ui_compare" => &["uids", "select"],
        "ui_cube" => &["mode", "tileId", "uid"],
        "script_analyze" => &["scriptText", "agents", "indexing", "durationMinutes", "wordsPerMinute"],
        "draft_plan" => &["request", "connector"],
        "draft_generate" => &["request", "connector", "promptSha256", "confirmed"],
        "viewport_get" => &[],
        "card_export" => &["uid", "format"],
        "library_rename" => &["clipId", "semanticName", "faceName"],
        "library_harvest" => &["clipId", "personaId", "apply"],
        "cube_layers" => &["clipId"],
        "voice_profile_get" => &["personaId", "agentName"],
        _ => &[],
    }
}

fn known_arguments(name: &str, args: &Value) -> Result<(), (i32, String)> {
    let allowed = allowed_arguments(name);
    let obj = args
        .as_object()
        .ok_or((-32602, "arguments must be an object".to_string()))?;
    for key in obj.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err((-32602, format!("unexpected argument {key}")));
        }
        let value = &obj[key];
        let valid = match key.as_str() {
            "execution" => gen_audio_core::execution::Execution::parse(value).is_ok(),
            "document" => value.is_object(),
            "probe" | "flipped" | "apply" | "focus" | "cancel" | "next" | "ok" => value.is_boolean(),
            "port" | "maxSteps" | "limit" | "offset" | "durationMin" | "maxNewTokens" | "seed" | "waitMs" | "seq" => value.as_u64().is_some(),
            "seconds" | "maxSeconds" | "duration_s" | "requested_t" => value.as_f64().is_some_and(f64::is_finite),
            "landed_t" => value.is_null() || value.as_f64().is_some_and(f64::is_finite),
            "uids" => value.as_array().is_some_and(|a| a.len() <= 2 && a.iter().all(Value::is_string)),
            "agents" => value.as_array().is_some_and(|a| a.len() <= 8 && a.iter().all(|v| v.as_str().is_some_and(|s| s.len() <= 64))),
            "voiceRefs" => value.as_array().is_some_and(|a| !a.is_empty() && a.len() <= 4 && a.iter().all(|v| v.as_str().is_some_and(|s| s.len() <= 180))),
            "referenceIds" => value.as_array().is_some_and(|a| !a.is_empty() && a.len() <= 4 && a.iter().all(|v| v.as_str().is_some_and(|s| s.len() <= 80))),
            _ => value.as_str().is_some_and(|s| s.len() <= if matches!(key.as_str(), "scriptText" | "persona" | "prompt") { 256 * 1024 } else { 4096 }),
        };
        if !valid {
            return Err((-32602, format!("invalid type or size for {key}")));
        }
    }
    Ok(())
}

fn synth(server: &Server, args: &Value) -> Result<Value, (i32, String)> {
    if args.get("execution").is_some() && args["voice"] != "vibevoice" {
        return Err((-32602,"This synthesis adapter does not accept an explicit execution policy".into()));
    }
    if args["voice"]=="vibevoice" {
        let repo=server.repo.as_deref().ok_or((-32603,"repository root not found".into()))?;
        let _=control::note_progress("vibevoice","preflight","Checking managed voices, runtime, tokenizer and memory",false);
        let mut ran=studio::generate(repo,&server.scratch,args).map_err(|e|(-32602,e))?;
        let speech=ran["synthesizedSpeech"]==true;
        let phase=ran["phase"].as_str().unwrap_or("refused").to_owned();
        let detail=if speech {"VibeVoice WAV and request provenance verified; transcript not verified"}
            else {"VibeVoice preflight/generation blocked; inspect the reported errors"};
        ran["progress"]=control::note_progress("vibevoice",&phase,detail,speech);
        save_generation(server,&mut ran);
        return Ok(ran);
    }
    match server.repo.as_deref() {
        Some(repo) => match bridge::plan(PythonTool::Synth, repo, &server.scratch, args) {
            Err(message) if message.contains("No speech was invented") => {
                let progress = control::note_progress("kokoro_onnx", "refused", &message, false);
                Ok(json!({
                    "ok": false,
                    "synthesized": false,
                    "synthesizedSpeech": false,
                    "phase": "refused",
                    "reason": message,
                    "mock": true,
                    "progress": progress
                }))
            }
            Err(message) => Err((-32602, message)),
            Ok(plan) => {
                let _ = control::note_progress("kokoro_onnx", "running", "kokoro-onnx synth started", false);
                let mut ran = match bridge::run_plan(&plan) {
                    Ok(ran) => ran,
                    Err(message) => {
                        let progress = control::note_progress("kokoro_onnx", "failed", &message, false);
                        return Ok(json!({"ok":false,"synthesizedSpeech":false,"phase":"failed","reason":message,"progress":progress}));
                    }
                };
                let speech = ran.get("synthesizedSpeech").and_then(Value::as_bool) == Some(true);
                if speech {
                    for field in ["wav", "manifest"] {
                        let name = ran[field].as_str().ok_or((-32603, "verified artifact name missing".into()))?;
                        server.scratch.adopt_file(name).map_err(|e| (-32603, e))?;
                    }
                }
                let phase = if speech { "ok" } else { "refused" };
                let detail = if speech {
                    "Kokoro speech WAV and manifest verified against requested text, cast, metadata and SHA-256"
                } else {
                    "synth finished without a speech claim"
                };
                ran["phase"] = json!(phase);
                ran["progress"] = control::note_progress("kokoro_onnx", phase, detail, speech);
                ran["personas"] = args["agents"].clone();
                ran["profileVersion"] = json!("2.0");
                save_generation(server,&mut ran);
                Ok(ran)
            }
        },
        None => Ok(json!({
            "ok": false,
            "synthesized": false,
            "synthesizedSpeech": false,
            "reason": "repository root not found; no speech was invented",
            "mock": true
        })),
    }
}

fn save_generation(server:&Server,result:&mut Value) {
    if cfg!(test) {return;}
    if result["synthesizedSpeech"]!=true {return;}
    match generation_store::save(&server.scratch,result) {
        Ok(saved)=>{result["assetUid"]=saved["uid"].clone();result["librarySaved"]=json!(true);},
        Err(message)=>{result["librarySaved"]=json!(false);result["libraryError"]=json!(redact_secrets(&message));}
    }
}

fn python_tool(server: &Server, tool: PythonTool, args: &Value) -> Result<Value, (i32, String)> {
    let repo = server
        .repo
        .as_ref()
        .ok_or((-32603, "repository root not found".to_string()))?;
    let plan =
        bridge::plan(tool, repo, &server.scratch, args).map_err(|message| (-32602, message))?;
    let ran = bridge::run_plan(&plan).map_err(|message| (-32603, message))?;
    let _ = server.scratch.adopt_new_files();
    Ok(ran)
}

fn fixture(server: &Server) -> Result<Value, (i32, String)> {
    let tone = write_fixture_tone(&server.scratch).map_err(|err| (-32603, err))?;
    let meta = std::fs::read_to_string(&tone.sidecar).map_err(|err| (-32603, err.to_string()))?;
    let meta: Value = serde_json::from_str(&meta).map_err(|err| (-32603, err.to_string()))?;
    Ok(json!({
        "ok": true,
        "wav": fixture::FIXTURE_WAV_NAME,
        "sidecar": meta
    }))
}

fn serve_health(args: &Value) -> Result<Value, (i32, String)> {
    let port = args.get("port").and_then(Value::as_u64).unwrap_or(8002);
    if !(1..=65535).contains(&port) {
        return Err((-32602, "port must be from 1 to 65535".into()));
    }
    let port = port as u16;
    let host = args
        .get("host")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|host| !host.is_empty());
    let Some(host) = host else {
        return Ok(json!({
            "healthy": false,
            "probed": false,
            "status": "unconfigured",
            "note": "No host is configured. This is not a live genaid-audio probe."
        }));
    };
    let base = node_base_url(host, port).map_err(|err| (-32602, err))?;
    let health = health_url(host, port).map_err(|err| (-32602, err))?;
    let ready = ready_url(host, port).map_err(|err| (-32602, err))?;
    let probe = args.get("probe").and_then(Value::as_bool).unwrap_or(true);
    if probe {
        serve::probe_host_allowed(host).map_err(|err| (-32602, err))?;
    }
    if !probe {
        return Ok(json!({
            "healthy": false,
            "probed": false,
            "baseUrl": base,
            "healthUrl": health,
            "readyUrl": ready,
            "expectedBody": serve::health_payload(),
            "note": "URL only. /health is liveness. /ready is readiness. This is not a live genaid-audio probe."
        }));
    }
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(2))
        .timeout_read(std::time::Duration::from_secs(2))
        .timeout(std::time::Duration::from_secs(4))
        .redirects(0)
        .build();
    match agent.get(&health).call() {
        Ok(response) => {
            let mut body = Vec::new();
            response.into_reader().take(4097).read_to_end(&mut body).map_err(|_| (-32603, "health response read failed".into()))?;
            if body.len() > 4096 { return Err((-32603, "health response exceeds limit".into())); }
            let parsed: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            let service = parsed.get("service").and_then(Value::as_str);
            let status = parsed.get("status").and_then(Value::as_str);
            let matches = service == Some(serve::SERVICE_NAME);
            Ok(json!({
                "ok": matches,
                "healthy": matches,
                "probed": true,
                "baseUrl": base,
                "healthUrl": health,
                "readyUrl": ready,
                "service": service,
                "status": status
            }))
        }
        Err(err) => Ok(json!({
            "ok": false,
            "healthy": false,
            "probed": true,
            "baseUrl": base,
            "healthUrl": health,
            "error": redact_secrets(&err.to_string())
        })),
    }
}

fn connector_health(args: &Value) -> Result<Value, (i32, String)> {
    match args.get("id").and_then(Value::as_str) {
        Some(id) => {
            let report =
                gen_audio_connectors::health_one(id).map_err(|err| (-32602, err.to_string()))?;
            Ok(json!(report))
        }
        None => Ok(json!({"connectors": health_all()})),
    }
}

fn harness_plan(server: &Server) -> Result<Value, (i32, String)> {
    let repo = server
        .repo
        .as_ref()
        .ok_or((-32603, "repository root not found".to_string()))?;
    let text = std::fs::read_to_string(
        paths::read_user_repo_file(repo, "examples/podcast_script_sample.txt")
            .map_err(|err| (-32603, err))?,
    )
    .map_err(|err| (-32603, err.to_string()))?;
    let turns = gen_audio_core::script::parse_script(&text).map_err(|err| (-32602, err))?;
    Ok(json!({
        "ok": true,
        "synthesized": false,
        "turns": turns.len(),
        "speakers": turns.iter().map(|turn| &turn.speaker_id).collect::<Vec<_>>(),
        "next": "fixture_tone then improve. synth stays refused until model files are configured."
    }))
}

fn error_response(id: Option<Value>, code: i32, message: &str) -> Value {
    error_response_data(id, code, message, None)
}

fn error_response_data(id: Option<Value>, code: i32, message: &str, data: Option<&Value>) -> Value {
    let mut error = json!({"code": code, "message": redact_secrets(message)});
    if let Some(data) = data {
        error["data"] = data.clone();
    }
    json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Value::Null),
        "error": error
    })
}

pub fn stdio_loop(server: &Server) {
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let mut stdout = std::io::stdout().lock();
    loop {
        let line = match gen_audio_core::subprocess::read_line_bounded(&mut input, 1024 * 1024) {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(error) => {
                let _ = writeln!(stdout, "{}", error_response(None, -32600, &error));
                let _ = stdout.flush();
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => {
                let _ = writeln!(stdout, "{}", error_response(None, -32700, "parse error"));
                let _ = stdout.flush();
                continue;
            }
        };
        match handle(server, message) {
            Ok(Some(response)) => {
                let _ = writeln!(stdout, "{response}");
                let _ = stdout.flush();
            }
            Ok(None) => {}
            Err(err) => {
                let _ = writeln!(stdout, "{}", error_response(None, -32603, &err));
                let _ = stdout.flush();
            }
        }
    }
}

pub mod assets;
pub mod control;
pub mod http;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_library_and_projection_are_real_bounded_native_tools() {
        let server = Server::isolated();
        let call = |name: &str, args: Value| handle(&server,json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args}})).unwrap().unwrap();
        let response = call("semantic_schema_library",json!({}));
        let library: Value = serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(library["schemas"].as_array().unwrap().len(),2);
        let mut doc=library["examples"][0].clone();
        let response=call("semantic_cube_project",json!({"document":doc,"limit":1}));
        let projected:Value=serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(projected["points"].as_array().unwrap().len(),1);
        assert_eq!(projected["totalPoints"],48);
        doc["layers"][0]["color"]=json!("red");
        assert_eq!(call("semantic_cube_project",json!({"document":doc}))["error"]["code"],-32602);
        assert_eq!(call("semantic_cube_project",json!({"document":"bad"}))["error"]["code"],-32602);
    }

    #[test]
    fn model_tools_have_explicit_engine_and_capability_schemas() {
        let definitions = tool_defs();
        for name in ["model_preflight", "model_generate"] {
            let schema = &definitions.iter().find(|tool| tool["name"] == name).unwrap()["inputSchema"];
            assert_eq!(schema["additionalProperties"], false);
            assert_eq!(schema["required"], json!(["engine"]));
            assert_eq!(schema["properties"]["engine"]["enum"], json!(["vibevoice", "personaplex"]));
            assert_eq!(schema["properties"]["voiceRefs"]["items"]["type"], "string");
            assert_eq!(schema["properties"]["maxSeconds"]["maximum"], 60);
            assert_eq!(schema["properties"]["maxNewTokens"]["maximum"], 1024);
            assert_eq!(schema["oneOf"].as_array().unwrap().len(), 2);
            assert_eq!(schema["properties"].get("output").is_some(), name == "model_generate");
            assert!(schema["properties"].get("python").is_none());
            assert!(schema["properties"].get("modelDir").is_none());
        }
    }

    #[test]
    fn model_tools_reject_passthrough_paths_and_invalid_types() {
        let mut server = Server::isolated();
        server.repo = None;
        for args in [
            json!({}), json!({"engine":"kokoro_onnx"}), json!({"engine":"personaplex","scriptText":"TTS"}),
            json!({"engine":"vibevoice","python":"python.exe"}),
            json!({"engine":"vibevoice","modelDir":"C:\\models"}),
            json!({"engine":"vibevoice","voiceRefs":["..\\escape.wav"]}),
            json!({"engine":"vibevoice","voiceRefs":["C:\\voice.wav"]}),
            json!({"engine":"vibevoice","voiceRefs":["unissued.wav"]}),
            json!({"engine":"vibevoice","voiceRefs":[123]}),
            json!({"engine":"vibevoice","voiceRefs":[]}),
            json!({"engine":"vibevoice","maxSeconds":0}),
            json!({"engine":"vibevoice","maxSeconds":"30"}),
            json!({"engine":"vibevoice","maxNewTokens":1.5}),
            json!({"engine":"vibevoice","maxNewTokens":1025}),
            json!({"engine":"vibevoice","seed":4294967296u64}),
            json!({"engine":"vibevoice","persona":null}),
            json!({"engine":"vibevoice","scriptText":"x".repeat(8193)}),
            json!({"engine":"vibevoice","output":"no-preflight-output.wav"}),
        ] {
            let response = handle(&server, json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
                "params":{"name":"model_preflight","arguments":args}})).unwrap().unwrap();
            assert_eq!(response["error"]["code"], -32602, "{args}: {response}");
        }
        for args in [json!({"engine":"vibevoice"}), json!({"engine":"personaplex","persona":"Helpful"})] {
            assert!(call_tool(&server, &json!({"name":"model_generate","arguments":args})).is_err());
        }
        let _ = std::fs::remove_dir_all(&server.scratch.dir);
    }

    #[test]
    fn model_preflight_and_generate_return_truthful_unavailability() {
        let mut server = Server::isolated();
        server.repo = None;
        for (engine, missing) in [
            ("vibevoice", json!(["scriptText","voiceRefs"])),
            ("personaplex", json!(["inputAudio","persona","voiceRefs"])),
        ] {
            let response = call_tool(&server, &json!({"name":"model_preflight","arguments":{"engine":engine}})).unwrap();
            assert_eq!(response["isError"], true);
            let payload: Value = serde_json::from_str(response["content"][0]["text"].as_str().unwrap()).unwrap();
            assert_eq!(payload["status"], "unavailable");
            assert_eq!(payload["ready"], false);
            assert_eq!(payload["synthesizedSpeech"], false);
            assert_eq!(payload["missing_fields"], missing);
            assert!(payload.get("mock").is_none());
        }
        write_fixture_tone(&server.scratch).unwrap();
        for args in [
            json!({"engine":"vibevoice","scriptText":"Private request text","voiceRefs":[fixture::FIXTURE_WAV_NAME]}),
            json!({"engine":"personaplex","inputAudio":fixture::FIXTURE_WAV_NAME,"persona":"Private persona",
                "voiceRefs":[fixture::FIXTURE_WAV_NAME]}),
        ] {
            let response = call_tool(&server, &json!({"name":"model_generate","arguments":args})).unwrap();
            assert_eq!(response["isError"], true);
            let text = response["content"][0]["text"].as_str().unwrap();
            let payload: Value = serde_json::from_str(text).unwrap();
            assert_eq!(payload["synthesizedSpeech"], false);
            assert_eq!(payload["status"], "unavailable");
            assert!(payload["errors"].as_array().unwrap().iter().any(|e| e["code"] == "missing_worker"));
            assert!(!text.contains("Private"));
            assert!(!text.contains("\"wav\""));
        }
        let _ = std::fs::remove_dir_all(&server.scratch.dir);
    }

    #[test]
    fn calls_do_not_require_a_stored_session() {
        let server = Server::boot();
        let first = handle(
            &server,
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_connectors","arguments":{}}}),
        )
        .unwrap()
        .unwrap();
        assert!(first["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("mcp"));
        let second = handle(
            &server,
            json!({"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"fixture_tone","arguments":{}}}),
        )
        .unwrap()
        .unwrap();
        assert!(second["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("notPodcast"));
        let note = handle(
            &server,
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        )
        .unwrap();
        assert!(note.is_none());
        let _ = std::fs::remove_dir_all(&server.scratch.dir);
    }

    #[test]
    fn untrusted_tool_args_cannot_read_arbitrary_files() {
        let server = Server::boot();
        let readme = handle(
            &server,
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"synth","arguments":{"output":"x.wav","script":"README.md"}}}),
        )
        .unwrap()
        .unwrap();
        let message = readme["error"]["message"].as_str().unwrap_or("");
        assert!(message.contains("examples/"), "{message}");
        let extra = handle(
            &server,
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"serve_health","arguments":{"host":"169.254.169.254","probe":true,"token":"sk-supersecret"}}}),
        )
        .unwrap()
        .unwrap();
        assert!(extra["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("unexpected argument"));
        let probe = handle(
            &server,
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"serve_health","arguments":{"host":"169.254.169.254","probe":true}}}),
        )
        .unwrap()
        .unwrap();
        assert!(probe["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("allowlist"));
        let _ = std::fs::remove_dir_all(&server.scratch.dir);
    }

    #[test]
    fn unprobed_serve_url_is_not_healthy_and_protocol_is_negotiated() {
        let server = Server::boot();
        let health = handle(
            &server,
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"serve_health","arguments":{}}}),
        )
        .unwrap()
        .unwrap();
        let text = health["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("\"healthy\":false"), "{text}");
        assert!(text.contains("\"probed\":false"), "{text}");
        assert!(text.contains("unconfigured"), "{text}");
        assert!(!text.contains("<node>"), "{text}");
        assert!(!text.contains("Placeholder host"), "{text}");
        assert_eq!(health["result"]["isError"], false);
        let init = handle(
            &server,
            json!({"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}),
        )
        .unwrap()
        .unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        let rejected = handle(
            &server,
            json!({"jsonrpc":"2.0","id":3,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}),
        )
        .unwrap()
        .unwrap();
        assert!(rejected["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unsupported"));
        let engines = handle(
            &server,
            json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"list_engines","arguments":{}}}),
        )
        .unwrap()
        .unwrap();
        let listed = engines["result"]["content"][0]["text"].as_str().unwrap();
        assert!(listed.contains("\"liveSynth\":false"), "{listed}");
        assert!(
            listed.contains("\"weightsBundled\":false")
                || listed.contains("\"weights_bundled\":false"),
            "{listed}"
        );
        let _ = std::fs::remove_dir_all(&server.scratch.dir);
    }

    #[test]
    fn sidepane_rejects_connector_ids_and_kokoro_pack_ids() {
        let server = Server::boot();
        let rejected_agent = handle(
            &server,
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"ui_set_sidepane","arguments":{"agents":["copilot"],"voice":"kokoro_onnx"}}}),
        )
        .unwrap()
        .unwrap();
        let agent_msg = rejected_agent["error"]["message"].as_str().unwrap_or("");
        assert!(
            agent_msg.contains("connector") || agent_msg.contains("persona"),
            "{agent_msg}"
        );
        let rejected_voice = handle(
            &server,
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"ui_set_sidepane","arguments":{"agents":["alice"],"voice":"af_heart"}}}),
        )
        .unwrap()
        .unwrap();
        let voice_msg = rejected_voice["error"]["message"].as_str().unwrap_or("");
        assert!(voice_msg.contains("af_heart"), "{voice_msg}");
        let ok = handle(
            &server,
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"ui_set_sidepane","arguments":{"agents":["alice","rocky"],"voice":"kokoro_onnx"}}}),
        )
        .unwrap()
        .unwrap();
        let text = ok["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("\"synthesizedSpeech\":false"), "{text}");
        assert!(text.contains("alice"), "{text}");
        let profile = handle(
            &server,
            json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"voice_profile_get","arguments":{"personaId":"alice"}}}),
        )
        .unwrap()
        .unwrap();
        let profile_text = profile["result"]["content"][0]["text"].as_str().unwrap();
        assert!(profile_text.contains("af_heart"), "{profile_text}");
        assert!(
            profile_text.contains("\"notPodcast\":true"),
            "{profile_text}"
        );
        assert!(profile_text.contains("kokoro_onnx"), "{profile_text}");
        let _ = std::fs::remove_dir_all(&server.scratch.dir);
    }

    #[test]
    fn harvest_attaches_a_clip_ref_without_decoding_audio() {
        // E3: hermetic. With GEN_AUDIO_KOKORO_* set, the synth call below
        // would start a real synth and the refusal asserts would not hold.
        let _env = gen_audio_core::bridge::NoKokoroEnv::new();
        let server = Server::boot();
        let harvested = handle(
            &server,
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"library_harvest","arguments":{"clipId":"lib-kokoro-onnx","personaId":"alice","apply":true}}}),
        )
        .unwrap()
        .unwrap();
        let text = harvested["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("\"decodedAudio\":false"), "{text}");
        assert!(text.contains("\"openedWav\":false"), "{text}");
        assert!(
            text.contains("816 words")
                || text.contains("sidecar-counts")
                || text.contains("kokoro_onnx"),
            "{text}"
        );
        assert!(!text.contains("voices-v1.0.bin"), "{text}");
        assert!(!text.contains("D:\\\\"), "{text}");
        let profile = handle(
            &server,
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"voice_profile_get","arguments":{"personaId":"alice"}}}),
        )
        .unwrap()
        .unwrap();
        let profile_text = profile["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            profile_text.contains("clip:lib-kokoro-onnx"),
            "{profile_text}"
        );
        assert!(
            profile_text.contains("\"notPodcast\":true"),
            "{profile_text}"
        );
        let cube = handle(
            &server,
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"cube_layers","arguments":{"clipId":"lib-kokoro-onnx"}}}),
        )
        .unwrap()
        .unwrap();
        let cube_text = cube["result"]["content"][0]["text"].as_str().unwrap();
        assert!(cube_text.contains("signal"), "{cube_text}");
        assert!(cube_text.contains("tonality"), "{cube_text}");
        assert!(cube_text.contains("confidence"), "{cube_text}");
        assert!(cube_text.contains("quality"), "{cube_text}");
        assert!(
            cube_text.contains("\"absolutePathsOmitted\":true"),
            "{cube_text}"
        );
        assert!(!cube_text.contains("points_preview"), "{cube_text}");
        assert!(!cube_text.contains("D:\\\\"), "{cube_text}");
        let empty = handle(
            &server,
            json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"cube_layers","arguments":{"clipId":"lib-magpie"}}}),
        )
        .unwrap()
        .unwrap();
        let empty_text = empty["result"]["content"][0]["text"].as_str().unwrap();
        assert!(empty_text.contains("unavailable"), "{empty_text}");
        assert!(!empty_text.contains("points_preview"), "{empty_text}");
        let flip = handle(
            &server,
            json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"ui_flip","arguments":{"tileId":"profile-alice","personaId":"alice","flipped":true}}}),
        )
        .unwrap()
        .unwrap();
        let flip_text = flip["result"]["content"][0]["text"].as_str().unwrap();
        assert!(flip_text.contains("\"op\":\"flip\""), "{flip_text}");
        assert!(flip_text.contains("af_heart"), "{flip_text}");
        let synth = handle(
            &server,
            json!({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"synth","arguments":{"output":"request.wav","script":"examples/podcast_script_sample.txt","castMap":"voices/cast_map.example.json"}}}),
        )
        .unwrap()
        .unwrap();
        let synth_text = synth["result"]["content"][0]["text"].as_str().unwrap();
        assert!(synth_text.contains("\"phase\":\"refused\""), "{synth_text}");
        assert!(
            synth_text.contains("No speech was invented"),
            "{synth_text}"
        );
        assert!(
            !synth_text.contains("\"synthesizedSpeech\":true"),
            "{synth_text}"
        );
        let _ = std::fs::remove_dir_all(&server.scratch.dir);
    }

    // One contract, tested from both sides: the fixtures below are exactly
    // what the desktop window posts (apps/desktop/tests/play-control.test.ts
    // asserts that from the desktop's own builders); here the server accepts them.
    const DESKTOP_USER_PLAY: &str = include_str!("../../../schemas/examples/control/desktop-user-play.json");
    const SEEK_ROUND_TRIP: &str = include_str!("../../../schemas/examples/control/seek-round-trip.json");

    fn tool_text(response: &Value) -> Value {
        let text = response["result"]["content"][0]["text"].as_str().unwrap_or_else(|| panic!("no tool text: {response}"));
        serde_json::from_str(text).expect("tool text is json")
    }

    fn bus_events_for(tile: &str, after: u64) -> Vec<Value> {
        control::since(after).events.into_iter().filter(|event| event["args"]["tileId"] == tile).collect()
    }

    #[test]
    fn contract_the_desktop_play_payload_is_accepted_and_carries_origin_user() {
        let _bus = control::TestBus::fresh().bind();
        let server = Server::boot();
        let request: Value = serde_json::from_str(DESKTOP_USER_PLAY).unwrap();
        let tile = request["params"]["arguments"]["tileId"].as_str().unwrap().to_string();
        let before = control::since(0).cursor;
        let response = handle(&server, request).unwrap().unwrap();
        assert!(response.get("error").is_none(), "the server rejected the desktop's Play payload: {response}");
        let body = tool_text(&response);
        assert_eq!(body["args"]["origin"], "user");
        let events = bus_events_for(&tile, before);
        assert!(events.iter().any(|event| event["op"] == "playback" && event["args"]["origin"] == "user"), "{events:?}");
    }

    #[test]
    fn ui_playback_origin_is_a_validated_enum_and_a_bad_value_changes_nothing() {
        let _bus = control::TestBus::fresh().bind();
        let server = Server::boot();
        let mut request: Value = serde_json::from_str(DESKTOP_USER_PLAY).unwrap();
        // A tile only this test uses, so the bus check cannot see another test's event.
        request["params"]["arguments"]["tileId"] = json!("contract-origin-bad");
        for bad in [json!("agent"), json!("mcp"), json!(""), json!(5), json!(null)] {
            request["params"]["arguments"]["origin"] = bad.clone();
            let before = control::since(0).cursor;
            let response = handle(&server, request.clone()).unwrap().unwrap();
            assert_eq!(response["error"]["code"], -32602, "origin {bad}: {response}");
            assert!(bus_events_for("contract-origin-bad", before).is_empty(), "origin {bad} reached the bus");
        }
        request["params"]["arguments"]["origin"] = json!("auto");
        let response = handle(&server, request).unwrap().unwrap();
        assert_eq!(tool_text(&response)["args"]["origin"], "auto");
    }

    #[test]
    fn every_tool_accepts_exactly_its_advertised_input_schema_properties() {
        for def in tool_defs() {
            let name = def["name"].as_str().unwrap();
            let mut advertised: Vec<&str> = def["inputSchema"]["properties"].as_object().unwrap().keys().map(String::as_str).collect();
            let mut allowed = allowed_arguments(name).to_vec();
            advertised.sort_unstable();
            allowed.sort_unstable();
            assert_eq!(advertised, allowed, "{name}: inputSchema and the allowed-args list disagree");
        }
    }

    #[test]
    fn contract_seek_round_trip_returns_where_the_window_landed() {
        let fixture: Value = serde_json::from_str(SEEK_ROUND_TRIP).unwrap();
        let seek = fixture["agentSeek"].clone();
        let seconds = seek["params"]["arguments"]["seconds"].as_f64().unwrap();
        let bus = control::TestBus::fresh();
        let _bus = bus.bind();
        let before = control::since(0).cursor;
        let child_bus = bus.clone();
        let agent = std::thread::spawn(move || {
            let _bus = child_bus.bind();
            let server = Server::boot();
            handle(&server, seek).unwrap().unwrap()
        });
        // The window: find the queued seek, then post the fixture report for its seq.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
        let seq = loop {
            let found = control::since(before).events.into_iter().find(|event| {
                event["op"] == "playback" && event["args"]["action"] == "seek" && event["args"]["seconds"].as_f64() == Some(seconds)
            });
            if let Some(event) = found {
                assert!(event["args"].get("waitMs").is_none(), "waitMs stays off the bus");
                break event["seq"].as_u64().unwrap();
            }
            assert!(std::time::Instant::now() < deadline, "the seek never reached the bus");
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        let mut report = fixture["desktopReport"].clone();
        report["params"]["arguments"]["seq"] = json!(seq);
        let server = Server::boot();
        let recorded = handle(&server, report.clone()).unwrap().unwrap();
        assert!(recorded.get("error").is_none(), "the server rejected the desktop's seek report: {recorded}");
        let result = tool_text(&agent.join().unwrap());
        for key in ["requested_t", "landed_t", "ok", "reason"] {
            let (got, want) = (&result[key], &fixture["agentResult"][key]);
            match want.as_f64() {
                Some(number) => assert_eq!(got.as_f64(), Some(number), "{key}: {result}"),
                None => assert_eq!(got, want, "{key}: {result}"),
            }
        }
        // A second report for the same seek is refused.
        assert_eq!(handle(&server, report).unwrap().unwrap()["error"]["code"], -32602);
    }

    #[test]
    fn a_seek_no_window_answers_says_so_and_bad_reports_record_nothing() {
        let _bus = control::TestBus::fresh().bind();
        let server = Server::boot();
        let call = |name: &str, args: Value| {
            handle(&server, json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name": name, "arguments": args}})).unwrap().unwrap()
        };
        let timed_out = call("ui_playback", json!({"tileId": "contract-seek-quiet", "action": "seek", "seconds": 3.25, "waitMs": 0}));
        let body = tool_text(&timed_out);
        assert_eq!(body["requested_t"], 3.25);
        assert!(body["landed_t"].is_null());
        assert_eq!(body["ok"], false);
        assert!(body["reason"].as_str().unwrap().contains("no window reported"), "{body}");
        let seq = body["seq"].as_u64().unwrap();
        let report = |args: Value| call("ui_seek_report", args);
        let code = |response: Value| response["error"]["code"].clone();
        assert_eq!(code(report(json!({"seq": seq + 100_000, "requested_t": 3.25, "landed_t": 3.25, "ok": true, "reason": "x"}))), -32602, "unknown seq");
        assert_eq!(code(report(json!({"seq": seq, "requested_t": 9.0, "landed_t": 3.25, "ok": true, "reason": "x"}))), -32602, "wrong requested_t");
        assert_eq!(code(report(json!({"seq": seq, "requested_t": 3.25, "landed_t": null, "ok": true, "reason": "x"}))), -32602, "ok without landed_t");
        assert_eq!(code(report(json!({"seq": seq, "requested_t": 3.25, "landed_t": -1, "ok": false, "reason": "x"}))), -32602, "negative landed_t");
        assert_eq!(code(report(json!({"seq": seq, "requested_t": 3.25, "landed_t": 0, "ok": false, "reason": "r".repeat(241)}))), -32602, "long reason");
        assert_eq!(code(call("ui_playback", json!({"tileId": "contract-seek-quiet", "action": "seek", "seconds": 1, "waitMs": 60_000}))), -32602, "waitMs cap");
        assert_eq!(code(call("ui_playback", json!({"tileId": "contract-seek-quiet", "action": "pause", "waitMs": 10}))), -32602, "waitMs on pause");
        // None of the bad reports was recorded: the honest one still is.
        let honest = report(json!({"seq": seq, "requested_t": 3.25, "landed_t": 0, "ok": false, "reason": "seek failed: this WAV source cannot seek (stays at 0:00)"}));
        assert!(honest.get("error").is_none(), "{honest}");
        assert_eq!(tool_text(&honest)["report"]["landed_t"], 0.0);
    }

    #[test]
    fn strict_types_ports_ids_and_protocols() {
        let server = Server::isolated();
        for args in [
            json!({"port":0}), json!({"port":65536}), json!({"port":-1}),
            json!({"port":"8002"}), json!({"probe":"false"}), json!({"host":null}),
        ] {
            assert!(call_tool(&server,&json!({"name":"serve_health","arguments":args})).is_err());
        }
        for id in [json!([]),json!({}),json!(true),json!(1.5),json!("x".repeat(129))] {
            let result = handle(&server,json!({"jsonrpc":"2.0","id":id,"method":"ping"})).unwrap().unwrap();
            assert_eq!(result["error"]["code"], -32600);
            assert_eq!(result["id"], Value::Null);
        }
        for version in [json!(true),json!(1),Value::Null] {
            let result = handle(&server,json!({"jsonrpc":"2.0","id":3,"method":"initialize","params":{"protocolVersion":version}})).unwrap().unwrap();
            assert_eq!(result["error"]["code"], -32602);
        }
        assert!(handle(&server,json!({"jsonrpc":"2.0","method":"ping"})).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&server.scratch.dir);
    }
}

//! MCP asset tools over the generated library catalog (Darbot's v1 contract).
//!
//! - Malformed uid or unknown kind: JSON-RPC -32602.
//! - Unknown uid: `{ok:false, code:"asset_not_found"}`.
//! - Prefix hit on more than one asset: `{ok:false, code:"asset_ambiguous_prefix", candidates}`.
//! - Stored asset that no longer matches its own uid/kind: -32603.
//! Media are `/library` URLs plus sha256; absolute paths are never returned.

use gen_audio_core::asset_catalog::{self, CatalogError, Resolve, LIST_DEFAULT};
use serde_json::{json, Value};

fn uid_arg<'a>(args: &'a Value, tool: &str) -> Result<&'a str, (i32, String)> {
    args.get("uid")
        .and_then(Value::as_str)
        .ok_or((-32602, format!("{tool} needs uid")))
}

pub fn asset_resolve(args: &Value) -> Result<Value, (i32, String)> {
    let uid = uid_arg(args, "asset_resolve")?;
    match asset_catalog::resolve(uid).map_err(CatalogError::rpc)? {
        Resolve::Found(asset) => {
            let found = asset["uid"].as_str().unwrap_or_default();
            let glyph = asset_catalog::glyph_info(found).map_err(CatalogError::rpc)?;
            Ok(json!({
                "ok": true,
                "synthesizedSpeech": false,
                "uid": found,
                "kind": asset["kind"],
                "glyph": glyph["glyph"],
                "hueClass": glyph["hueClass"],
                "ariaLabel": glyph["ariaLabel"],
                "tileId": asset_catalog::tile_id_for(asset),
                "asset": asset_catalog::public_view(asset),
                "absolutePathsOmitted": true
            }))
        }
        Resolve::NotFound => Ok(json!({
            "ok": false,
            "code": "asset_not_found",
            "uid": uid,
            "synthesizedSpeech": false
        })),
        Resolve::Ambiguous(candidates) => Ok(json!({
            "ok": false,
            "code": "asset_ambiguous_prefix",
            "uid": uid,
            "candidates": candidates,
            "synthesizedSpeech": false
        })),
    }
}

pub fn asset_list(args: &Value) -> Result<Value, (i32, String)> {
    let kind = match args.get("kind") {
        None => None,
        Some(value) => Some(value.as_str().ok_or((-32602, "kind must be a string".to_string()))?),
    };
    let cursor = match args.get("cursor") {
        None => None,
        Some(value) => Some(value.as_str().ok_or((-32602, "cursor must be a uid string".to_string()))?),
    };
    let limit = match args.get("limit") {
        None => LIST_DEFAULT,
        Some(value) => value
            .as_u64()
            .ok_or((-32602, "limit must be an integer from 1 to 100".to_string()))? as usize,
    };
    let (items, next) = asset_catalog::list(kind, cursor, limit).map_err(CatalogError::rpc)?;
    Ok(json!({
        "ok": true,
        "synthesizedSpeech": false,
        "count": items.len(),
        "assets": items,
        "nextCursor": next,
        "absolutePathsOmitted": true
    }))
}

pub fn asset_glyph(args: &Value) -> Result<Value, (i32, String)> {
    let uid = uid_arg(args, "asset_glyph")?;
    let mut info = asset_catalog::glyph_info(uid).map_err(CatalogError::rpc)?;
    info["ok"] = json!(true);
    info["note"] = json!("Glyph is a 16-bit visual hint; collisions are expected. Resolve by uid.");
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{handle, Server};

    fn call(name: &str, arguments: Value) -> Value {
        let server = Server::isolated();
        handle(
            &server,
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":arguments}}),
        )
        .unwrap()
        .unwrap()
    }

    fn payload(response: &Value) -> Value {
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
    }

    #[test]
    fn resolve_returns_library_urls_and_no_absolute_paths() {
        let response = call("asset_resolve", json!({"uid": "ga:cube_ihdr:biaxxmnibxtcur7nxdu3ffvna4"}));
        assert_eq!(response["result"]["isError"], false);
        let body = payload(&response);
        assert_eq!(body["glyph"], "\u{280a}\u{2801}");
        assert_eq!(body["hueClass"], "ga-kind-cube_ihdr");
        assert_eq!(body["tileId"], "lib-misaki-kokoro");
        assert_eq!(body["absolutePathsOmitted"], true);
        let text = body.to_string();
        assert!(!text.contains(":\\\\") && !text.contains("\"path\""), "{text}");
        assert!(body["asset"]["media"][0]["url"].as_str().unwrap().starts_with("/library/"));
    }

    #[test]
    fn resolve_error_contract() {
        assert_eq!(call("asset_resolve", json!({"uid": "ga:widget:biaxxmnibxtcur7nxdu3ffvna4"}))["error"]["code"], -32602);
        assert_eq!(call("asset_resolve", json!({"uid": "not-a-uid"}))["error"]["code"], -32602);
        assert_eq!(call("asset_resolve", json!({"uid": "ga:cube_ihdr:biaxxmnibxtcur7nxdu3ffvnaf"}))["error"]["code"], -32602);
        let missing = call("asset_resolve", json!({"uid": "ga:cube_ihdr:aaaaaaaaaaaaaaaaaaaaaaaaaa"}));
        assert_eq!(missing["result"]["isError"], true);
        assert_eq!(payload(&missing)["code"], "asset_not_found");
        let prefix = call("asset_resolve", json!({"uid": "ga:cube_ihdr:biaxxmni"}));
        assert_eq!(payload(&prefix)["uid"], "ga:cube_ihdr:biaxxmnibxtcur7nxdu3ffvna4");
    }

    #[test]
    fn list_and_glyph() {
        let page = payload(&call("asset_list", json!({"kind": "audio_clip"})));
        assert_eq!(page["count"], 5); // cube explainer, kokoro-onnx, kokoro, misaki, bitdot
        assert!(page["nextCursor"].is_null());
        let first = payload(&call("asset_list", json!({"limit": 2})));
        assert_eq!(first["count"], 2);
        let cursor = first["nextCursor"].as_str().unwrap().to_string();
        let second = payload(&call("asset_list", json!({"limit": 2, "cursor": cursor})));
        assert!(second["assets"][0]["uid"].as_str().unwrap() > cursor.as_str());
        assert_eq!(call("asset_list", json!({"limit": 0}))["error"]["code"], -32602);
        assert_eq!(call("asset_list", json!({"limit": 101}))["error"]["code"], -32602);
        assert_eq!(call("asset_list", json!({"kind": "agent/VoiceProfile"}))["error"]["code"], -32602);
        let glyph = payload(&call("asset_glyph", json!({"uid": "ga:transcript:ac3t5gk3b27ue5vriw5qo27ucy"})));
        assert_eq!(glyph["codepoints"][0], 0x2800);
        assert_eq!(glyph["inCatalog"], false);
    }

    #[test]
    fn ui_tools_accept_a_matching_uid_only() {
        let clip = asset_catalog::uid_for_legacy("audio_clip", "lib-misaki-kokoro").unwrap();
        let ok = call("ui_playback", json!({"uid": clip, "action": "pause"}));
        assert_eq!(payload(&ok)["args"]["tileId"], "lib-misaki-kokoro");
        let agree = call("ui_navigate", json!({"slide": "library", "tileId": "lib-misaki-kokoro", "uid": clip}));
        assert_eq!(agree["result"]["isError"], false);
        let clash = call("ui_select_tile", json!({"tileId": "lib-kokoro", "uid": clip}));
        assert_eq!(clash["error"]["code"], -32602);
        let unknown = call("ui_flip", json!({"uid": "ga:card:aaaaaaaaaaaaaaaaaaaaaaaaaa"}));
        assert_eq!(unknown["error"]["code"], -32602);
    }
}

//! Read-only view of the generated library catalog (`public/library/assets.json`).
//!
//! Shared by MCP (`asset_resolve`, `asset_list`, `asset_glyph`, optional `uid`
//! on UI tools) and ACP (`session/new` `assets`). Media come back as `/library`
//! web paths plus sha256; machine paths never leave this module.

use std::sync::OnceLock;

use serde_json::{json, Value};

use crate::asset::{
    glyph_bytes_from_uid, glyph_dots, glyph_from_uid, hue_class, is_kind, parse_uid, validate_envelope, validate_set,
    AssetError,
};

const CATALOG_JSON: &str = include_str!("../../../apps/desktop/public/library/assets.json");
pub const MIN_PREFIX_CHARS: usize = 8;
pub const MAX_CANDIDATES: usize = 10;
pub const LIST_DEFAULT: usize = 50;
pub const LIST_MAX: usize = 100;
pub const MAX_SESSION_ASSETS: usize = 8;

/// Parse a catalog document and run the set checks (`validate_set`: every
/// envelope, unique uids, relations resolve, no cycles) before anything is
/// served. A catalog that fails is not served at all.
pub fn load_catalog(text: &str) -> Result<Vec<Value>, String> {
    let doc: Value = serde_json::from_str(text).map_err(|error| format!("bad_json: assets.json does not parse: {error}"))?;
    let mut assets = doc
        .get("assets")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| "bad_catalog: assets.json has no assets array".to_string())?;
    validate_set(&assets).map_err(|error| format!("catalog set check failed: {error}"))?;
    assets.sort_by(|a, b| uid_of(a).cmp(uid_of(b)));
    Ok(assets)
}

fn loaded() -> &'static Result<Vec<Value>, String> {
    static CATALOG: OnceLock<Result<Vec<Value>, String>> = OnceLock::new();
    CATALOG.get_or_init(|| load_catalog(CATALOG_JSON))
}

/// Ok(asset count) when the embedded catalog passed the set checks.
pub fn catalog_health() -> Result<usize, String> {
    loaded().as_ref().map(Vec::len).map_err(Clone::clone)
}

fn catalog() -> Vec<Value> {
    loaded().as_ref().cloned().unwrap_or_default()
}

fn uid_of(asset: &Value) -> &str {
    asset.get("uid").and_then(Value::as_str).unwrap_or_default()
}

/// Baked catalog plus runtime envelopes. A runtime uid replaces the baked row.
pub fn assets() -> Vec<Value> {
    merged()
}

fn merged() -> Vec<Value> {
    let mut assets = catalog().clone();
    for runtime in crate::library_store::runtime_assets() {
        let uid = uid_of(&runtime).to_string();
        if uid.is_empty() {
            continue;
        }
        if let Some(slot) = assets.iter_mut().find(|item| uid_of(item) == uid) {
            *slot = runtime;
        } else {
            assets.push(runtime);
        }
    }
    assets.sort_by(|a, b| uid_of(a).cmp(uid_of(b)));
    assets
}

/// Outcome of resolving a full uid or a uid prefix.
#[derive(Debug)]
pub enum Resolve {
    Found(Value),
    NotFound,
    Ambiguous(Vec<String>),
}

/// Errors map to JSON-RPC codes: `Invalid` is -32602, `Corrupt` is -32603.
#[derive(Debug)]
pub enum CatalogError {
    Invalid(String),
    Corrupt(String),
}

impl CatalogError {
    pub fn rpc(self) -> (i32, String) {
        match self {
            CatalogError::Invalid(text) => (-32602, text),
            CatalogError::Corrupt(text) => (-32603, text),
        }
    }
}

fn invalid(error: AssetError) -> CatalogError {
    CatalogError::Invalid(error.to_string())
}

/// Accept `ga:<kind>:<26 chars>` or `ga:<kind>:<8..25 chars>` (prefix).
pub fn resolve(text: &str) -> Result<Resolve, CatalogError> {
    let rest = text
        .strip_prefix("ga:")
        .ok_or_else(|| CatalogError::Invalid(format!("bad_uid: {text:?} must start with ga:")))?;
    let (kind, body) = rest.split_once(':').ok_or_else(|| {
        CatalogError::Invalid(format!("bad_uid: {text:?} must be ga:<kind>:<base32>"))
    })?;
    if !is_kind(kind) {
        return Err(CatalogError::Invalid(format!(
            "unknown_kind: {kind:?} is not a v1 asset kind"
        )));
    }
    if body.len() >= 26 {
        parse_uid(text).map_err(invalid)?;
    } else if body.len() < MIN_PREFIX_CHARS
        || !body
            .bytes()
            .all(|byte| matches!(byte, b'a'..=b'z' | b'2'..=b'7'))
    {
        return Err(CatalogError::Invalid(format!(
            "bad_uid: prefix needs at least {MIN_PREFIX_CHARS} lowercase base32 chars after ga:{kind}:"
        )));
    }
    let matches: Vec<Value> = merged()
        .into_iter()
        .filter(|asset| uid_of(asset).starts_with(text))
        .collect();
    let found = match matches.len() {
        0 => return Ok(Resolve::NotFound),
        1 => matches.into_iter().next().expect("one match"),
        _ => {
            return Ok(Resolve::Ambiguous(
                matches
                    .iter()
                    .take(MAX_CANDIDATES)
                    .map(|asset| uid_of(asset).to_string())
                    .collect(),
            ));
        }
    };
    check_stored(&found, kind)?;
    Ok(Resolve::Found(found))
}

/// Stored data must still agree with its own uid (kind prefix, identity, honesty).
fn check_stored(asset: &Value, kind: &str) -> Result<(), CatalogError> {
    if asset.get("kind").and_then(Value::as_str) != Some(kind) {
        return Err(CatalogError::Corrupt(format!(
            "kind_mismatch: stored asset {} does not have kind {kind}",
            uid_of(asset)
        )));
    }
    validate_envelope(asset).map_err(|error| {
        CatalogError::Corrupt(format!(
            "stored asset {} failed validation: {error}",
            uid_of(asset)
        ))
    })?;
    Ok(())
}

/// Full uid that must exist in the catalog (used by ACP and UI tools).
pub fn require(uid: &str) -> Result<Value, CatalogError> {
    parse_uid(uid).map_err(invalid)?;
    match resolve(uid)? {
        Resolve::Found(asset) => Ok(asset),
        _ => Err(CatalogError::Invalid(format!("asset_not_found: {uid}"))),
    }
}

/// The envelope with media reduced to `/library` URLs plus sha256.
pub fn public_view(asset: &Value) -> Value {
    let mut out = asset.clone();
    let media: Vec<Value> = asset
        .get("media")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    json!({
                        "role": item.get("role").cloned().unwrap_or(Value::Null),
                        "url": format!("/library/{}", item.get("path").and_then(Value::as_str).unwrap_or_default()),
                        "sha256": item.get("sha256").cloned().unwrap_or(Value::Null)
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    out["media"] = Value::Array(media);
    out
}

pub fn summary(asset: &Value) -> Value {
    let uid = uid_of(asset);
    let kind = asset
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    json!({
        "uid": uid,
        "kind": kind,
        "legacyId": asset.get("legacy_id").cloned().unwrap_or(Value::Null),
        "status": asset.get("status").cloned().unwrap_or(Value::Null),
        "title": asset.pointer("/display/title").cloned().unwrap_or(Value::Null),
        "glyph": asset.pointer("/display/glyph").cloned().unwrap_or(Value::Null),
        "hueClass": hue_class(kind)
    })
}

pub fn aria_label(uid: &str) -> String {
    let kind = parse_uid(uid).map(|(kind, _)| kind).unwrap_or("asset");
    format!("{} {uid}", kind.replace('_', " "))
}

pub fn glyph_info(uid: &str) -> Result<Value, CatalogError> {
    let (kind, _) = parse_uid(uid).map_err(invalid)?;
    let glyph = glyph_from_uid(uid).map_err(invalid)?;
    let bytes = glyph_bytes_from_uid(uid).map_err(invalid)?;
    Ok(json!({
        "uid": uid,
        "glyph": glyph,
        "codepoints": glyph.chars().map(|ch| ch as u32).collect::<Vec<_>>(),
        "dots": bytes.iter().map(|byte| glyph_dots(*byte)).collect::<Vec<_>>(),
        "hueClass": hue_class(kind),
        "ariaLabel": aria_label(uid),
        "inCatalog": merged().iter().any(|asset| uid_of(asset) == uid)
    }))
}

/// Page through the catalog in uid order; the cursor is the last uid returned.
pub fn list(
    kind: Option<&str>,
    cursor: Option<&str>,
    limit: usize,
) -> Result<(Vec<Value>, Option<String>), CatalogError> {
    if let Some(kind) = kind {
        if !is_kind(kind) {
            return Err(CatalogError::Invalid(format!(
                "unknown_kind: {kind:?} is not a v1 asset kind"
            )));
        }
    }
    if let Some(cursor) = cursor {
        parse_uid(cursor).map_err(invalid)?;
    }
    if !(1..=LIST_MAX).contains(&limit) {
        return Err(CatalogError::Invalid(format!(
            "limit must be 1 to {LIST_MAX}"
        )));
    }
    let merged = merged();
    let mut page: Vec<&Value> = merged
        .iter()
        .filter(|asset| {
            kind.is_none_or(|kind| asset.get("kind").and_then(Value::as_str) == Some(kind))
        })
        .filter(|asset| cursor.is_none_or(|cursor| uid_of(asset) > cursor))
        .take(limit + 1)
        .collect();
    let more = page.len() > limit;
    page.truncate(limit);
    let next = if more {
        page.last().map(|asset| uid_of(asset).to_string())
    } else {
        None
    };
    Ok((page.into_iter().map(summary).collect(), next))
}

/// Card/tile ids a uid stands for, so UI tools can check `uid` against `tileId`.
pub fn tile_id_for(asset: &Value) -> Option<String> {
    let legacy = asset.get("legacy_id").and_then(Value::as_str)?;
    match asset.get("kind").and_then(Value::as_str)? {
        "card" => asset
            .pointer("/fields/card_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        "audio_clip" => Some(legacy.to_string()),
        "voice_profile" => Some(format!("profile-{legacy}")),
        "cube_ihdr" | "spectrogram_2d" => {
            let parent = asset
                .get("src")
                .and_then(Value::as_array)?
                .first()?
                .as_str()?;
            merged()
                .iter()
                .find(|item| uid_of(item) == parent)?
                .get("legacy_id")?
                .as_str()
                .map(str::to_string)
        }
        _ => None,
    }
}

/// Bound audio clip uid for a library tile id (card id == clip id), if any.
pub fn uid_for_legacy(kind: &str, legacy_id: &str) -> Option<String> {
    merged()
        .into_iter()
        .find(|asset| {
            asset.get("kind").and_then(Value::as_str) == Some(kind)
                && asset.get("legacy_id").and_then(Value::as_str) == Some(legacy_id)
        })
        .and_then(|asset| asset.get("uid").and_then(Value::as_str).map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MISAKI_CUBE: &str = "ga:cube_ihdr:biaxxmnibxtcur7nxdu3ffvna4";

    #[test]
    fn resolves_full_uid_and_prefix() {
        assert!(matches!(resolve(MISAKI_CUBE).unwrap(), Resolve::Found(asset) if asset["legacy_id"] == "lib-misaki-kokoro.cube"));
        assert!(matches!(resolve("ga:cube_ihdr:biaxxmni").unwrap(), Resolve::Found(_)));
        assert!(matches!(resolve("ga:cube_ihdr:aaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap(), Resolve::NotFound));
        assert!(matches!(resolve("ga:cube_ihdr:biax"), Err(CatalogError::Invalid(_))));
        assert!(matches!(resolve("ga:widget:biaxxmnibxtcur7nxdu3ffvna4"), Err(CatalogError::Invalid(_))));
        assert!(matches!(resolve("ga:cube_ihdr:biaxxmnibxtcur7nxdu3ffvnaf"), Err(CatalogError::Invalid(_))));
    }

    #[test]
    fn every_stored_asset_resolves_and_maps_media_to_library_urls() {
        for asset in assets() {
            let uid = uid_of(&asset);
            assert!(matches!(resolve(uid).unwrap(), Resolve::Found(_)), "{uid}");
            for media in public_view(&asset)["media"].as_array().unwrap() {
                let url = media["url"].as_str().unwrap();
                assert!(url.starts_with("/library/") && !url.contains(".."), "{url}");
                assert!(media.get("path").is_none());
            }
        }
    }

    #[test]
    fn runtime_load_runs_the_set_check() {
        assert_eq!(catalog_health().unwrap(), assets().len());
        let mut doc: Value = serde_json::from_str(CATALOG_JSON).unwrap();
        // Drop the misaki clip: every envelope is still valid on its own, but
        // its cube and spectrogram now derive from a uid outside the set.
        let clip = resolve(MISAKI_CUBE).ok().and_then(|found| match found {
            Resolve::Found(cube) => cube["src"][0].as_str().map(str::to_string),
            _ => None,
        });
        let clip = clip.unwrap();
        doc["assets"].as_array_mut().unwrap().retain(|asset| asset["uid"] != clip.as_str());
        let error = load_catalog(&doc.to_string()).unwrap_err();
        assert!(error.contains("derived_source_missing"), "{error}");
        assert!(load_catalog("{}").is_err());
    }

    #[test]
    fn list_pages_by_uid_cursor() {
        let (first, next) = list(Some("layer"), None, 5).unwrap();
        assert_eq!(first.len(), 5);
        let (second, after) = list(Some("layer"), next.as_deref(), 100).unwrap();
        let layers = list(Some("layer"), None, 100).unwrap().0.len();
        assert_eq!(layers % 4, 0, "four layers per cube");
        assert_eq!(second.len(), layers - 5);
        assert!(after.is_none());
        assert!(first.last().unwrap()["uid"].as_str() < second[0]["uid"].as_str());
        assert!(list(None, None, 0).is_err());
        assert!(list(None, None, 101).is_err());
        assert!(list(Some("Card"), None, 5).is_err());
    }

    #[test]
    fn tiles_map_back_from_uids() {
        let clip = uid_for_legacy("audio_clip", "lib-misaki-kokoro").unwrap();
        assert_eq!(
            tile_id_for(&require(&clip).unwrap()).as_deref(),
            Some("lib-misaki-kokoro")
        );
        assert_eq!(
            tile_id_for(&require(MISAKI_CUBE).unwrap()).as_deref(),
            Some("lib-misaki-kokoro")
        );
        let card = uid_for_legacy("card", "profile-optimus").unwrap();
        assert_eq!(
            tile_id_for(&require(&card).unwrap()).as_deref(),
            Some("profile-optimus")
        );
        let profile = uid_for_legacy("voice_profile", "optimus").unwrap();
        assert_eq!(
            tile_id_for(&require(&profile).unwrap()).as_deref(),
            Some("profile-optimus")
        );
    }
}

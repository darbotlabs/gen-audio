//! Portable metric cubes. Validation and projection run natively, without
//! Python, remote schema resolution, executable expressions, or audio assets.
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::OnceLock;

pub const MAX_BYTES: usize = 1024 * 1024;
const CUBE: &str = include_str!("../../../schemas/semantic-cube.schema.json");
const RECORD: &str = include_str!("../../../schemas/semantic-record.schema.json");
const EXAMPLES: &str = include_str!("../../../schemas/examples/semantic-cubes.json");

pub fn library() -> Value {
    json!({"schemaVersion":"1.0.0", "schemas":[
        serde_json::from_str::<Value>(RECORD).expect("compiled record schema"),
        serde_json::from_str::<Value>(CUBE).expect("compiled cube schema")],
        "examples":serde_json::from_str::<Value>(EXAMPLES).expect("compiled examples"),
        "limits":{"documentBytes":MAX_BYTES,"records":2048,"layers":16},
        "financialPrecision":"Chart observations are binary64; do not use as an order or accounting ledger."})
}

fn validator() -> &'static jsonschema::Validator {
    static VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();
    VALIDATOR.get_or_init(|| {
        let record: Value = serde_json::from_str(RECORD).expect("compiled record schema");
        let cube: Value = serde_json::from_str(CUBE).expect("compiled cube schema");
        jsonschema::options()
            .with_draft(jsonschema::Draft::Draft202012)
            .should_validate_formats(true)
            .with_resource(record["$id"].as_str().unwrap(), jsonschema::Resource::from_contents(record.clone()).unwrap())
            .build(&cube).expect("compiled semantic schema")
    })
}

/// Keep every MCP response below its JSON transport budget. All records are
/// validated on each request; paging changes only the returned projection.
pub fn project_page(doc: &Value, offset: usize, limit: usize) -> Result<Value, String> {
    if limit == 0 || limit > 2048 { return Err("Projection limit must be 1..2048".into()); }
    let mut result = project(doc)?;
    let points = result["points"].as_array().unwrap();
    let total = points.len();
    if offset > total { return Err("Projection offset exceeds point count".into()); }
    let end = offset.saturating_add(limit).min(total);
    let page = points[offset..end].to_vec();
    result["points"] = json!(page);
    result["totalPoints"] = json!(total);
    result["nextOffset"] = if end < total { json!(end) } else { Value::Null };
    Ok(result)
}

fn numeric(value: &Value) -> f64 { value.as_f64().expect("validated number") }
fn log_span(value: f64, base: f64) -> f64 {
    let relative = (value-base)/base;
    if relative.is_finite() { relative.ln_1p() } else { value.ln()-base.ln() }
}
fn range(value: &Value, label: &str) -> Result<(f64, f64), String> {
    let lo = numeric(&value["minimum"]);
    let hi = numeric(&value["maximum"]);
    if lo >= hi { return Err(format!("{label}: minimum must be less than maximum")); }
    if value["unit"] == "unix_s" && (lo < 0.0 || hi > 253402300799.0) { return Err(format!("{label}: Unix seconds must be within 1970–9999")); }
    if value["scale"] == "log" && lo <= 0.0 { return Err(format!("{label}: log scale requires a positive minimum")); }
    Ok((lo, hi))
}

pub fn project(doc: &Value) -> Result<Value, String> {
    if serde_json::to_vec(doc).map_err(|e|e.to_string())?.len() > MAX_BYTES {
        return Err("Semantic cube exceeds 1 MiB".into());
    }
    if let Some(error) = validator().iter_errors(doc).next() {
        // Report location and keyword, not potentially large/sensitive records.
        return Err(format!("Invalid semantic cube at {} ({})", error.instance_path, error.schema_path));
    }
    let layers = doc["layers"].as_array().unwrap();
    let records = doc["records"].as_array().unwrap();
    let mut ids = HashSet::new();
    let mut planes = Vec::new();
    let mut ranges = Vec::new();
    for layer in layers {
        let id = layer["id"].as_str().unwrap();
        if !ids.insert(id) { return Err(format!("Duplicate layer id: {id}")); }
        let plane = numeric(&layer["position"]);
        if planes.iter().any(|other: &f64| (plane-other).abs() < 0.001) {
            return Err("Layer positions must be distinct by at least 0.001".into());
        }
        planes.push(plane);
        ranges.push(range(layer, id)?);
        let metric = layer["metric"].as_str().unwrap();
        if !records.iter().any(|r| r["metrics"].get(metric).is_some()) {
            return Err(format!("Layer {id} refers to absent metric {metric}"));
        }
    }
    let (xmin, xmax) = range(&doc["axes"]["x"], "x axis")?;
    let (ymin, ymax) = range(&doc["axes"]["y"], "y axis")?;
    ids.clear();
    let mut points = Vec::new();
    let mut skipped = 0;
    let mut clipped = 0;
    for record in records {
        let id = record["id"].as_str().unwrap();
        if !ids.insert(id) { return Err(format!("Duplicate record id: {id}")); }
        let x = numeric(&record["x"]);
        let y = numeric(&record["y"]);
        if !(xmin..=xmax).contains(&x) || !(ymin..=ymax).contains(&y) {
            return Err(format!("Record {id}: coordinate outside declared axes"));
        }
        let metrics = &record["metrics"];
        if doc["domain"].as_str().unwrap().starts_with("market.") {
            let (open, high, low, close) = (numeric(&metrics["open"]),numeric(&metrics["high"]),numeric(&metrics["low"]),numeric(&metrics["close"]));
            if low > high || open < low || open > high || close < low || close > high {
                return Err(format!("Record {id}: inconsistent OHLC candle"));
            }
        }
        for (index, layer) in layers.iter().enumerate() {
            let Some(raw) = metrics[layer["metric"].as_str().unwrap()].as_f64() else { skipped += 1; continue; };
            let (lo, hi) = ranges[index];
            if layer["scale"] == "log" && raw <= 0.0 {
                return Err(format!("Record {id}: log metric must be positive"));
            }
            if raw < lo || raw > hi {
                if layer["outside"] != "clip" { return Err(format!("Record {id}: metric outside layer range")); }
                clipped += 1;
            }
            let value = raw.clamp(lo,hi);
            let v = if layer["scale"] == "log" { log_span(value,lo)/log_span(hi,lo) } else { (value-lo)/(hi-lo) };
            points.push(json!({"t":(x-xmin)/(xmax-xmin),"f":(y-ymin)/(ymax-ymin),
                "z":planes[index],"v":v,"layer":layer["id"],"recordId":id,"raw":raw}));
        }
    }
    if points.is_empty() { return Err("Semantic cube has no observed metric values".into()); }
    Ok(json!({"ok":true,"points":points,"skipped":skipped,"clipped":clipped,"records":records.len()}))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn example(index: usize) -> Value { library()["examples"][index].clone() }
    #[test]
    fn all_domains_project_without_audio_or_external_resources() {
        for doc in library()["examples"].as_array().unwrap() {
            let result = project(doc).unwrap();
            assert_eq!(result["records"],12);
            assert_eq!(result["skipped"],0);
            assert_eq!(result["clipped"],0);
            assert_eq!(result["points"].as_array().unwrap().len(),12*doc["layers"].as_array().unwrap().len());
        }
    }
    #[test]
    fn rejects_invalid_references_coordinates_ranges_and_candles() {
        for (pointer,value) in [("/layers/0/maximum",json!(0)),("/layers/1/id",json!("latency_ms")),
            ("/layers/0/color",json!("url(file:///secret)")),("/layers/0/metric",json!("absent")),
            ("/records/0/x",json!(-1)),("/records/1/id",json!("observation-1")),
            ("/layers/0/position",json!(1)),("/records/0/metrics/confidence",json!(2))] {
            let mut doc=example(0); *doc.pointer_mut(pointer).unwrap()=value;
            assert!(project(&doc).is_err(),"{pointer}");
        }
        let mut market=example(3);market["records"][0]["metrics"]["low"]=json!(103);
        assert!(project(&market).unwrap_err().contains("OHLC"));
    }
    #[test]
    fn null_is_missing_zero_is_observed_and_clipping_is_explicit() {
        let mut doc=example(0);doc["records"][0]["metrics"]["latency_ms"]=Value::Null;
        doc["records"][0]["metrics"]["tokens"]=json!(0);
        let result=project(&doc).unwrap();assert_eq!(result["skipped"],1);
        assert_eq!(result["points"][0]["v"],0.0);
        doc["records"][1]["metrics"]["latency_ms"]=json!(3000);
        assert!(project(&doc).is_err());doc["layers"][0]["outside"]=json!("clip");
        assert_eq!(project(&doc).unwrap()["clipped"],1);
        doc["layers"][0]["scale"]=json!("log");
        assert!(project(&doc).is_err());
    }
    #[test]
    fn paging_is_bounded_and_dates_are_real() {
        let doc=example(0);
        let page=project_page(&doc,0,1).unwrap();assert_eq!(page["totalPoints"],48);assert_eq!(page["nextOffset"],1);
        assert_eq!(project_page(&doc,47,1).unwrap()["nextOffset"],Value::Null);
        assert!(project_page(&doc,49,1).is_err());assert!(project_page(&doc,0,2049).is_err());
        let mut market=example(3);market["records"][0]["context"]["timestamp"]=json!("2023-02-30T22:13:20Z");
        assert!(project(&market).is_err());
    }
    #[test]
    fn logarithms_preserve_adjacent_large_values_and_subnormals() {
        for (lo,hi) in [(1e15-0.125,1e15),(f64::from_bits(1),1e15)] {
            let mut doc=example(0);doc["domain"]=json!("custom");
            doc["layers"].as_array_mut().unwrap().truncate(1);
            doc["records"].as_array_mut().unwrap().truncate(2);
            doc["layers"][0]["scale"]=json!("log");
            doc["layers"][0]["minimum"]=json!(lo);doc["layers"][0]["maximum"]=json!(hi);
            doc["records"][0]["metrics"]["latency_ms"]=json!(lo);
            doc["records"][1]["metrics"]["latency_ms"]=json!(hi);
            let result=project(&doc).unwrap();
            assert_eq!(result["points"][0]["v"],0.0);assert_eq!(result["points"][1]["v"],1.0);
        }
    }
}

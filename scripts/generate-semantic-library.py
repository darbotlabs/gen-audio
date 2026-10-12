"""Generate the portable semantic cube contracts and explicitly authored examples.

No network, data feeds, or generated observations. Domain metrics use display
numbers; financial records are chart observations, never order/ledger amounts.
"""
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BASE = "https://darbotlabs.dev/gen-audio/schemas/"


def obj(properties, required=None):
    return {"type": "object", "properties": properties,
            "required": list(properties) if required is None else required,
            "additionalProperties": False}


def number(lo=-1e15, hi=1e15):
    return {"type": "number", "minimum": lo, "maximum": hi}


def text(limit=120):
    return {"type": "string", "minLength": 1, "maxLength": limit,
            "pattern": r"^[^\u0000-\u001f\u007f]+$"}


ID = {"type": "string", "pattern": "^[a-z][a-z0-9_.-]{0,63}$"}
COLOR = {"type": "string", "pattern": "^#[0-9a-fA-F]{6}$"}
STAMP = {"type": "string", "maxLength": 24, "format": "date-time",
         "pattern": r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d{3})?Z$"}

# Every metric has a declared unit, legal observation range, and default view
# range. Views can choose their own ranges; records retain their raw values.
DOMAINS = {
    "agent.session": {
        "title": "Agent sessions", "x": ("Elapsed time", "s", 0, 60), "y": ("Agent index", "index", 0, 4),
        "context": obj({"session_id": text(), "agent_name": text(),
                        "state": {"enum": ["queued", "running", "completed", "failed", "cancelled"]}}),
        "metrics": {"latency_ms": ("Latency", "ms", 0, 86400000, 2000), "tokens": ("Tokens", "count", 0, 1e12, 4096),
                    "cost_usd": ("Cost", "USD", 0, 1e12, 1), "confidence": ("Confidence", "ratio", 0, 1, 1)},
        "context_example": {"session_id": "example-session", "agent_name": "Alice", "state": "running"},
    },
    "storyline": {
        "title": "Storyline", "x": ("Story time", "s", 0, 120), "y": ("Scene index", "index", 0, 8),
        "context": obj({"story_id": text(), "scene_id": text(), "beat": text(240)}),
        "metrics": {"tension": ("Tension", "ratio", 0, 1, 1), "pace": ("Pace", "ratio", 0, 1, 1),
                    "sentiment": ("Sentiment", "score", -1, 1, 1)},
        "context_example": {"story_id": "example-story", "scene_id": "scene-1", "beat": "Opening"},
    },
    "camera": {
        "title": "Camera angles", "x": ("Shot time", "s", 0, 30), "y": ("Camera index", "index", 0, 4),
        "context": obj({"shot_id": text(), "camera_id": text(), "coordinate_system": {"const": "right-handed-y-up"}}),
        "metrics": {"yaw_deg": ("Yaw", "deg", -180, 180, 180), "pitch_deg": ("Pitch", "deg", -90, 90, 90),
                    "roll_deg": ("Roll", "deg", -180, 180, 180), "fov_deg": ("Field of view", "deg", 1, 179, 179)},
        "context_example": {"shot_id": "shot-1", "camera_id": "camera-a", "coordinate_system": "right-handed-y-up"},
    },
}
for domain, title, context in [
    ("market.stock", "Stock observations", {"symbol": "EXAMPLE", "venue": "EXAMPLE", "currency": "USD"}),
    ("market.crypto", "Crypto observations", {"base_asset": "EXAMPLE", "quote_asset": "USD", "venue": "EXAMPLE"}),
]:
    DOMAINS[domain] = {
        "title": title, "x": ("Unix time", "unix_s", 1700000000, 1700000300), "y": ("Instrument index", "index", 0, 4),
        "context": obj({**{key: text(32) for key in context}, "timestamp": STAMP}),
        "metrics": {**{key: (key.capitalize(), "quote", 0, 1e15, 200) for key in ["open", "high", "low", "close"]},
                    "volume": ("Volume", "base", 0, 1e15, 10000), "change_pct": ("Change", "%", -100, 1e15, 100)},
        "context_example": {**context, "timestamp": "2023-11-14T22:13:20Z"},
    }


def contracts():
    numeric_values = {"type": "object", "minProperties": 1, "maxProperties": 32,
                      "propertyNames": ID, "additionalProperties": {"anyOf": [number(), {"type": "null"}]}}
    record = obj({"id": ID, "x": number(), "y": number(), "metrics": numeric_values,
                  "context": {"type": "object", "maxProperties": 16, "propertyNames": ID,
                              "additionalProperties": text(240)}}, ["id", "x", "y", "metrics", "context"])
    records_schema = {"$schema": "https://json-schema.org/draft/2020-12/schema",
                      "$id": BASE + "semantic-record.schema.json", "$defs": {"record": record}}
    for name, spec in DOMAINS.items():
        metrics = obj({key: {"anyOf": [number(metric[2], metric[3]), {"type": "null"}]}
                       for key, metric in spec["metrics"].items()}, [])
        metrics["minProperties"] = 1
        if name == "agent.session":
            metrics["properties"]["tokens"]["anyOf"][0]["type"] = "integer"
        if name.startswith("market."):
            metrics["required"] = ["open", "high", "low", "close", "volume"]
            # A candle must contain measured prices; missing candles are omitted.
            for key in metrics["required"]:
                metrics["properties"][key] = number(0, 1e15)
        records_schema["$defs"][name] = {"allOf": [{"$ref": "#/$defs/record"},
            {"type": "object", "properties": {"context": spec["context"], "metrics": metrics}}]}

    axis = obj({"label": text(64), "unit": text(24), "minimum": number(), "maximum": number()})
    layer = obj({"id": ID, "label": text(64), "metric": ID, "unit": text(24),
                 "minimum": number(), "maximum": number(), "scale": {"enum": ["linear", "log"]},
                 "outside": {"enum": ["reject", "clip"]}, "position": number(0, 1), "color": COLOR,
                 "opacity": number(0, 1), "gain": number(0.25, 3), "blend": {"enum": ["normal", "add", "multiply"]},
                 "enabled": {"type": "boolean"}})
    schema = {"$schema": "https://json-schema.org/draft/2020-12/schema",
              "$id": BASE + "semantic-cube.schema.json", "title": "Semantic metric cube v1",
              "description": "Portable observations and authored spatial mappings. No code, URLs, implicit metrics, or live data feeds. Null observations are omitted from rendering. At most 1 MiB UTF-8 per document.",
              **obj({"schema_version": {"const": "1.0.0"}, "kind": {"const": "semantic_cube"}, "id": ID, "title": text(),
                     "domain": {"enum": [*DOMAINS, "custom"]},
                     "source": obj({"kind": {"enum": ["measured", "authored", "example"]}, "label": text(240)}),
                     "axes": obj({"x": axis, "y": axis}), "layers": {"type": "array", "minItems": 1, "maxItems": 16, "items": layer},
                     "records": {"type": "array", "minItems": 1, "maxItems": 2048,
                                 "items": {"$ref": BASE + "semantic-record.schema.json#/$defs/record"}}}),
              "allOf": []}
    for name in DOMAINS:
        schema["allOf"].append({"if": {"properties": {"domain": {"const": name}}}, "then": {
            "properties": {"records": {"items": {"$ref": BASE + "semantic-record.schema.json#/$defs/" + name}},
                           "layers": {"items": {"properties": {"metric": {"enum": list(DOMAINS[name]["metrics"])}},
                                "allOf": [{"if": {"properties": {"metric": {"const": key}}},
                                           "then": {"properties": {"unit": {"const": metric[1]}}}}
                                          for key, metric in DOMAINS[name]["metrics"].items()]}}}}})
    return records_schema, schema


def examples():
    colors = ["#248BFF", "#29DE5C", "#FFBF1F", "#F0458F", "#AB8AFF", "#35D7D0"]
    docs = []
    for domain, spec in DOMAINS.items():
        layers = []
        for i, (key, (label, unit, lo, _, view_max)) in enumerate(spec["metrics"].items()):
            layers.append({"id": key, "label": label, "metric": key, "unit": unit, "minimum": lo,
                           "maximum": view_max, "scale": "linear", "outside": "reject",
                           "position": i / max(1, len(spec["metrics"]) - 1), "color": colors[i % len(colors)],
                           "opacity": 1, "gain": 1, "blend": "normal", "enabled": True})
        records = []
        for i in range(12):
            metrics = {layer["metric"]: layer["minimum"] + (layer["maximum"] - layer["minimum"]) * (i + 1) / 13 for layer in layers}
            if domain == "agent.session":
                metrics["tokens"] = round(metrics["tokens"])
            if domain.startswith("market."):
                metrics.update(open=100+i, low=98+i, high=104+i, close=102+i, volume=500+i*100, change_pct=2)
            x = spec["x"][2] + (spec["x"][3]-spec["x"][2]) * i / 12
            records.append({"id": f"observation-{i+1}", "x": x, "y": i % 3, "metrics": metrics, "context": dict(spec["context_example"])})
            if domain.startswith("market."):
                from datetime import datetime, timezone
                records[-1]["context"]["timestamp"] = datetime.fromtimestamp(x, timezone.utc).isoformat().replace("+00:00", "Z")
        docs.append({"schema_version": "1.0.0", "kind": "semantic_cube", "id": domain + ".example",
                     "title": spec["title"] + " · example", "domain": domain,
                     "source": {"kind": "example", "label": "Authored schema example; replace with your observations"},
                     "axes": {axis: dict(zip(["label", "unit", "minimum", "maximum"], spec[axis])) for axis in ["x", "y"]},
                     "layers": layers, "records": records})
    return docs


if __name__ == "__main__":
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    records, cube = contracts()
    files = {"schemas/semantic-record.schema.json": records, "schemas/semantic-cube.schema.json": cube,
             "schemas/examples/semantic-cubes.json": examples()}
    for name, doc in files.items():
        output = json.dumps(doc, ensure_ascii=False, indent=2, allow_nan=False) + "\n"
        path = ROOT / name
        if args.check:
            if path.read_text(encoding="utf-8") != output:
                raise SystemExit(f"Stale semantic contract: {name}")
        else:
            path.write_text(output, encoding="utf-8")

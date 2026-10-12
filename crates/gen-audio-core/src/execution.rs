//! Shared requested execution policy. Availability is established in the
//! isolated runtime; accepting a policy never asserts that a model ran there.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::OnceLock;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Execution {
    pub backend: String,
    pub device: String,
    pub device_index: u8,
    pub precision: String,
    pub attention: String,
    pub fallback: String,
}

impl Default for Execution {
    fn default() -> Self {
        Self { backend: "auto".into(), device: "auto".into(), device_index: 0,
            precision: "auto".into(), attention: "auto".into(), fallback: "error".into() }
    }
}

pub fn schema() -> Value {
    serde_json::from_str(include_str!("../../../schemas/execution.schema.json")).expect("compiled execution schema")
}

impl Execution {
    pub fn parse(value: &Value) -> Result<Self, String> {
        static VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();
        if !VALIDATOR.get_or_init(|| jsonschema::validator_for(&schema()).expect("execution schema")).is_valid(value) {
            return Err("Invalid execution policy; use the execution schema".into());
        }
        let result: Self = serde_json::from_value(value.clone()).map_err(|_| "Invalid execution policy")?;
        let device = match result.backend.as_str() {
            "cpu" => Some("cpu"), "cuda" | "rocm" | "directml" => Some("gpu"), "vitisai" => Some("npu"), _ => None,
        };
        if device.is_some_and(|want| result.device != "auto" && result.device != want)
            || ((result.backend == "cpu" || result.device == "cpu") && result.device_index != 0) {
            return Err("Execution backend, device and index disagree".into());
        }
        Ok(result)
    }

    pub fn configured(prefix: &str) -> Result<Self, String> {
        let value = crate::runtime_config::setting(&format!("{prefix}_EXECUTION"))
            .or_else(|| crate::runtime_config::setting("GEN_AUDIO_EXECUTION"));
        match value {
            Some(raw) => Self::parse(&serde_json::from_str(&raw.to_string_lossy()).map_err(|_| "Configured execution must be valid JSON")?),
            None => Self::parse(&json!({"backend":"cpu"})),
        }
    }

    pub fn verify_result(&self, provenance: &Value) -> bool {
        let resolved = &provenance["execution"];
        // Backward-compatible CPU receipts remain valid for the original policy.
        if resolved.is_null() {
            return self == &Self { backend: "cpu".into(), ..Self::default() }
                && provenance["device"] == "cpu" && provenance["dtype"] == "float32";
        }
        if resolved["requested"] != serde_json::to_value(self).expect("execution serialization") { return false; }
        let Some(backend) = resolved["backend"].as_str() else { return false; };
        let cpu = backend == "cpu";
        if !matches!(backend, "cpu" | "rocm" | "cuda") { return false; }
        if self.backend != "auto" && self.backend != backend
            && !(cpu && self.fallback == "cpu" && resolved["fallback_reason"].as_str().is_some_and(|s| !s.is_empty())) { return false; }
        if self.device != "auto" && self.device != if cpu { "cpu" } else { "gpu" }
            && !(cpu && self.fallback == "cpu") { return false; }
        let device = if cpu { "cpu".into() } else { format!("cuda:{}", self.device_index) };
        let precision = provenance["dtype"].as_str().unwrap_or("");
        provenance["device"] == device && resolved["torch_device"] == device
            && resolved["device"] == if cpu { "cpu" } else { "gpu" }
            && resolved["precision"] == precision
            && resolved["attention"] == self.attention
            && (if self.precision == "auto" { matches!(precision, "float32" | "float16" | "bfloat16") && (!cpu || precision != "float16") }
                else { precision == self.precision })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_shared_policy_and_preserves_cpu_receipts() {
        for value in [json!({"backend":"cpu","device":"gpu"}), json!({"device_index":16}),
            json!({"precision":null}), json!({"python":"untrusted"}), json!({"backend":"cpu","device_index":1})] {
            assert!(Execution::parse(&value).is_err());
        }
        let cpu = Execution::parse(&json!({"backend":"cpu"})).unwrap();
        assert!(cpu.verify_result(&json!({"device":"cpu","dtype":"float32"})));
        let gpu = Execution::parse(&json!({"backend":"rocm","precision":"bfloat16"})).unwrap();
        let result = json!({"device":"cuda:0","dtype":"bfloat16","execution":{
            "requested":gpu,"backend":"rocm","device":"gpu","torch_device":"cuda:0","precision":"bfloat16","attention":"auto"}});
        assert!(gpu.verify_result(&result));
        assert!(!cpu.verify_result(&result));
    }
}

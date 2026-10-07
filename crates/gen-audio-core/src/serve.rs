//! Per-node Gen-Audio URL builder. Does not open a socket.

pub const SERVICE_NAME: &str = "genaid-audio";
pub const DEFAULT_PORT: u16 = 8002;
pub const ROUTE_PREFIX: &str = "/genaid-audio";
pub const SHARED_GATEWAY_HOST: &str = "10.1.8.70";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PowerRow {
    pub id: String,
    pub role: String,
    pub host: String,
    pub port: u16,
    pub base_url: String,
    pub health_url: String,
}

pub fn node_base_url(host: &str, port: u16) -> Result<String, String> {
    let bare = bare_host(host)?;
    Ok(format!("http://{bare}:{port}{ROUTE_PREFIX}"))
}

pub fn health_url(host: &str, port: u16) -> Result<String, String> {
    Ok(format!("{}/health", node_base_url(host, port)?))
}

pub fn ready_url(host: &str, port: u16) -> Result<String, String> {
    Ok(format!("{}/ready", node_base_url(host, port)?))
}

pub fn health_payload() -> serde_json::Value {
    serde_json::json!({
        "status": "ok",
        "service": SERVICE_NAME,
        "route_prefix": ROUTE_PREFIX
    })
}

pub fn power_row(id: &str, host: &str, role: &str, port: u16) -> Result<PowerRow, String> {
    let label = id.trim();
    if label.is_empty() {
        return Err("node id is empty".into());
    }
    let duty = role.trim();
    if duty != "node" && duty != "shared-gateway" {
        return Err("role must be node or shared-gateway".into());
    }
    if port == 0 {
        return Err("port must be between 1 and 65535".into());
    }
    let base_url = node_base_url(host, port)?;
    Ok(PowerRow {
        id: label.to_string(),
        role: duty.to_string(),
        host: bare_host(host)?,
        port,
        health_url: format!("{base_url}/health"),
        base_url,
    })
}

/// Probe targets are loopback, the documented shared gateway, or an explicit
/// `GEN_AUDIO_PROBE_HOSTS` entry. Arbitrary tool hosts are refused.
pub fn probe_host_allowed(host: &str) -> Result<(), String> {
    let bare = bare_host(host)?;
    let inner = bare.trim_start_matches('[').trim_end_matches(']');
    if inner == "127.0.0.1" || inner == "localhost" || inner == "::1" || bare == SHARED_GATEWAY_HOST {
        return Ok(());
    }
    if let Ok(list) = std::env::var("GEN_AUDIO_PROBE_HOSTS") {
        if list.split(',').any(|item| item.trim() == bare) {
            return Ok(());
        }
    }
    Err("probe host is not allowlisted (loopback, the shared gateway, or GEN_AUDIO_PROBE_HOSTS)".into())
}

pub fn shared_gateway_row() -> PowerRow {
    power_row("shared-gateway", SHARED_GATEWAY_HOST, "shared-gateway", DEFAULT_PORT)
        .expect("shared gateway constants are valid")
}

fn bare_host(host: &str) -> Result<String, String> {
    let value = host.trim();
    if value.is_empty() {
        return Err("host is empty".into());
    }
    if value.contains("://") || value.contains('/') {
        return Err("pass a host such as 10.1.8.21, not a URL".into());
    }
    if value.contains(':') && !(value.starts_with('[') && value.ends_with(']')) {
        return Err("pass the port separately, not inside the host".into());
    }
    Ok(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_and_gateway_are_different_rows() {
        let node = power_row("n", "10.1.8.21", "node", 8002).unwrap();
        let shared = shared_gateway_row();
        assert_eq!(node.health_url, "http://10.1.8.21:8002/genaid-audio/health");
        assert_ne!(node.base_url, shared.base_url);
        assert!(shared.health_url.ends_with("/genaid-audio/health"));
        assert_eq!(
            ready_url("10.1.8.21", 8002).unwrap(),
            "http://10.1.8.21:8002/genaid-audio/ready"
        );
        assert!(node_base_url("http://10.1.8.21", 8002).is_err());
        assert!(node_base_url("10.1.8.21:8002", 8002).is_err());
        assert!(probe_host_allowed("127.0.0.1").is_ok());
        assert!(probe_host_allowed(SHARED_GATEWAY_HOST).is_ok());
        assert!(probe_host_allowed("169.254.169.254").is_err());
        assert!(probe_host_allowed("example.com").is_err());
    }
}

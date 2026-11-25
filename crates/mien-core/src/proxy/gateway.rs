//! Xray bootstrap configuration and stable resource tags.
use super::{api::XrayApi, engine::EngineController};
use crate::error::CoreError;
use serde_json::{Value, json};

/// Build the minimum engine config. Port 0 is a request to allocate a loopback
/// port; process startup replaces it because Xray v26.11.9 does not bind port 0.
pub fn bootstrap_config(api_port: u16) -> Result<Value, CoreError> {
    let mut c = json!({"log":{"loglevel":"warning"},"api":{"tag":"api","services":["HandlerService","RoutingService"]},"inbounds":[{"tag":"api","listen":"127.0.0.1","port":api_port,"protocol":"dokodemo-door","settings":{"address":"127.0.0.1","network":"tcp"}}],"outbounds":[{"tag":"block","protocol":"blackhole"}],"routing":{"domainStrategy":"AsIs","rules":[{"type":"field","inboundTag":["api"],"outboundTag":"api"}]}});
    if api_port == 0 {
        c["inbounds"][0]["port"] = json!(0)
    }
    verify_bootstrap_config(&c)?;
    Ok(c)
}
pub fn config_with_api_port(api_port: u16) -> Result<Value, CoreError> {
    if api_port == 0 {
        return Err(CoreError::Validation(
            "PROXY_CONFIG_INVALID:api_port".into(),
        ));
    }
    let mut c = bootstrap_config(0)?;
    c["inbounds"][0]["port"] = json!(api_port);
    verify_bootstrap_config(&c)?;
    Ok(c)
}
pub fn verify_bootstrap_config(c: &Value) -> Result<(), CoreError> {
    let outs = c["outbounds"]
        .as_array()
        .map(|x| {
            x.iter()
                .filter_map(|o| o["tag"].as_str())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if outs.first().copied() != Some("block")
        || outs.len() != 1
        || c["inbounds"].as_array().is_none_or(|a| a.len() != 1)
        || c["inbounds"][0]["listen"] != "127.0.0.1"
        || c["inbounds"][0]["port"].as_u64().is_none()
        || c["routing"].get("final").is_some()
        || c["routing"].get("script").is_some()
        || c["routing"].get("balancers").is_some()
    {
        return Err(CoreError::Validation("PROXY_ENGINE_START_FAILED".into()));
    }
    Ok(())
}
pub fn inbound_tag(id: &str) -> String {
    format!("rt:{id}:in")
}
pub fn rule_tag(id: &str) -> String {
    format!("rt:{id}:rule")
}
pub fn proxy_server_arg(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}
pub fn verify_binary_sha256(path: &std::path::Path, expected: &str) -> Result<(), CoreError> {
    if !path.is_file() {
        return Err(CoreError::Validation("PROXY_ENGINE_NOT_FOUND".into()));
    }
    let bytes =
        std::fs::read(path).map_err(|_| CoreError::Validation("PROXY_ENGINE_NOT_FOUND".into()))?;
    let actual = super::keys::sha256(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    if actual != expected {
        return Err(CoreError::Validation(
            "PROXY_ENGINE_CHECKSUM_MISMATCH".into(),
        ));
    }
    Ok(())
}
pub fn assert_bootstrap<A: XrayApi>(e: &EngineController<A>, c: &Value) -> Result<(), CoreError> {
    verify_bootstrap_config(c)?;
    let tags = c["outbounds"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x["tag"].as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    e.assert_bootstrap(&tags, &c["routing"])
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bootstrap_is_loopback_block_first_and_ephemeral_request() {
        let c = bootstrap_config(0).unwrap();
        assert_eq!(c["inbounds"][0]["listen"], "127.0.0.1");
        assert_eq!(c["inbounds"][0]["port"], 0);
        assert_eq!(c["outbounds"][0]["tag"], "block");
        assert!(verify_bootstrap_config(&c).is_ok())
    }
    #[test]
    fn allocated_config_is_fixed_loopback() {
        let c = config_with_api_port(12345).unwrap();
        assert_eq!(c["inbounds"][0]["port"], 12345)
    }
    #[test]
    fn bad_bootstrap_rejected() {
        let mut c = bootstrap_config(0).unwrap();
        c["outbounds"] = json!([{"tag":"direct"},{"tag":"block"}]);
        assert!(verify_bootstrap_config(&c).is_err())
    }
    #[test]
    fn tags_and_proxy_arg() {
        assert_eq!(inbound_tag("x"), "rt:x:in");
        assert_eq!(rule_tag("x"), "rt:x:rule");
        assert_eq!(proxy_server_arg(9), "http://127.0.0.1:9")
    }
}

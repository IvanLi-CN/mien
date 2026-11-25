//! Proxy configuration normalization and shared outbound identity.

use std::{collections::BTreeMap, fmt};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::error::CoreError;

/// Protocols handled by Xray. TUIC remains representable so it can be rejected explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyProtocol {
    Vless,
    Vmess,
    Shadowsocks,
    Trojan,
    Hysteria2,
    Wireguard,
    Http,
    Https,
    Socks5,
    Tuic,
}

impl ProxyProtocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Vless => "vless",
            Self::Vmess => "vmess",
            Self::Shadowsocks => "shadowsocks",
            Self::Trojan => "trojan",
            Self::Hysteria2 => "hysteria2",
            Self::Wireguard => "wireguard",
            Self::Http => "http",
            Self::Https => "https",
            Self::Socks5 => "socks5",
            Self::Tuic => "tuic",
        }
    }
}

/// Xray stream transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    #[default]
    Tcp,
    Ws,
    Grpc,
    Quic,
    Hysteria,
}

impl Transport {
    fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Ws => "ws",
            Self::Grpc => "grpc",
            Self::Quic => "quic",
            Self::Hysteria => "hysteria",
        }
    }
}

/// Non-secret TLS and REALITY fields. Every field contributes to the outbound key.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TlsSettings {
    pub server_name: Option<String>,
    pub fingerprint: Option<String>,
    pub allow_insecure: bool,
    pub reality_public_key: Option<String>,
    pub reality_short_id: Option<String>,
}

/// Xray mux options. Xray v26.11.9 normalizes enabled concurrency to at least 128.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MuxSettings {
    pub enabled: bool,
    pub concurrency: u16,
}

impl MuxSettings {
    /// Effective setting reported and used by Xray v26.11.9.
    pub fn effective(self) -> Self {
        if self.enabled {
            Self {
                enabled: true,
                concurrency: self.concurrency.max(128),
            }
        } else {
            Self {
                enabled: false,
                concurrency: 0,
            }
        }
    }
}

/// Credentials stay in process memory and `Debug` exposes field names only.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Credentials {
    pub values: BTreeMap<String, String>,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Credentials")
            .field("fields", &self.values.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Proxy input. Credentials are deliberately excluded from serialization and persistence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyConfig {
    pub protocol: ProxyProtocol,
    pub server: String,
    pub port: u16,
    #[serde(default)]
    pub transport: Transport,
    #[serde(default)]
    pub tls: TlsSettings,
    #[serde(default)]
    pub mux: MuxSettings,
    #[serde(default)]
    pub options: BTreeMap<String, String>,
    #[serde(skip)]
    pub credentials: Credentials,
}

impl ProxyConfig {
    pub fn validate(&self) -> Result<(), CoreError> {
        if self.server.trim().is_empty() || self.port == 0 {
            return Err(config_error("server and port are required"));
        }
        if self.protocol == ProxyProtocol::Tuic {
            return Err(CoreError::Validation("TUIC_NOT_SUPPORTED".into()));
        }
        if self.protocol == ProxyProtocol::Hysteria2 {
            if self.transport != Transport::Hysteria {
                return Err(config_error("HYSTERIA2_REQUIRES_HYSTERIA_TRANSPORT"));
            }
            if self.tls.server_name.as_deref().is_none_or(str::is_empty) {
                return Err(config_error("HYSTERIA2_TLS_REQUIRED"));
            }
        }
        Ok(())
    }
}

/// Stable outbound identity (`out:` followed by 64 lowercase hexadecimal characters).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OutboundKey(String);

impl OutboundKey {
    pub fn new(value: String) -> Result<Self, CoreError> {
        if value.len() != 68
            || !value.starts_with("out:")
            || !value[4..].bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(config_error("invalid outbound key"));
        }
        Ok(Self(value.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for OutboundKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Process-local Xray configuration for `AddOutbound`; never persist this value.
#[derive(Debug, Clone, PartialEq)]
pub struct OutboundSpec {
    pub key: OutboundKey,
    pub tag: String,
    pub protocol: String,
    pub settings: Value,
    pub stream_settings: Value,
    pub mux: MuxSettings,
}

/// Compute the key from node, HMACed credentials, transport, TLS/REALITY and effective mux.
pub fn outbound_key(config: &ProxyConfig, master_key: &[u8]) -> Result<OutboundKey, CoreError> {
    config.validate()?;
    let credentials = serde_json::to_vec(&config.credentials.values)
        .map_err(|_| config_error("credentials cannot be normalized"))?;
    let credential_fingerprint = format!(
        "hmac-sha256:{}",
        hex_encode(&hmac_sha256(master_key, &credentials))
    );
    let tls = tls_canonical_value(&config.tls);
    let tls_fingerprint = format!(
        "sha256:{}",
        hex_encode(&sha256(
            &serde_json::to_vec(&tls).expect("TLS JSON value is serializable")
        ))
    );
    let mut material = Map::new();
    material.insert(
        "credential_fingerprint".into(),
        Value::String(credential_fingerprint),
    );
    material.insert(
        "mux".into(),
        json!({
            "enabled": config.mux.effective().enabled,
            "concurrency": config.mux.effective().concurrency
        }),
    );
    material.insert(
        "options".into(),
        serde_json::to_value(&config.options).expect("options are serializable"),
    );
    material.insert("port".into(), Value::from(config.port));
    material.insert(
        "protocol".into(),
        Value::String(config.protocol.as_str().into()),
    );
    material.insert(
        "security".into(),
        Value::String(security_name(&config.tls).into()),
    );
    material.insert("server".into(), Value::String(config.server.clone()));
    material.insert("tls_fingerprint".into(), Value::String(tls_fingerprint));
    material.insert(
        "transport".into(),
        Value::String(config.transport.as_str().into()),
    );
    let bytes = serde_json::to_vec(&Value::Object(material))
        .expect("normalized outbound key material is serializable");
    Ok(OutboundKey(format!("out:{}", hex_encode(&sha256(&bytes)))))
}

/// Build a process-local Xray outbound; sensitive fields exist only in the returned value.
pub fn build_outbound(config: &ProxyConfig, master_key: &[u8]) -> Result<OutboundSpec, CoreError> {
    let key = outbound_key(config, master_key)?;
    let credential = |name: &str| config.credentials.values.get(name).cloned();
    let (protocol, settings) = match config.protocol {
        ProxyProtocol::Vless => {
            let id = credential("id").ok_or_else(|| config_error("VLESS_CREDENTIAL_REQUIRED"))?;
            (
                "vless",
                json!({ "vnext": [{ "address": config.server, "port": config.port, "users": [{ "id": id, "encryption": "none" }] }] }),
            )
        }
        ProxyProtocol::Vmess => {
            let id = credential("id").ok_or_else(|| config_error("VMESS_CREDENTIAL_REQUIRED"))?;
            (
                "vmess",
                json!({ "vnext": [{ "address": config.server, "port": config.port, "users": [{ "id": id, "security": "auto" }] }] }),
            )
        }
        ProxyProtocol::Shadowsocks => {
            let password = credential("password")
                .ok_or_else(|| config_error("SHADOWSOCKS_CREDENTIAL_REQUIRED"))?;
            let method = config
                .options
                .get("method")
                .cloned()
                .unwrap_or_else(|| "aes-128-gcm".into());
            (
                "shadowsocks",
                json!({ "servers": [{ "address": config.server, "port": config.port, "method": method, "password": password }] }),
            )
        }
        ProxyProtocol::Trojan => {
            let password =
                credential("password").ok_or_else(|| config_error("TROJAN_CREDENTIAL_REQUIRED"))?;
            (
                "trojan",
                json!({ "servers": [{ "address": config.server, "port": config.port, "password": password }] }),
            )
        }
        ProxyProtocol::Hysteria2 => {
            let auth =
                credential("auth").ok_or_else(|| config_error("HYSTERIA2_CREDENTIAL_REQUIRED"))?;
            (
                "hysteria",
                json!({ "version": 2, "address": config.server, "port": config.port, "auth": auth }),
            )
        }
        ProxyProtocol::Wireguard => ("wireguard", basic_settings(config)),
        ProxyProtocol::Http | ProxyProtocol::Https => ("http", basic_settings(config)),
        ProxyProtocol::Socks5 => ("socks", basic_settings(config)),
        ProxyProtocol::Tuic => return Err(CoreError::Validation("TUIC_NOT_SUPPORTED".into())),
    };
    let stream_settings = build_stream_settings(config);
    Ok(OutboundSpec {
        key: key.clone(),
        tag: key.to_string(),
        protocol: protocol.into(),
        settings,
        stream_settings,
        mux: config.mux.effective(),
    })
}

fn basic_settings(config: &ProxyConfig) -> Value {
    let mut settings = Map::new();
    settings.insert("servers".into(), json!([{
        "address": config.server,
        "port": config.port,
        "users": config.credentials.values.get("username").zip(config.credentials.values.get("password")).map(|(user, pass)| json!([{ "user": user, "pass": pass }])).unwrap_or_else(|| json!([]))
    }]));
    Value::Object(settings)
}

fn build_stream_settings(config: &ProxyConfig) -> Value {
    if config.protocol == ProxyProtocol::Hysteria2 {
        return json!({
            "network": "hysteria",
            "security": "tls",
            "tlsSettings": {
                "serverName": config.tls.server_name,
                "allowInsecure": config.tls.allow_insecure,
                "fingerprint": config.tls.fingerprint
            },
            "hysteriaSettings": { "version": 2 }
        });
    }
    if config.tls.reality_public_key.is_some() {
        return json!({
            "network": config.transport.as_str(),
            "security": "reality",
            "realitySettings": {
                "serverName": config.tls.server_name,
                "fingerprint": config.tls.fingerprint,
                "publicKey": config.tls.reality_public_key,
                "shortId": config.tls.reality_short_id
            }
        });
    }
    if config.tls.server_name.is_some() {
        return json!({
            "network": config.transport.as_str(),
            "security": "tls",
            "tlsSettings": {
                "serverName": config.tls.server_name,
                "allowInsecure": config.tls.allow_insecure,
                "fingerprint": config.tls.fingerprint
            }
        });
    }
    if config.transport == Transport::Tcp {
        Value::Null
    } else {
        json!({ "network": config.transport.as_str() })
    }
}

fn tls_canonical_value(tls: &TlsSettings) -> Value {
    let mut value = Map::new();
    value.insert("allow_insecure".into(), Value::Bool(tls.allow_insecure));
    value.insert("fingerprint".into(), optional_value(&tls.fingerprint));
    value.insert(
        "reality_public_key".into(),
        optional_value(&tls.reality_public_key),
    );
    value.insert(
        "reality_short_id".into(),
        optional_value(&tls.reality_short_id),
    );
    value.insert("server_name".into(), optional_value(&tls.server_name));
    Value::Object(value)
}

fn security_name(tls: &TlsSettings) -> &'static str {
    if tls.reality_public_key.is_some() {
        "reality"
    } else if tls.server_name.is_some() {
        "tls"
    } else {
        "none"
    }
}

fn optional_value(value: &Option<String>) -> Value {
    value.clone().map_or(Value::Null, Value::String)
}

fn config_error(message: &str) -> CoreError {
    CoreError::Validation(format!("PROXY_CONFIG_INVALID:{message}"))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut key_block = [0u8; 64];
    if key.len() > key_block.len() {
        key_block[..32].copy_from_slice(&sha256(key));
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut inner = [0x36u8; 64];
    let mut outer = [0x5cu8; 64];
    for index in 0..64 {
        inner[index] ^= key_block[index];
        outer[index] ^= key_block[index];
    }
    let mut inner_data = Vec::with_capacity(64 + message.len());
    inner_data.extend_from_slice(&inner);
    inner_data.extend_from_slice(message);
    let inner_hash = sha256(&inner_data);
    let mut outer_data = Vec::with_capacity(96);
    outer_data.extend_from_slice(&outer);
    outer_data.extend_from_slice(&inner_hash);
    sha256(&outer_data)
}

/// SHA-256 used for stable key material and binary integrity checks.
pub(crate) fn sha256(input: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut state = [
        0x6a09e667u32,
        0xbb67ae85,
        0x3c6ef372,
        0xa54ff53a,
        0x510e527f,
        0x9b05688c,
        0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_len = (input.len() as u64).wrapping_mul(8);
    let block_count = (input.len() + 9).div_ceil(64);
    let mut padded = vec![0u8; block_count * 64];
    padded[..input.len()].copy_from_slice(input);
    padded[input.len()] = 0x80;
    let length_start = padded.len() - 8;
    padded[length_start..].copy_from_slice(&bit_len.to_be_bytes());
    for chunk in padded.chunks_exact(64) {
        let mut words = [0u32; 64];
        for (index, word) in words.iter_mut().enumerate().take(16) {
            let offset = index * 4;
            *word = u32::from_be_bytes(chunk[offset..offset + 4].try_into().expect("four bytes"));
        }
        for index in 16..64 {
            let x = words[index - 15];
            let y = words[index - 2];
            let s0 = x.rotate_right(7) ^ x.rotate_right(18) ^ (x >> 3);
            let s1 = y.rotate_right(17) ^ y.rotate_right(19) ^ (y >> 10);
            words[index] = words[index - 16]
                .wrapping_add(s0)
                .wrapping_add(words[index - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h) = (
            state[0], state[1], state[2], state[3], state[4], state[5], state[6], state[7],
        );
        for index in 0..64 {
            let sum1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choose = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(sum1)
                .wrapping_add(choose)
                .wrapping_add(K[index])
                .wrapping_add(words[index]);
            let sum0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = sum0.wrapping_add(majority);
            (h, g, f, e, d, c, b, a) = (
                g,
                f,
                e,
                d.wrapping_add(temp1),
                c,
                b,
                a,
                temp1.wrapping_add(temp2),
            );
        }
        for (slot, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }
    let mut output = [0u8; 32];
    for (index, word) in state.into_iter().enumerate() {
        output[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> ProxyConfig {
        ProxyConfig {
            protocol: ProxyProtocol::Vless,
            server: "node.example".into(),
            port: 443,
            transport: Transport::Tcp,
            tls: TlsSettings::default(),
            mux: MuxSettings {
                enabled: true,
                concurrency: 8,
            },
            options: BTreeMap::new(),
            credentials: Credentials {
                values: [("id".into(), "secret".into())].into_iter().collect(),
            },
        }
    }

    #[test]
    fn sha256_matches_standard_vector() {
        assert_eq!(
            hex_encode(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn mux_key_uses_effective_concurrency() {
        let requested = outbound_key(&config(), b"master").unwrap();
        let effective = outbound_key(
            &ProxyConfig {
                mux: MuxSettings {
                    enabled: true,
                    concurrency: 128,
                },
                ..config()
            },
            b"master",
        )
        .unwrap();
        assert_eq!(requested, effective);
    }

    #[test]
    fn credentials_are_hmaced_and_change_key() {
        let key = outbound_key(&config(), b"master").unwrap().to_string();
        assert!(!key.contains("secret"));
        let mut other = config();
        other
            .credentials
            .values
            .insert("id".into(), "different".into());
        assert_ne!(key, outbound_key(&other, b"master").unwrap().to_string());
    }

    #[test]
    fn tuic_has_stable_rejection() {
        let error = outbound_key(
            &ProxyConfig {
                protocol: ProxyProtocol::Tuic,
                ..config()
            },
            b"master",
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "validation failed: TUIC_NOT_SUPPORTED");
    }

    #[test]
    fn hysteria2_requires_transport_tls_and_builds_them() {
        let invalid = ProxyConfig {
            protocol: ProxyProtocol::Hysteria2,
            ..config()
        };
        assert!(outbound_key(&invalid, b"master").is_err());
        let valid = ProxyConfig {
            protocol: ProxyProtocol::Hysteria2,
            transport: Transport::Hysteria,
            tls: TlsSettings {
                server_name: Some("node.example".into()),
                ..TlsSettings::default()
            },
            credentials: Credentials {
                values: [("auth".into(), "secret".into())].into_iter().collect(),
            },
            ..config()
        };
        let spec = build_outbound(&valid, b"master").unwrap();
        assert_eq!(spec.protocol, "hysteria");
        assert_eq!(spec.stream_settings["network"], "hysteria");
        assert!(spec.stream_settings.get("hysteriaSettings").is_some());
        assert!(spec.stream_settings.get("tlsSettings").is_some());
        assert_eq!(spec.settings["auth"], "secret");
    }

    #[test]
    fn credentials_never_serialize() {
        let json = serde_json::to_string(&config()).unwrap();
        assert!(!json.contains("secret"));
        assert!(!format!("{:?}", config()).contains("secret"));
    }
}

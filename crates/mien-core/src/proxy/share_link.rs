//! 分享链接解析：单条 `vless`/`vmess`/`ss`/`trojan`/`hysteria2` 链接 → [`ProxyDraft`]。
//!
//! 契约：`docs/contracts/openapi.yaml` 的 `proxiesImportLink`/`ProxyDraft` 与
//! `docs/specs/proxy-gateway/SPEC.md` REQ-PROXY-015。解析是纯计算：不联网、不执行代码、
//! 不保存、不启动；草稿的 `Debug` 与序列化只暴露凭据字段名，绝不回显凭据、完整链接或
//! 链接中的值。
//!
//! 失败关闭：会静默降低安全性或改变线上语义的输入必须拒绝，而不是忽略——未知 `security`
//! （否则显式 TLS/REALITY 会退化成明文）、未知传输、REALITY 缺 `pbk`、Shadowsocks
//! `plugin`、超大链接与畸形 base64。可安全忽略的参数只产生 [`ProxyDraft::warnings`]，
//! 文本仅含参数名。
//!
//! 选项键（`options`，非秘密）：`transport`（`tcp`/`ws`/`grpc`/`hysteria`）、`security`
//! （`none`/`tls`/`reality`）、`sni`、`host`、`path`、`service_name`、`flow`、
//! `fingerprint`、`reality_pbk`、`reality_sid`、`allow_insecure`、`method`。
//! 凭据字段名：`id`（VLESS、VMess）、`password`（Shadowsocks、Trojan）、`auth`（Hysteria2）。

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD},
};
use serde::{Serialize, Serializer, ser::SerializeStruct};
use serde_json::Value;

use crate::error::{CoreError, ErrorCode};
use crate::ports::proxy::{ProxyInput, ProxyKind};

use super::keys::{Credentials, MuxSettings, ProxyConfig, ProxyProtocol, TlsSettings, Transport};

/// 单条链接上限，与 `proxiesImportLink` 的 `maxLength: 16384` 一致。
pub const MAX_LINK_BYTES: usize = 16_384;

/// 解析出的代理草稿。凭据只留在进程内，`Debug` 与序列化只含字段名。
#[derive(Clone, PartialEq, Eq)]
pub struct ProxyDraft {
    kind: ProxyKind,
    server: String,
    port: u16,
    options: BTreeMap<String, Value>,
    credentials: Credentials,
    credential_fields: Vec<String>,
    warnings: Vec<String>,
}

impl ProxyDraft {
    /// 代理种类，与 `proxies.kind` 一致。
    pub fn kind(&self) -> ProxyKind {
        self.kind
    }

    /// 节点主机名或 IP 字面量（IPv6 不带方括号）。
    pub fn server(&self) -> &str {
        &self.server
    }

    /// 节点端口。
    pub fn port(&self) -> u16 {
        self.port
    }

    /// 非秘密选项，键名见模块文档。
    pub fn options(&self) -> &BTreeMap<String, Value> {
        &self.options
    }

    /// 已设置的凭据字段名（升序），与 `ProxyView.credential_fields` 同语义。
    pub fn credential_fields(&self) -> &[String] {
        &self.credential_fields
    }

    /// 未应用但不影响安全的参数说明；文本只含参数名。
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// 组装共享引擎出站所需的 [`ProxyConfig`]；凭据随草稿进入进程内配置。
    ///
    /// # Errors
    /// 草稿选项被篡改为不受支持的值时返回 `PROXY_CONFIG_INVALID`。
    pub fn proxy_config(&self) -> Result<ProxyConfig, CoreError> {
        let transport = match self.option_str("transport") {
            Some("tcp") => Transport::Tcp,
            Some("ws") => Transport::Ws,
            Some("grpc") => Transport::Grpc,
            Some("hysteria") => Transport::Hysteria,
            _ => return Err(invalid("option `transport` is not supported")),
        };
        let mut options = BTreeMap::new();
        for key in ["path", "host", "service_name", "flow", "method"] {
            if let Some(value) = self.option_str(key) {
                options.insert(key.to_owned(), value.to_owned());
            }
        }
        let config = ProxyConfig {
            protocol: protocol_of(self.kind)?,
            server: self.server.clone(),
            port: self.port,
            transport,
            tls: TlsSettings {
                server_name: self.option_string("sni"),
                fingerprint: self.option_string("fingerprint"),
                allow_insecure: self
                    .options
                    .get("allow_insecure")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                reality_public_key: self.option_string("reality_pbk"),
                reality_short_id: self.option_string("reality_sid"),
            },
            mux: MuxSettings::default(),
            options,
            credentials: self.credentials.clone(),
        };
        config.validate()?;
        Ok(config)
    }

    /// 组装仓储写入用的 `ProxyInput`；名称由调用方给出，凭据仍是进程内值。
    ///
    /// # Errors
    /// 草稿种类不是分享链接协议（理论上不可达）时返回 `PROXY_CONFIG_INVALID`。
    pub fn proxy_input(&self, name: impl Into<String>) -> Result<ProxyInput, CoreError> {
        protocol_of(self.kind)?;
        Ok(ProxyInput {
            name: name.into(),
            kind: self.kind,
            server: self.server.clone(),
            port: self.port,
            options: self.options.clone(),
            credentials: self.credentials.values.clone(),
        })
    }

    fn option_str(&self, key: &str) -> Option<&str> {
        self.options.get(key).and_then(Value::as_str)
    }

    fn option_string(&self, key: &str) -> Option<String> {
        self.option_str(key).map(str::to_owned)
    }
}

impl fmt::Debug for ProxyDraft {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProxyDraft")
            .field("kind", &self.kind)
            .field("server", &self.server)
            .field("port", &self.port)
            .field("options", &self.options)
            .field("credential_fields", &self.credential_fields)
            .field("warnings", &self.warnings)
            .field("credentials", &self.credentials)
            .finish()
    }
}

/// 线上形状：`ProxyDraft{kind,server,port,options,credential_fields?,warnings?}`。
impl Serialize for ProxyDraft {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut draft = serializer.serialize_struct("ProxyDraft", 6)?;
        draft.serialize_field("kind", &self.kind)?;
        draft.serialize_field("server", &self.server)?;
        draft.serialize_field("port", &self.port)?;
        draft.serialize_field("options", &self.options)?;
        draft.serialize_field("credential_fields", &self.credential_fields)?;
        draft.serialize_field("warnings", &self.warnings)?;
        draft.end()
    }
}

/// 解析单条分享链接；不保存、不启动、不联网。
///
/// # Errors
/// 链接为空、超过 16384 字节、scheme 不受支持（含 `tuic`/`wireguard`）、必填字段缺失、
/// base64/JSON 畸形、传输或 `security` 不受支持时返回 [`ErrorCode::ProxyConfigInvalid`]；
/// 消息只含字段名，不含字段值。
pub fn parse_share_link(link: &str) -> Result<ProxyDraft, CoreError> {
    if link.len() > MAX_LINK_BYTES {
        return Err(invalid("link must be at most 16384 bytes"));
    }
    let link = link.trim();
    if link.is_empty() {
        return Err(invalid("link must not be empty"));
    }
    let (scheme, rest) = link
        .split_once("://")
        .ok_or_else(|| invalid("link must start with a supported scheme"))?;
    match scheme.to_ascii_lowercase().as_str() {
        "vless" => parse_vless(rest),
        "vmess" => parse_vmess(rest),
        "ss" => parse_ss(rest),
        "trojan" => parse_trojan(rest),
        "hysteria2" => parse_hysteria2(rest),
        "tuic" => Err(invalid("tuic share links are not supported")),
        "wireguard" => Err(invalid("wireguard share links are not supported")),
        _ => Err(invalid(
            "link scheme must be one of vless, vmess, ss, trojan, hysteria2",
        )),
    }
}

/// TLS 模式；未知取值必须拒绝，避免显式 TLS/REALITY 静默退化成明文。
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Security {
    #[default]
    None,
    Tls,
    Reality,
}

impl Security {
    fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Tls => "tls",
            Self::Reality => "reality",
        }
    }
}

fn parse_security(value: Option<&str>) -> Result<Security, CoreError> {
    match value.unwrap_or("none") {
        "" | "none" => Ok(Security::None),
        "tls" => Ok(Security::Tls),
        "reality" => Ok(Security::Reality),
        _ => Err(invalid("link `security` must be one of tls, reality, none")),
    }
}

fn parse_transport(value: Option<&str>) -> Result<Transport, CoreError> {
    match value.unwrap_or("tcp") {
        "" | "tcp" => Ok(Transport::Tcp),
        "ws" => Ok(Transport::Ws),
        "grpc" => Ok(Transport::Grpc),
        _ => Err(invalid("link `type` must be one of tcp, ws, grpc")),
    }
}

fn transport_name(transport: Transport) -> &'static str {
    match transport {
        Transport::Tcp => "tcp",
        Transport::Ws => "ws",
        Transport::Grpc => "grpc",
        Transport::Quic => "quic",
        Transport::Hysteria => "hysteria",
    }
}

fn protocol_of(kind: ProxyKind) -> Result<ProxyProtocol, CoreError> {
    match kind {
        ProxyKind::Vless => Ok(ProxyProtocol::Vless),
        ProxyKind::Vmess => Ok(ProxyProtocol::Vmess),
        ProxyKind::Shadowsocks => Ok(ProxyProtocol::Shadowsocks),
        ProxyKind::Trojan => Ok(ProxyProtocol::Trojan),
        ProxyKind::Hysteria2 => Ok(ProxyProtocol::Hysteria2),
        _ => Err(invalid("kind is not a share-link protocol")),
    }
}

/// TLS/REALITY 的已归一化输入；`security` 为 `None` 时其余字段只产生警告。
#[derive(Default)]
struct TlsInput {
    security: Security,
    sni: Option<String>,
    host: Option<String>,
    reality_public_key: Option<String>,
    reality_short_id: Option<String>,
    insecure: Option<(String, String)>,
}

/// 写入 TLS/REALITY 选项。
///
/// 显式 TLS/REALITY 必须留下 `sni`：缺省时按 Xray 的惯例回退到节点地址，并产生警告，
/// 这样 `ProxyConfig::validate` 与 `outbound_key` 仍看到 TLS，不会退化成明文。
fn apply_tls(
    options: &mut BTreeMap<String, Value>,
    warnings: &mut Vec<String>,
    input: TlsInput,
    server: &str,
) -> Result<(), CoreError> {
    let TlsInput {
        security,
        sni,
        host,
        reality_public_key,
        reality_short_id,
        insecure,
    } = input;
    match security {
        Security::None => {
            if sni.is_some() {
                warnings.push("parameter `sni` is ignored because `security` is none".into());
            }
            return Ok(());
        }
        Security::Tls | Security::Reality => {}
    }
    if security == Security::Reality && reality_public_key.is_none() {
        return Err(invalid("reality requires `pbk`"));
    }
    if let Some(public_key) = reality_public_key {
        options.insert("reality_pbk".into(), Value::String(public_key));
    }
    if let Some(short_id) = reality_short_id {
        options.insert("reality_sid".into(), Value::String(short_id));
    }
    let server_name = sni.or(host).unwrap_or_else(|| {
        warnings.push("parameter `sni` defaults to the node server address".into());
        server.to_owned()
    });
    options.insert("sni".into(), Value::String(server_name));
    if let Some(insecure) = insecure {
        options.insert("allow_insecure".into(), Value::Bool(parse_flag(insecure)?));
    }
    Ok(())
}

fn base_options(transport: Transport, security: Security) -> BTreeMap<String, Value> {
    BTreeMap::from([
        (
            "security".to_owned(),
            Value::String(security.name().to_owned()),
        ),
        (
            "transport".to_owned(),
            Value::String(transport_name(transport).to_owned()),
        ),
    ])
}

/// `scheme://userinfo@host:port?params#name` 的通用分解；`userinfo` 保持原文，由各协议解码。
struct Target<'a> {
    userinfo: Option<&'a str>,
    host: String,
    port: u16,
    params: Params,
}

fn split_target(rest: &str) -> Result<Target<'_>, CoreError> {
    let (authority, query) = split_query(strip_fragment(rest));
    let (userinfo, hostport) = match authority.rsplit_once('@') {
        Some((userinfo, hostport)) => (Some(userinfo), hostport),
        None => (None, authority),
    };
    let (host, port) = split_host_port(hostport)?;
    Ok(Target {
        userinfo,
        host: validate_host(host)?,
        port: parse_port(port)?,
        params: Params::parse(query)?,
    })
}

fn parse_vless(rest: &str) -> Result<ProxyDraft, CoreError> {
    let Target {
        userinfo,
        host: server,
        port,
        mut params,
    } = split_target(rest)?;
    let id = percent_decode(userinfo.ok_or_else(|| missing("vless", "id"))?)?;
    if id.is_empty() {
        return Err(missing("vless", "id"));
    }
    if !is_uuid(&id) {
        return Err(invalid("vless link `id` must be a UUID"));
    }
    if param_truthy(&params.take(&["encryption"]), "none") {
        return Err(invalid("vless link `encryption` must be none"));
    }
    let transport = parse_transport(params.take(&["type"]).as_deref())?;
    let security = parse_security(params.take(&["security"]).as_deref())?;
    let sni = non_empty(params.take(&["sni"]));
    let host = non_empty(params.take(&["host"]));
    let path = non_empty(params.take(&["path"]));
    let service = non_empty(params.take(&["serviceName", "service_name"]));
    let flow = non_empty(params.take(&["flow"]));
    let fingerprint = non_empty(params.take(&["fp"]));
    let reality_public_key = non_empty(params.take(&["pbk"]));
    let reality_short_id = non_empty(params.take(&["sid"]));
    let insecure = params.take_named(&["allowInsecure", "insecure"]);

    let mut options = base_options(transport, security);
    let mut warnings = Vec::new();
    insert_optional(&mut options, "path", path);
    insert_optional(&mut options, "host", host.clone());
    if transport == Transport::Grpc {
        insert_optional(&mut options, "service_name", service);
    }
    insert_optional(&mut options, "flow", flow);
    insert_optional(&mut options, "fingerprint", fingerprint);
    apply_tls(
        &mut options,
        &mut warnings,
        TlsInput {
            security,
            sni,
            host,
            reality_public_key,
            reality_short_id,
            insecure,
        },
        &server,
    )?;
    params.warn_unknown(&mut warnings);
    let credentials = BTreeMap::from([("id".to_owned(), id)]);
    Ok(draft(
        ProxyKind::Vless,
        server,
        port,
        options,
        credentials,
        warnings,
    ))
}

fn parse_trojan(rest: &str) -> Result<ProxyDraft, CoreError> {
    let Target {
        userinfo,
        host: server,
        port,
        mut params,
    } = split_target(rest)?;
    let password = percent_decode(userinfo.ok_or_else(|| missing("trojan", "password"))?)?;
    if password.is_empty() {
        return Err(missing("trojan", "password"));
    }
    if param_truthy(&params.take(&["security"]), "tls") {
        return Err(invalid("trojan link `security` must be tls"));
    }
    let transport = parse_transport(params.take(&["type"]).as_deref())?;
    let sni = non_empty(params.take(&["sni"]));
    let host = non_empty(params.take(&["host"]));
    let path = non_empty(params.take(&["path"]));
    let service = non_empty(params.take(&["serviceName", "service_name"]));
    let fingerprint = non_empty(params.take(&["fp"]));
    let insecure = params.take_named(&["allowInsecure", "insecure"]);

    let mut options = base_options(transport, Security::Tls);
    let mut warnings = Vec::new();
    insert_optional(&mut options, "path", path);
    insert_optional(&mut options, "host", host.clone());
    if transport == Transport::Grpc {
        insert_optional(&mut options, "service_name", service);
    }
    insert_optional(&mut options, "fingerprint", fingerprint);
    apply_tls(
        &mut options,
        &mut warnings,
        TlsInput {
            security: Security::Tls,
            sni,
            host,
            insecure,
            ..TlsInput::default()
        },
        &server,
    )?;
    params.warn_unknown(&mut warnings);
    let credentials = BTreeMap::from([("password".to_owned(), password)]);
    Ok(draft(
        ProxyKind::Trojan,
        server,
        port,
        options,
        credentials,
        warnings,
    ))
}

fn parse_hysteria2(rest: &str) -> Result<ProxyDraft, CoreError> {
    let Target {
        userinfo,
        host: server,
        port,
        mut params,
    } = split_target(rest)?;
    let auth = percent_decode(userinfo.ok_or_else(|| missing("hysteria2", "auth"))?)?;
    if auth.is_empty() {
        return Err(missing("hysteria2", "auth"));
    }
    let sni = non_empty(params.take(&["sni"]));
    let insecure = params.take_named(&["insecure", "allowInsecure"]);
    let mut options = base_options(Transport::Hysteria, Security::Tls);
    let mut warnings = Vec::new();
    apply_tls(
        &mut options,
        &mut warnings,
        TlsInput {
            security: Security::Tls,
            sni,
            insecure,
            ..TlsInput::default()
        },
        &server,
    )?;
    params.warn_unknown(&mut warnings);
    let credentials = BTreeMap::from([("auth".to_owned(), auth)]);
    Ok(draft(
        ProxyKind::Hysteria2,
        server,
        port,
        options,
        credentials,
        warnings,
    ))
}

fn parse_vmess(rest: &str) -> Result<ProxyDraft, CoreError> {
    let payload = strip_fragment(rest).trim();
    if payload.is_empty() {
        return Err(missing("vmess", "add"));
    }
    let decoded =
        decode_base64(payload).ok_or_else(|| invalid("vmess link payload is not valid base64"))?;
    let text =
        String::from_utf8(decoded).map_err(|_| invalid("vmess link payload is not valid UTF-8"))?;
    let object: serde_json::Map<String, Value> = match serde_json::from_str(&text) {
        Ok(Value::Object(object)) => object,
        _ => return Err(invalid("vmess link payload must be a JSON object")),
    };
    let mut params = Params::from_object(object);
    let server = params
        .take(&["add"])
        .ok_or_else(|| missing("vmess", "add"))?;
    let server = validate_host(&server)?;
    let port = params
        .take(&["port"])
        .ok_or_else(|| missing("vmess", "port"))?;
    let port = parse_port(&port)?;
    let id = params.take(&["id"]).ok_or_else(|| missing("vmess", "id"))?;
    if !is_uuid(&id) {
        return Err(invalid("vmess link `id` must be a UUID"));
    }
    let transport = parse_transport(params.take(&["net"]).as_deref())?;
    let security = match params.take(&["tls"]).unwrap_or_default().as_str() {
        "" | "none" => Security::None,
        "tls" => Security::Tls,
        _ => return Err(invalid("vmess link `tls` must be tls or none")),
    };

    let mut options = base_options(transport, security);
    let mut warnings = Vec::new();
    // 空 `host` 与缺省等价，避免把空字符串写进出站选项。
    let host = non_empty(params.take(&["host"]));
    let path = non_empty(params.take(&["path"]));
    if transport == Transport::Grpc {
        insert_optional(&mut options, "service_name", path);
    } else {
        insert_optional(&mut options, "path", path);
    }
    insert_optional(&mut options, "host", host.clone());
    insert_optional(&mut options, "fingerprint", non_empty(params.take(&["fp"])));
    apply_tls(
        &mut options,
        &mut warnings,
        TlsInput {
            security,
            sni: non_empty(params.take(&["sni"])),
            host,
            ..TlsInput::default()
        },
        &server,
    )?;
    if params.take(&["aid"]).is_some_and(|aid| aid != "0") {
        warnings.push("vmess `aid` is ignored because only AEAD is supported".into());
    }
    if params.take(&["scy"]).is_some_and(|scy| scy != "auto") {
        warnings.push("vmess `scy` is normalized to auto".into());
    }
    if params
        .take(&["type"])
        .is_some_and(|header| !header.is_empty() && header != "none")
    {
        warnings.push("vmess header `type` is ignored".into());
    }
    // `v`/`ps` 只是展示元数据；其余未知字段（如 `alpn`）只记字段名。
    params.ignore(&["v", "ps"]);
    params.warn_unknown(&mut warnings);
    let credentials = BTreeMap::from([("id".to_owned(), id)]);
    Ok(draft(
        ProxyKind::Vmess,
        server,
        port,
        options,
        credentials,
        warnings,
    ))
}

fn parse_ss(rest: &str) -> Result<ProxyDraft, CoreError> {
    let (authority, query) = split_query(strip_fragment(rest));
    let mut params = Params::parse(query)?;
    let (userinfo, hostport) = match authority.rsplit_once('@') {
        Some((userinfo, hostport)) => (userinfo.to_owned(), hostport.to_owned()),
        None => {
            let decoded = decode_base64(authority)
                .ok_or_else(|| invalid("ss link payload is not valid base64"))?;
            let decoded = String::from_utf8(decoded)
                .map_err(|_| invalid("ss link payload is not valid UTF-8"))?;
            let (userinfo, hostport) = decoded
                .rsplit_once('@')
                .ok_or_else(|| missing("ss", "method"))?;
            (userinfo.to_owned(), hostport.to_owned())
        }
    };
    // `plugin` 改变线上封装；忽略会让连接静默失效，因此显式拒绝。
    if params
        .take(&["plugin"])
        .is_some_and(|plugin| !plugin.is_empty())
    {
        return Err(invalid("ss link `plugin` is not supported"));
    }
    let (method, password) = ss_credentials(&userinfo)?;
    let (host, port) = split_host_port(&hostport)?;
    let server = validate_host(host)?;
    let port = parse_port(port)?;
    let mut options = base_options(Transport::Tcp, Security::None);
    options.insert("method".into(), Value::String(method.clone()));
    let mut warnings = Vec::new();
    if !SS_METHODS.contains(&method.as_str()) {
        warnings.push("ss `method` is not in the known method list".into());
    }
    params.warn_unknown(&mut warnings);
    let credentials = BTreeMap::from([("password".to_owned(), password)]);
    Ok(draft(
        ProxyKind::Shadowsocks,
        server,
        port,
        options,
        credentials,
        warnings,
    ))
}

/// Shadowsocks 凭据：`base64(method:password)` 或明文 `method:password`。
fn ss_credentials(userinfo: &str) -> Result<(String, String), CoreError> {
    let (method, password) = match userinfo.split_once(':') {
        Some((method, password)) => (percent_decode(method)?, percent_decode(password)?),
        None => {
            let decoded = decode_base64(userinfo)
                .ok_or_else(|| invalid("ss link user info is not valid base64"))?;
            let text = String::from_utf8(decoded)
                .map_err(|_| invalid("ss link user info is not valid UTF-8"))?;
            text.split_once(':')
                .map(|(method, password)| (method.to_owned(), password.to_owned()))
                .ok_or_else(|| missing("ss", "method"))?
        }
    };
    if method.is_empty() {
        return Err(missing("ss", "method"));
    }
    Ok((method, password))
}

fn strip_fragment(rest: &str) -> &str {
    rest.split('#').next().unwrap_or(rest)
}

fn split_query(rest: &str) -> (&str, &str) {
    rest.split_once('?').unwrap_or((rest, ""))
}

/// 只接受 `host:port` 或 `[ipv6]:port`；authority 尾巴的 `/` 视为分隔符。
fn split_host_port(value: &str) -> Result<(&str, &str), CoreError> {
    let value = value.strip_suffix('/').unwrap_or(value);
    if let Some(rest) = value.strip_prefix('[') {
        let (host, after) = rest
            .split_once(']')
            .ok_or_else(|| invalid("link `host` is malformed"))?;
        let port = after
            .strip_prefix(':')
            .ok_or_else(|| invalid("link `port` is required"))?;
        return Ok((host, port));
    }
    if value.contains('/') {
        return Err(invalid("link path is not supported"));
    }
    value
        .rsplit_once(':')
        .ok_or_else(|| invalid("link `port` is required"))
}

/// 主机必须是 IPv6 字面量或 ASCII 主机名；IPv6 归一化后再进入配置与出站键。
fn validate_host(host: &str) -> Result<String, CoreError> {
    if host.is_empty() || host.len() > 253 {
        return Err(invalid("link `host` must be a non-empty host name"));
    }
    if host.contains(':') {
        return host
            .parse::<std::net::Ipv6Addr>()
            .map(|address| address.to_string())
            .map_err(|_| invalid("link `host` is not a valid IPv6 literal"));
    }
    let allowed = host
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'));
    if !allowed {
        return Err(invalid("link `host` contains unsupported characters"));
    }
    Ok(host.to_owned())
}

fn parse_port(value: &str) -> Result<u16, CoreError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid(
            "link `port` must be an integer between 1 and 65535",
        ));
    }
    match value.parse::<u16>() {
        Ok(port) if port > 0 => Ok(port),
        _ => Err(invalid(
            "link `port` must be an integer between 1 and 65535",
        )),
    }
}

fn parse_flag(param: (String, String)) -> Result<bool, CoreError> {
    let (name, value) = param;
    match value.as_str() {
        "" | "0" | "false" => Ok(false),
        "1" | "true" => Ok(true),
        _ => Err(invalid(format!("link `{name}` must be 0 or 1"))),
    }
}

/// 参数存在且与默认值外的取值不同（空值等同缺省）。
fn param_truthy(value: &Option<String>, accepted: &str) -> bool {
    value
        .as_deref()
        .is_some_and(|text| !text.is_empty() && text != accepted)
}

fn is_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                *byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

fn insert_optional(options: &mut BTreeMap<String, Value>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        options.insert(key.to_owned(), Value::String(value));
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.is_empty())
}

fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

/// 已解码的参数集合；`take` 标记已消费的名字，其余进入警告。
struct Params {
    map: BTreeMap<String, String>,
    used: BTreeSet<String>,
}

impl Params {
    fn parse(query: &str) -> Result<Self, CoreError> {
        let mut map = BTreeMap::new();
        for pair in query.split('&').filter(|pair| !pair.is_empty()) {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            let name = percent_decode(name)?;
            if name.is_empty() {
                return Err(invalid("link parameter names must not be empty"));
            }
            map.insert(name, percent_decode(value)?);
        }
        Ok(Self {
            map,
            used: BTreeSet::new(),
        })
    }

    /// VMess 的 base64 JSON 对象复用同一套取值/警告逻辑。
    fn from_object(object: serde_json::Map<String, Value>) -> Self {
        let map = object
            .into_iter()
            .filter_map(|(key, value)| scalar_text(&value).map(|text| (key, text)))
            .collect();
        Self {
            map,
            used: BTreeSet::new(),
        }
    }

    /// 取第一个存在的（参数名, 值）；参数名用于错误消息，避免回显值。
    fn take_named(&mut self, names: &[&str]) -> Option<(String, String)> {
        let value = self.take(names)?;
        let name = names
            .iter()
            .find(|name| self.map.get(**name) == Some(&value))
            .map(|name| (*name).to_owned())?;
        Some((name, value))
    }

    /// 取第一个存在的别名值，并把全部别名标记为已消费。
    fn take(&mut self, names: &[&str]) -> Option<String> {
        let mut found = None;
        for name in names {
            if let Some(value) = self.map.get(*name) {
                self.used.insert((*name).to_owned());
                if found.is_none() {
                    found = Some(value.clone());
                }
            }
        }
        found
    }

    /// 标记只作展示的字段（VMess `v`/`ps`）为已消费：它们不影响连接语义。
    fn ignore(&mut self, names: &[&str]) {
        for name in names {
            if self.map.contains_key(*name) {
                self.used.insert((*name).to_owned());
            }
        }
    }

    fn warn_unknown(&mut self, warnings: &mut Vec<String>) {
        for name in self.map.keys() {
            if !self.used.contains(name) {
                warnings.push(format!("unsupported parameter `{name}` is ignored"));
            }
        }
    }
}

fn percent_decode(input: &str) -> Result<String, CoreError> {
    let bytes = input.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = bytes.get(index + 1).and_then(|byte| hex_value(*byte));
            let low = bytes.get(index + 2).and_then(|byte| hex_value(*byte));
            match (high, low) {
                (Some(high), Some(low)) => decoded.push(high * 16 + low),
                _ => return Err(invalid("link contains an invalid percent escape")),
            }
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).map_err(|_| invalid("link contains invalid UTF-8"))
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// 同时接受标准与 URL-safe 字母表、有无 padding；失败返回 `None`，绝不 panic。
fn decode_base64(input: &str) -> Option<Vec<u8>> {
    let compact: String = input.chars().filter(|ch| !ch.is_whitespace()).collect();
    if compact.is_empty() {
        return None;
    }
    let url_alphabet = compact.contains('-') || compact.contains('_');
    let padded = compact.ends_with('=');
    let engine = match (url_alphabet, padded) {
        (false, true) => &STANDARD,
        (false, false) => &STANDARD_NO_PAD,
        (true, true) => &URL_SAFE,
        (true, false) => &URL_SAFE_NO_PAD,
    };
    engine.decode(&compact).ok()
}

/// 已知的 Shadowsocks AEAD/2022 方法；其他取值只产生警告，交给引擎判定。
const SS_METHODS: [&str; 11] = [
    "aes-128-gcm",
    "aes-256-gcm",
    "chacha20-ietf-poly1305",
    "xchacha20-ietf-poly1305",
    "2022-blake3-aes-128-gcm",
    "2022-blake3-aes-256-gcm",
    "2022-blake3-chacha20-poly1305",
    "none",
    "plain",
    "rc4-md5",
    "aes-192-gcm",
];

fn draft(
    kind: ProxyKind,
    server: String,
    port: u16,
    options: BTreeMap<String, Value>,
    credentials: BTreeMap<String, String>,
    warnings: Vec<String>,
) -> ProxyDraft {
    let credential_fields = credentials.keys().cloned().collect();
    ProxyDraft {
        kind,
        server,
        port,
        options,
        credentials: Credentials {
            values: credentials,
        },
        credential_fields,
        warnings,
    }
}

fn invalid(message: impl Into<String>) -> CoreError {
    CoreError::validation(ErrorCode::ProxyConfigInvalid, message)
}

fn missing(scheme: &str, field: &str) -> CoreError {
    invalid(format!("{scheme} link is missing `{field}`"))
}

//! `POST /api/v1/proxies/{proxy_id}/check`（`REQ-API-017`、`REQ-PROXY-013`）。
//!
//! 检查复用**唯一**的共享引擎：临时创建 `chk:<check_id>:in` 回环 HTTP 入站、
//! `chk:<check_id>:out` 出站与 `chk:<check_id>:rule` 路由规则，经它们发起一次
//! HTTP 探测，然后无论成功或失败都按 tag 删除这三项资源。不启动第二个引擎，
//! 不写 `proxy_outbounds`，不改变任何运行档案的 `ref_count`。
//!
//! 失败分类（`ProxyCheckErrorCode`）：
//! - 节点主机解析失败 → `PROXY_DNS_FAILED`；解析超时 → `PROXY_TIMEOUT`。
//! - 节点 TCP 不可达 → `PROXY_CONNECTION_REFUSED` / `PROXY_TIMEOUT` / `PROXY_CONNECTION_FAILED`。
//! - 经引擎的探测失败 → `PROXY_CONNECTION_FAILED` / `PROXY_TIMEOUT`。
//!
//! 引擎的 HTTP 入站把出站失败折叠成空的 `503`，管理 API 不暴露底层原因，因此
//! `PROXY_AUTH_FAILED` 与 `PROXY_TLS_FAILED` 目前无法与控制面区分；此类失败以
//! `PROXY_CONNECTION_FAILED` 上报，而不是猜测一个更细的码。所有节点失败都是
//! `200 + ProxyCheck.error_code`；只有认证/授权/校验/存储/引擎不可用才用 HTTP 错误。

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex;

use mien_core::error::ErrorCode;
use mien_core::ports::proxy::{ProxyId, ProxyKind, ProxyRepository, ProxySecret, ProxyView};
use mien_core::proxy::api::{self, XrayApi};
use mien_core::proxy::keys::{
    Credentials, MuxSettings, OutboundSpec, ProxyConfig, ProxyProtocol, TlsSettings, Transport,
    build_outbound,
};
use mien_core::secrets::MasterKey;

use super::view::{CheckErrorCode, ProxyCheck};
use super::{map_repository_error, not_found, proxy_not_ready, repository};
use crate::RequestId;
use crate::auth::{AuditEvent, record_audit};
use crate::events::store;
use crate::profiles::ApiError;
use crate::profiles::ensure_engine;
use crate::state::AppState;

/// 探测响应的读取上限；回显端点只需很小的一段 JSON。
const MAX_RESPONSE_BYTES: usize = 64 * 1024;
/// 节点 TCP 预检的独立上限：不让不可达节点吃掉全部预算。
const MAX_REACH_TIMEOUT: Duration = Duration::from_secs(5);

/// 进程内检查串行锁：`chk:*` 资源的创建、探测与清理与其他检查互斥，
/// 与接管时的孤儿清理共用同一把锁（`REQ-PROXY-013`/`REQ-PROXY-023`）。
static CHECK_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// 一次检查的结果；节点失败用左值，控制面失败用 [`ApiError`]。
type ProbeOutcome = Result<ParsedResponse, CheckErrorCode>;

/// 解析后的探测响应。
struct ParsedResponse {
    status: u16,
    echo: Option<EchoBody>,
}

/// 回显端点里我们只保留两个非秘密标量。
struct EchoBody {
    exit_ip: Option<String>,
    region: Option<String>,
}

/// 检查一条已保存的代理，并写入结果、审计与事件。
///
/// # Errors
/// 代理不存在、存储不可用、内建配置无法编码、共享引擎不可用或临时资源创建失败。
pub async fn run(
    state: &AppState,
    principal_subject: &str,
    request_id: &RequestId,
    proxy_id: &str,
) -> Result<ProxyCheck, ApiError> {
    let view = super::get(state, proxy_id).await?;
    let target = ProbeTarget::parse(&echo_endpoint(state)?).ok_or_else(|| {
        ApiError::new(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::InternalUnexpected,
            "The proxy check echo endpoint is not a valid http:// URL.",
        )
    })?;
    let spec = check_spec(state, &view).await?;
    let timeout = state.proxy_check().timeout;
    // 引擎不可用属于控制面失败：契约声明 503，而不是 200 的节点错误。
    let engine = ensure_engine(state).await?;

    let _serialized = CHECK_LOCK.lock().await;
    let check_id = ProxyId::new().0;
    let inbound_tag = format!("chk:{check_id}:in");
    let rule_tag = format!("chk:{check_id}:rule");
    let outbound_tag = format!("chk:{check_id}:out");
    let port = free_loopback_port().map_err(|_| {
        ApiError::new(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::ProxyInboundConflict,
            "No loopback port is available for a proxy check.",
        )
    })?;
    let api = engine.api();
    // 临时资源创建失败是控制面错误：回收已建的部分，再返回稳定错误码。
    if let Err(error) = create_resources(&*api, &inbound_tag, &outbound_tag, &rule_tag, port, &spec)
    {
        cleanup_resources(&*api, &inbound_tag, &outbound_tag, &rule_tag);
        return Err(ApiError::from_core(error));
    }

    let started = Instant::now();
    let outcome = probe(&view, &target, port, timeout).await;
    let latency_ms = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
    // 无论成功、失败还是提前返回，都在同一个栈帧内回收三项临时资源。
    cleanup_resources(&*api, &inbound_tag, &outbound_tag, &rule_tag);

    let check = match outcome {
        Ok(ParsedResponse { status, echo }) if (200..500).contains(&status) => {
            let (exit_ip, region) = echo.map_or((None, None), |echo| (echo.exit_ip, echo.region));
            ProxyCheck::success(latency_ms, exit_ip, region)
        }
        Ok(ParsedResponse { status, .. }) => {
            // 引擎把出站失败折叠成空的 5xx；回显端点自身不会这样回复。
            tracing::debug!(
                request_id = request_id.as_str(),
                proxy_id,
                status,
                "the shared engine reported a failing outbound"
            );
            ProxyCheck::failure(CheckErrorCode::ProxyConnectionFailed)
        }
        Err(code) => {
            tracing::debug!(
                request_id = request_id.as_str(),
                proxy_id,
                code = code.as_str(),
                "the proxy check failed"
            );
            ProxyCheck::failure(code)
        }
    };

    persist(state, &view.id, &check).await?;
    record(state, principal_subject, request_id, &view, &check).await;
    Ok(check)
}

/// 回显端点：未配置时检查端点返回 503，而不是伪造一次节点失败。
fn echo_endpoint(state: &AppState) -> Result<String, ApiError> {
    state.proxy_check().url.clone().ok_or_else(|| {
        ApiError::new(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::ProxyEngineNotReady,
            "No proxy check echo endpoint is configured.",
        )
    })
}

/// 读取并解密已保存代理，装配共享引擎出站。
///
/// 凭据只留在本进程的 [`ProxySecret`] 与 [`OutboundSpec`] 中；错误消息来自
/// `mien_core`，只含字段名。
async fn check_spec(state: &AppState, view: &ProxyView) -> Result<OutboundSpec, ApiError> {
    let repository = repository(state)?;
    let secret = repository
        .load_secret(&view.id)
        .await
        .map_err(map_repository_error)?;
    let key = MasterKey::load(state.data_root()).map_err(|_| proxy_not_ready())?;
    let config = proxy_config(view, &secret);
    build_outbound(&config, key.as_bytes()).map_err(ApiError::from_core)
}

/// 把脱敏视图 + 进程内凭据还原为引擎配置；传输/TLS 字段取自非秘密 `options`。
fn proxy_config(view: &ProxyView, secret: &ProxySecret) -> ProxyConfig {
    let option = |name: &str| {
        secret
            .options
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    ProxyConfig {
        protocol: protocol_of(view.kind),
        server: view.server.clone(),
        port: view.port,
        transport: match option("transport").as_deref() {
            Some("ws") => Transport::Ws,
            Some("grpc") => Transport::Grpc,
            Some("hysteria") => Transport::Hysteria,
            Some("quic") => Transport::Quic,
            _ => Transport::Tcp,
        },
        tls: TlsSettings {
            server_name: option("sni"),
            fingerprint: option("fingerprint"),
            allow_insecure: secret
                .options
                .get("allow_insecure")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            reality_public_key: option("reality_pbk"),
            reality_short_id: option("reality_sid"),
        },
        mux: MuxSettings::default(),
        options: secret
            .options
            .iter()
            .filter_map(|(name, value)| value.as_str().map(|text| (name.clone(), text.to_owned())))
            .collect::<BTreeMap<_, _>>(),
        credentials: Credentials {
            values: secret.credentials.clone(),
        },
    }
}

fn protocol_of(kind: ProxyKind) -> ProxyProtocol {
    match kind {
        ProxyKind::Vless => ProxyProtocol::Vless,
        ProxyKind::Vmess => ProxyProtocol::Vmess,
        ProxyKind::Shadowsocks => ProxyProtocol::Shadowsocks,
        ProxyKind::Trojan => ProxyProtocol::Trojan,
        ProxyKind::Hysteria2 => ProxyProtocol::Hysteria2,
        ProxyKind::Wireguard => ProxyProtocol::Wireguard,
        ProxyKind::Http => ProxyProtocol::Http,
        ProxyKind::Https => ProxyProtocol::Https,
        ProxyKind::Socks5 => ProxyProtocol::Socks5,
    }
}

/// 两段式探测：先预检节点可达性以分类，再经引擎的临时资源发起 HTTP 请求。
async fn probe(
    view: &ProxyView,
    target: &ProbeTarget,
    port: u16,
    timeout: Duration,
) -> ProbeOutcome {
    let addresses = resolve_node(&view.server, view.port, timeout).await?;
    reach_node(&addresses, timeout).await?;
    exchange(port, target, timeout).await
}

/// 解析节点主机；只有代理节点主机名允许本地 bootstrap 解析（`REQ-PROXY-008`）。
async fn resolve_node(
    host: &str,
    port: u16,
    timeout: Duration,
) -> Result<Vec<SocketAddr>, CheckErrorCode> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(vec![SocketAddr::new(ip, port)]);
    }
    match tokio::time::timeout(timeout, tokio::net::lookup_host((host, port))).await {
        Ok(Ok(addresses)) => {
            let addresses: Vec<SocketAddr> = addresses.collect();
            if addresses.is_empty() {
                Err(CheckErrorCode::ProxyDnsFailed)
            } else {
                Ok(addresses)
            }
        }
        Ok(Err(_)) => Err(CheckErrorCode::ProxyDnsFailed),
        Err(_) => Err(CheckErrorCode::ProxyTimeout),
    }
}

/// 节点 TCP 可达性预检；区分拒绝连接与超时。
async fn reach_node(addresses: &[SocketAddr], timeout: Duration) -> Result<(), CheckErrorCode> {
    let budget = timeout.min(MAX_REACH_TIMEOUT);
    let mut last = CheckErrorCode::ProxyConnectionFailed;
    for address in addresses {
        match tokio::time::timeout(budget, TcpStream::connect(address)).await {
            Ok(Ok(stream)) => {
                drop(stream);
                return Ok(());
            }
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
                last = CheckErrorCode::ProxyConnectionRefused;
            }
            Ok(Err(_)) => last = CheckErrorCode::ProxyConnectionFailed,
            Err(_) => last = CheckErrorCode::ProxyTimeout,
        }
    }
    Err(last)
}

/// 创建检查用临时入站、出站与规则；任一失败由调用方回收已建部分。
fn create_resources(
    api: &dyn XrayApi,
    inbound_tag: &str,
    outbound_tag: &str,
    rule_tag: &str,
    port: u16,
    spec: &OutboundSpec,
) -> Result<(), mien_core::CoreError> {
    api::ensure_inbound(
        api,
        api::Inbound {
            tag: inbound_tag.to_owned(),
            listen: "127.0.0.1".to_owned(),
            port,
            protocol: "http".to_owned(),
            settings: json!({}),
        },
    )?;
    api::ensure_outbound(
        api,
        api::Outbound {
            tag: outbound_tag.to_owned(),
            protocol: spec.protocol.clone(),
            mux: spec.mux,
            transport: Transport::Tcp,
            settings: spec.settings.clone(),
            stream_settings: spec.stream_settings.clone(),
        },
    )?;
    api::ensure_rule(
        api,
        api::Rule {
            tag: rule_tag.to_owned(),
            inbound_tag: inbound_tag.to_owned(),
            outbound_tag: outbound_tag.to_owned(),
        },
    )
}

/// 按 tag 回收检查资源：先规则、再入站、最后出站。
///
/// 失败只记日志，绝不把节点失败改写成控制面错误；残留的 `chk:*` 由引擎层
/// 孤儿清理负责（`REQ-PROXY-013`），不在这里吞掉秘密（本就只有 tag）。
fn cleanup_resources(api: &dyn XrayApi, inbound_tag: &str, outbound_tag: &str, rule_tag: &str) {
    for (kind, tag, result) in [
        ("rule", rule_tag, api::remove_rule(api, rule_tag)),
        (
            "inbound",
            inbound_tag,
            api::remove_inbound(api, inbound_tag),
        ),
        (
            "outbound",
            outbound_tag,
            api::remove_outbound(api, outbound_tag),
        ),
    ] {
        if let Err(error) = result {
            tracing::warn!(
                kind,
                tag,
                code = error.code().as_str(),
                "a proxy check resource could not be removed; it remains an engine orphan"
            );
        }
    }
}

/// 一条探测请求的目标。
#[derive(Debug, Clone, PartialEq, Eq)]
struct ProbeTarget {
    /// `host[:port]`，用于 `Host` 头与绝对 URI。
    authority: String,
    /// 以 `/` 开头的路径与查询串。
    path: String,
}

impl ProbeTarget {
    /// 只接受 `http://host[:port][/path][?query]`；拒绝 userinfo、https 与空主机。
    fn parse(raw: &str) -> Option<Self> {
        let rest = raw.strip_prefix("http://")?;
        let (authority, path) = rest.split_once('/').map_or((rest, ""), |(a, p)| (a, p));
        if authority.is_empty()
            || authority.contains('@')
            || authority
                .chars()
                .any(|c| c.is_whitespace() || c.is_control())
        {
            return None;
        }
        let host = authority
            .rsplit_once(':')
            .filter(|(host, port)| {
                !host.is_empty() && !host.contains(':') && port.parse::<u16>().is_ok()
            })
            .map_or(authority, |(host, _)| host);
        if host.is_empty() {
            return None;
        }
        Some(Self {
            authority: authority.to_owned(),
            path: format!("/{path}"),
        })
    }

    /// 经 HTTP 代理发送的请求行使用绝对形式（RFC 9110 §7.1）。
    fn absolute_uri(&self) -> String {
        format!("http://{}{}", self.authority, self.path)
    }
}

/// 经临时入站发出一次 HTTP 探测，并按截止时间读取有界响应。
async fn exchange(port: u16, target: &ProbeTarget, timeout: Duration) -> ProbeOutcome {
    let mut stream =
        match tokio::time::timeout(timeout, TcpStream::connect(("127.0.0.1", port))).await {
            Ok(Ok(stream)) => stream,
            Ok(Err(_)) | Err(_) => return Err(CheckErrorCode::ProxyConnectionFailed),
        };
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: mien-proxy-check/1\r\nAccept: application/json\r\nConnection: close\r\n\r\n",
        target.absolute_uri(),
        target.authority
    );
    let exchange = async {
        stream.write_all(request.as_bytes()).await.ok()?;
        let mut buffer = Vec::with_capacity(1024);
        let mut chunk = [0u8; 4096];
        loop {
            if buffer.len() >= MAX_RESPONSE_BYTES {
                break;
            }
            let read = stream.read(&mut chunk).await.ok()?;
            if read == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..read]);
        }
        Some(buffer)
    };
    let bytes = match tokio::time::timeout(timeout, exchange).await {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return Err(CheckErrorCode::ProxyConnectionFailed),
        Err(_) => return Err(CheckErrorCode::ProxyTimeout),
    };
    parse_response(&bytes).ok_or(CheckErrorCode::ProxyConnectionFailed)
}

/// 解析状态行、头与有界响应体；畸形响应按连接失败处理。
fn parse_response(bytes: &[u8]) -> Option<ParsedResponse> {
    let head_end = bytes.windows(4).position(|window| window == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&bytes[..head_end]).ok()?;
    let mut lines = head.split("\r\n");
    let status_line = lines.next()?;
    let mut parts = status_line.split_whitespace();
    if !parts.next()?.starts_with("HTTP/1.") {
        return None;
    }
    let status: u16 = parts.next()?.parse().ok()?;
    let chunked = lines.any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case("transfer-encoding")
                && value.to_ascii_lowercase().contains("chunked")
        })
    });
    let body = &bytes[head_end + 4..];
    let body = if chunked {
        dechunk(body).unwrap_or_default()
    } else {
        body.to_vec()
    };
    Some(ParsedResponse {
        status,
        echo: parse_echo(&body),
    })
}

/// 最小 chunked 解码；畸形输入按空体处理。
fn dechunk(mut body: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let line_end = body.windows(2).position(|window| window == b"\r\n")?;
        let size_text = std::str::from_utf8(&body[..line_end]).ok()?;
        let size = usize::from_str_radix(size_text.split(';').next()?.trim(), 16).ok()?;
        body = &body[line_end + 2..];
        if size == 0 {
            return Some(out);
        }
        if body.len() < size {
            return None;
        }
        out.extend_from_slice(&body[..size]);
        body = body.get(size + 2..)?;
        if out.len() > MAX_RESPONSE_BYTES {
            return None;
        }
    }
}

/// 回显端点可选地给出出口 IP 与地区；其它字段一律丢弃，绝不回显原文。
fn parse_echo(body: &[u8]) -> Option<EchoBody> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let field = |name: &str| {
        value
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .filter(|text| !text.is_empty() && text.chars().count() <= 128)
    };
    Some(EchoBody {
        exit_ip: field("ip"),
        region: field("region"),
    })
}

fn free_loopback_port() -> Result<u16, mien_core::CoreError> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(|_| {
        mien_core::CoreError::validation(
            ErrorCode::ProxyInboundConflict,
            "no loopback port available",
        )
    })?;
    listener
        .local_addr()
        .map(|address| address.port())
        .map_err(|_| {
            mien_core::CoreError::validation(
                ErrorCode::ProxyInboundConflict,
                "no loopback port available",
            )
        })
}

/// 写入 `proxies.last_check_json`；不改变 `updated_at`（那是配置变更时间）。
async fn persist(state: &AppState, id: &ProxyId, check: &ProxyCheck) -> Result<(), ApiError> {
    let pool = state.db().ok_or_else(ApiError::internal)?;
    let text = serde_json::to_string(check).map_err(|_| ApiError::internal())?;
    let rows = sqlx::query("UPDATE proxies SET last_check_json=?1 WHERE id=?2")
        .bind(text)
        .bind(id.as_str())
        .execute(pool)
        .await
        .map_err(|error| ApiError::storage(&error))?;
    if rows.rows_affected() == 0 {
        return Err(not_found());
    }
    Ok(())
}

/// 写审计事件与 `proxy.check.completed`；失败只记日志，不改写检查结果。
async fn record(
    state: &AppState,
    actor: &str,
    request_id: &RequestId,
    view: &ProxyView,
    check: &ProxyCheck,
) {
    let Some(pool) = state.db() else {
        return;
    };
    let error_code = check.error_code.map(CheckErrorCode::as_str);
    record_audit(
        pool,
        AuditEvent {
            actor,
            event_type: "proxy.check.completed",
            action: "proxy_check",
            target: &format!("proxy:{}", view.id.as_str()),
            result: if check.ok { "success" } else { "failure" },
            request_id: request_id.as_str(),
            metadata: json!({
                "ok": check.ok,
                "error_code": error_code,
                "latency_ms": check.latency_ms,
            }),
        },
    )
    .await;
    let payload = json!({
        "proxy_id": view.id.as_str(),
        "ok": check.ok,
        "latency_ms": check.latency_ms,
        "exit_ip": check.exit_ip,
        "region": check.region,
        "error_code": error_code,
        "detail": check.message,
    });
    if let Err(error) = store::append(
        pool,
        state.events().limits(),
        "proxy.check.completed",
        &payload,
    )
    .await
    {
        tracing::warn!(
            request_id = request_id.as_str(),
            proxy_id = view.id.as_str(),
            error = ?error,
            "cannot append the proxy check event"
        );
    }
}

//! 网络防护 canary：在本机真实内核上测量 DoH / IPv6 / 非代理 UDP / WebRTC。
//!
//! `MIEN_KERNEL_BIN` 未设置时打印显式跳过原因并返回，不允许静默通过。
//!
//! 每个判定都走「观测 → 判定 → 与声明对账」三段，并且允许结论是 `Unverified`：
//! 本机实测不支持的声明绝不被升级成 `Declared`。
//!
//! 观测负载全部在本进程内：
//! - UDP+TCP DNS 桩，记录查询名，`*.canary.test` 返回固定 A/AAAA，其余 `NXDOMAIN`；
//! - ECHO HTTP 端点，返回对端 IP、地址族与 `Host`，用于判定是否直连泄漏；
//! - 探针页由本进程提供服务，脚本经 CDP `Runtime.evaluate` 注入。
//!
//! 内核按产品路径启动：`headful` + direct（无代理），argv 来自
//! [`build_launch_argv_for_mode`]，因此 `LaunchPlan.network` 的声明与真实 argv 一一对应。
//! 额外只加测量通道（`--remote-debugging-port=0`、`--log-net-log`），它们不是产品参数。

use std::{
    io::Write as _,
    net::Ipv6Addr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use futures_util::{SinkExt, StreamExt};
use mien_core::fingerprint::capability::NetworkDohMode;
use mien_core::fingerprint::spec::{FingerprintSpec, Platform};
use mien_core::fingerprint::{
    CapabilityManifest, Enforcement, build_launch_argv_for_mode, load_pinned_manifest,
};
use mien_core::network::{
    ClassifiedQuery, DnsVerdict, QuerySource, WebRtcVerdict, classify_dns_observation,
    doh_observation_from_netlog, doh_state, host_resolutions_from_netlog, ipv6_reachable,
    reconcile, webrtc_candidates,
};
use mien_core::runtime::probe::wait_for_cdp;
use mien_core::runtime::supervisor::LAUNCHER_FLAGS;
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
};
use tokio_tungstenite::tungstenite::Message;

/// 未配置内核时必须打印的跳过原因。
const SKIP_MESSAGE: &str = "skipped: MIEN_KERNEL_BIN not set";

/// canary 目标名；浏览器不得在本地解析它。
const CANARY_TARGET: &str = "network.canary.test";
const CANARY_ADDRESS: &str = "127.0.0.1";

/// 单次 CDP 往返上限；卡住的浏览器不能把测试挂死。
const CDP_TIMEOUT: Duration = Duration::from_secs(15);

/// 导航到探针页的总窗口。
const NAVIGATE_TIMEOUT: Duration = Duration::from_secs(30);

fn kernel() -> Option<PathBuf> {
    match std::env::var_os("MIEN_KERNEL_BIN") {
        Some(value) => {
            let path = PathBuf::from(value);
            assert!(path.is_file(), "MIEN_KERNEL_BIN is not a file: {path:?}");
            Some(path)
        }
        None => {
            println!("{SKIP_MESSAGE}");
            None
        }
    }
}

fn sha256(path: &Path) -> String {
    let bytes = std::fs::read(path).expect("kernel readable");
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

// ---------------------------------------------------------------------------
// DNS 桩：UDP + TCP
// ---------------------------------------------------------------------------

struct DnsStub {
    port: u16,
    tcp_port: u16,
    queries: Arc<Mutex<Vec<ClassifiedQuery>>>,
}

impl DnsStub {
    async fn start() -> Self {
        let queries = Arc::new(Mutex::new(Vec::new()));
        let udp = UdpSocket::bind("127.0.0.1:0").await.expect("udp bind");
        let port = udp.local_addr().expect("udp addr").port();
        // UDP 与 TCP 分端口：`MAP <name> 127.0.0.1:<port>` 只描述一个 UDP 端点，
        // 浏览器对同名目标可能直接试 TCP，同端口的 TCP 会让它停在半开连接上。
        let tcp = TcpListener::bind("127.0.0.1:0").await.expect("tcp bind");
        let tcp_port = tcp.local_addr().expect("tcp addr").port();

        let recorded = queries.clone();
        tokio::spawn(async move {
            let mut buffer = vec![0u8; 1500];
            loop {
                let Ok((length, peer)) = udp.recv_from(&mut buffer).await else {
                    return;
                };
                let message = buffer[..length].to_vec();
                let Some((name, qtype)) = parse_question(&message) else {
                    continue;
                };
                recorded.lock().expect("dns lock").push(ClassifiedQuery {
                    name: name.clone(),
                    source: QuerySource::Canary,
                });
                let response = build_response(&message, &name, qtype);
                let _ = udp.send_to(&response, peer).await;
            }
        });

        let recorded = queries.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = tcp.accept().await else {
                    return;
                };
                let recorded = recorded.clone();
                tokio::spawn(async move {
                    // DNS over TCP 是两字节长度前缀 + 报文；不能读到 EOF，桩自己不会关连接。
                    let mut prefix = [0u8; 2];
                    if stream.read_exact(&mut prefix).await.is_err() {
                        return;
                    }
                    let length = usize::from(u16::from_be_bytes(prefix));
                    if length == 0 || length > 4096 {
                        return;
                    }
                    let mut body = vec![0u8; length];
                    if stream.read_exact(&mut body).await.is_err() {
                        return;
                    }
                    let Some((name, qtype)) = parse_question(&body) else {
                        return;
                    };
                    recorded.lock().expect("dns lock").push(ClassifiedQuery {
                        name: name.clone(),
                        source: QuerySource::Canary,
                    });
                    let response = build_response(&body, &name, qtype);
                    let mut framed = (response.len() as u16).to_be_bytes().to_vec();
                    framed.extend_from_slice(&response);
                    let _ = stream.write_all(&framed).await;
                    let _ = stream.flush().await;
                });
            }
        });

        Self {
            port,
            tcp_port,
            queries,
        }
    }

    fn take(&self) -> Vec<ClassifiedQuery> {
        self.queries.lock().expect("dns lock").drain(..).collect()
    }
}

/// 解析 DNS 问题段；只接受未压缩的标签，足够 canary 使用。
fn parse_question(message: &[u8]) -> Option<(String, u16)> {
    if message.len() < 12 {
        return None;
    }
    let mut offset = 12usize;
    let mut labels = Vec::new();
    loop {
        let length = *message.get(offset)? as usize;
        if length == 0 {
            offset += 1;
            break;
        }
        if length & 0xc0 != 0 {
            return None;
        }
        let start = offset + 1;
        labels.push(String::from_utf8_lossy(message.get(start..start + length)?).into_owned());
        offset = start + length;
    }
    let qtype = u16::from_be_bytes([*message.get(offset)?, *message.get(offset + 1)?]);
    Some((labels.join("."), qtype))
}

fn question_section_end(message: &[u8]) -> Option<usize> {
    if message.len() < 12 {
        return None;
    }
    let mut offset = 12usize;
    loop {
        let length = *message.get(offset)? as usize;
        if length == 0 {
            return Some(offset + 1 + 4);
        }
        if length & 0xc0 != 0 {
            return None;
        }
        offset += 1 + length;
    }
}

/// `*.canary.test` 返回固定 A/AAAA；其余 `NXDOMAIN`。
fn build_response(query: &[u8], name: &str, qtype: u16) -> Vec<u8> {
    let question_end = question_section_end(query).unwrap_or(query.len());
    let lower = name.to_ascii_lowercase();
    let known = lower.ends_with(".canary.test") || lower == "canary.test";
    let known = known && matches!(qtype, 1 | 28);
    let mut response = vec![0u8; 12];
    response[..2].copy_from_slice(&query[..2]);
    response[2] = 0x81;
    response[3] = if known { 0x80 } else { 0x83 };
    response[4..6].copy_from_slice(&1u16.to_be_bytes());
    response[6..8].copy_from_slice(&u16::from(known).to_be_bytes());
    response.extend_from_slice(&query[12..question_end]);
    if !known {
        return response;
    }
    let record = if qtype == 28 {
        let mut bytes = vec![0u8; 16];
        bytes[15] = 1;
        bytes
    } else {
        CANARY_ADDRESS
            .parse::<std::net::Ipv4Addr>()
            .expect("v4")
            .octets()
            .to_vec()
    };
    response.extend_from_slice(&[0xc0, 0x0c]);
    response.extend_from_slice(&qtype.to_be_bytes());
    response.extend_from_slice(&1u16.to_be_bytes());
    response.extend_from_slice(&30u32.to_be_bytes());
    response.extend_from_slice(&(record.len() as u16).to_be_bytes());
    response.extend_from_slice(&record);
    response
}

// ---------------------------------------------------------------------------
// ECHO 端点
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct EchoHit {
    peer: String,
    family: &'static str,
    host: String,
}

struct Echo {
    port: u16,
    hits: Arc<Mutex<Vec<EchoHit>>>,
}

impl Echo {
    async fn start(bind: &str, family: &'static str) -> Self {
        let listener = TcpListener::bind(bind).await.expect("echo bind");
        let port = listener.local_addr().expect("echo addr").port();
        let hits = Arc::new(Mutex::new(Vec::new()));
        let recorded = hits.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let recorded = recorded.clone();
                tokio::spawn(async move {
                    let mut buffer = vec![0u8; 4096];
                    let Ok(read) = stream.read(&mut buffer).await else {
                        return;
                    };
                    let request = String::from_utf8_lossy(&buffer[..read]).into_owned();
                    let host = request
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("host")
                                .then(|| value.trim().to_owned())
                        })
                        .unwrap_or_default();
                    let peer = stream.peer_addr().expect("peer").ip().to_string();
                    recorded.lock().expect("echo lock").push(EchoHit {
                        peer: peer.clone(),
                        family,
                        host: host.clone(),
                    });
                    let body = format!(
                        "{{\"peer\":\"{peer}\",\"family\":\"{family}\",\"host\":\"{host}\"}}"
                    );
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\naccess-control-allow-origin: *\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.flush().await;
                });
            }
        });
        Self { port, hits }
    }

    fn take(&self) -> Vec<EchoHit> {
        self.hits.lock().expect("echo lock").drain(..).collect()
    }
}

// ---------------------------------------------------------------------------
// 探针页
// ---------------------------------------------------------------------------

struct ProbePage {
    port: u16,
}

impl ProbePage {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("page bind");
        let port = listener.local_addr().expect("page addr").port();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    let mut buffer = vec![0u8; 2048];
                    let _ = stream.read(&mut buffer).await;
                    let body =
                        "<!doctype html><meta charset=utf-8><title>mien network probe</title>";
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });
        Self { port }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/", self.port)
    }
}

// ---------------------------------------------------------------------------
// CDP：最小 WS 客户端
// ---------------------------------------------------------------------------

struct Cdp {
    stream: tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>,
    next_id: u64,
}

impl Cdp {
    /// 附加并导航到 `url`，直到页面真的落在它上面。
    ///
    /// 浏览器启动期间会替换自己的初始 target，附着到「第一个 page」后可能立刻失效。
    /// 因此每轮都重新从 `/json/list` 找一个页面 target，附着、导航、确认
    /// `location.href`；确认不了就整轮重来。端点令牌只存在于内存。
    async fn attach_at(port: u16, url: &str) -> Result<Self, String> {
        let deadline = tokio::time::Instant::now() + NAVIGATE_TIMEOUT;
        let endpoint = format!("http://127.0.0.1:{port}/json/list");
        loop {
            if let Some(page_url) = read_page_websocket(&endpoint).await {
                match Self::connect(page_url).await {
                    Ok(mut cdp) => {
                        if let Ok(()) = cdp.navigate(url).await {
                            return Ok(cdp);
                        }
                    }
                    Err(error) => println!("cdp: {error}"),
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(format!("no live page target reached {url}"));
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }

    async fn connect(page_url: String) -> Result<Self, String> {
        let (stream, _) = tokio_tungstenite::connect_async(page_url)
            .await
            .map_err(|error| format!("cdp connect failed: {error}"))?;
        Ok(Self { stream, next_id: 1 })
    }

    async fn call(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let request = serde_json::json!({"id": id, "method": method, "params": params});
        self.stream
            .send(Message::Text(request.to_string().into()))
            .await
            .map_err(|error| format!("cdp send failed: {error}"))?;
        loop {
            let next = tokio::time::timeout(CDP_TIMEOUT, self.stream.next())
                .await
                .map_err(|_| format!("cdp {method} timed out after {CDP_TIMEOUT:?}"))?;
            let message = next
                .ok_or_else(|| format!("cdp closed while awaiting {method}"))?
                .map_err(|error| format!("cdp read failed: {error}"))?;
            let Message::Text(text) = message else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                continue;
            };
            if value.get("id").and_then(serde_json::Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = value.get("error") {
                return Err(format!("cdp {method} error: {error}"));
            }
            return Ok(value
                .get("result")
                .cloned()
                .unwrap_or(serde_json::Value::Null));
        }
    }

    /// 求值一个 Promise 表达式并取回 JSON 值。
    async fn evaluate(&mut self, expression: &str) -> Result<serde_json::Value, String> {
        self.evaluate_value(expression).await
    }

    /// 求值并以字符串返回；字符串结果原样返回，其余按 JSON 序列化。
    async fn evaluate_text(&mut self, expression: &str) -> Result<String, String> {
        let value = self.evaluate_value(expression).await?;
        match value {
            serde_json::Value::String(text) => Ok(text),
            other => Ok(other.to_string()),
        }
    }

    async fn evaluate_value(&mut self, expression: &str) -> Result<serde_json::Value, String> {
        let response = self
            .call(
                "Runtime.evaluate",
                serde_json::json!({
                    "expression": expression,
                    "awaitPromise": true,
                    "returnByValue": true,
                    "timeout": 30_000,
                }),
            )
            .await?;
        if let Some(details) = response.get("exceptionDetails") {
            let description = details
                .get("exception")
                .and_then(|value| value.get("description"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("<no description>");
            return Err(format!("page exception: {description}"));
        }
        Ok(response
            .get("result")
            .and_then(|result| result.get("value"))
            .cloned()
            .unwrap_or(serde_json::Value::Null))
    }

    /// 导航一次并确认 `location.href`；失败交给 [`Self::attach_at`] 重试。
    async fn navigate(&mut self, url: &str) -> Result<(), String> {
        let expected = serde_json::Value::String(url.to_owned());
        for _ in 0..15 {
            let _ = self
                .call(
                    "Page.navigate",
                    serde_json::json!({"url": url, "timeout": 5_000}),
                )
                .await;
            if let Ok(value) = self.evaluate_value("({url: location.href})").await {
                if value.get("url") == Some(&expected) {
                    return Ok(());
                }
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        Err(format!("page never reached {url}"))
    }
}

/// 浏览器级 WS 端点，只用于 `Browser.close`。
async fn browser_websocket(port: u16) -> Option<String> {
    let response = http_get(&format!("http://127.0.0.1:{port}/json/version"))
        .await
        .ok()?;
    let version: serde_json::Value = serde_json::from_str(&response).ok()?;
    version
        .get("webSocketDebuggerUrl")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

async fn read_page_websocket(endpoint: &str) -> Option<String> {
    let response = http_get(endpoint).await.ok()?;
    let targets: serde_json::Value = serde_json::from_str(&response).ok()?;
    targets
        .as_array()?
        .iter()
        .find(|target| target.get("type").and_then(serde_json::Value::as_str) == Some("page"))
        .and_then(|target| target.get("webSocketDebuggerUrl"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

/// 最小 HTTP/1.1 GET，只用于 loopback 的 CDP 端点。
async fn http_get(url: &str) -> Result<String, String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| "only loopback http endpoints are read".to_owned())?;
    let (authority, path) = match rest.split_once('/') {
        Some((authority, path)) => (authority, format!("/{path}")),
        None => (rest, "/".to_owned()),
    };
    let mut stream = TcpStream::connect(authority)
        .await
        .map_err(|error| format!("connect {authority} failed: {error}"))?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|error| format!("request write failed: {error}"))?;
    let mut response = Vec::new();
    // CDP 不保证会关闭连接，所以按 Content-Length 分帧，而不是读到 EOF。
    let deadline = tokio::time::Instant::now() + CDP_TIMEOUT;
    let (head, mut body) = loop {
        if let Some(index) = find_head_end(&response) {
            let head = String::from_utf8_lossy(&response[..index]).into_owned();
            let body = response[index + 4..].to_vec();
            let length = content_length(&head);
            if body.len() >= length {
                break (head, body);
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("timed out reading the CDP discovery response".to_owned());
        }
        let mut chunk = [0u8; 4096];
        let read = tokio::time::timeout(CDP_TIMEOUT, stream.read(&mut chunk))
            .await
            .map_err(|_| "timed out reading the CDP discovery response".to_owned())?
            .map_err(|error| format!("response read failed: {error}"))?;
        if read == 0 {
            return Err("CDP discovery response ended before it was complete".to_owned());
        }
        response.extend_from_slice(&chunk[..read]);
    };
    body.truncate(content_length(&head));
    let body = String::from_utf8_lossy(&body).into_owned();
    let status = head.lines().next().unwrap_or_default();
    if status.starts_with("HTTP/1.1 200") || status.starts_with("HTTP/1.0 200") {
        Ok(body)
    } else {
        Err(format!("unexpected status line: {status}"))
    }
}

fn find_head_end(response: &[u8]) -> Option<usize> {
    response.windows(4).position(|window| window == b"\r\n\r\n")
}

fn content_length(head: &str) -> usize {
    head.lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())?
        })
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// 内核会话
// ---------------------------------------------------------------------------

/// 一个按产品路径启动的 headful direct 内核。
struct KernelSession {
    child: std::process::Child,
    profile_dir: PathBuf,
    netlog_path: PathBuf,
    _workdir: tempfile::TempDir,
    /// 内核启动即打开的探针页。
    page_url: String,
}

impl KernelSession {
    async fn start(binary: &Path, page_url: &str) -> Self {
        Self::start_pref(binary, page_url, &[], &[]).await
    }

    /// `extra` 只用于对照运行；`prefs` 是相对 user data 根写入的偏好文件。
    async fn start_pref(
        binary: &Path,
        page_url: &str,
        extra: &[String],
        prefs: &[(&str, &str)],
    ) -> Self {
        let workdir = tempfile::TempDir::new().expect("workdir");
        let profile_dir = workdir.path().join("profile");
        std::fs::create_dir_all(&profile_dir).expect("profile dir");
        let netlog_path = workdir.path().join("netlog.json");
        for (name, contents) in prefs {
            let path = profile_dir.join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("prefs dir");
            }
            std::fs::write(path, contents).expect("prefs file");
        }

        let manifest: CapabilityManifest = load_pinned_manifest().expect("pinned manifest");
        let spec = FingerprintSpec::fresh(Platform::Macos).expect("fresh spec");
        // 产品 argv：headful + 无代理。`LaunchPlan.network` 与这份 argv 一一对应。
        let plan = build_launch_argv_for_mode(
            &spec,
            &manifest,
            &profile_dir,
            mien_core::domain::profile::ProfileMode::Headful,
        )
        .expect("launch plan");
        let mut args = plan.argv.clone();
        args.extend(extra.iter().cloned());
        args.push(format!("--user-data-dir={}", profile_dir.display()));
        args.push("--remote-debugging-port=0".to_owned());
        args.push("--mien-runtime-id=network-canary".to_owned());
        // 直接 spawn 内核时必须自己带上宿主抑制参数；用产品常量，不复制一份。
        args.extend(LAUNCHER_FLAGS.iter().map(|flag| (*flag).to_owned()));
        // 测量通道：只读观测内核自己的解析器状态，不改变网络控制。
        args.push(format!("--log-net-log={}", netlog_path.display()));
        args.push("--net-log-capture-mode=Default".to_owned());

        let mut command = std::process::Command::new(binary);
        command
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        args.push(page_url.to_owned());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let child = command.spawn().expect("kernel spawn");
        Self {
            child,
            profile_dir,
            netlog_path,
            _workdir: workdir,
            page_url: page_url.to_owned(),
        }
    }

    async fn cdp_port(&self) -> u16 {
        wait_for_cdp(&self.profile_dir, Duration::from_secs(20))
            .await
            .expect("kernel CDP endpoint")
    }
}

impl Drop for KernelSession {
    fn drop(&mut self) {
        let pid = self.child.id();
        self.terminate(pid);
    }
}

impl KernelSession {
    /// 用 `Browser.close` 干净退出。netlog 只在干净退出时写全。
    async fn stop_and_read_netlog(mut self, port: u16) -> Option<String> {
        if let Some(url) = browser_websocket(port).await {
            if let Ok((mut stream, _)) = tokio_tungstenite::connect_async(url).await {
                let request = serde_json::json!({"id": 1, "method": "Browser.close"});
                let _ = stream.send(Message::Text(request.to_string().into())).await;
                // 等浏览器自己关连接；不依赖 close 帧的时序。
                let _ = stream.next().await;
            }
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        while tokio::time::Instant::now() < deadline {
            if self.child.try_wait().ok().flatten().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let _ = self.child.wait();
        self.wait_for_netlog().await
    }

    /// 运行中读一次 netlog；写到一半时返回空串。
    async fn read_netlog(&self) -> String {
        std::fs::read_to_string(&self.netlog_path).unwrap_or_default()
    }

    /// netlog 是进程关闭时才写全的，给它一个短窗口。
    async fn wait_for_netlog(&self) -> Option<String> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(netlog) = std::fs::read_to_string(&self.netlog_path) {
                if !host_resolutions_from_netlog(&netlog).is_empty()
                    || doh_observation_from_netlog(&netlog).is_some()
                {
                    return Some(netlog);
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    fn terminate(&mut self, pid: u32) {
        #[cfg(unix)]
        unsafe {
            let _ = libc::kill(-(pid as i32), libc::SIGTERM);
        }
        std::thread::sleep(Duration::from_millis(1_200));
        #[cfg(unix)]
        unsafe {
            let _ = libc::kill(-(pid as i32), libc::SIGKILL);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ---------------------------------------------------------------------------
// 探针脚本
// ---------------------------------------------------------------------------

fn webrtc_probe_script(gather_ms: u64) -> String {
    format!(
        r#"new Promise((resolve) => {{
  const pc = new RTCPeerConnection({{iceServers: []}});
  const candidates = [];
  pc.onicecandidate = (event) => {{
    if (event.candidate && event.candidate.candidate) candidates.push(event.candidate.candidate);
  }};
  pc.createDataChannel('canary');
  pc.createOffer()
    .then((offer) => pc.setLocalDescription(offer))
    .then(() => setTimeout(() => {{
      const sdp = pc.localDescription ? pc.localDescription.sdp : '';
      const state = pc.iceGatheringState;
      const stats = [];
      pc.getStats().then((report) => {{
        report.forEach((entry) => {{
          if (entry.type === 'local-candidate') stats.push(entry.protocol + '/' + entry.candidateType);
        }});
        pc.close();
        resolve({{candidates, sdp, state, stats}});
      }}).catch(() => {{ pc.close(); resolve({{candidates, sdp, state, stats}}); }});
    }}, {gather_ms}))
    .catch((error) => resolve({{error: String(error), candidates, sdp: '', state: 'failed', stats: []}}));
}})"#
    )
}

fn fetch_script(url: &str) -> String {
    format!(
        r#"fetch({url}, {{cache: 'no-store'}}).then((response) => response.text()).then((body) => 'ok:' + body).catch((error) => 'error:' + String(error))"#,
        url = serde_json::Value::String(url.to_owned())
    )
}

/// 取本机一个非回环 IPv6 地址；没有 IPv6 时返回 None。
fn global_ipv6() -> Option<Ipv6Addr> {
    let socket = std::net::UdpSocket::bind("[::]:0").ok()?;
    // 未连接前的 connect 只做路由选择，不发送流量。
    socket.connect("[2001:4860:4860::8888]:53").ok()?;
    match socket.local_addr().ok()?.ip() {
        std::net::IpAddr::V6(address) if !address.is_loopback() && !address.is_unspecified() => {
            Some(address)
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[test]
fn unset_kernel_env_reports_explicit_skip() {
    match std::env::var_os("MIEN_KERNEL_BIN") {
        // 环境里设了内核时这条不能断言跳过，改断言解析结果。
        Some(value) => {
            assert!(
                Path::new(&value).is_file(),
                "MIEN_KERNEL_BIN must be a file"
            );
            assert_eq!(kernel().as_deref(), Some(Path::new(&value)));
        }
        None => assert_eq!(
            kernel(),
            None,
            "a missing kernel must produce the explicit skip text, not a silent pass"
        ),
    }
}

#[test]
fn dns_stub_answers_canary_names_and_denies_the_rest() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let stub = runtime.block_on(async {
        let stub = DnsStub::start().await;
        let query = |name: &str, qtype: u16| {
            let mut message = vec![0x12, 0x34, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0];
            for label in name.split('.') {
                message.push(label.len() as u8);
                message.extend_from_slice(label.as_bytes());
            }
            message.push(0);
            message.extend_from_slice(&qtype.to_be_bytes());
            message.extend_from_slice(&1u16.to_be_bytes());
            message
        };
        // TCP 通道单独验证：同一端口契约下必须按两字节长度前缀应答。
        let mut tcp = TcpStream::connect(("127.0.0.1", stub.tcp_port))
            .await
            .expect("tcp client");
        let message = query(CANARY_TARGET, 1);
        let mut framed = (message.len() as u16).to_be_bytes().to_vec();
        framed.extend_from_slice(&message);
        tcp.write_all(&framed).await.expect("tcp send");
        let mut length = [0u8; 2];
        tcp.read_exact(&mut length).await.expect("tcp length");
        let mut answer = vec![0u8; usize::from(u16::from_be_bytes(length))];
        tcp.read_exact(&mut answer).await.expect("tcp answer");
        assert_eq!(u16::from_be_bytes([answer[6], answer[7]]), 1);

        let socket = UdpSocket::bind("127.0.0.1:0").await.expect("client");
        for message in [query(CANARY_TARGET, 1), query("other.example.test", 1)] {
            socket
                .send_to(&message, ("127.0.0.1", stub.port))
                .await
                .expect("send");
            let mut buffer = vec![0u8; 1500];
            let (length, _) = socket.recv_from(&mut buffer).await.expect("recv");
            let answers = u16::from_be_bytes([buffer[6], buffer[7]]);
            let rcode = buffer[3] & 0x0f;
            if buffer[2] & 0x80 == 0 {
                panic!("stub must answer with QR set");
            }
            if rcode == 3 {
                assert_eq!(answers, 0, "NXDOMAIN carries no answer");
            } else {
                assert_eq!(rcode, 0, "canary name must resolve");
                assert_eq!(answers, 1, "canary name carries one A record");
            }
            assert!(length > 12);
        }
        stub.take()
    });
    assert_eq!(
        stub.len(),
        3,
        "the stub must record every query, UDP and TCP"
    );
}

/// 在真实内核上测量 DNS canary、WebRTC 候选、IPv6 出口与 DoH 状态，并与声明对账。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn network_canary_measures_dns_webrtc_ipv6_and_doh() {
    let Some(binary) = kernel() else { return };
    assert_eq!(std::env::consts::OS, "macos", "the canary is macOS-only");
    let digest = sha256(&binary);
    println!("kernel sha256={digest}");

    let dns = DnsStub::start().await;
    let page = ProbePage::start().await;
    let echo_v4 = Echo::start("127.0.0.1:0", "IPv4").await;
    let echo_v6 = Echo::start("[::]:0", "IPv6").await;
    let ipv6_target = global_ipv6();
    println!(
        "stubs: dns={} page={} echo_v4={} echo_v6={} global_ipv6={ipv6_target:?}",
        dns.port, page.port, echo_v4.port, echo_v6.port
    );

    let session = KernelSession::start(&binary, &page.url()).await;
    println!("kernel: spawned at {}", session.profile_dir.display());
    let port = session.cdp_port().await;
    println!("kernel: cdp port ready");
    let mut cdp = Cdp::attach_at(port, &session.page_url)
        .await
        .expect("cdp attach");
    println!("kernel: attached on the probe page");

    // --- DNS canary ------------------------------------------------------
    let dns_attempt = cdp
        .evaluate_text(&fetch_script(&format!(
            "http://{CANARY_TARGET}:{}/probe",
            echo_v4.port
        )))
        .await
        .expect("canary fetch");
    // 本机 macOS 由系统解析（mDNSResponder），本地桩通常看不到流量，因此同时读内核
    // 自己 netlog 里的解析器请求；两者都是真实观测。
    let stub_queries = dns.take();
    let resolved_later = session.read_netlog().await;
    let mut observed = stub_queries.clone();
    observed.extend(
        host_resolutions_from_netlog(&resolved_later)
            .into_iter()
            .map(|resolution| ClassifiedQuery {
                name: resolution.host,
                source: QuerySource::External,
            }),
    );
    let dns_verdict = classify_dns_observation(&[CANARY_TARGET.to_owned()], &observed);
    println!(
        "dns: attempt={dns_attempt} stub_queries={} netlog_queries={} verdict={dns_verdict:?}",
        stub_queries.len(),
        observed.len() - stub_queries.len()
    );

    // --- 直连泄漏控制项：回环 ECHO 必须可达，否则探针失效 ----------------
    let echo_text_v4 = cdp
        .evaluate_text(&fetch_script(&format!(
            "http://127.0.0.1:{}/echo",
            echo_v4.port
        )))
        .await
        .expect("v4 echo");
    let echo_text_v6 = cdp
        .evaluate_text(&fetch_script(&format!(
            "http://[::1]:{}/echo",
            echo_v6.port
        )))
        .await
        .expect("v6 echo");
    let v4_hits = echo_v4.take();
    let v6_hits = echo_v6.take();
    println!(
        "echo: v4={echo_text_v4} v6={echo_text_v6} hits={:?}",
        v4_hits
            .iter()
            .chain(v6_hits.iter())
            .map(|hit| format!("{}/{}/{}", hit.family, hit.peer, hit.host))
            .collect::<Vec<_>>()
    );

    // --- WebRTC 候选 ------------------------------------------------------
    let gathered = cdp
        .evaluate(&webrtc_probe_script(2_500))
        .await
        .expect("webrtc gather");
    let candidates: Vec<String> = gathered
        .get("candidates")
        .and_then(serde_json::Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let mut rtc_input = candidates.clone();
    if let Some(sdp) = gathered.get("sdp").and_then(serde_json::Value::as_str) {
        rtc_input.extend(
            sdp.split("\r\n")
                .filter(|line| line.starts_with("a=candidate:"))
                .map(str::to_owned),
        );
    }
    let webrtc = webrtc_candidates(&rtc_input);
    let gathering_state = gathered
        .get("state")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("<none>");
    println!(
        "webrtc: candidates={} input={:?} state={gathering_state} stats={} verdict={webrtc:?}",
        candidates.len(),
        rtc_input,
        gathered
            .get("stats")
            .and_then(serde_json::Value::as_array)
            .map(Vec::len)
            .unwrap_or_default()
    );

    // --- IPv6 出口：目标是本机全局地址，对端看到的必须是非回环地址 ----------
    let ipv6_text = match ipv6_target {
        Some(address) => {
            echo_v6.take();
            Some(
                cdp.evaluate_text(&fetch_script(&format!(
                    "http://[{address}]:{}/echo",
                    echo_v6.port
                )))
                .await
                .expect("ipv6 egress attempt"),
            )
        }
        None => None,
    };
    // 只把对端看到的非回环地址当作出口证据。
    let ipv6_observation = echo_v6
        .take()
        .into_iter()
        .map(|hit| hit.peer)
        .find(|peer| peer != "::1");
    let ipv6 = match (&ipv6_target, &ipv6_observation, &ipv6_text) {
        (Some(_), Some(peer), _) => ipv6_reachable(Some(peer)),
        (Some(_), None, Some(text)) if text.starts_with("error:") => ipv6_reachable(Some(text)),
        _ => ipv6_reachable(None),
    };
    println!("ipv6: text={ipv6_text:?} observed_route={ipv6_observation:?} verdict={ipv6:?}");

    // --- DoH：读内核自己的 netlog 解析器状态 -----------------------------
    // netlog 在进程退出时写盘，所以先优雅停内核再读。
    drop(cdp);
    let netlog = session.stop_and_read_netlog(port).await;
    let doh = netlog.as_deref().and_then(doh_observation_from_netlog);
    let doh_verdict = doh
        .as_ref()
        .map_or(mien_core::network::DohVerdict::Inconclusive, doh_state);
    println!("doh: observation={doh:?} verdict={doh_verdict:?}");

    // --- 与 `LaunchPlan.network` 对账 ------------------------------------
    let manifest: CapabilityManifest = load_pinned_manifest().expect("pinned manifest");
    let claim_doh = match manifest.network_controls.doh_mode {
        NetworkDohMode::Disabled => Enforcement::Declared,
        NetworkDohMode::Unsupported => Enforcement::Unsupported,
    };
    let doh_status = reconcile(claim_doh, doh_verdict.evidence_for_disabled());
    let ipv6_status = reconcile(Enforcement::Declared, ipv6.evidence_for_block());
    let webrtc_status = reconcile(
        Enforcement::Unverified,
        webrtc.evidence_for_disable_non_proxied_udp(),
    );
    println!(
        "reconciled: doh={doh_status:?} ipv6={ipv6_status:?} webrtc={webrtc_status:?} dns={dns_verdict:?}"
    );

    // 探针自身有效性：回环 IPv4 ECHO 必须可达。
    assert!(
        echo_text_v4.starts_with("ok:"),
        "loopback IPv4 echo must answer, got {echo_text_v4}"
    );
    assert!(
        echo_text_v6.starts_with("ok:"),
        "loopback IPv6 echo must answer, got {echo_text_v6}"
    );
    // DNS 判定只能落在三个状态之一。
    assert!(
        matches!(
            dns_verdict,
            DnsVerdict::Leaked | DnsVerdict::NotLeaked | DnsVerdict::Inconclusive
        ),
        "dns verdict must be one of the three states"
    );
    // 没有直连候选时，非代理 UDP 绝不能被判成 Declared。
    if webrtc == WebRtcVerdict::Inconclusive {
        assert_ne!(
            webrtc_status,
            Enforcement::Declared,
            "an empty gatherer can never declare non-proxied UDP blocked"
        );
    }
    if ipv6 == mien_core::network::Ipv6Verdict::Reachable {
        assert_ne!(
            ipv6_status,
            Enforcement::Declared,
            "observed IPv6 egress contradicts ipv6_mode=block"
        );
    }
    let _ = std::io::stdout().flush();
}

/// 对照一：在 `Local State` 里把安全 DNS 打开。若探针有效，DoH 必须从
/// `Disabled` 翻成 `Enabled`——否则「主测试读到 disabled」就无法区分
/// 「确实关闭」和「根本读不到」。
///
/// 这一对照刻意只做 DoH：本机 macOS 的页面解析走 mDNSResponder，
/// `--host-resolver-rules` 与 `--dns-server` 都不会把页面解析打到本地桩上，
/// 所以 DNS 通道在这里只能保持 `Inconclusive`（见 docs/spikes/network）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn network_canary_control_inverts_doh_state() {
    let Some(binary) = kernel() else { return };
    let page = ProbePage::start().await;
    // 模板指向不会解析成功的合成域名；Secure 模式本身不依赖它成功。
    let doh_prefs = vec![
        (
            "Local State",
            r#"{"dns_over_https":{"mode":"secure","templates":"https://doh.canary.test/dns-query"}}"#,
        ),
        ("Default/Preferences", "{}"),
    ];
    let session = KernelSession::start_pref(&binary, &page.url(), &[], &doh_prefs).await;
    let port = session.cdp_port().await;
    let cdp = Cdp::attach_at(port, &session.page_url)
        .await
        .expect("cdp attach");
    drop(cdp);

    let netlog = session.stop_and_read_netlog(port).await;
    let doh = netlog.as_deref().and_then(doh_observation_from_netlog);
    let doh_verdict = doh
        .as_ref()
        .map_or(mien_core::network::DohVerdict::Inconclusive, doh_state);
    println!("control: doh={doh:?} verdict={doh_verdict:?}");
    let observation = doh.expect("the netlog must carry the resolver configuration");
    assert_eq!(
        observation.secure_dns_mode,
        Some(2),
        "the preseeded Local State must select secure DNS"
    );
    assert_eq!(
        doh_verdict,
        mien_core::network::DohVerdict::Enabled,
        "a configured secure-DNS mode must read as DoH enabled"
    );
}

/// 对照二：本机系统 Chrome 用同一探针必须收集到直连候选。若系统 Chrome 也有候选
/// 而内核没有，缺候选就是内核的问题，而不是探针的问题。系统 Chrome 不在时跳过。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn webrtc_probe_detects_candidates_on_the_system_browser() {
    const SYSTEM_CHROME: &str = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
    if kernel().is_none() {
        return;
    }
    let system = Path::new(SYSTEM_CHROME);
    if !system.is_file() {
        println!("skipped: {SYSTEM_CHROME} not installed; no WebRTC control available");
        return;
    }
    let page = ProbePage::start().await;
    let session = KernelSession::start(system, &page.url()).await;
    let port = session.cdp_port().await;
    let mut cdp = Cdp::attach_at(port, &session.page_url)
        .await
        .expect("cdp attach");

    let gathered = cdp
        .evaluate(&webrtc_probe_script(3_000))
        .await
        .expect("control gather");
    let mut entries: Vec<String> = gathered
        .get("candidates")
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    if let Some(sdp) = gathered.get("sdp").and_then(serde_json::Value::as_str) {
        entries.extend(
            sdp.split("\r\n")
                .filter(|line| line.starts_with("a=candidate:"))
                .map(str::to_owned),
        );
    }
    let verdict = webrtc_candidates(&entries);
    println!(
        "control: system-browser candidates={} verdict={verdict:?}",
        entries.len()
    );
    assert_eq!(
        verdict,
        WebRtcVerdict::Direct,
        "the probe must observe candidates on a browser that produces them"
    );
}

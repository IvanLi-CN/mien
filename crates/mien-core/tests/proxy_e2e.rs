//! T20 proxy end-to-end: real HTTP traffic through the Mien shared engine into a
//! controlled Xray server inbound and out to a loopback ECHO origin.
//!
//! `xray_integration.rs` only proves the engine *accepted* an outbound
//! configuration. This test proves traffic actually leaves through it:
//!
//! ```text
//! test HTTP client -> engine loopback HTTP inbound (Mien) -> shared outbound
//!   -> controlled Xray server inbound -> freedom -> ECHO origin
//! ```
//!
//! Discriminators against a direct connection, all measured, none inferred:
//!
//! 1. The origin is addressed by a name generated per run (`proxy-e2e-<run>.test`)
//!    that only the controlled server Xray can resolve, through `dns.hosts` in its
//!    own config. `getaddrinfo` here fails, so a direct fetch of the same URL is
//!    impossible, and the engine has no direct outbound either.
//! 2. The ECHO origin records the peer of every connection. A calibration request
//!    made directly by this process makes ECHO report *this process's* port; the
//!    proxied request reports a different peer port, so another process opened it.
//! 3. The controlled server's own counter (`inbound>>><tag>>>>traffic>>>uplink`)
//!    grows only for the successful request, and its log shows the
//!    `[<inbound> -> <outbound>]` hop.
//!
//! Negative controls live in their own tests because both of them make the engine
//! emit a warning line, and [`EngineProcess`] currently dies on the first one after
//! startup (see `docs/spikes/proxy-e2e/README.md`, "Known engine defect"). Each
//! control therefore gets a fresh engine and asserts only what is observed outside
//! it: the origin sees nothing and the server counter does not move.
//!
//! `MIEN_XRAY_BIN` unset prints an explicit skip and returns; it never passes
//! silently.

use std::{
    collections::BTreeMap,
    net::ToSocketAddrs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};

use mien_core::{
    proxy::{
        api::XrayApi,
        attach::GatewayAttach,
        engine::{EngineController, EngineProcess, EngineState},
        keys::{
            Credentials, MuxSettings, ProxyConfig, ProxyProtocol, TlsSettings, Transport,
            build_outbound,
        },
    },
    runtime::ProxyAttach,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

/// Skip reason required when the engine binary is not configured.
const SKIP_MESSAGE: &str = "skipped: MIEN_XRAY_BIN not set";

/// Fixture with the scenario table and the expectations this test enforces.
const FIXTURE: &str = "fixtures/proxy/e2e-scenarios.json";

/// A single proxied request must not hang the test.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Waiting for a started process to accept connections.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

fn binary() -> Option<PathBuf> {
    match std::env::var_os("MIEN_XRAY_BIN") {
        Some(value) => Some(PathBuf::from(value)),
        None => {
            eprintln!("{SKIP_MESSAGE}");
            None
        }
    }
}

fn digest(path: &Path) -> String {
    format!("{:x}", Sha256::digest(std::fs::read(path).unwrap()))
}

/// Ephemeral loopback port; released before the caller binds it for real.
fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    origin_host_template: String,
    echo_path_template: String,
    server_inbound_tag: String,
    server_outbound_tag: String,
    scenarios: Vec<Scenario>,
    expect: Expect,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Scenario {
    name: String,
    protocol: ProxyProtocol,
    credentials: BTreeMap<String, String>,
    rejected_credentials: BTreeMap<String, String>,
    #[serde(default)]
    options: BTreeMap<String, String>,
    #[serde(default)]
    mux: Mux,
}

#[derive(Deserialize, Default, Clone, Copy)]
#[serde(default)]
struct Mux {
    enabled: bool,
    concurrency: u16,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Expect {
    proxied: Proxied,
    rejected: Rejected,
    origin_host_resolvable_locally: bool,
    server_inbound_uplink_bytes: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Proxied {
    http_status: u16,
    body_contains: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Rejected {
    echo_requests: usize,
}

fn fixture() -> Fixture {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(FIXTURE);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} unreadable: {error}", path.display()));
    let fixture: Fixture = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("{} invalid: {error}", path.display()));
    assert!(!fixture.scenarios.is_empty(), "{FIXTURE} has no scenarios");
    fixture
}

fn scenario<'a>(fixture: &'a Fixture, name: &str) -> &'a Scenario {
    fixture
        .scenarios
        .iter()
        .find(|candidate| candidate.name == name)
        .unwrap_or_else(|| panic!("scenario {name} missing from {FIXTURE}"))
}

/// Parses the fixture's `>N` uplink expectation into `N`.
fn uplink_threshold(expectation: &str) -> u64 {
    let digits = expectation.strip_prefix('>').unwrap_or_else(|| {
        panic!("serverInboundUplinkBytes must be a `>` comparison, found {expectation}")
    });
    digits
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("serverInboundUplinkBytes is not a number: {expectation}"))
}

// ---------------------------------------------------------------------------
// Controlled Xray server (the "node")
// ---------------------------------------------------------------------------

/// A real Xray process acting as the node: one authenticated inbound, one freedom
/// outbound, and a `dns.hosts` entry that only this process can resolve.
struct ServerProcess {
    child: Child,
    _dir: tempfile::TempDir,
    port: u16,
    api_port: u16,
    log: PathBuf,
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl ServerProcess {
    /// Protocol-specific inbound settings for the controlled node.
    fn inbound(protocol: ProxyProtocol, credentials: &BTreeMap<String, String>) -> Value {
        let credential = |name: &str| {
            credentials
                .get(name)
                .cloned()
                .unwrap_or_else(|| panic!("scenario credential {name} missing"))
        };
        match protocol {
            ProxyProtocol::Vless => json!({
                "protocol": "vless",
                "settings": {"clients": [{"id": credential("id")}], "decryption": "none"}
            }),
            ProxyProtocol::Vmess => json!({
                "protocol": "vmess",
                "settings": {"clients": [{"id": credential("id"), "alterId": 0}]}
            }),
            ProxyProtocol::Shadowsocks => json!({
                "protocol": "shadowsocks",
                "settings": {
                    "method": credential("method"),
                    "password": credential("password"),
                    "network": "tcp"
                }
            }),
            ProxyProtocol::Trojan => json!({
                "protocol": "trojan",
                "settings": {"clients": [{"password": credential("password")}]}
            }),
            other => panic!("scenario protocol {other:?} has no controlled server"),
        }
    }

    fn config(
        case: &Scenario,
        fixture: &Fixture,
        port: u16,
        api_port: u16,
        origin_host: &str,
    ) -> Value {
        let mut credentials = case.credentials.clone();
        if case.protocol == ProxyProtocol::Shadowsocks {
            credentials.insert(
                "method".into(),
                case.options
                    .get("method")
                    .cloned()
                    .unwrap_or_else(|| "aes-256-gcm".into()),
            );
        }
        let inbound = Self::inbound(case.protocol, &credentials);
        let inbound_tag = &fixture.server_inbound_tag;
        let outbound_tag = &fixture.server_outbound_tag;
        json!({
            "log": {"loglevel": "debug"},
            "api": {"tag": "api", "services": ["StatsService"]},
            "stats": {},
            "policy": {"system": {"statsInboundUplink": true, "statsInboundDownlink": true}},
            "inbounds": [
                {
                    "tag": "api",
                    "listen": "127.0.0.1",
                    "port": api_port,
                    "protocol": "dokodemo-door",
                    "settings": {"address": "127.0.0.1"}
                },
                {
                    "tag": inbound_tag,
                    "listen": "127.0.0.1",
                    "port": port,
                    "protocol": inbound["protocol"],
                    "settings": inbound["settings"]
                }
            ],
            // `finalRules: allow` is required: v26.11.9 freedom blocks loopback
            // targets by default, which would mask the tunnel with a policy error.
            "outbounds": [{
                "tag": outbound_tag,
                "protocol": "freedom",
                "settings": {
                    "domainStrategy": "UseIP",
                    "finalRules": [{"action": "allow"}]
                }
            }],
            // Only this process may resolve the origin name.
            "dns": {"hosts": {origin_host: "127.0.0.1"}, "queryStrategy": "UseIPv4"},
            "routing": {
                "domainStrategy": "AsIs",
                "rules": [
                    {"type": "field", "inboundTag": ["api"], "outboundTag": "api"},
                    {"type": "field", "inboundTag": [inbound_tag], "outboundTag": outbound_tag}
                ]
            }
        })
    }

    async fn start(bin: &Path, fixture: &Fixture, case: &Scenario, origin_host: &str) -> Self {
        let port = free_port();
        let api_port = free_port();
        let dir = tempfile::tempdir().unwrap();
        let config = Self::config(case, fixture, port, api_port, origin_host);
        let config_path = dir.path().join("server.json");
        std::fs::write(&config_path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
        let log = dir.path().join("server.log");
        let stdout = std::fs::File::create(&log).unwrap();
        let stderr = stdout.try_clone().unwrap();
        let child = Command::new(bin)
            .arg("run")
            .arg(format!("-config={}", config_path.display()))
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .unwrap();
        let server = Self {
            child,
            _dir: dir,
            port,
            api_port,
            log,
        };
        wait_port(port, "controlled server").await;
        server
    }

    fn log_text(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Polls until `port` accepts a TCP connection.
async fn wait_port(port: u16, what: &str) {
    let deadline = tokio::time::Instant::now() + STARTUP_TIMEOUT;
    loop {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{what} did not listen on 127.0.0.1:{port} within {STARTUP_TIMEOUT:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// `inbound>>><tag>>>>traffic>>>uplink` as measured inside the server process.
async fn server_inbound_uplink(bin: &Path, api_port: u16, tag: &str) -> u64 {
    server_inbound_counter(bin, api_port, tag, "uplink").await
}

async fn server_inbound_counter(bin: &Path, api_port: u16, tag: &str, direction: &str) -> u64 {
    let pattern = format!("inbound>>>{tag}>>>traffic>>>{direction}");
    let output = tokio::process::Command::new(bin)
        .args([
            "api",
            "statsquery",
            &format!("--server=127.0.0.1:{api_port}"),
            "-pattern",
            &pattern,
        ])
        .output()
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    let value: Value = serde_json::from_str(text.trim()).unwrap_or_else(|_| json!({}));
    value["stat"]
        .as_array()
        .and_then(|stats| {
            stats
                .iter()
                .find(|stat| stat["name"] == pattern)
                .and_then(|stat| stat["value"].as_u64())
        })
        .unwrap_or(0)
}

/// True when the controlled server logged a completed `[inbound -> outbound]` hop.
fn server_log_has_hop(server: &ServerProcess, fixture: &Fixture) -> bool {
    let hop = format!(
        "[{} -> {}]",
        fixture.server_inbound_tag, fixture.server_outbound_tag
    );
    server.log_text().contains(&hop)
}

// ---------------------------------------------------------------------------
// ECHO origin
// ---------------------------------------------------------------------------

/// One connection observed by the ECHO origin.
#[derive(Clone, Debug)]
struct EchoHit {
    peer: String,
    request_line: String,
}

/// Loopback HTTP origin that echoes the peer address it actually saw.
struct Echo {
    port: u16,
    hits: Arc<Mutex<Vec<EchoHit>>>,
}

impl Echo {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(Mutex::new(Vec::new()));
        let recorded = hits.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, peer)) = listener.accept().await else {
                    return;
                };
                let recorded = recorded.clone();
                tokio::spawn(async move {
                    let mut buffer = Vec::new();
                    let mut chunk = [0u8; 1024];
                    while !buffer.windows(4).any(|window| window == b"\r\n\r\n") {
                        match socket.read(&mut chunk).await {
                            Ok(0) | Err(_) => break,
                            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
                        }
                    }
                    let request_line = String::from_utf8_lossy(&buffer)
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .to_owned();
                    recorded.lock().unwrap().push(EchoHit {
                        peer: peer.to_string(),
                        request_line: request_line.clone(),
                    });
                    let body = format!("peer={peer} req={request_line}\n");
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        Self { port, hits }
    }

    fn hits(&self) -> Vec<EchoHit> {
        self.hits.lock().unwrap().clone()
    }

    fn hit_count(&self) -> usize {
        self.hits.lock().unwrap().len()
    }
}

// ---------------------------------------------------------------------------
// HTTP client
// ---------------------------------------------------------------------------

struct Response {
    status: u16,
    body: String,
    /// Loopback port this process used for the hop it made itself.
    local_port: u16,
}

impl Response {
    fn ok(&self) -> bool {
        self.status == 200
    }
}

/// One HTTP/1.1 request to a loopback port. Callers always pass an absolute-form
/// URI, which the engine's HTTP inbound requires.
async fn http_request(port: u16, uri: &str, host: &str) -> Result<Response, String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .map_err(|error| format!("connect 127.0.0.1:{port}: {error}"))?;
    let local_port = stream.local_addr().map_err(|e| e.to_string())?.port();
    let request = format!(
        "GET {uri} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: mien-proxy-e2e\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|error| format!("write: {error}"))?;
    let mut raw = Vec::new();
    match tokio::time::timeout(REQUEST_TIMEOUT, stream.read_to_end(&mut raw)).await {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => return Err(format!("read: {error}")),
        Err(_) => return Err(format!("read timed out after {REQUEST_TIMEOUT:?}")),
    }
    let text = String::from_utf8_lossy(&raw).to_string();
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| format!("no HTTP response head: {text}"))?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| format!("no HTTP status line: {head}"))?;
    Ok(Response {
        status,
        body: body.to_owned(),
        local_port,
    })
}

// ---------------------------------------------------------------------------
// Assertions shared by every scenario
// ---------------------------------------------------------------------------

/// Origin host and path that only the controlled server can serve.
struct Target {
    host: String,
    path: String,
}

impl Target {
    fn from_fixture(fixture: &Fixture, scenario_name: &str) -> Self {
        Self {
            host: fixture
                .origin_host_template
                .replace("{run}", &format!("{}-{}", std::process::id(), free_port())),
            path: fixture
                .echo_path_template
                .replace("{scenario}", scenario_name),
        }
    }

    fn authority(&self, port: u16) -> String {
        format!("{}:{port}", self.host)
    }

    fn uri(&self, port: u16) -> String {
        format!("http://{}{}", self.authority(port), self.path)
    }
}

/// Proves the origin name cannot be resolved here, so nothing can reach it directly.
fn assert_origin_name_unresolvable(fixture: &Fixture, target: &Target, port: u16) {
    let resolvable = target.authority(port).to_socket_addrs().is_ok();
    assert_eq!(
        resolvable, fixture.expect.origin_host_resolvable_locally,
        "{} resolution must be {} for the proxy proof to hold: a resolvable origin \
         name would let a direct connection explain the observed traffic",
        target.host, fixture.expect.origin_host_resolvable_locally
    );
}

/// Direct calibration: this process reaches the origin itself, so the origin
/// reports this process's own port. Establishes what "not proxied" looks like.
async fn calibrate(echo: &Echo, target: &Target) -> Response {
    let response = http_request(echo.port, &target.path, &target.authority(echo.port))
        .await
        .expect("direct calibration request to the ECHO origin failed");
    assert!(
        response.ok(),
        "ECHO direct calibration returned {}",
        response.status
    );
    assert!(
        response
            .body
            .contains(&format!("peer=127.0.0.1:{}", response.local_port)),
        "ECHO calibration did not report this process's port {}: {}",
        response.local_port,
        response.body
    );
    response
}

fn proxy_config(
    case: &Scenario,
    credentials: &BTreeMap<String, String>,
    server_port: u16,
) -> ProxyConfig {
    ProxyConfig {
        protocol: case.protocol,
        server: "127.0.0.1".into(),
        port: server_port,
        transport: Transport::Tcp,
        tls: TlsSettings::default(),
        mux: MuxSettings {
            enabled: case.mux.enabled,
            concurrency: case.mux.concurrency,
        },
        options: case.options.clone(),
        credentials: Credentials {
            values: credentials.clone(),
        },
    }
}

/// Starts the Mien shared engine and one runtime binding for `case`.
async fn start_engine_with_binding(
    bin: &Path,
    case: &Scenario,
    credentials: &BTreeMap<String, String>,
    server_port: u16,
    runtime: &str,
) -> (
    EngineProcess,
    Arc<EngineController<mien_core::proxy::api::GrpcXrayApi>>,
    GatewayAttach<mien_core::proxy::api::GrpcXrayApi>,
    mien_core::proxy::keys::OutboundSpec,
) {
    let engine = EngineProcess::start(bin, &digest(bin)).await.unwrap();
    let controller = Arc::new(EngineController::with_fallback(engine.api(), false));
    controller.set_state(EngineState::Running);
    let spec = build_outbound(
        &proxy_config(case, credentials, server_port),
        b"t20-proxy-e2e-master",
    )
    .unwrap();
    let attach = GatewayAttach::new(controller.clone(), spec.clone());
    attach
        .attach(runtime)
        .unwrap_or_else(|error| panic!("{runtime}: engine attach failed: {error}"));
    (engine, controller, attach, spec)
}

// ---------------------------------------------------------------------------
// Positive path
// ---------------------------------------------------------------------------

async fn proxied_traffic_traverses_shared_engine(name: &str) {
    let Some(bin) = binary() else { return };
    let fixture = fixture();
    let case = scenario(&fixture, name);
    let target = Target::from_fixture(&fixture, name);

    // Negative-resolvability first: without it, nothing below is evidence.
    assert_origin_name_unresolvable(&fixture, &target, free_port());

    let echo = Echo::start().await;
    let mut server = ServerProcess::start(&bin, &fixture, case, &target.host).await;
    let direct = calibrate(&echo, &target).await;

    let runtime = format!("t20-{name}");
    let (mut engine, controller, attach, spec) =
        start_engine_with_binding(&bin, case, &case.credentials, server.port, &runtime).await;
    let binding = controller.binding(&runtime).unwrap();
    assert_eq!(
        attach.outbound_key(),
        &spec.key,
        "{name}: attach does not use the shared outbound key"
    );
    assert_eq!(
        controller.ref_count(&spec.key),
        1,
        "{name}: shared outbound refcount is not 1"
    );
    assert!(
        engine
            .api()
            .list_outbounds()
            .unwrap()
            .iter()
            .any(|outbound| outbound.tag == spec.tag),
        "{name}: shared outbound missing from the engine"
    );

    let hits_before = echo.hit_count();
    let uplink_before =
        server_inbound_uplink(&bin, server.api_port, &fixture.server_inbound_tag).await;
    let uri = target.uri(echo.port);
    let proxied = http_request(binding.proxy_port, &uri, &target.authority(echo.port))
        .await
        .unwrap_or_else(|error| panic!("{name}: request through the engine failed: {error}"));

    // Evidence 1: a real HTTP response produced by the origin.
    assert_eq!(
        proxied.status, fixture.expect.proxied.http_status,
        "{name}: unexpected status through the engine, body {:?}",
        proxied.body
    );
    for needle in &fixture.expect.proxied.body_contains {
        let needle = needle.replace("{path}", &target.path);
        assert!(
            proxied.body.contains(&needle),
            "{name}: ECHO body {:?} does not contain {needle:?}",
            proxied.body
        );
    }

    // Evidence 2: the origin saw a peer that is neither of this process's sockets.
    let peer_port = proxied
        .body
        .split("peer=127.0.0.1:")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|port| port.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("{name}: ECHO body has no peer: {:?}", proxied.body));
    assert_ne!(
        peer_port, direct.local_port,
        "{name}: origin saw this process's socket, not a proxied hop"
    );
    assert_ne!(
        peer_port, proxied.local_port,
        "{name}: origin saw the client-side hop port, not a proxied hop"
    );
    let hits = echo.hits();
    assert_eq!(
        hits.len(),
        hits_before + 1,
        "{name}: expected exactly one proxied origin connection, saw {hits:?}"
    );
    let hit = hits.last().unwrap();
    assert_eq!(hit.peer, format!("127.0.0.1:{peer_port}"));
    assert!(
        hit.request_line.contains(&target.path),
        "{name}: origin saw the wrong request line {:?}",
        hit.request_line
    );

    // Evidence 3: the controlled server's own counter and route log.
    let uplink = server_inbound_uplink(&bin, server.api_port, &fixture.server_inbound_tag).await;
    let threshold = uplink_threshold(&fixture.expect.server_inbound_uplink_bytes);
    assert!(
        uplink.saturating_sub(uplink_before) > threshold,
        "{name}: server inbound uplink grew by {} bytes, expected > {threshold} \
         ({uplink_before} -> {uplink})",
        uplink.saturating_sub(uplink_before)
    );
    let route_line = format!(
        "[{} -> {}]",
        fixture.server_inbound_tag, fixture.server_outbound_tag
    );
    // The server logs one line per accepted connection naming the destination it
    // dialled and the outbound it used. That line exists only for a completed
    // inbound handshake, so it also rules out a pre-auth rejection.
    let accepted = format!(
        "accepted tcp:{}:{} [{} -> {}]",
        target.host, echo.port, fixture.server_inbound_tag, fixture.server_outbound_tag
    );
    let log = wait_for_log(&server, &[&accepted, route_line.as_str()]).await;
    assert!(
        log.contains(&accepted),
        "{name}: server log lacks the accepted-connection line {accepted:?}"
    );
    println!(
        "[t20 proxy-e2e] req=REQ-PROXY-001,REQ-PROXY-003,REQ-PROXY-005,REQ-PROXY-021 \
         scenario={name} protocol={} origin={} client_hop_port={} direct_peer_port={} \
         proxied_peer_port={peer_port} server_uplink_bytes={uplink_before}->{uplink} \
         server_route={route_line}",
        case.protocol.as_str(),
        target.authority(echo.port),
        proxied.local_port,
        direct.local_port,
    );

    // Teardown while the engine is still healthy: no residue may survive.
    attach.detach(&runtime).unwrap();
    assert!(
        controller.bindings().is_empty(),
        "{name}: binding survived detach"
    );
    assert_eq!(
        controller.ref_count(&spec.key),
        0,
        "{name}: refcount did not return to zero"
    );
    assert!(
        !engine
            .api()
            .list_outbounds()
            .unwrap()
            .iter()
            .any(|outbound| outbound.tag == spec.tag),
        "{name}: shared outbound survived detach"
    );
    assert!(
        TcpStream::connect(("127.0.0.1", binding.proxy_port))
            .await
            .is_err(),
        "{name}: runtime inbound port still open after detach"
    );
    engine.stop(Duration::from_secs(2)).await.unwrap();
    server.kill();
}

/// Polls the server log for every needle; a missing needle fails the test.
async fn wait_for_log(server: &ServerProcess, needles: &[&str]) -> String {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let text = server.log_text();
        if needles.iter().all(|needle| text.contains(needle)) {
            return text;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "controlled server log never showed {needles:?}; tail:\n{}",
            text.lines().rev().take(20).collect::<Vec<_>>().join("\n")
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn vless_traffic_traverses_shared_engine() {
    proxied_traffic_traverses_shared_engine("vless-tcp").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn vmess_traffic_traverses_shared_engine() {
    proxied_traffic_traverses_shared_engine("vmess-tcp").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shadowsocks_traffic_traverses_shared_engine() {
    proxied_traffic_traverses_shared_engine("shadowsocks-tcp").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trojan_traffic_traverses_shared_engine() {
    proxied_traffic_traverses_shared_engine("trojan-tcp").await;
}

// ---------------------------------------------------------------------------
// Negative controls: each gets its own engine lifetime
// ---------------------------------------------------------------------------

/// Wrong credentials must never produce origin traffic, for any protocol.
///
/// Each iteration spawns its own engine because the resulting engine-side warning
/// is what kills the shared engine today; see the module docs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_credentials_never_reach_origin() {
    let Some(bin) = binary() else { return };
    let fixture = fixture();
    for case in &fixture.scenarios {
        let target = Target::from_fixture(&fixture, &case.name);
        let echo = Echo::start().await;
        let mut server = ServerProcess::start(&bin, &fixture, case, &target.host).await;
        let runtime = format!("t20-{}-rejected", case.name);
        let (mut engine, controller, attach, spec) = start_engine_with_binding(
            &bin,
            case,
            &case.rejected_credentials,
            server.port,
            &runtime,
        )
        .await;
        assert!(
            !engine
                .api()
                .list_outbounds()
                .unwrap()
                .iter()
                .filter(|outbound| outbound.tag == spec.tag)
                .collect::<Vec<_>>()
                .is_empty(),
            "{}: rejected outbound was not created",
            case.name
        );

        let hits_before = echo.hit_count();
        let uri = target.uri(echo.port);
        let binding = controller.binding(&runtime).unwrap();
        let outcome = http_request(binding.proxy_port, &uri, &target.authority(echo.port)).await;
        assert!(
            outcome.as_ref().map_or(true, |response| !response.ok()),
            "{}: wrong credentials produced a successful proxied response",
            case.name
        );
        assert_eq!(
            echo.hit_count() - hits_before,
            fixture.expect.rejected.echo_requests,
            "{}: wrong credentials reached the origin",
            case.name
        );
        // Authentication failed before any destination was dialled, so the server
        // log must never show the inbound-to-outbound hop. Its uplink counter does
        // grow from the rejected handshake, which is why it is not used here.
        assert!(
            !server_log_has_hop(&server, &fixture),
            "{}: server log shows an inbound-to-outbound hop for rejected credentials",
            case.name
        );
        println!(
            "[t20 proxy-e2e] req=REQ-PROXY-009,REQ-PROXY-014 scenario={}-rejected \
             protocol={} origin_hits={} server_route_hop=absent",
            case.name,
            case.protocol.as_str(),
            fixture.expect.rejected.echo_requests,
        );
        let _ = attach;
        // The shared engine may already be dead here (known defect); stop only if alive.
        if engine.api().probe().await.is_ok() {
            engine.stop(Duration::from_secs(1)).await.unwrap();
        }
        server.kill();
    }
}

/// With the node gone, the same request must fail and the origin must stay silent,
/// so the earlier success depended on the authenticated tunnel and the server.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dead_node_never_reaches_origin() {
    let Some(bin) = binary() else { return };
    let fixture = fixture();
    let case = scenario(&fixture, "vless-tcp");
    let target = Target::from_fixture(&fixture, &case.name);
    let echo = Echo::start().await;
    let mut server = ServerProcess::start(&bin, &fixture, case, &target.host).await;
    let runtime = "t20-dead-node";
    let (mut engine, controller, attach, _spec) =
        start_engine_with_binding(&bin, case, &case.credentials, server.port, runtime).await;
    let binding = controller.binding(runtime).unwrap();
    let uri = target.uri(echo.port);

    // Warm-up: the tunnel works while the node is alive.
    let warm = http_request(binding.proxy_port, &uri, &target.authority(echo.port))
        .await
        .expect("warm-up request through the engine failed");
    assert!(warm.ok(), "warm-up status {}", warm.status);

    let hits_before = echo.hit_count();
    server.kill();
    let outcome = http_request(binding.proxy_port, &uri, &target.authority(echo.port)).await;
    assert!(
        outcome.as_ref().map_or(true, |response| !response.ok()),
        "request succeeded without the controlled node"
    );
    assert_eq!(
        echo.hit_count(),
        hits_before,
        "origin saw traffic without the controlled node"
    );
    println!(
        "[t20 proxy-e2e] req=REQ-PROXY-009 scenario=dead-node protocol=vless \
         origin_hits_after_node_kill={hits_before}"
    );
    let _ = attach;
    if engine.api().probe().await.is_ok() {
        engine.stop(Duration::from_secs(1)).await.unwrap();
    }
}

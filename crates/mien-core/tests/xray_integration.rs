//! Real Xray v26.11.9 gRPC integration: shared outbound, dynamic route/inbound, CRUD idempotency, crash/rebuild and TUIC rejection.
use mien_core::error::ErrorCode;
use mien_core::proxy::{
    api::{self, XrayApi},
    engine::{EngineController, EngineProcess, EngineState, RuntimeBinding},
    keys::{
        Credentials, MuxSettings, OutboundKey, ProxyConfig, ProxyProtocol, TlsSettings, Transport,
        build_outbound,
    },
};
use serde_json::json;
use std::{collections::BTreeMap, env, path::PathBuf, time::Duration};
fn binary() -> Option<PathBuf> {
    let v = env::var_os("MIEN_XRAY_BIN");
    if v.is_none() {
        eprintln!("skipped: MIEN_XRAY_BIN not set")
    }
    v.map(PathBuf::from)
}
fn digest(p: &std::path::Path) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(std::fs::read(p).unwrap()))
}
fn cfg(p: u16) -> ProxyConfig {
    ProxyConfig {
        protocol: ProxyProtocol::Vless,
        server: "127.0.0.1".into(),
        port: p,
        transport: Transport::Tcp,
        tls: TlsSettings::default(),
        mux: MuxSettings {
            enabled: true,
            concurrency: 8,
        },
        options: BTreeMap::new(),
        credentials: Credentials {
            values: [("id".into(), "00000000-0000-4000-8000-000000000001".into())]
                .into_iter()
                .collect(),
        },
    }
}
fn port() -> u16 {
    let s = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    s.local_addr().unwrap().port()
}
fn binding(id: &str, k: OutboundKey, p: u16) -> RuntimeBinding {
    RuntimeBinding {
        runtime_id: id.into(),
        outbound_key: k,
        inbound_tag: format!("rt:{id}:in"),
        rule_tag: format!("rt:{id}:rule"),
        proxy_port: p,
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_xray_gateway_and_refcounts() {
    let Some(bin) = binary() else { return };
    let mut engine = EngineProcess::start(&bin, &digest(&bin)).await.unwrap();
    let api = engine.api();
    let ctl = EngineController::new(api.clone());
    ctl.set_state(EngineState::Running);
    let spec = build_outbound(&cfg(port()), b"test-master").unwrap();
    let a = binding("a", spec.key.clone(), port());
    let b = binding("b", spec.key.clone(), port());
    ctl.acquire(a.clone(), &spec).unwrap();
    ctl.acquire(b.clone(), &spec).unwrap();
    assert_eq!(ctl.ref_count(&spec.key), 2);
    assert_eq!(
        api.list_outbounds()
            .unwrap()
            .iter()
            .filter(|x| x.tag == spec.tag)
            .count(),
        1
    );
    assert_eq!(
        api.list_rules()
            .unwrap()
            .iter()
            .filter(|x| x.tag == a.rule_tag || x.tag == b.rule_tag)
            .count(),
        2
    );
    ctl.release("a").unwrap();
    assert_eq!(ctl.ref_count(&spec.key), 1);
    assert!(
        api.list_outbounds()
            .unwrap()
            .iter()
            .any(|x| x.tag == spec.tag)
    );
    ctl.release("b").unwrap();
    assert!(
        !api.list_outbounds()
            .unwrap()
            .iter()
            .any(|x| x.tag == spec.tag)
    );
    let tuic = ProxyConfig {
        protocol: ProxyProtocol::Tuic,
        ..cfg(1)
    };
    assert_eq!(
        build_outbound(&tuic, b"key").unwrap_err().code(),
        ErrorCode::TuicNotSupported
    );
    engine.stop(Duration::from_secs(1)).await.unwrap()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_xray_duplicate_tags_and_unknown_remove() {
    let Some(bin) = binary() else { return };
    let mut e = EngineProcess::start(&bin, &digest(&bin)).await.unwrap();
    let api = e.api();
    let x = api::Inbound {
        tag: "rt:duplicate:in".into(),
        listen: "127.0.0.1".into(),
        port: port(),
        protocol: "http".into(),
        settings: json!({}),
    };
    api::ensure_inbound(&*api, x.clone()).unwrap();
    api::ensure_inbound(&*api, x).unwrap();
    assert_eq!(
        api.list_inbounds()
            .unwrap()
            .iter()
            .filter(|x| x.tag == "rt:duplicate:in")
            .count(),
        1
    );
    assert!(api::remove_inbound(&*api, "unknown:in").is_ok());
    e.stop(Duration::from_secs(1)).await.unwrap()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_xray_crash_then_rebuild_original_api_port() {
    let Some(bin) = binary() else { return };
    let mut e = EngineProcess::start(&bin, &digest(&bin)).await.unwrap();
    let p = e.api_port();
    let pid = e.pid().unwrap();
    unsafe {
        libc::kill(pid as i32, libc::SIGKILL);
    };
    e.force_kill().await.unwrap();
    let mut rebuilt = EngineProcess::start_with_api_port(&bin, p).await.unwrap();
    assert_eq!(rebuilt.api_port(), p);
    assert!(rebuilt.api().probe().await.is_ok());
    rebuilt.stop(Duration::from_secs(1)).await.unwrap()
}
fn tier_a(
    protocol: ProxyProtocol,
    credentials: &[(&str, &str)],
    options: &[(&str, &str)],
) -> ProxyConfig {
    ProxyConfig {
        protocol,
        server: "127.0.0.1".into(),
        port: 443,
        transport: Transport::Tcp,
        tls: TlsSettings::default(),
        mux: MuxSettings::default(),
        options: options
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect(),
        credentials: Credentials {
            values: credentials
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into()))
                .collect(),
        },
    }
}

async fn assert_tier_a_round_trip(config: ProxyConfig) {
    let Some(bin) = binary() else { return };
    let mut engine = EngineProcess::start(&bin, &digest(&bin)).await.unwrap();
    let api = engine.api();
    let ctl = EngineController::new(api.clone());
    ctl.set_state(EngineState::Running);
    let spec = build_outbound(&config, b"tier-a-master").unwrap();
    let b = binding("tier-a", spec.key.clone(), port());
    ctl.acquire(b, &spec).unwrap();
    assert_eq!(ctl.ref_count(&spec.key), 1);
    assert!(
        api.list_outbounds()
            .unwrap()
            .iter()
            .any(|x| x.tag == spec.tag),
        "{} outbound missing from engine list",
        config.protocol.as_str()
    );
    ctl.release("tier-a").unwrap();
    assert!(
        !api.list_outbounds()
            .unwrap()
            .iter()
            .any(|x| x.tag == spec.tag),
        "{} outbound survived release",
        config.protocol.as_str()
    );
    engine.stop(Duration::from_secs(1)).await.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_xray_vmess_outbound() {
    assert_tier_a_round_trip(tier_a(
        ProxyProtocol::Vmess,
        &[("id", "00000000-0000-4000-8000-000000000002")],
        &[],
    ))
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_xray_shadowsocks_outbound() {
    assert_tier_a_round_trip(tier_a(
        ProxyProtocol::Shadowsocks,
        &[("password", "tier-a-secret")],
        &[("method", "aes-256-gcm")],
    ))
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_xray_trojan_outbound() {
    assert_tier_a_round_trip(tier_a(
        ProxyProtocol::Trojan,
        &[("password", "tier-a-secret")],
        &[],
    ))
    .await
}

/// 回归：引擎写一条 warning 后必须存活。
///
/// `pump` 曾因接收端被丢弃而 `break`，读端随任务结束关闭，Xray 下一次写日志命中
/// 已关闭管道，收到 `SIGPIPE` 退出——表现为「第一个出站失败后整个网关失效」。
/// 因此必须真的经入站发一次必然失败的请求，让引擎自己写出那一行日志。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn engine_survives_a_warning_after_the_log_consumer_is_dropped() {
    let Some(bin) = binary() else { return };
    let mut engine = EngineProcess::start(&bin, &digest(&bin)).await.unwrap();
    let api = engine.api();

    // 出站指向一个没人监听的回环端口；经入站发请求必然失败并让引擎写日志。
    let dead = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let spec = build_outbound(&cfg(dead), b"test-master").unwrap();
    let ctl = EngineController::new(api.clone());
    ctl.set_state(EngineState::Running);
    let proxy_port = port();
    ctl.acquire(
        RuntimeBinding {
            runtime_id: "warn".into(),
            outbound_key: spec.key.clone(),
            inbound_tag: "rt:warn:in".into(),
            rule_tag: "rt:warn:rule".into(),
            proxy_port,
        },
        &spec,
    )
    .unwrap();

    // 真实请求：用绝对 URI 走 HTTP 入站，引擎会尝试 dial 死端口并写 warning。
    use tokio::io::AsyncWriteExt;
    for _ in 0..3 {
        if let Ok(mut stream) = tokio::net::TcpStream::connect(("127.0.0.1", proxy_port)).await {
            let _ = stream
                .write_all(b"GET http://127.0.0.1:1/ HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
                .await;
            let _ = stream.shutdown().await;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }

    assert!(
        api.probe().await.is_ok(),
        "engine died after writing a log line (SIGPIPE regression)"
    );
    let pid = engine.pid().expect("engine pid");
    let out = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "stat="])
        .output()
        .expect("ps");
    let stat = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stat.trim().is_empty() && !stat.trim_start().starts_with('Z'),
        "engine process is gone or a zombie: stat={stat:?}"
    );

    let _ = engine.stop(Duration::from_secs(1)).await;
}

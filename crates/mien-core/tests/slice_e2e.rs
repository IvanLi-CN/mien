//! M1 vertical slice: create -> start -> stop -> start again.
//!
//! Runs against the pinned kernel when `MIEN_KERNEL_BIN` is set. Without it the
//! test reports an explicit skip marker instead of passing silently.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mien_core::ErrorCode;
use mien_core::domain::profile::{CreateProfile, ProfileMode, ProfileStatus};
use mien_core::fingerprint::load_pinned_manifest;
use mien_core::fingerprint::spec::{FingerprintSpec, Platform};
use mien_core::ports::repository::ProfileRepository;
use mien_core::proxy::api::XrayApi;
use mien_core::proxy::gateway::proxy_server_arg;
use mien_core::runtime::{
    ProxyAttach, ProxyAttachment, RuntimeSnapshot, RuntimeState, RuntimeSupervisor, StartProfile,
};
use mien_core::storage::{open_and_migrate, sqlite::SqliteProfileRepository};
use sha2::{Digest, Sha256};

struct StubProxy {
    attached: Mutex<Vec<String>>,
    detached: Mutex<Vec<String>>,
}

impl StubProxy {
    fn new() -> Self {
        Self {
            attached: Mutex::new(Vec::new()),
            detached: Mutex::new(Vec::new()),
        }
    }
}

impl ProxyAttach for StubProxy {
    fn attach(&self, runtime_id: &str) -> Result<ProxyAttachment, mien_core::CoreError> {
        self.attached
            .lock()
            .expect("lock")
            .push(runtime_id.to_owned());
        Ok(ProxyAttachment {
            port: 34567,
            outbound_key: format!("out:{runtime_id}"),
        })
    }

    fn detach(&self, runtime_id: &str) -> Result<(), mien_core::CoreError> {
        self.detached
            .lock()
            .expect("lock")
            .push(runtime_id.to_owned());
        Ok(())
    }
}

fn kernel() -> Option<PathBuf> {
    match std::env::var_os("MIEN_KERNEL_BIN") {
        None => {
            println!("skipped: MIEN_KERNEL_BIN not set");
            None
        }
        Some(value) => {
            let path = PathBuf::from(value);
            assert!(path.is_file(), "MIEN_KERNEL_BIN is not a file: {path:?}");
            Some(path)
        }
    }
}

fn seed_proxy(pool: &sqlx::SqlitePool, id: &str) {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let now = "2026-01-01T00:00:00Z";
    runtime
        .block_on(async {
            sqlx::query(
                "INSERT INTO proxies (id,name,kind,server,port,config_ciphertext,credential_fields_json,created_at,updated_at) VALUES (?1,?2,'vless','127.0.0.1',443,x'00','{}',?3,?3)",
            )
            .bind(id)
            .bind("seed proxy")
            .bind(now)
            .execute(pool)
            .await
        })
        .expect("seed proxy");
}

fn sha256(path: &Path) -> String {
    let bytes = std::fs::read(path).expect("kernel readable");
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn profile(name: &str, proxy_id: Option<&str>) -> CreateProfile {
    CreateProfile {
        name: name.to_owned(),
        fingerprint: FingerprintSpec::fresh(Platform::Macos).expect("fresh spec"),
        start_url: None,
        proxy_id: proxy_id.map(str::to_owned),
        notes: None,
        kernel_version: "148".into(),
        mode: ProfileMode::Headful,
        automation_api: true,
    }
}

async fn start_profile(
    profile: &mien_core::domain::profile::Profile,
    binary: &Path,
    digest: &str,
    proxy: Option<&dyn ProxyAttach>,
    root: &Path,
    supervisor: &RuntimeSupervisor,
) -> mien_core::runtime::StartedProfile {
    let manifest = load_pinned_manifest().expect("pinned manifest");
    let request = StartProfile {
        profile,
        binary: binary.to_path_buf(),
        binary_sha256: digest.to_owned(),
        manifest: &manifest,
        data_root: root.to_path_buf(),
        sandbox_kind: "seatbelt".into(),
        proxy,
        stop_timeout: Duration::from_secs(5),
    };
    request
        .start(supervisor, &format!("rt-{}", profile.id.0))
        .await
        .expect("start")
}

/// Stops the runtime on drop so a failed assertion cannot leave an orphan kernel.
struct StopOnDrop<'a> {
    supervisor: &'a RuntimeSupervisor,
    runtime_id: String,
}

impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        if let Ok(runtime) = tokio::runtime::Runtime::new() {
            let _ = runtime.block_on(self.supervisor.stop(&self.runtime_id));
        }
    }
}

fn stop(supervisor: &RuntimeSupervisor, runtime_id: &str) -> RuntimeSnapshot {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(supervisor.stop(runtime_id)).expect("stop")
}

#[test]
fn slice_starts_stops_and_restarts() {
    let Some(binary) = kernel() else {
        return;
    };
    let digest = sha256(&binary);
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let dir = tempfile::tempdir().expect("tempdir");
    let database = dir.path().join("mien.db");
    let root = dir.path().join("data");
    let pool = open_and_migrate(&database).expect("migrate");
    let repository = SqliteProfileRepository::new(pool, &root).expect("repository");

    let stored = runtime
        .block_on(repository.create(profile("slice-direct", None)))
        .expect("create");
    assert_eq!(stored.status, ProfileStatus::Active);
    assert!(stored.proxy_id.is_none());

    let supervisor = RuntimeSupervisor::new(&root);
    let first = runtime.block_on(async {
        start_profile(&stored, &binary, &digest, None, &root, &supervisor).await
    });
    let _guard = StopOnDrop {
        supervisor: &supervisor,
        runtime_id: format!("rt-{}", stored.id.0),
    };
    assert_eq!(first.snapshot.state, RuntimeState::Running);
    assert!(first.snapshot.pid.is_some(), "kernel pid missing");
    assert!(first.snapshot.cdp_port.is_some(), "cdp port missing");
    assert!(first.proxy.is_none());
    assert!(
        first
            .launch
            .argv
            .iter()
            .all(|arg| !arg.starts_with("--proxy-server"))
    );
    assert!(
        first
            .disclosures
            .iter()
            .any(|disclosure| disclosure.contains("direct mode"))
    );

    let stopped = stop(&supervisor, &format!("rt-{}", stored.id.0));
    assert_eq!(stopped.state, RuntimeState::Stopped);

    drop(_guard);
    let second = runtime.block_on(async {
        start_profile(&stored, &binary, &digest, None, &root, &supervisor).await
    });
    let _guard2 = StopOnDrop {
        supervisor: &supervisor,
        runtime_id: format!("rt-{}", stored.id.0),
    };
    assert_eq!(second.snapshot.state, RuntimeState::Running);
    let stopped = stop(&supervisor, &format!("rt-{}", stored.id.0));
    assert_eq!(stopped.state, RuntimeState::Stopped);
}

#[test]
fn slice_binds_a_proxy_and_releases_it() {
    let Some(binary) = kernel() else {
        return;
    };
    let digest = sha256(&binary);
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let dir = tempfile::tempdir().expect("tempdir");
    let database = dir.path().join("mien.db");
    let root = dir.path().join("data");
    let pool = open_and_migrate(&database).expect("migrate");
    let repository = SqliteProfileRepository::new(pool, &root).expect("repository");
    let proxy = Arc::new(StubProxy::new());

    seed_proxy(repository.pool(), "proxy-1");
    let stored = runtime
        .block_on(repository.create(profile("slice-proxy", Some("proxy-1"))))
        .expect("create");
    assert_eq!(stored.proxy_id.as_deref(), Some("proxy-1"));

    let supervisor = RuntimeSupervisor::new(&root);
    let started = runtime.block_on(async {
        start_profile(
            &stored,
            &binary,
            &digest,
            Some(proxy.as_ref()),
            &root,
            &supervisor,
        )
        .await
    });
    let _guard = StopOnDrop {
        supervisor: &supervisor,
        runtime_id: format!("rt-{}", stored.id.0),
    };
    assert_eq!(started.snapshot.state, RuntimeState::Running);
    assert!(started.snapshot.pid.is_some(), "kernel pid missing");
    assert_eq!(started.snapshot.proxy_port, Some(34567));
    let expected = format!("--proxy-server={}", proxy_server_arg(34567));
    assert!(
        started.launch.argv.contains(&expected),
        "missing proxy arg: {:?}",
        started.launch.argv
    );
    assert_eq!(proxy.attached.lock().expect("lock").len(), 1);

    stop(&supervisor, &format!("rt-{}", stored.id.0));
}

#[test]
fn inactive_profiles_are_refused() {
    let Some(binary) = kernel() else {
        return;
    };
    let digest = sha256(&binary);
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let dir = tempfile::tempdir().expect("tempdir");
    let database = dir.path().join("mien.db");
    let root = dir.path().join("data");
    let pool = open_and_migrate(&database).expect("migrate");
    let repository = SqliteProfileRepository::new(pool, &root).expect("repository");

    let stored = runtime
        .block_on(repository.create(profile("slice-archived", None)))
        .expect("create");
    let archived = runtime
        .block_on(repository.archive(&stored.id))
        .expect("archive");
    assert_eq!(archived.status, ProfileStatus::Archived);

    let manifest = load_pinned_manifest().expect("manifest");
    let supervisor = RuntimeSupervisor::new(&root);
    let request = StartProfile {
        profile: &archived,
        binary: binary.clone(),
        binary_sha256: digest.clone(),
        manifest: &manifest,
        data_root: root.clone(),
        sandbox_kind: "seatbelt".into(),
        proxy: None,
        stop_timeout: Duration::from_secs(2),
    };
    let error = runtime
        .block_on(request.start(&supervisor, "rt-archived"))
        .expect_err("must refuse");
    assert_eq!(error.code(), ErrorCode::ValidationFailed);
}

#[test]
fn slice_attaches_the_real_shared_engine() {
    let Some(binary) = kernel() else {
        return;
    };
    let Some(xray) = std::env::var_os("MIEN_XRAY_BIN").map(PathBuf::from) else {
        println!("skipped: MIEN_XRAY_BIN not set");
        return;
    };
    let digest = sha256(&binary);
    let xray_digest = sha256(&xray);
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let dir = tempfile::tempdir().expect("tempdir");
    let database = dir.path().join("mien.db");
    let root = dir.path().join("data");
    let pool = open_and_migrate(&database).expect("migrate");
    let repository = SqliteProfileRepository::new(pool, &root).expect("repository");

    seed_proxy(repository.pool(), "proxy-1");
    let stored = runtime
        .block_on(repository.create(profile("slice-engine", Some("proxy-1"))))
        .expect("create");

    let (mut engine, controller) = runtime.block_on(async {
        let engine = mien_core::proxy::engine::EngineProcess::start(&xray, &xray_digest)
            .await
            .expect("xray start");
        let controller = Arc::new(mien_core::proxy::engine::EngineController::new(
            engine.api(),
        ));
        controller.set_state(mien_core::proxy::engine::EngineState::Running);
        (engine, controller)
    });

    let config = mien_core::proxy::keys::ProxyConfig {
        protocol: mien_core::proxy::keys::ProxyProtocol::Vless,
        server: "127.0.0.1".into(),
        port: 1,
        transport: mien_core::proxy::keys::Transport::Tcp,
        tls: mien_core::proxy::keys::TlsSettings::default(),
        mux: mien_core::proxy::keys::MuxSettings::default(),
        options: std::collections::BTreeMap::new(),
        credentials: mien_core::proxy::keys::Credentials {
            values: [(
                "id".to_owned(),
                "00000000-0000-4000-8000-000000000001".to_owned(),
            )]
            .into_iter()
            .collect(),
        },
    };
    let spec = mien_core::proxy::keys::build_outbound(&config, b"slice-master").expect("spec");
    let attach = mien_core::proxy::attach::GatewayAttach::new(controller.clone(), spec.clone());

    let supervisor = RuntimeSupervisor::new(&root);
    let manifest = load_pinned_manifest().expect("pinned manifest");
    let runtime_id = format!("rt-{}", stored.id.0);
    let request = StartProfile {
        profile: &stored,
        binary: binary.clone(),
        binary_sha256: digest.clone(),
        manifest: &manifest,
        data_root: root.clone(),
        sandbox_kind: "seatbelt".into(),
        proxy: Some(&attach),
        stop_timeout: Duration::from_secs(5),
    };
    let started = runtime
        .block_on(async { request.start(&supervisor, &runtime_id).await })
        .expect("start");
    let _guard = StopOnDrop {
        supervisor: &supervisor,
        runtime_id: runtime_id.clone(),
    };
    assert_eq!(started.snapshot.state, RuntimeState::Running);
    let port = started.proxy.as_ref().expect("attachment").port;
    assert_eq!(started.snapshot.proxy_port, Some(port));
    let inbounds = engine.api().list_inbounds().expect("inbounds");
    assert!(
        inbounds
            .iter()
            .any(|inbound| inbound.tag == format!("rt:{runtime_id}:in"))
    );
    assert_eq!(
        controller.ref_count(attach.outbound_key()),
        1,
        "shared outbound must be referenced once"
    );

    runtime
        .block_on(async { request.stop(&supervisor, &runtime_id).await })
        .expect("stop");
    assert_eq!(
        controller.ref_count(attach.outbound_key()),
        0,
        "outbound must be released after stop"
    );
    assert!(
        !engine
            .api()
            .list_inbounds()
            .expect("inbounds")
            .iter()
            .any(|inbound| inbound.tag == format!("rt:{runtime_id}:in"))
    );
    runtime
        .block_on(engine.stop(Duration::from_secs(1)))
        .expect("xray stop");
}

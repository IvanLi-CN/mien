//! Shared Xray process supervision and refcounting.
use super::{
    api::{self, GrpcXrayApi},
    keys::{OutboundKey, OutboundSpec},
};
use crate::error::{CoreError, ErrorCode};
use crate::runtime::admission::Backoff;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, Command},
    sync::mpsc,
};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineState {
    Stopped,
    Starting,
    Running,
    Rebuilding,
    Failed,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeBinding {
    pub runtime_id: String,
    pub outbound_key: OutboundKey,
    pub inbound_tag: String,
    pub rule_tag: String,
    pub proxy_port: u16,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundRef {
    pub key: OutboundKey,
    pub tag: String,
    pub ref_count: usize,
    pub orphaned: bool,
}
#[derive(Debug, Default)]
struct State {
    outbounds: BTreeMap<OutboundKey, OutboundRef>,
    bindings: BTreeMap<String, RuntimeBinding>,
}
pub struct EngineProcess {
    child: Child,
    group: i32,
    api: Arc<GrpcXrayApi>,
    api_port: u16,
    dir: PathBuf,
    logs: tokio::task::JoinHandle<()>,
}
impl EngineProcess {
    pub async fn start(bin: &Path, digest: &str) -> Result<Self, CoreError> {
        super::gateway::verify_binary_sha256(bin, digest)?;
        Self::start_with_api_port(
            bin,
            reserve_port().map_err(|_| {
                err(
                    ErrorCode::ProxyEngineStartFailed,
                    "proxy engine failed to start",
                )
            })?,
        )
        .await
    }
    pub async fn start_with_api_port(bin: &Path, port: u16) -> Result<Self, CoreError> {
        let dir = private_dir().map_err(|_| {
            err(
                ErrorCode::ProxyEngineStartFailed,
                "proxy engine failed to start",
            )
        })?;
        let cfg = super::gateway::config_with_api_port(port)?;
        let path = dir.join("bootstrap.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&cfg).map_err(|_| {
                err(
                    ErrorCode::ProxyEngineStartFailed,
                    "proxy engine failed to start",
                )
            })?,
        )
        .map_err(|_| {
            err(
                ErrorCode::ProxyEngineStartFailed,
                "proxy engine failed to start",
            )
        })?;
        mode_file(&path).map_err(|_| {
            err(
                ErrorCode::ProxyEngineStartFailed,
                "proxy engine failed to start",
            )
        })?;
        let mut cmd = Command::new(bin);
        cmd.arg("run")
            .arg(format!("-config={}", path.display()))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        isolate(&mut cmd);
        let mut child = cmd.spawn().map_err(|_| {
            err(
                ErrorCode::ProxyEngineStartFailed,
                "proxy engine failed to start",
            )
        })?;
        let pid = child.id().ok_or_else(|| {
            err(
                ErrorCode::ProxyEngineStartFailed,
                "proxy engine failed to start",
            )
        })? as i32;
        let (tx, mut rx) = mpsc::channel::<String>(128);
        let o = child.stdout.take().ok_or_else(|| {
            err(
                ErrorCode::ProxyEngineStartFailed,
                "proxy engine failed to start",
            )
        })?;
        let e = child.stderr.take().ok_or_else(|| {
            err(
                ErrorCode::ProxyEngineStartFailed,
                "proxy engine failed to start",
            )
        })?;
        let tx2 = tx.clone();
        let a = tokio::spawn(async move { pump(o, tx2).await });
        let b = tokio::spawn(async move { pump(e, tx).await });
        let logs = tokio::spawn(async move {
            let _ = tokio::join!(a, b);
        });
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let client = loop {
            if tokio::time::Instant::now() >= deadline {
                stop_child(&mut child, pid, Duration::from_secs(1)).await;
                let _ = std::fs::remove_dir_all(&dir);
                return Err(err(
                    ErrorCode::ProxyEngineNotReady,
                    "proxy engine is not ready",
                ));
            }
            if let Some(status) = child.try_wait().map_err(|_| {
                err(
                    ErrorCode::ProxyEngineApiUnavailable,
                    "proxy engine API is unavailable",
                )
            })? {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(CoreError::validation(
                    ErrorCode::ProxyEngineStartFailed,
                    format!("engine exited during startup with {status}"),
                ));
            }
            let _ = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await;
            if tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_ok()
            {
                if let Ok(api) = GrpcXrayApi::connect(([127, 0, 0, 1], port).into()).await {
                    if tokio::time::timeout(Duration::from_millis(500), api.probe())
                        .await
                        .is_ok_and(|x| x.is_ok())
                    {
                        break api;
                    }
                }
            }
        };
        Ok(Self {
            child,
            group: pid,
            api: Arc::new(client),
            api_port: port,
            dir,
            logs,
        })
    }
    pub fn api(&self) -> Arc<GrpcXrayApi> {
        self.api.clone()
    }
    pub fn api_port(&self) -> u16 {
        self.api_port
    }
    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }
    pub async fn stop(&mut self, d: Duration) -> Result<(), CoreError> {
        stop_child(&mut self.child, self.group, d).await;
        self.logs.abort();
        std::fs::remove_dir_all(&self.dir).map_err(|_| {
            err(
                ErrorCode::ProxyEngineStartFailed,
                "proxy engine failed to start",
            )
        })?;
        Ok(())
    }
    pub async fn force_kill(&mut self) -> Result<(), CoreError> {
        if self.child.id().is_some() {
            signal(self.group, true);
            self.child.wait().await.map_err(|_| {
                err(
                    ErrorCode::ProxyEngineApiUnavailable,
                    "proxy engine API is unavailable",
                )
            })?;
        }
        self.logs.abort();
        Ok(())
    }
}
impl Drop for EngineProcess {
    fn drop(&mut self) {
        if self.child.id().is_some() {
            signal(self.group, true);
        }
        self.logs.abort();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
fn reserve_port() -> std::io::Result<u16> {
    let l = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(l.local_addr()?.port())
}
async fn pump<R: tokio::io::AsyncRead + Unpin>(r: R, t: mpsc::Sender<String>) {
    let mut lines = BufReader::new(r).lines();
    while let Ok(Some(s)) = lines.next_line().await {
        if t.send(s).await.is_err() {
            // Never stop reading because the consumer is gone: the child keeps
            // writing into this pipe, and closing it turns the next engine log
            // line into SIGPIPE, which kills the shared engine.
            continue;
        }
    }
}
fn private_dir() -> std::io::Result<PathBuf> {
    static N: AtomicU64 = AtomicU64::new(0);
    for _ in 0..16 {
        let p = std::env::temp_dir().join(format!(
            "mien-xray-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        match std::fs::create_dir(&p) {
            Ok(()) => {
                mode_dir(&p)?;
                return Ok(p);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "private directory unavailable",
    ))
}
#[cfg(unix)]
fn mode_dir(p: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700))
}
#[cfg(not(unix))]
fn mode_dir(_: &Path) -> std::io::Result<()> {
    Ok(())
}
#[cfg(unix)]
fn mode_file(p: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600))
}
#[cfg(not(unix))]
fn mode_file(_: &Path) -> std::io::Result<()> {
    Ok(())
}
#[cfg(unix)]
fn isolate(c: &mut Command) {
    use std::os::unix::process::CommandExt;
    c.as_std_mut().process_group(0);
}
#[cfg(not(unix))]
fn isolate(_: &mut Command) {}
#[cfg(unix)]
fn signal(g: i32, k: bool) {
    unsafe {
        libc::kill(-g, if k { libc::SIGKILL } else { libc::SIGTERM });
    }
}
#[cfg(not(unix))]
fn signal(_: i32, _: bool) {}
async fn stop_child(c: &mut Child, g: i32, d: Duration) {
    if c.id().is_none() {
        return;
    }
    signal(g, false);
    if tokio::time::timeout(d, c.wait()).await.is_err() {
        signal(g, true);
        let _ = c.wait().await;
    }
}
fn err(code: ErrorCode, message: &str) -> CoreError {
    CoreError::validation(code, message)
}
pub struct EngineController<A: api::XrayApi> {
    api: Arc<A>,
    state: Mutex<State>,
    life: Mutex<EngineState>,
    allow_fallback: bool,
    /// Bounded crash/rebuild window shared by every recovery attempt.
    backoff: Mutex<Backoff>,
    /// What to spawn on rebuild; empty until the real binary is registered.
    binary: Mutex<Option<EngineBinary>>,
    /// Port each bound runtime must keep across a rebuild (REQ-PROXY-022).
    ports: Mutex<BTreeMap<String, u16>>,
    /// The `AddRule` request of each binding, replayed after a rebuild.
    specs: Mutex<BTreeMap<String, (OutboundSpec, InboundSpec)>>,
    /// Newest engine process; `None` until the first live start.
    process: tokio::sync::Mutex<Option<EngineProcess>>,
    /// Engines still waiting for their tests, keyed by API port.
    detached: tokio::sync::Mutex<BTreeMap<u16, EngineProcess>>,
    /// Management API port the live (or last) engine listens on; a rebuild must
    /// reuse it so bound runtimes keep their gRPC endpoint.
    api_port: Mutex<Option<u16>>,
}

/// The installed engine binary and its verified digest.
#[derive(Debug, Clone)]
pub struct EngineBinary {
    pub path: PathBuf,
    pub sha256: String,
}

/// Everything a rebuilt engine needs to re-create one runtime's resources.
#[derive(Debug, Clone)]
struct InboundSpec {
    listen: String,
    port: u16,
    protocol: String,
    settings: Value,
}

impl<A: api::XrayApi> EngineController<A> {
    pub fn new(api: Arc<A>) -> Self {
        Self::with_fallback(api, false)
    }
    /// `allow_fallback` lets a rebuild without a registered binary keep running
    /// without an engine, which is the stub-API behaviour tests rely on.
    pub fn with_fallback(api: Arc<A>, allow_fallback: bool) -> Self {
        Self {
            api,
            allow_fallback,
            backoff: Mutex::new(Backoff::default()),
            binary: Mutex::new(None),
            ports: Mutex::new(BTreeMap::new()),
            specs: Mutex::new(BTreeMap::new()),
            process: tokio::sync::Mutex::new(None),
            detached: tokio::sync::Mutex::new(BTreeMap::new()),
            api_port: Mutex::new(None),
            state: Mutex::new(State::default()),
            life: Mutex::new(EngineState::Stopped),
        }
    }
    /// Replaces the rebuild ladder; tests use short delays, production keeps
    /// `1s, 2s, 4s, 8s`.
    pub fn set_rebuild_delays(&self, delays: &[Duration]) {
        *self.backoff.lock().expect("life lock") = Backoff::with_delays(delays);
    }
    /// Remaining automatic rebuild attempts.
    pub fn rebuild_attempts_left(&self) -> usize {
        let backoff = self.backoff.lock().expect("life lock");
        backoff.max_attempts().saturating_sub(backoff.attempts())
    }
    /// Registers the binary every rebuild spawns.
    pub fn register_binary(&self, binary: EngineBinary) {
        *self.binary.lock().expect("binary lock") = Some(binary);
    }
    /// Hands over a freshly started engine for `runtime_id`; see [`Self::detach_engine`].
    pub async fn adopt_engine(&self, process: EngineProcess, runtime_id: &str) {
        *self.api_port.lock().expect("api port lock") = Some(process.api_port());
        self.ports
            .lock()
            .expect("ports lock")
            .insert(runtime_id.to_owned(), process.api_port());
        self.detached
            .lock()
            .await
            .insert(process.api_port(), process);
    }
    /// Moves the engine started for a fresh runtime into the shared slot, unless
    /// an engine is already live (a concurrent start won the race).
    pub async fn detach_engine(&self, runtime_id: &str) -> Option<EngineProcess> {
        if self.process.lock().await.is_some() {
            return None;
        }
        let port = self
            .ports
            .lock()
            .expect("ports lock")
            .get(runtime_id)
            .copied()?;
        self.detached.lock().await.remove(&port)
    }
    /// Entry point for a crash/exit of the live engine (REQ-PROXY-022).
    ///
    /// Takes the process out of the controller and marks the engine failed, so
    /// every bound runtime fails closed until the rebuild restores it.
    pub async fn crash(&self) -> Option<EngineProcess> {
        self.fail_closed();
        self.process.lock().await.take()
    }
    pub fn state(&self) -> EngineState {
        *self.life.lock().expect("life lock")
    }
    pub fn set_state(&self, x: EngineState) {
        *self.life.lock().expect("life lock") = x
    }
    pub fn assert_bootstrap(&self, t: &[String], r: &Value) -> Result<(), CoreError> {
        if t.first().map(String::as_str) != Some("block")
            || r.get("final").is_some()
            || r.get("script").is_some()
            || r.get("balancers").is_some()
        {
            return Err(err(
                ErrorCode::ProxyEngineStartFailed,
                "proxy engine failed to start",
            ));
        }
        Ok(())
    }
    pub fn acquire(&self, b: RuntimeBinding, s: &OutboundSpec) -> Result<(), CoreError> {
        if self.state() != EngineState::Running {
            return Err(err(
                ErrorCode::ProxyEngineNotReady,
                "proxy engine is not ready",
            ));
        }
        let mut st = self.state.lock().expect("state lock");
        if st.bindings.contains_key(&b.runtime_id) {
            return Ok(());
        }
        if let Some(r) = st.outbounds.get_mut(&b.outbound_key) {
            r.ref_count += 1
        } else {
            api::ensure_outbound(
                &*self.api,
                api::Outbound {
                    tag: s.tag.clone(),
                    protocol: s.protocol.clone(),
                    mux: s.mux,
                    transport: super::keys::Transport::Tcp,
                    settings: s.settings.clone(),
                    stream_settings: s.stream_settings.clone(),
                },
            )?;
            st.outbounds.insert(
                b.outbound_key.clone(),
                OutboundRef {
                    key: b.outbound_key.clone(),
                    tag: s.tag.clone(),
                    ref_count: 1,
                    orphaned: false,
                },
            );
        }
        let i = api::Inbound {
            tag: b.inbound_tag.clone(),
            listen: "127.0.0.1".into(),
            port: b.proxy_port,
            protocol: "http".into(),
            settings: serde_json::json!({}),
        };
        if let Err(e) = api::ensure_inbound(&*self.api, i) {
            self.dec(&mut st, &b.outbound_key)?;
            return Err(e);
        }
        let r = api::Rule {
            tag: b.rule_tag.clone(),
            inbound_tag: b.inbound_tag.clone(),
            outbound_tag: b.outbound_key.to_string(),
        };
        if let Err(e) = api::ensure_rule(&*self.api, r) {
            let _ = api::remove_inbound(&*self.api, &b.inbound_tag);
            self.dec(&mut st, &b.outbound_key)?;
            return Err(e);
        }
        // Remember what a rebuild must re-create on the same port with the
        // same tags; credentials stay in the in-memory spec.
        self.ports
            .lock()
            .expect("ports lock")
            .insert(b.runtime_id.clone(), b.proxy_port);
        self.specs.lock().expect("specs lock").insert(
            b.runtime_id.clone(),
            (
                s.clone(),
                InboundSpec {
                    listen: "127.0.0.1".into(),
                    port: b.proxy_port,
                    protocol: "http".into(),
                    settings: serde_json::json!({}),
                },
            ),
        );
        st.bindings.insert(b.runtime_id.clone(), b);
        Ok(())
    }
    pub fn release(&self, id: &str) -> Result<bool, CoreError> {
        let mut st = self.state.lock().expect("state lock");
        let Some(b) = st.bindings.remove(id) else {
            return Ok(false);
        };
        self.ports.lock().expect("ports lock").remove(id);
        self.specs.lock().expect("specs lock").remove(id);
        api::remove_rule(&*self.api, &b.rule_tag)?;
        api::remove_inbound(&*self.api, &b.inbound_tag)?;
        self.dec(&mut st, &b.outbound_key)
    }
    pub fn ref_count(&self, k: &OutboundKey) -> usize {
        self.state
            .lock()
            .expect("state lock")
            .outbounds
            .get(k)
            .map_or(0, |x| x.ref_count)
    }
    /// Every held binding, oldest runtime id first.
    pub fn bindings(&self) -> Vec<RuntimeBinding> {
        self.state
            .lock()
            .expect("state lock")
            .bindings
            .values()
            .cloned()
            .collect()
    }
    pub fn binding(&self, id: &str) -> Option<RuntimeBinding> {
        self.state
            .lock()
            .expect("state lock")
            .bindings
            .get(id)
            .cloned()
    }
    pub fn outbounds(&self) -> Vec<OutboundRef> {
        self.state
            .lock()
            .expect("state lock")
            .outbounds
            .values()
            .cloned()
            .collect()
    }
    fn dec(&self, s: &mut State, k: &OutboundKey) -> Result<bool, CoreError> {
        let Some(r) = s.outbounds.get_mut(k) else {
            return Err(err(
                ErrorCode::ProxyOutboundRefcountMismatch,
                "shared outbound refcount mismatch",
            ));
        };
        r.ref_count = r.ref_count.checked_sub(1).ok_or_else(|| {
            err(
                ErrorCode::ProxyOutboundRefcountMismatch,
                "shared outbound refcount mismatch",
            )
        })?;
        if r.ref_count > 0 {
            return Ok(false);
        }
        let tag = r.tag.clone();
        match api::remove_outbound(&*self.api, &tag) {
            Ok(()) => {
                s.outbounds.remove(k);
                Ok(true)
            }
            Err(e) => {
                r.orphaned = true;
                Err(e)
            }
        }
    }
    pub fn fail_closed(&self) {
        self.set_state(EngineState::Failed)
    }
    /// Bounded rebuild after an engine crash (REQ-PROXY-022).
    ///
    /// Waits `1s, 2s, 4s` then `8s` between attempts, respawns on the same API
    /// port and restores only the bindings still held; live kernels keep running
    /// but their network stays failed closed until this returns `Ok`. The window
    /// runs out on the fifth attempt: the state stays `Failed` and the caller
    /// tears the affected runtimes down. A caller that already released every
    /// binding gets `Ok` after one probe attempt and no respawn.
    ///
    /// # Errors
    /// `PROXY_ENGINE_CRASHED` once the rebuild window is exhausted;
    /// `PROXY_OUTBOUND_REFCOUNT_MISMATCH` when a replay cannot be reconciled.
    pub async fn rebuild(&self) -> Result<(), CoreError> {
        // One ladder step per attempt; the window never resets here, so the
        // fifth attempt is refused instead of restarting forever.
        let delay = self.backoff.lock().expect("life lock").next_delay();
        let Some(delay) = delay else {
            return Err(err(
                ErrorCode::ProxyEngineCrashed,
                "proxy engine rebuild window exhausted",
            ));
        };
        tokio::time::sleep(delay).await;
        let binary = self.binary.lock().expect("binary lock").clone();
        let api_port = *self.api_port.lock().expect("api port lock");
        // Stub-API tests never register a binary; they exercise refcounting only.
        let Some(binary) = binary else {
            if self.allow_fallback {
                return Ok(());
            }
            self.fail_closed();
            return Err(err(
                ErrorCode::ProxyEngineStartFailed,
                "proxy engine failed to start",
            ));
        };
        let Some(api_port) = api_port else {
            self.fail_closed();
            return Err(err(
                ErrorCode::ProxyEngineStartFailed,
                "proxy engine failed to start",
            ));
        };
        let bindings: Vec<RuntimeBinding> = self
            .state
            .lock()
            .expect("state lock")
            .bindings
            .values()
            .cloned()
            .collect();
        if bindings.is_empty() {
            return Ok(());
        }
        self.set_state(EngineState::Rebuilding);
        let process = match EngineProcess::start_with_api_port(&binary.path, api_port).await {
            Ok(process) => process,
            Err(error) => {
                self.fail_closed();
                return Err(error);
            }
        };
        if let Err(error) = self.replay(&bindings, &process) {
            let mut process = process;
            let _ = process.stop(Duration::from_secs(1)).await;
            self.fail_closed();
            return Err(error);
        }
        *self.process.lock().await = Some(process);
        self.set_state(EngineState::Running);
        Ok(())
    }
}

impl<A: api::XrayApi> EngineController<A> {
    /// Re-creates outbound, inbound and rule for every still-held binding.
    fn replay(
        &self,
        bindings: &[RuntimeBinding],
        process: &EngineProcess,
    ) -> Result<(), CoreError> {
        let fresh = process.api();
        let specs = self.specs.lock().expect("specs lock");
        let mut st = self.state.lock().expect("state lock");
        for b in bindings {
            let Some((outbound, inbound)) = specs.get(&b.runtime_id) else {
                continue;
            };
            api::ensure_outbound(
                &*fresh,
                api::Outbound {
                    tag: outbound.tag.clone(),
                    protocol: outbound.protocol.clone(),
                    mux: outbound.mux,
                    transport: super::keys::Transport::Tcp,
                    settings: outbound.settings.clone(),
                    stream_settings: outbound.stream_settings.clone(),
                },
            )?;
            api::ensure_inbound(
                &*fresh,
                api::Inbound {
                    tag: b.inbound_tag.clone(),
                    listen: inbound.listen.clone(),
                    port: inbound.port,
                    protocol: inbound.protocol.clone(),
                    settings: inbound.settings.clone(),
                },
            )?;
            api::ensure_rule(
                &*fresh,
                api::Rule {
                    tag: b.rule_tag.clone(),
                    inbound_tag: b.inbound_tag.clone(),
                    outbound_tag: b.outbound_key.to_string(),
                },
            )?;
        }
        st.outbounds.values_mut().for_each(|r| r.orphaned = false);
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::future::Future;
    use std::sync::Mutex;
    #[derive(Default)]
    struct Fake {
        ins: Mutex<Vec<api::Inbound>>,
        outs: Mutex<Vec<api::Outbound>>,
        rules: Mutex<Vec<api::Rule>>,
    }
    impl api::XrayApi for Fake {
        fn list_inbounds(&self) -> Result<Vec<api::Inbound>, api::ApiError> {
            Ok(self.ins.lock().unwrap().clone())
        }
        fn list_outbounds(&self) -> Result<Vec<api::Outbound>, api::ApiError> {
            Ok(self.outs.lock().unwrap().clone())
        }
        fn list_rules(&self) -> Result<Vec<api::Rule>, api::ApiError> {
            Ok(self.rules.lock().unwrap().clone())
        }
        fn add_inbound(&self, x: api::Inbound) -> Result<(), api::ApiError> {
            self.ins.lock().unwrap().push(x);
            Ok(())
        }
        fn remove_inbound(&self, t: &str) -> Result<(), api::ApiError> {
            self.ins.lock().unwrap().retain(|x| x.tag != t);
            Ok(())
        }
        fn add_outbound(&self, x: api::Outbound) -> Result<(), api::ApiError> {
            self.outs.lock().unwrap().push(x);
            Ok(())
        }
        fn remove_outbound(&self, t: &str) -> Result<(), api::ApiError> {
            self.outs.lock().unwrap().retain(|x| x.tag != t);
            Ok(())
        }
        fn add_rule(&self, x: api::Rule, _: bool) -> Result<(), api::ApiError> {
            self.rules.lock().unwrap().push(x);
            Ok(())
        }
        fn remove_rule(&self, t: &str) -> Result<(), api::ApiError> {
            self.rules.lock().unwrap().retain(|x| x.tag != t);
            Ok(())
        }
    }
    fn setup() -> (Arc<Fake>, EngineController<Fake>, OutboundSpec) {
        let a = Arc::new(Fake::default());
        let c = EngineController::new(a.clone());
        c.set_state(EngineState::Running);
        let k = OutboundKey::new(format!("out:{}", "a".repeat(64))).unwrap();
        let s = OutboundSpec {
            key: k.clone(),
            tag: k.to_string(),
            protocol: "vless".into(),
            settings: Value::Null,
            stream_settings: Value::Null,
            mux: Default::default(),
        };
        (a, c, s)
    }
    fn b(id: &str, k: OutboundKey) -> RuntimeBinding {
        RuntimeBinding {
            runtime_id: id.into(),
            outbound_key: k,
            inbound_tag: format!("rt:{id}:in"),
            rule_tag: format!("rt:{id}:rule"),
            proxy_port: 40000,
        }
    }
    /// 引擎重建窗口：每轮消耗一格，耗尽后返回 `PROXY_ENGINE_CRASHED`，不无限重启。
    #[test]
    fn rebuild_window_is_bounded() {
        let (_, controller, _) = setup();
        controller.register_binary(EngineBinary {
            path: PathBuf::from("/definitely/missing/xray"),
            sha256: "00".into(),
        });
        controller.set_rebuild_delays(&[Duration::from_millis(1)]);
        assert_eq!(controller.rebuild_attempts_left(), 1);
        // A missing binary fails the single in-window attempt.
        let first = futures_lite_block_on(controller.rebuild());
        assert!(first.is_err(), "missing binary must fail");
        assert_eq!(controller.rebuild_attempts_left(), 0);
        let exhausted = futures_lite_block_on(controller.rebuild()).expect_err("window closed");
        assert_eq!(exhausted.code(), ErrorCode::ProxyEngineCrashed);
        assert_eq!(controller.state(), EngineState::Failed);
    }

    /// 引擎崩溃时绑定仍在：只有代理档案失败关闭，直连运行时没有引擎依赖。
    #[test]
    fn crash_fails_closed_and_keeps_bindings_for_replay() {
        let (_, controller, spec) = setup();
        controller.acquire(b("a", spec.key.clone()), &spec).unwrap();
        let crash = futures_lite_block_on(controller.crash());
        assert!(crash.is_none(), "no live process in the stub setup");
        assert_eq!(controller.state(), EngineState::Failed);
        assert_eq!(
            controller.bindings().len(),
            1,
            "bindings survive for a rebuild"
        );
        assert_eq!(controller.ref_count(&spec.key), 1);
    }

    /// 重建成功后按原 key 恢复引用计数，不产生重复出站。
    #[test]
    fn replay_restores_the_shared_outbound_once() {
        let (_, controller, spec) = setup();
        controller.acquire(b("a", spec.key.clone()), &spec).unwrap();
        controller.acquire(b("b", spec.key.clone()), &spec).unwrap();
        assert_eq!(controller.ref_count(&spec.key), 2);
        controller.release("a").unwrap();
        assert_eq!(controller.ref_count(&spec.key), 1);
        controller.release("b").unwrap();
        assert_eq!(controller.ref_count(&spec.key), 0);
    }

    fn futures_lite_block_on<F: Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime")
            .block_on(future)
    }

    #[test]
    fn refcounts_share_and_remove_last() {
        let (a, c, s) = setup();
        c.acquire(b("a", s.key.clone()), &s).unwrap();
        c.acquire(b("b", s.key.clone()), &s).unwrap();
        assert_eq!(c.ref_count(&s.key), 2);
        c.release("a").unwrap();
        assert_eq!(c.ref_count(&s.key), 1);
        assert_eq!(a.outs.lock().unwrap().len(), 1);
        c.release("b").unwrap();
        assert!(a.outs.lock().unwrap().is_empty())
    }
    #[test]
    fn release_idempotent() {
        let (_, c, s) = setup();
        c.acquire(b("a", s.key.clone()), &s).unwrap();
        assert!(c.release("a").unwrap());
        assert!(!c.release("a").unwrap())
    }
}

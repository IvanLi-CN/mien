//! macOS 有头内核监督：状态机、独立进程组、事件、日志与对账。

use std::{
    collections::HashMap,
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, BufReader},
    process::{Child, Command},
};

use crate::{
    domain::profile::{ProfileId, ProfileMode},
    error::{CoreError, ErrorCode},
    fingerprint::argv::{HEADLESS_ARG, ModeArgs},
};

use super::{
    admission::{AdmissionPermit, Backoff, DEFAULT_RESTART_DELAYS, RuntimeAdmission},
    logs::RuntimeLogs,
    probe::{
        self, process_alive, process_belongs_to, process_group_alive, process_group_has_live,
        signal_process_group,
    },
    state::{self, Isolation, RuntimeState},
};

const DEFAULT_STOP_TIMEOUT: Duration = Duration::from_secs(5);

/// Chromium flags every launch carries so host dialogs stay out of the way.
///
/// `--use-mock-keychain` is the only switch that actually stops the macOS
/// SecurityAgent prompt on this kernel: the fingerprint-chromium build is ad-hoc
/// signed with no Team ID, so any real `Chromium Safe Storage` access raises the
/// dialog and blocks startup. `--password-store=basic` alone is a no-op on macOS
/// (verified: removing the mock keychain stalled CDP until the prompt was
/// answered). The remaining two flags only drop the first-run and
/// default-browser dialogs; Mien owns the profile in the data directory, so
/// persisting the browser choice in the host store buys nothing.
pub const LAUNCHER_FLAGS: &[&str] = &[
    "--use-mock-keychain",
    "--password-store=basic",
    "--no-first-run",
    "--no-default-browser-check",
];

/// Host values the kernel may inherit, as fixed by `docs/SECURITY.md` §5.3.
///
/// The kernel never inherits the server environment: it receives only these
/// names when the server has them, a fixed `PATH`, and its runtime id. Windows
/// are addressed through `DISPLAY` plus the Xvfb auth cookie in `XAUTHORITY`;
/// dropping either leaves a container headful launch without an X server.
pub const CHILD_ENV_WHITELIST: &[&str] = &[
    "HOME",
    "XDG_CONFIG_HOME",
    "XDG_CACHE_HOME",
    "TMPDIR",
    "DISPLAY",
    "XAUTHORITY",
    "LANG",
    "LC_ALL",
    "TZ",
];

/// Fixed child `PATH`; the supervisor's own `PATH` is never inherited.
pub const CHILD_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

/// Runtime id passed to the kernel; also embedded in argv for reconciliation.
pub const CHILD_RUNTIME_ID_ENV: &str = "MIEN_RUNTIME_ID";

/// Resolve the child environment from a lookup over the server environment.
///
/// Pure so the whitelist can be asserted without spawning a process.
fn child_environment(
    runtime_id: &str,
    lookup: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> Vec<(String, std::ffi::OsString)> {
    let mut environment: Vec<(String, std::ffi::OsString)> = CHILD_ENV_WHITELIST
        .iter()
        .filter_map(|name| lookup(name).map(|value| ((*name).to_owned(), value)))
        .collect();
    environment.push(("PATH".to_owned(), CHILD_PATH.into()));
    environment.push((CHILD_RUNTIME_ID_ENV.to_owned(), runtime_id.into()));
    environment
}
const START_GRACE: Duration = Duration::from_millis(100);
const MONITOR_INTERVAL: Duration = Duration::from_millis(50);
/// A run this long clears the crash ladder; anything shorter keeps counting.
const STABLE_RUN: Duration = Duration::from_secs(30);

/// Whether the manifest itself says the artifact cannot sandbox.
///
/// Only `none` qualifies. A real kernel manifest never declares it (linux is
/// `namespace`, macOS `seatbelt`, Windows `win32-restricted-token`), so a kernel
/// that promises a mechanism still gets that mechanism verified; an empty or
/// unknown kind is not a licence either.
fn declares_no_sandbox(sandbox_kind: &str) -> bool {
    sandbox_kind == "none"
}

fn spawn_failed(message: &str) -> CoreError {
    CoreError::validation(ErrorCode::RuntimeSpawnFailed, message)
}

/// 崩溃重启策略。默认关闭（SPEC REQ-RUNTIME-010：默认不自动重启）；启用时严格按
/// `1s,2s,4s,8s` 四次上限，耗尽后进入 `failed`，不会无限重启。
#[derive(Debug, Clone)]
pub struct RestartPolicy {
    pub delays: Vec<Duration>,
    /// 稳定运行多久后清零阶梯。
    pub stable_after: Duration,
}

impl RestartPolicy {
    /// 不自动重启；崩溃直接写 `crashed`。
    #[must_use]
    pub fn none() -> Self {
        Self {
            delays: Vec::new(),
            stable_after: STABLE_RUN,
        }
    }

    /// 文档化的有界阶梯：`1s, 2s, 4s, 8s`，最多四次。
    #[must_use]
    pub fn bounded() -> Self {
        Self {
            delays: DEFAULT_RESTART_DELAYS.to_vec(),
            stable_after: STABLE_RUN,
        }
    }

    #[must_use]
    pub fn with_delays(delays: &[Duration], stable_after: Duration) -> Self {
        Self {
            delays: delays.to_vec(),
            stable_after,
        }
    }

    #[must_use]
    pub fn is_enabled(&self) -> bool {
        !self.delays.is_empty()
    }
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self::none()
    }
}

/// 启动参数。`launch_args` 只作为参数数组传给内核，不经过 shell。
#[derive(Clone)]
pub struct RuntimeOptions {
    pub runtime_id: String,
    pub profile_id: ProfileId,
    pub binary: PathBuf,
    pub binary_sha256: String,
    pub manifest_sandbox_kind: String,
    pub mode: ProfileMode,
    /// Kernel manifest `headless_chrome_ua_patch`; the only evidence `pseudo_headful` accepts.
    pub headless_chrome_ua_patch: bool,
    pub automation_api: bool,
    pub launch_args: Vec<String>,
    pub data_dir: PathBuf,
    pub direct: bool,
    pub proxy_ready: bool,
    pub proxy_port: Option<u16>,
    pub stop_timeout: Duration,
    /// 崩溃后的重启策略；默认关闭，显式启用才重启。
    pub restart: RestartPolicy,
}

impl std::fmt::Debug for RuntimeOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeOptions")
            .field("runtime_id", &self.runtime_id)
            .field("profile_id", &self.profile_id)
            .field("mode", &self.mode)
            .field("direct", &self.direct)
            .finish_non_exhaustive()
    }
}

impl RuntimeOptions {
    pub fn new(
        runtime_id: impl Into<String>,
        profile_id: ProfileId,
        binary: impl Into<PathBuf>,
        binary_sha256: impl Into<String>,
        data_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            runtime_id: runtime_id.into(),
            profile_id,
            binary: binary.into(),
            binary_sha256: binary_sha256.into(),
            manifest_sandbox_kind: "seatbelt".into(),
            mode: ProfileMode::Headful,
            headless_chrome_ua_patch: false,
            automation_api: false,
            launch_args: Vec::new(),
            data_dir: data_dir.into(),
            direct: false,
            proxy_ready: true,
            proxy_port: None,
            stop_timeout: DEFAULT_STOP_TIMEOUT,
            restart: RestartPolicy::none(),
        }
    }
}

/// 与 events.schema.json 的 runtime payload 对齐；文本字段只放脱敏诊断。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeEventPayload {
    pub runtime_id: String,
    pub profile_id: String,
    pub state: RuntimeState,
    pub mode: ProfileMode,
    pub isolation: Isolation,
    pub direct: bool,
    pub reduced_assurance: bool,
    pub ua_scope: Option<String>,
    pub proxy_port: Option<u16>,
    pub exit_code: Option<i32>,
    pub error_code: Option<String>,
    pub detail: Option<String>,
}

/// 运行时事件信封。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeEvent {
    pub schema_version: String,
    pub id: String,
    pub sequence: u64,
    #[serde(rename = "type")]
    pub kind: String,
    pub ts: String,
    pub request_id: Option<String>,
    pub payload: RuntimeEventPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeSnapshot {
    pub runtime_id: String,
    pub profile_id: String,
    pub state: RuntimeState,
    pub mode: ProfileMode,
    pub isolation: Isolation,
    pub direct: bool,
    pub reduced_assurance: bool,
    /// UA scope this launch may claim; `Some("chrome")` only for `pseudo_headful`.
    pub ua_scope: Option<String>,
    pub pid: Option<u32>,
    pub pgid: Option<i32>,
    pub cdp_port: Option<u16>,
    pub proxy_port: Option<u16>,
    pub exit_code: Option<i32>,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RuntimeMetadata {
    runtime_id: String,
    profile_id: String,
    binary: PathBuf,
    data_dir: PathBuf,
    mode: ProfileMode,
    isolation: Isolation,
    direct: bool,
    #[serde(default)]
    ua_scope: Option<String>,
    proxy_port: Option<u16>,
    pid: u32,
    pgid: i32,
    cdp_port: Option<u16>,
    state: RuntimeState,
}

pub(crate) struct RuntimeRecord {
    options: RuntimeOptions,
    logs: Arc<RuntimeLogs>,
    state: Mutex<RecordState>,
    child: Mutex<Option<Child>>,
    stop_requested: Mutex<bool>,
    /// Bounded crash ladder; empty means no auto restart.
    backoff: Mutex<Backoff>,
    /// Concurrency slot, held from admission until a terminal state.
    permit: Mutex<Option<AdmissionPermit>>,
    /// Mode arguments are replayed on every bounded restart.
    mode_args: ModeArgs,
    /// Start of the current process; a run past `stable_after` clears the ladder.
    started_at: Mutex<Option<Instant>>,
    events: Arc<Mutex<Vec<RuntimeEvent>>>,
    /// 关闭闸门；与监督器共享，落下后不再拉起内核。
    shutting_down: Arc<AtomicBool>,
}

#[derive(Debug, Clone)]
struct RecordState {
    state: RuntimeState,
    isolation: Isolation,
    pid: Option<u32>,
    pgid: Option<i32>,
    cdp_port: Option<u16>,
    exit_code: Option<i32>,
    error_code: Option<String>,
}

impl RuntimeRecord {
    fn new(
        options: RuntimeOptions,
        events: Arc<Mutex<Vec<RuntimeEvent>>>,
        mode_args: ModeArgs,
        shutting_down: Arc<AtomicBool>,
    ) -> Result<Self, CoreError> {
        let logs = Arc::new(
            RuntimeLogs::new(options.data_dir.join("logs"))
                .map_err(|_| spawn_failed("kernel process could not be created"))?,
        );
        let backoff = Backoff::with_delays(&options.restart.delays);
        Ok(Self {
            options,
            logs,
            state: Mutex::new(RecordState {
                state: RuntimeState::Stopped,
                isolation: Isolation::Unknown,
                pid: None,
                pgid: None,
                cdp_port: None,
                exit_code: None,
                error_code: None,
            }),
            child: Mutex::new(None),
            stop_requested: Mutex::new(false),
            backoff: Mutex::new(backoff),
            permit: Mutex::new(None),
            mode_args,
            started_at: Mutex::new(None),
            events,
            shutting_down,
        })
    }

    fn transition(
        &self,
        target: RuntimeState,
        error_code: Option<String>,
        detail: Option<String>,
    ) -> Result<(), CoreError> {
        let mut state = self.state.lock().expect("runtime state lock");
        // Re-entering the same state is a no-op: `stop` and the crash monitor
        // both observe terminal transitions, and the state machine must emit
        // each lifecycle event exactly once.
        if state.state == target {
            return Ok(());
        }
        if !state::valid_transition(state.state, target) {
            return Err(CoreError::InvalidTransition);
        }
        state.state = target;
        if error_code.is_some() {
            state.error_code = error_code.clone();
        }
        drop(state);
        self.emit(target, error_code, detail);
        self.persist_metadata();
        Ok(())
    }

    /// [`Self::transition`] for callers outside the supervisor module.
    pub(crate) fn transition_check(
        &self,
        target: RuntimeState,
        error_code: Option<String>,
        detail: Option<String>,
    ) -> Result<(), CoreError> {
        self.transition(target, error_code, detail)
    }

    fn emit(&self, state: RuntimeState, error_code: Option<String>, detail: Option<String>) {
        let snapshot = self.snapshot();
        let mut events = self.events.lock().expect("event lock");
        let sequence = events.len() as u64 + 1;
        events.push(RuntimeEvent {
            schema_version: "1".into(),
            id: format!("{}-{sequence}", self.options.runtime_id),
            sequence,
            kind: state.event_name().into(),
            ts: crate::storage::now_utc(),
            request_id: None,
            payload: RuntimeEventPayload {
                runtime_id: snapshot.runtime_id,
                profile_id: snapshot.profile_id,
                state,
                mode: snapshot.mode,
                isolation: snapshot.isolation,
                direct: snapshot.direct,
                reduced_assurance: snapshot.reduced_assurance,
                ua_scope: snapshot.ua_scope,
                proxy_port: snapshot.proxy_port,
                exit_code: snapshot.exit_code,
                error_code: error_code.or(snapshot.error_code),
                detail: detail.map(|value| super::logs::redact(&value)),
            },
        });
    }

    pub(crate) fn snapshot(&self) -> RuntimeSnapshot {
        let state = self.state.lock().expect("runtime state lock").clone();
        RuntimeSnapshot {
            runtime_id: self.options.runtime_id.clone(),
            profile_id: self.options.profile_id.0.clone(),
            state: state.state,
            mode: self.options.mode,
            isolation: state.isolation,
            direct: self.options.direct,
            reduced_assurance: state::reduced_assurance(
                self.options.mode,
                self.options.direct,
                state.isolation,
            ),
            ua_scope: (self.options.mode == ProfileMode::PseudoHeadful)
                .then(|| "chrome".to_owned()),
            pid: state.pid,
            pgid: state.pgid,
            cdp_port: state.cdp_port,
            proxy_port: self.options.proxy_port,
            exit_code: state.exit_code,
            error_code: state.error_code,
        }
    }

    pub(crate) fn persist_metadata(&self) {
        let snapshot = self.snapshot();
        let (Some(pid), Some(pgid)) = (snapshot.pid, snapshot.pgid) else {
            return;
        };
        let metadata = RuntimeMetadata {
            runtime_id: snapshot.runtime_id,
            profile_id: snapshot.profile_id,
            binary: self.options.binary.clone(),
            data_dir: self.options.data_dir.clone(),
            mode: snapshot.mode,
            isolation: snapshot.isolation,
            direct: snapshot.direct,
            ua_scope: snapshot.ua_scope,
            proxy_port: snapshot.proxy_port,
            pid,
            pgid,
            cdp_port: snapshot.cdp_port,
            state: snapshot.state,
        };
        if let Ok(bytes) = serde_json::to_vec_pretty(&metadata) {
            let _ = std::fs::write(self.options.data_dir.join("runtime.json"), bytes);
        }
    }

    fn set_process(&self, child: Child, pid: u32) {
        let pgid = pid as i32;
        *self.child.lock().expect("child lock") = Some(child);
        let mut state = self.state.lock().expect("runtime state lock");
        state.pid = Some(pid);
        state.pgid = Some(pgid);
        drop(state);
        self.persist_metadata();
    }

    fn set_isolation(&self, isolation: Isolation) {
        self.state.lock().expect("runtime state lock").isolation = isolation;
    }

    fn set_cdp_port(&self, port: u16) {
        self.state.lock().expect("runtime state lock").cdp_port = Some(port);
        self.persist_metadata();
    }

    fn set_exit(&self, status: std::process::ExitStatus) {
        self.state.lock().expect("runtime state lock").exit_code = status.code();
    }

    pub(crate) fn mark_stop_requested(&self) {
        *self.stop_requested.lock().expect("stop lock") = true;
    }

    fn stop_requested(&self) -> bool {
        *self.stop_requested.lock().expect("stop lock")
    }

    fn shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::SeqCst)
    }

    fn clear_process(&self) {
        self.state.lock().expect("runtime state lock").pid = None;
        self.state.lock().expect("runtime state lock").pgid = None;
        let _ = std::fs::remove_file(self.options.data_dir.join("runtime.json"));
    }

    pub(crate) fn persistable_state(&self) -> RuntimeState {
        self.state.lock().expect("runtime state lock").state
    }

    /// Drops the admission slot; safe on any terminal path.
    pub(crate) fn release_permit(&self) {
        let _ = self.permit.lock().expect("permit lock").take();
    }

    /// Next restart delay, or `None` once the window is exhausted. A run longer
    /// than `stable_after` clears the ladder first (SPEC REQ-RUNTIME-010).
    fn next_restart_delay(&self) -> Option<Duration> {
        if !self.options.restart.is_enabled() {
            return None;
        }
        let stable_after = self.options.restart.stable_after;
        let mut backoff = self.backoff.lock().expect("backoff lock");
        if self
            .started_at
            .lock()
            .expect("started_at lock")
            .is_some_and(|started| started.elapsed() >= stable_after)
        {
            backoff.reset();
        }
        backoff.next_delay()
    }

    fn exhausted_detail(&self) -> String {
        format!(
            "kernel crashed {} times; restart window exhausted",
            self.backoff.lock().expect("backoff lock").max_attempts()
        )
    }

    fn crash_error_code(&self) -> &'static str {
        ErrorCode::RuntimeSpawnFailed.as_str()
    }

    /// Persist an externally supplied state (used by engine-failure teardown).
    pub(crate) fn persist_state(&self, state: RuntimeState) {
        self.state.lock().expect("runtime state lock").state = state;
        self.persist_metadata();
    }

    /// Spawn the kernel once more without touching the state machine.
    async fn respawn_once(&self) -> Result<(), CoreError> {
        *self.stop_requested.lock().expect("stop lock") = false;
        self.start_inner().await
    }

    pub(crate) async fn stop_process(&self, timeout: Duration) -> Result<(), CoreError> {
        let pgid = self.state.lock().expect("runtime state lock").pgid;
        if let Some(pgid) = pgid {
            signal_process_group(pgid, false);
        }
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let exited = {
                let mut child = self.child.lock().expect("child lock");
                match child.as_mut() {
                    Some(child) => match child.try_wait() {
                        Ok(Some(status)) => {
                            self.set_exit(status);
                            true
                        }
                        Ok(None) => false,
                        Err(_) => return Err(spawn_failed("kernel process could not be created")),
                    },
                    None => true,
                }
            };
            if exited && !pgid.is_some_and(process_group_alive) {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                if let Some(pgid) = pgid {
                    signal_process_group(pgid, true);
                }
                let kill_deadline = tokio::time::Instant::now() + Duration::from_secs(2);
                while tokio::time::Instant::now() < kill_deadline {
                    let gone = {
                        let mut child = self.child.lock().expect("child lock");
                        match child.as_mut() {
                            Some(child) => child.try_wait().ok().flatten().is_some(),
                            None => true,
                        }
                    };
                    if gone && !pgid.is_some_and(process_group_alive) {
                        break;
                    }
                    tokio::time::sleep(MONITOR_INTERVAL).await;
                }
                if pgid.is_some_and(process_group_alive) {
                    return Err(spawn_failed("kernel process could not be created"));
                }
                break;
            }
            tokio::time::sleep(MONITOR_INTERVAL).await;
        }
        self.child.lock().expect("child lock").take();
        self.clear_process();
        Ok(())
    }

    async fn start_inner(&self) -> Result<(), CoreError> {
        let options = &self.options;
        if !options.direct && !options.proxy_ready {
            return Err(CoreError::validation(
                ErrorCode::ProxyEngineNotReady,
                "shared proxy engine is not ready",
            ));
        }
        // `sandbox_kind: none` is the manifest's own statement that this artifact
        // cannot sandbox (`docs/contracts/kernel-manifest.schema.json`). Requiring
        // seatbelt or a namespace tree from such a kernel would reject it on every
        // host, so it is reported as `unsandboxed` up front instead. Every other
        // kind stays fail-closed: the platform-specific check below must confirm
        // the mechanism the manifest promises, and an unknown kind is not a licence.
        let declared_unsandboxed = declares_no_sandbox(&options.manifest_sandbox_kind);
        #[cfg(target_os = "linux")]
        let isolation = if declared_unsandboxed {
            Isolation::Unsandboxed
        } else {
            Isolation::Unknown
        };
        #[cfg(not(target_os = "linux"))]
        let isolation = if declared_unsandboxed {
            Isolation::Unsandboxed
        } else {
            probe::probe_macos_sandbox(&options.manifest_sandbox_kind)?
        };
        probe::verify_kernel(&options.binary, &options.binary_sha256)?;
        std::fs::create_dir_all(options.data_dir.join("chrome"))
            .map_err(|_| spawn_failed("kernel process could not be created"))?;
        std::fs::create_dir_all(options.data_dir.join("downloads"))
            .map_err(|_| spawn_failed("kernel process could not be created"))?;
        self.set_isolation(isolation);

        let mut command = Command::new(&options.binary);
        let mut args = options.launch_args.clone();
        args.retain(|arg| {
            !arg.starts_with("--user-data-dir=") && !arg.starts_with("--remote-debugging-port=")
        });
        // The mode owns this one flag; a caller plan may already carry it.
        if self.mode_args.headless && !args.iter().any(|arg| arg == HEADLESS_ARG) {
            args.push(HEADLESS_ARG.to_owned());
        }
        args.push(format!(
            "--user-data-dir={}",
            options.data_dir.join("chrome").display()
        ));
        args.push(format!("--mien-runtime-id={}", options.runtime_id));
        // Fresh profiles get the shared launcher flags; the profile's own argv
        // may already carry one, so never duplicate them.
        for flag in LAUNCHER_FLAGS {
            if !args.iter().any(|arg| arg == flag) {
                args.push((*flag).to_owned());
            }
        }
        if options.automation_api {
            args.push("--remote-debugging-port=0".into());
        }
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // 最后一道防线：记录被异常丢弃时，直接子进程不会变成孤儿。
            // 它只覆盖直接子进程；进程组由 `stop_process` 与关闭遍历负责。
            .kill_on_drop(true)
            .env_clear();
        for (name, value) in child_environment(&options.runtime_id, |key| std::env::var_os(key)) {
            command.env(name, value);
        }
        isolate_process(&mut command);
        let mut child = command
            .spawn()
            .map_err(|_| spawn_failed("kernel process could not be created"))?;
        let pid = child
            .id()
            .ok_or_else(|| spawn_failed("kernel process could not be created"))?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        self.set_process(child, pid);
        if let Some(stdout) = stdout {
            spawn_log_pump(stdout, self.logs.clone(), "stdout");
        }
        if let Some(stderr) = stderr {
            spawn_log_pump(stderr, self.logs.clone(), "stderr");
        }

        if options.automation_api {
            let port =
                probe::wait_for_cdp(&options.data_dir.join("chrome"), Duration::from_secs(10))
                    .await?;
            self.set_cdp_port(port);
        } else {
            tokio::time::sleep(START_GRACE).await;
            if !process_alive(pid) {
                return Err(spawn_failed("kernel process could not be created"));
            }
        }
        #[cfg(target_os = "linux")]
        {
            // The declared-`none` case already skipped the check above; everything
            // else must show a sandboxed tree here or the launch is refused.
            if declared_unsandboxed {
                self.set_isolation(Isolation::Unsandboxed);
            } else {
                match probe::require_linux_isolation(pid).await {
                    Ok(isolation) => self.set_isolation(isolation),
                    Err(error) => {
                        let _ = self.stop_process(Duration::from_secs(2)).await;
                        return Err(error);
                    }
                }
            }
        }
        Ok(())
    }
}

/// 运行时监督器；一个档案最多一个非终态实例。
pub struct RuntimeSupervisor {
    records: Mutex<HashMap<String, Arc<RuntimeRecord>>>,
    events: Arc<Mutex<Vec<RuntimeEvent>>>,
    root: PathBuf,
    /// 并发准入闸门；默认 `max_concurrent_runtimes = 10`。
    admission: Arc<RuntimeAdmission>,
    /// 关闭闸门；与全部记录共享，落下后拒绝新启动。
    shutting_down: Arc<AtomicBool>,
}

impl RuntimeSupervisor {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            records: Mutex::new(HashMap::new()),
            events: Arc::new(Mutex::new(Vec::new())),
            root: root.into(),
            admission: RuntimeAdmission::default_limit(),
            shutting_down: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 使用显式并发上限（测试与受限宿主）。
    #[must_use]
    pub fn with_admission(mut self, admission: Arc<RuntimeAdmission>) -> Self {
        self.admission = admission;
        self
    }

    #[must_use]
    pub fn admission(&self) -> &Arc<RuntimeAdmission> {
        &self.admission
    }

    pub fn events(&self) -> Vec<RuntimeEvent> {
        self.events.lock().expect("event lock").clone()
    }

    pub(crate) fn record(&self, runtime_id: &str) -> Option<Arc<RuntimeRecord>> {
        self.records
            .lock()
            .expect("records lock")
            .get(runtime_id)
            .cloned()
    }

    pub fn state(&self, runtime_id: &str) -> Option<RuntimeSnapshot> {
        self.records
            .lock()
            .expect("records lock")
            .get(runtime_id)
            .map(|record| record.snapshot())
    }

    /// 活动记录快照（`starting`/`running`/`stopping`），按 runtime id 排序。
    ///
    /// 控制面优雅关闭用它枚举要停止的行；只读访问器，不改变状态机，也不
    /// 复制任何停止逻辑。
    pub fn active_runtimes(&self) -> Vec<RuntimeSnapshot> {
        let mut snapshots: Vec<RuntimeSnapshot> = self
            .records
            .lock()
            .expect("records lock")
            .values()
            .map(|record| record.snapshot())
            .filter(|snapshot| !snapshot.state.is_terminal())
            .collect();
        snapshots.sort_by(|left, right| left.runtime_id.cmp(&right.runtime_id));
        snapshots
    }

    /// 落下关闭闸门：拒绝新启动，并让等待重启的记录不再拉起内核。
    ///
    /// 必须在 [`Self::stop`] 之前调用，且要早于 HTTP 排空：排空期间仍可能有
    /// keep-alive 连接送达启动请求，闸门是它们唯一的拒绝点。
    pub fn begin_shutdown(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
    }

    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::SeqCst)
    }

    pub fn logs_tail(&self, runtime_id: &str, limit: usize) -> Result<Vec<String>, CoreError> {
        self.records
            .lock()
            .expect("records lock")
            .get(runtime_id)
            .map(|record| record.logs.tail(limit))
            .ok_or(CoreError::NotFound)
    }

    /// 启动内核；运行模式由清单能力决定，不做静默降级。
    ///
    /// `pseudo_headful` 要求 `RuntimeOptions::headless_chrome_ua_patch`，缺失时返回
    /// `RUNTIME_MODE_UNSUPPORTED`，且不会创建运行时记录或进程。超出并发上限时返回
    /// `RUNTIME_CONFLICT`，同样不创建记录、进程或代理绑定。
    pub async fn start(&self, options: RuntimeOptions) -> Result<RuntimeSnapshot, CoreError> {
        // 关闭闸门先于一切：不得在退出途中拉起新内核，也不得创建记录。
        if self.is_shutting_down() {
            return Err(CoreError::RuntimeConflict);
        }
        let mode_args = ModeArgs::resolve(options.mode, options.headless_chrome_ua_patch)?;
        let runtime_id = options.runtime_id.clone();
        let record = Arc::new(RuntimeRecord::new(
            options,
            self.events.clone(),
            mode_args,
            self.shutting_down.clone(),
        )?);
        {
            let mut records = self.records.lock().expect("records lock");
            if let Some(existing) = records.get(&record.options.runtime_id) {
                if !existing.snapshot().state.is_terminal() {
                    return Ok(existing.snapshot());
                }
            }
            let permit = self.admission.try_admit(&runtime_id)?;
            *record.permit.lock().expect("permit lock") = Some(permit);
            records.insert(record.options.runtime_id.clone(), record.clone());
        }
        record.transition(RuntimeState::Starting, None, None)?;
        match record.start_inner().await {
            Ok(()) => {
                *record.started_at.lock().expect("started_at lock") = Some(Instant::now());
                record.transition(RuntimeState::Running, None, None)?;
                spawn_monitor_record(record.clone());
                Ok(record.snapshot())
            }
            Err(start_error) => {
                let _ = record.transition(
                    RuntimeState::Stopping,
                    Some(start_error.code_str().to_owned()),
                    None,
                );
                let _ = record.stop_process(record.options.stop_timeout).await;
                let _ = record.transition(
                    RuntimeState::Failed,
                    Some(start_error.code_str().to_owned()),
                    Some(start_error.to_string()),
                );
                record.release_permit();
                Err(start_error)
            }
        }
    }

    pub async fn stop(&self, runtime_id: &str) -> Result<RuntimeSnapshot, CoreError> {
        let record = self
            .records
            .lock()
            .expect("records lock")
            .get(runtime_id)
            .cloned()
            .ok_or(CoreError::NotFound)?;
        let state = record.persistable_state();
        if state == RuntimeState::Stopped {
            return Ok(record.snapshot());
        }
        // Recorded before the transition so the monitor observes a user stop.
        record.mark_stop_requested();
        // `crashed`/`failed` also settle through `stopping` (SPEC transition table).
        let target = match state {
            RuntimeState::Crashed | RuntimeState::Failed => RuntimeState::Stopped,
            _ => RuntimeState::Stopping,
        };
        record.transition(target, None, None)?;
        record.stop_process(record.options.stop_timeout).await?;
        record.transition(RuntimeState::Stopped, None, None)?;
        record.release_permit();
        Ok(record.snapshot())
    }

    /// 测试和故障处理使用：不标记用户停止，因此监督器发出 crashed。
    pub fn force_kill(&self, runtime_id: &str) -> Result<(), CoreError> {
        let record = self
            .records
            .lock()
            .expect("records lock")
            .get(runtime_id)
            .cloned()
            .ok_or(CoreError::NotFound)?;
        let pgid = record
            .state
            .lock()
            .expect("runtime state lock")
            .pgid
            .ok_or(CoreError::NotFound)?;
        signal_process_group(pgid, true);
        Ok(())
    }

    /// 控制面重启对账：只终止命令行可安全归属的残留进程。
    /// 两遍：第一遍终止可归属进程组；第二遍让「同一进程组被另一行终止」的重复行
    /// 也落定，避免永久 `stopping` 与重复 `profile_id`。
    pub async fn reconcile(&self) -> Result<Vec<RuntimeSnapshot>, CoreError> {
        let mut outcomes = Vec::with_capacity(self.reconcile_entries()?.len());
        let mut pending = Vec::new();
        for (directory, metadata) in self.reconcile_entries()? {
            let snapshot = self.reconcile_entry(metadata).await;
            if snapshot.state == RuntimeState::Stopping {
                pending.push((directory, snapshot));
                continue;
            }
            let _ = std::fs::remove_file(directory.join("runtime.json"));
            outcomes.push(snapshot);
        }
        for (directory, snapshot) in pending {
            let pgid = snapshot.pgid.unwrap_or_default();
            let still_live = process_group_alive(pgid) && process_group_has_live(pgid);
            if still_live {
                outcomes.push(snapshot);
                continue;
            }
            let _ = std::fs::remove_file(directory.join("runtime.json"));
            outcomes.push(RuntimeSnapshot {
                state: RuntimeState::Crashed,
                pid: None,
                pgid: None,
                ..snapshot
            });
        }
        Ok(outcomes)
    }

    /// 深度优先扫描数据根下的 `runtime.json`；档案目录可以嵌在 `profiles/` 下。
    fn reconcile_entries(&self) -> Result<Vec<(PathBuf, RuntimeMetadata)>, CoreError> {
        let mut found = Vec::new();
        let mut pending = vec![self.root.clone()];
        while let Some(directory) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&directory) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    pending.push(path);
                    continue;
                }
                if path.file_name().is_none_or(|name| name != "runtime.json") {
                    continue;
                }
                let Some(parent) = path.parent() else {
                    continue;
                };
                let Ok(bytes) = std::fs::read(&path) else {
                    continue;
                };
                let Ok(metadata) = serde_json::from_slice::<RuntimeMetadata>(&bytes) else {
                    continue;
                };
                found.push((parent.to_path_buf(), metadata));
            }
        }
        Ok(found)
    }

    /// 单个残留行：只有命令行可归属才终止。仍存活且不可归属时保留行、端口与绑定。
    async fn reconcile_entry(&self, metadata: RuntimeMetadata) -> RuntimeSnapshot {
        if !metadata.state.is_terminal() {
            let mut attributable = false;
            let deadline = tokio::time::Instant::now() + DEFAULT_STOP_TIMEOUT;
            loop {
                if process_alive(metadata.pid)
                    && process_belongs_to(metadata.pid, &metadata.binary, &metadata.runtime_id)
                {
                    if !attributable {
                        attributable = true;
                        signal_process_group(metadata.pgid, false);
                    }
                    if !process_group_has_live(metadata.pgid) {
                        break;
                    }
                    if tokio::time::Instant::now() >= deadline {
                        signal_process_group(metadata.pgid, true);
                    }
                } else if attributable && !process_group_alive(metadata.pgid) {
                    break;
                }
                if tokio::time::Instant::now() >= deadline + Duration::from_secs(2) {
                    break;
                }
                tokio::time::sleep(MONITOR_INTERVAL).await;
            }
            // Only a *live* member blocks settlement; unreaped zombies do not.
            if process_group_alive(metadata.pgid) && process_group_has_live(metadata.pgid) {
                return reconciled_snapshot(metadata, RuntimeState::Stopping);
            }
        }
        reconciled_snapshot(metadata, RuntimeState::Crashed)
    }
}

/// 对账结果快照：内核已退出时清空 pid/pgid，归属不明时保留原 pid。
fn reconciled_snapshot(metadata: RuntimeMetadata, state: RuntimeState) -> RuntimeSnapshot {
    let settled = state != RuntimeState::Stopping;
    RuntimeSnapshot {
        reduced_assurance: state::reduced_assurance(
            metadata.mode,
            metadata.direct,
            metadata.isolation,
        ),
        runtime_id: metadata.runtime_id,
        profile_id: metadata.profile_id,
        state,
        mode: metadata.mode,
        isolation: metadata.isolation,
        direct: metadata.direct,
        ua_scope: metadata.ua_scope,
        pid: (!settled).then_some(metadata.pid),
        pgid: (!settled).then_some(metadata.pgid),
        cdp_port: metadata.cdp_port,
        proxy_port: metadata.proxy_port,
        exit_code: None,
        error_code: None,
    }
}

/// 崩溃后的有界重启：等待给定阶梯间隔后重拉内核；每次失败继续消耗阶梯，
/// 窗口耗尽才写 `failed`。阶梯的消耗点集中在 [`RuntimeRecord::next_restart_delay`]，
/// 因此 `runtime.crashed` 仍是一次转移一条事件。
async fn respawn(record: Arc<RuntimeRecord>, delay: Duration) {
    tokio::time::sleep(delay).await;
    // 关闭闸门落下后不再拉起任何内核：`crashed` 已是终态，记录保持真实结论。
    if record.shutting_down() {
        return;
    }
    // A user stop during the wait wins and never restarts.
    if record.persistable_state() != RuntimeState::Crashed {
        return;
    }
    let _ = record.transition(RuntimeState::Starting, None, None);
    match record.respawn_once().await {
        Ok(()) => {
            *record.started_at.lock().expect("started_at lock") = Some(Instant::now());
            let _ = record.transition(RuntimeState::Running, None, None);
            spawn_monitor_record(record);
        }
        Err(error) => {
            let code = error.code_str().to_owned();
            let _ = record.transition(
                RuntimeState::Stopping,
                Some(code.clone()),
                Some(error.to_string()),
            );
            let _ = record.stop_process(record.options.stop_timeout).await;
            match record.next_restart_delay() {
                Some(next) => {
                    let _ = record.transition(
                        RuntimeState::Crashed,
                        Some(code),
                        Some(format!("restarting in {next:?}")),
                    );
                    Box::pin(respawn(record, next)).await;
                }
                None => {
                    let _ = record.transition(
                        RuntimeState::Failed,
                        Some(code),
                        Some(record.exhausted_detail()),
                    );
                    record.release_permit();
                }
            }
        }
    }
}

/// Monitor one record; the task outlives the caller.
fn spawn_monitor_record(record: Arc<RuntimeRecord>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(MONITOR_INTERVAL).await;
            let status = {
                let mut child = record.child.lock().expect("child lock");
                match child.as_mut() {
                    Some(child) => child.try_wait().ok().flatten(),
                    None => return,
                }
            };
            let Some(status) = status else { continue };
            record.set_exit(status);
            record.child.lock().expect("child lock").take();
            record.clear_process();
            match record.persistable_state() {
                RuntimeState::Starting | RuntimeState::Running => {
                    let _ = record.transition(
                        RuntimeState::Stopping,
                        None,
                        Some("kernel exited".into()),
                    );
                    if record.stop_requested() {
                        let _ = record.transition(RuntimeState::Stopped, None, None);
                        record.release_permit();
                        return;
                    }
                    // Default policy is no auto restart: a crash stays `crashed`.
                    if !record.options.restart.is_enabled() {
                        let _ = record.transition(RuntimeState::Crashed, None, None);
                        record.release_permit();
                        return;
                    }
                    match record.next_restart_delay() {
                        Some(delay) => {
                            let _ = record.transition(
                                RuntimeState::Crashed,
                                Some(record.crash_error_code().to_owned()),
                                Some(format!("restarting in {delay:?}")),
                            );
                            tokio::spawn(respawn(record.clone(), delay));
                        }
                        // Exhausted: `stopping -> failed` right after this exit's
                        // teardown; `crashed` alone would be a dead end.
                        None => {
                            let _ = record.transition(
                                RuntimeState::Failed,
                                Some(record.crash_error_code().to_owned()),
                                Some(record.exhausted_detail()),
                            );
                            record.release_permit();
                        }
                    }
                    return;
                }
                RuntimeState::Stopping => {
                    let target = if record.stop_requested() {
                        RuntimeState::Stopped
                    } else {
                        RuntimeState::Crashed
                    };
                    let _ = record.transition(target, None, None);
                    record.release_permit();
                }
                _ => {}
            }
            return;
        }
    });
}

fn spawn_log_pump<R: AsyncRead + Unpin + Send + 'static>(
    reader: R,
    logs: Arc<RuntimeLogs>,
    stream: &'static str,
) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            logs.record(stream, &line);
        }
    });
}

#[cfg(unix)]
fn isolate_process(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.as_std_mut().process_group(0);
}
#[cfg(not(unix))]
fn isolate_process(_: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_payload_marks_non_headful_as_reduced() {
        assert!(state::reduced_assurance(
            ProfileMode::Headless,
            false,
            Isolation::Enforced
        ));
    }

    #[tokio::test]
    async fn pseudo_headful_without_the_ua_patch_is_rejected_before_spawn() {
        let root = std::env::temp_dir().join(format!("mien-runtime-{}", std::process::id()));
        let supervisor = RuntimeSupervisor::new(&root);
        let mut options = RuntimeOptions::new("r", ProfileId::new(), "/bin/true", "00", &root);
        options.mode = ProfileMode::PseudoHeadful;
        let error = supervisor.start(options).await.unwrap_err();
        assert_eq!(error.code(), ErrorCode::RuntimeModeUnsupported);
        assert!(
            supervisor.state("r").is_none(),
            "a refused mode must not create a runtime record"
        );
        assert_eq!(supervisor.admission().active(), 0, "no slot on refusal");
        let _ = std::fs::remove_dir_all(root);
    }

    /// Only the manifest's own `none` skips the platform sandbox check.
    #[test]
    fn only_a_declared_none_manifest_skips_sandbox_verification() {
        assert!(declares_no_sandbox("none"));
        for kind in [
            "namespace",
            "seatbelt",
            "win32-restricted-token",
            "unknown",
            "",
        ] {
            assert!(!declares_no_sandbox(kind), "`{kind}` must stay fail-closed");
        }
    }

    #[test]
    fn child_environment_is_limited_to_the_documented_whitelist() {
        let environment = child_environment("rt-test", |key| match key {
            "DISPLAY" => Some(":99".into()),
            "XAUTHORITY" => Some("/tmp/mien-Xauthority".into()),
            // Secrets and configuration never reach the kernel.
            "MIEN_MASTER_KEY" | "MIEN_BIND" | "MIEN_PUBLIC_URL" => Some("leak".into()),
            _ => None,
        });
        let names: Vec<&str> = environment.iter().map(|(name, _)| name.as_str()).collect();
        assert!(names.contains(&"DISPLAY"));
        assert!(names.contains(&"XAUTHORITY"));
        assert!(names.contains(&CHILD_RUNTIME_ID_ENV));
        assert!(!names.contains(&"MIEN_MASTER_KEY"));
        assert!(!names.contains(&"MIEN_BIND"));
        assert!(!names.contains(&"MIEN_PUBLIC_URL"));
        let path = environment
            .iter()
            .find(|(name, _)| name == "PATH")
            .map(|(_, value)| value.clone())
            .expect("PATH is always fixed");
        assert_eq!(path, std::ffi::OsString::from(CHILD_PATH));
        assert_eq!(
            environment
                .iter()
                .find(|(name, _)| name == CHILD_RUNTIME_ID_ENV)
                .map(|(_, value)| value.clone()),
            Some(std::ffi::OsString::from("rt-test"))
        );
    }
}

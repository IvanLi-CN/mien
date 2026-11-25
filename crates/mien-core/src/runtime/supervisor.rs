//! macOS 有头内核监督：状态机、独立进程组、事件、日志与对账。

use std::{
    collections::HashMap,
    path::PathBuf,
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, BufReader},
    process::{Child, Command},
};

use crate::{
    domain::profile::{ProfileId, ProfileMode},
    error::{CoreError, ErrorCode},
};

use super::{
    logs::RuntimeLogs,
    probe::{self, process_alive, process_belongs_to, process_group_alive, signal_process_group},
    state::{self, Isolation, RuntimeState},
};

const DEFAULT_STOP_TIMEOUT: Duration = Duration::from_secs(5);
const START_GRACE: Duration = Duration::from_millis(100);
const MONITOR_INTERVAL: Duration = Duration::from_millis(50);

fn spawn_failed(message: &str) -> CoreError {
    CoreError::validation(ErrorCode::RuntimeSpawnFailed, message)
}

/// 启动参数。`launch_args` 只作为参数数组传给内核，不经过 shell。
#[derive(Debug, Clone)]
pub struct RuntimeOptions {
    pub runtime_id: String,
    pub profile_id: ProfileId,
    pub binary: PathBuf,
    pub binary_sha256: String,
    pub manifest_sandbox_kind: String,
    pub mode: ProfileMode,
    pub automation_api: bool,
    pub launch_args: Vec<String>,
    pub data_dir: PathBuf,
    pub direct: bool,
    pub proxy_ready: bool,
    pub proxy_port: Option<u16>,
    pub stop_timeout: Duration,
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
            automation_api: false,
            launch_args: Vec::new(),
            data_dir: data_dir.into(),
            direct: false,
            proxy_ready: true,
            proxy_port: None,
            stop_timeout: DEFAULT_STOP_TIMEOUT,
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
    proxy_port: Option<u16>,
    pid: u32,
    pgid: i32,
    cdp_port: Option<u16>,
    state: RuntimeState,
}

struct RuntimeRecord {
    options: RuntimeOptions,
    logs: Arc<RuntimeLogs>,
    state: Mutex<RecordState>,
    child: Mutex<Option<Child>>,
    stop_requested: Mutex<bool>,
    events: Arc<Mutex<Vec<RuntimeEvent>>>,
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
    ) -> Result<Self, CoreError> {
        let logs = Arc::new(
            RuntimeLogs::new(options.data_dir.join("logs"))
                .map_err(|_| spawn_failed("kernel process could not be created"))?,
        );
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
            events,
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
                ua_scope: (snapshot.mode == ProfileMode::PseudoHeadful).then(|| "chrome".into()),
                proxy_port: snapshot.proxy_port,
                exit_code: snapshot.exit_code,
                error_code: error_code.or(snapshot.error_code),
                detail: detail.map(|value| super::logs::redact(&value)),
            },
        });
    }

    fn snapshot(&self) -> RuntimeSnapshot {
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
            pid: state.pid,
            pgid: state.pgid,
            cdp_port: state.cdp_port,
            proxy_port: self.options.proxy_port,
            exit_code: state.exit_code,
            error_code: state.error_code,
        }
    }

    fn persist_metadata(&self) {
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

    fn mark_stop_requested(&self) {
        *self.stop_requested.lock().expect("stop lock") = true;
    }

    fn stop_requested(&self) -> bool {
        *self.stop_requested.lock().expect("stop lock")
    }

    fn clear_process(&self) {
        self.state.lock().expect("runtime state lock").pid = None;
        self.state.lock().expect("runtime state lock").pgid = None;
        let _ = std::fs::remove_file(self.options.data_dir.join("runtime.json"));
    }

    fn persistable_state(&self) -> RuntimeState {
        self.state.lock().expect("runtime state lock").state
    }

    async fn stop_process(&self, timeout: Duration) -> Result<(), CoreError> {
        self.mark_stop_requested();
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
}

/// 运行时监督器；一个档案最多一个非终态实例。
pub struct RuntimeSupervisor {
    records: Mutex<HashMap<String, Arc<RuntimeRecord>>>,
    events: Arc<Mutex<Vec<RuntimeEvent>>>,
    root: PathBuf,
}

impl RuntimeSupervisor {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            records: Mutex::new(HashMap::new()),
            events: Arc::new(Mutex::new(Vec::new())),
            root: root.into(),
        }
    }

    pub fn events(&self) -> Vec<RuntimeEvent> {
        self.events.lock().expect("event lock").clone()
    }

    pub fn state(&self, runtime_id: &str) -> Option<RuntimeSnapshot> {
        self.records
            .lock()
            .expect("records lock")
            .get(runtime_id)
            .map(|record| record.snapshot())
    }

    pub fn logs_tail(&self, runtime_id: &str, limit: usize) -> Result<Vec<String>, CoreError> {
        self.records
            .lock()
            .expect("records lock")
            .get(runtime_id)
            .map(|record| record.logs.tail(limit))
            .ok_or(CoreError::NotFound)
    }

    /// 启动 macOS 有头内核；无头与伪有头显式拒绝。
    pub async fn start(&self, options: RuntimeOptions) -> Result<RuntimeSnapshot, CoreError> {
        if options.mode != ProfileMode::Headful {
            return Err(CoreError::validation(
                ErrorCode::RuntimeModeUnsupported,
                "only headful runtime mode is supported",
            ));
        }
        let record = Arc::new(RuntimeRecord::new(options, self.events.clone())?);
        {
            let mut records = self.records.lock().expect("records lock");
            if let Some(existing) = records.get(&record.options.runtime_id) {
                if !existing.snapshot().state.is_terminal() {
                    return Ok(existing.snapshot());
                }
            }
            records.insert(record.options.runtime_id.clone(), record.clone());
        }
        record.transition(RuntimeState::Starting, None, None)?;
        let result = self.start_inner(&record).await;
        match result {
            Ok(()) => {
                record.transition(RuntimeState::Running, None, None)?;
                self.spawn_monitor(record.clone());
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
                Err(start_error)
            }
        }
    }

    async fn start_inner(&self, record: &Arc<RuntimeRecord>) -> Result<(), CoreError> {
        let options = &record.options;
        if !options.direct && !options.proxy_ready {
            return Err(CoreError::validation(
                ErrorCode::ProxyEngineNotReady,
                "shared proxy engine is not ready",
            ));
        }
        #[cfg(target_os = "linux")]
        let isolation = Isolation::Unknown;
        #[cfg(not(target_os = "linux"))]
        let isolation = probe::probe_macos_sandbox(&options.manifest_sandbox_kind)?;
        probe::verify_kernel(&options.binary, &options.binary_sha256)?;
        std::fs::create_dir_all(options.data_dir.join("chrome"))
            .map_err(|_| spawn_failed("kernel process could not be created"))?;
        std::fs::create_dir_all(options.data_dir.join("downloads"))
            .map_err(|_| spawn_failed("kernel process could not be created"))?;
        record.set_isolation(isolation);

        let mut command = Command::new(&options.binary);
        let mut args = options.launch_args.clone();
        args.retain(|arg| {
            !arg.starts_with("--user-data-dir=") && !arg.starts_with("--remote-debugging-port=")
        });
        args.push(format!(
            "--user-data-dir={}",
            options.data_dir.join("chrome").display()
        ));
        args.push(format!("--mien-runtime-id={}", options.runtime_id));
        // Never touch the host Keychain: use Chromium's in-memory mock keychain
        // and the basic password store for every supervised launch.
        args.push("--use-mock-keychain".into());
        args.push("--password-store=basic".into());
        if options.automation_api {
            args.push("--remote-debugging-port=0".into());
        }
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_clear();
        command.env("MIEN_RUNTIME_ID", &options.runtime_id);
        isolate_process(&mut command);
        let mut child = command
            .spawn()
            .map_err(|_| spawn_failed("kernel process could not be created"))?;
        let pid = child
            .id()
            .ok_or_else(|| spawn_failed("kernel process could not be created"))?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        record.set_process(child, pid);
        if let Some(stdout) = stdout {
            spawn_log_pump(stdout, record.logs.clone(), "stdout");
        }
        if let Some(stderr) = stderr {
            spawn_log_pump(stderr, record.logs.clone(), "stderr");
        }

        if options.automation_api {
            let port =
                probe::wait_for_cdp(&options.data_dir.join("chrome"), Duration::from_secs(10))
                    .await?;
            record.set_cdp_port(port);
        } else {
            tokio::time::sleep(START_GRACE).await;
            if !process_alive(pid) {
                return Err(spawn_failed("kernel process could not be created"));
            }
        }
        #[cfg(target_os = "linux")]
        {
            match probe::require_linux_isolation(pid).await {
                Ok(isolation) => record.set_isolation(isolation),
                Err(error) => {
                    let _ = record.stop_process(Duration::from_secs(2)).await;
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    fn spawn_monitor(&self, record: Arc<RuntimeRecord>) {
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
                        let target = if record.stop_requested() {
                            RuntimeState::Stopped
                        } else {
                            RuntimeState::Crashed
                        };
                        let _ = record.transition(target, None, None);
                    }
                    RuntimeState::Stopping => {
                        let target = if record.stop_requested() {
                            RuntimeState::Stopped
                        } else {
                            RuntimeState::Crashed
                        };
                        let _ = record.transition(target, None, None);
                    }
                    _ => {}
                }
                return;
            }
        });
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
        if state.is_terminal() {
            return Ok(record.snapshot());
        }
        record.transition(RuntimeState::Stopping, None, None)?;
        record.stop_process(record.options.stop_timeout).await?;
        record.transition(RuntimeState::Stopped, None, None)?;
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
    pub async fn reconcile(&self) -> Result<Vec<RuntimeSnapshot>, CoreError> {
        let mut snapshots = Vec::new();
        let entries = std::fs::read_dir(&self.root).map_err(|_| CoreError::Storage)?;
        for entry in entries.flatten() {
            let path = entry.path().join("runtime.json");
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let Ok(metadata) = serde_json::from_slice::<RuntimeMetadata>(&bytes) else {
                continue;
            };
            if metadata.state.is_terminal() {
                let _ = std::fs::remove_file(path);
                continue;
            }
            if process_alive(metadata.pid)
                && process_belongs_to(metadata.pid, &metadata.binary, &metadata.runtime_id)
            {
                signal_process_group(metadata.pgid, false);
                let deadline = tokio::time::Instant::now() + DEFAULT_STOP_TIMEOUT;
                while process_group_alive(metadata.pgid) && tokio::time::Instant::now() < deadline {
                    tokio::time::sleep(MONITOR_INTERVAL).await;
                }
                if process_group_alive(metadata.pgid) {
                    signal_process_group(metadata.pgid, true);
                }
            }
            if !process_group_alive(metadata.pgid) {
                let _ = std::fs::remove_file(path);
                snapshots.push(RuntimeSnapshot {
                    runtime_id: metadata.runtime_id,
                    profile_id: metadata.profile_id,
                    state: RuntimeState::Crashed,
                    mode: metadata.mode,
                    isolation: metadata.isolation,
                    direct: metadata.direct,
                    reduced_assurance: state::reduced_assurance(
                        metadata.mode,
                        metadata.direct,
                        metadata.isolation,
                    ),
                    pid: None,
                    pgid: None,
                    cdp_port: metadata.cdp_port,
                    proxy_port: metadata.proxy_port,
                    exit_code: None,
                    error_code: None,
                });
            }
        }
        Ok(snapshots)
    }
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
    async fn modes_are_explicitly_rejected() {
        let root = std::env::temp_dir().join(format!("mien-runtime-{}", std::process::id()));
        let supervisor = RuntimeSupervisor::new(&root);
        let mut options = RuntimeOptions::new("r", ProfileId::new(), "/bin/true", "00", &root);
        options.mode = ProfileMode::PseudoHeadful;
        let error = supervisor.start(options).await.unwrap_err();
        assert_eq!(error.code(), ErrorCode::RuntimeModeUnsupported);
        let _ = std::fs::remove_dir_all(root);
    }
}

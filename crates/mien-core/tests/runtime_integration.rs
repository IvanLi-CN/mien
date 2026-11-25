//! 真实 macOS 有头内核的生命周期、强杀与重复循环。
//!
//! 需要真实解包内核：设置 `MIEN_KERNEL_BIN` 才会执行；未设置时显式跳过并打印原因，
//! 不允许静默通过。

use mien_core::domain::profile::ProfileId;
use mien_core::runtime::probe::process_group_alive;
use mien_core::runtime::{RuntimeOptions, RuntimeState, RuntimeSupervisor};
use std::{
    env,
    ffi::OsString,
    net::TcpStream,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// 内核可执行文件的 sha256，与 T4 解包记录一致。
const KERNEL_SHA256: &str = "d918c691ce56648bc555f8f399c29fc83eb02224165a086491688d01707170cb";
/// 未配置内核时必须打印的跳过原因。
const SKIP_MESSAGE: &str = "skipped: MIEN_KERNEL_BIN not set";

fn kernel_from(value: Option<OsString>) -> Result<PathBuf, &'static str> {
    match value {
        Some(path) => {
            let path = PathBuf::from(path);
            assert!(
                path.is_file(),
                "MIEN_KERNEL_BIN is not a file: {}",
                path.display()
            );
            Ok(path)
        }
        None => Err(SKIP_MESSAGE),
    }
}

fn kernel() -> Option<PathBuf> {
    match kernel_from(env::var_os("MIEN_KERNEL_BIN")) {
        Ok(path) => Some(path),
        Err(skip) => {
            eprintln!("{skip}");
            None
        }
    }
}

fn options(binary: &Path, root: &Path, runtime_id: &str) -> RuntimeOptions {
    let mut options = RuntimeOptions::new(
        runtime_id,
        ProfileId::new(),
        binary,
        KERNEL_SHA256,
        root.join(runtime_id),
    );
    options.automation_api = true;
    options.launch_args = vec![
        "--window-size=800,600".into(),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        "--disable-background-networking".into(),
        "--disable-gpu".into(),
        "--use-mock-keychain".into(),
        "--password-store=basic".into(),
    ];
    options
}

/// `pgrep -f <binary>`：主进程与任何继承该可执行路径的进程。
fn pgrep_path(binary: &Path) -> Vec<u32> {
    pgrep(&binary.to_string_lossy())
}

/// `pgrep -f --mien-runtime-id=<id>`：该运行时启动的全部子进程。
fn pgrep_runtime(runtime_id: &str) -> Vec<u32> {
    pgrep(&format!("--mien-runtime-id={runtime_id}"))
}

fn pgrep(pattern: &str) -> Vec<u32> {
    let output = std::process::Command::new("pgrep")
        .args(["-f", pattern])
        .output()
        .expect("pgrep");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .collect()
}

async fn wait_until<F>(mut check: F)
where
    F: FnMut() -> bool,
{
    let deadline = Instant::now() + Duration::from_secs(10);
    while !check() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// 未设置 `MIEN_KERNEL_BIN` 时必须得到显式跳过文本，而不是静默通过。
#[test]
fn unset_kernel_env_reports_explicit_skip() {
    assert_eq!(kernel_from(None).unwrap_err(), SKIP_MESSAGE);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_macos_headful_lifecycle_crash_and_twenty_cycles() {
    let Some(binary) = kernel() else { return };
    assert_eq!(std::env::consts::OS, "macos");
    let root = env::temp_dir().join(format!("mien-runtime-integration-{}", std::process::id()));
    let supervisor = RuntimeSupervisor::new(&root);

    // 1. 真实启动 → running：状态、进程、进程组、CDP 都可观察。
    let started = Instant::now();
    let running = supervisor
        .start(options(&binary, &root, "lifecycle"))
        .await
        .expect("real kernel start");
    let start_elapsed = started.elapsed();
    assert_eq!(running.state, RuntimeState::Running);
    let pid = running.pid.expect("kernel pid");
    let pgid = running.pgid.expect("kernel pgid");
    let cdp_port = running.cdp_port.expect("CDP port");
    assert!(TcpStream::connect(("127.0.0.1", cdp_port)).is_ok());
    assert!(!pgrep_path(&binary).is_empty());

    // 2. 真实停止 → stopped：进程组无残留，CDP 端口不再可连。
    let stopped_at = Instant::now();
    let stopped = supervisor
        .stop("lifecycle")
        .await
        .expect("real kernel stop");
    let stop_elapsed = stopped_at.elapsed();
    assert_eq!(stopped.state, RuntimeState::Stopped);
    assert!(stopped.pid.is_none());
    wait_until(|| pgrep_path(&binary).is_empty()).await;
    assert!(pgrep_path(&binary).is_empty(), "residual kernel processes");
    assert!(
        pgrep_runtime("lifecycle").is_empty(),
        "residual kernel descendants"
    );
    assert!(!process_group_alive(pgid), "residual process group {pgid}");
    assert!(TcpStream::connect(("127.0.0.1", cdp_port)).is_err());

    let kinds: Vec<_> = supervisor
        .events()
        .into_iter()
        .filter(|event| event.payload.runtime_id == "lifecycle")
        .map(|event| event.kind)
        .collect();
    assert_eq!(
        kinds,
        [
            "runtime.starting",
            "runtime.running",
            "runtime.stopping",
            "runtime.stopped"
        ],
        "each lifecycle event must be emitted exactly once"
    );
    println!(
        "lifecycle: pid={pid} pgid={pgid} cdp_port={cdp_port} start={start_elapsed:?} stop={stop_elapsed:?} events={kinds:?}"
    );

    // 3. SIGKILL 内核 → crashed，并产生对应的 runtime.crashed 事件。
    let crash = supervisor
        .start(options(&binary, &root, "crash"))
        .await
        .expect("crash scenario start");
    let crash_pgid = crash.pgid.expect("crash pgid");
    supervisor.force_kill("crash").expect("SIGKILL kernel");
    wait_until(|| {
        supervisor
            .state("crash")
            .is_some_and(|snapshot| snapshot.state == RuntimeState::Crashed)
    })
    .await;
    let crashed = supervisor.state("crash").expect("crashed snapshot");
    assert_eq!(crashed.state, RuntimeState::Crashed);
    let crash_event = supervisor
        .events()
        .into_iter()
        .find(|event| event.kind == "runtime.crashed" && event.payload.runtime_id == "crash")
        .expect("runtime.crashed event");
    assert_eq!(crash_event.payload.state, RuntimeState::Crashed);
    wait_until(|| !process_group_alive(crash_pgid)).await;
    assert!(
        !process_group_alive(crash_pgid),
        "crashed group still alive"
    );
    assert!(pgrep_path(&binary).is_empty(), "residual after crash");
    println!(
        "crash: state={:?} exit_code={:?} event={} sequence={}",
        crashed.state, crashed.exit_code, crash_event.kind, crash_event.sequence
    );

    // 4. 20 次启动/停止循环：每轮都确认无残留进程、无残留端口。
    let loop_started = Instant::now();
    for index in 0..20 {
        let id = format!("cycle-{index}");
        let runtime = supervisor
            .start(options(&binary, &root, &id))
            .await
            .unwrap_or_else(|error| panic!("cycle {index} start: {error}"));
        assert_eq!(runtime.state, RuntimeState::Running);
        let port = runtime.cdp_port.expect("cycle CDP port");
        let pgid = runtime.pgid.expect("cycle pgid");
        assert!(TcpStream::connect(("127.0.0.1", port)).is_ok());
        let stopped = supervisor
            .stop(&id)
            .await
            .unwrap_or_else(|error| panic!("cycle {index} stop: {error}"));
        assert_eq!(stopped.state, RuntimeState::Stopped);
        wait_until(|| pgrep_path(&binary).is_empty()).await;
        assert!(pgrep_path(&binary).is_empty(), "orphan after cycle {index}");
        assert!(
            pgrep_runtime(&id).is_empty(),
            "orphan descendants after cycle {index}"
        );
        assert!(
            !process_group_alive(pgid),
            "process group {pgid} alive after cycle {index}"
        );
        assert!(
            TcpStream::connect(("127.0.0.1", port)).is_err(),
            "CDP still open after cycle {index}"
        );
    }
    println!(
        "twenty cycles: elapsed={:?} residual_processes={:?}",
        loop_started.elapsed(),
        pgrep_path(&binary)
    );
    let _ = std::fs::remove_dir_all(root);
}

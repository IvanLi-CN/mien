use mien_core::domain::profile::ProfileId;
use mien_core::runtime::{RuntimeOptions, RuntimeState, RuntimeSupervisor};
use std::{
    env,
    net::TcpStream,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const KERNEL_SHA256: &str = "d918c691ce56648bc555f8f399c29fc83eb02224165a086491688d01707170cb";

fn kernel() -> Option<PathBuf> {
    match env::var_os("MIEN_KERNEL_BIN") {
        Some(path) => {
            let path = PathBuf::from(path);
            assert!(
                path.is_file(),
                "MIEN_KERNEL_BIN is not a file: {}",
                path.display()
            );
            Some(path)
        }
        None => {
            println!("skipped: MIEN_KERNEL_BIN not set");
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

fn pgrep_path(binary: &Path) -> Vec<u32> {
    let output = std::process::Command::new("pgrep")
        .args(["-f", &binary.to_string_lossy()])
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_macos_headful_lifecycle_crash_and_twenty_cycles() {
    let Some(binary) = kernel() else { return };
    assert_eq!(std::env::consts::OS, "macos");
    let root = env::temp_dir().join(format!("mien-runtime-integration-{}", std::process::id()));
    let supervisor = RuntimeSupervisor::new(&root);

    let first = options(&binary, &root, "lifecycle");
    let started = Instant::now();
    let running = supervisor.start(first).await.expect("real kernel start");
    let start_elapsed = started.elapsed();
    assert_eq!(running.state, RuntimeState::Running);
    assert!(running.pid.is_some());
    let cdp_port = running.cdp_port.expect("CDP port");
    assert!(TcpStream::connect(("127.0.0.1", cdp_port)).is_ok());
    assert!(!pgrep_path(&binary).is_empty());

    let stopped_at = Instant::now();
    let stopped = supervisor
        .stop("lifecycle")
        .await
        .expect("real kernel stop");
    let stop_elapsed = stopped_at.elapsed();
    assert_eq!(stopped.state, RuntimeState::Stopped);
    assert!(stopped.pid.is_none());
    wait_until(|| pgrep_path(&binary).is_empty()).await;
    assert!(pgrep_path(&binary).is_empty());
    assert!(TcpStream::connect(("127.0.0.1", cdp_port)).is_err());
    let kinds: Vec<_> = supervisor
        .events()
        .into_iter()
        .map(|event| event.kind)
        .collect();
    assert_eq!(
        &kinds[..4],
        &[
            "runtime.starting",
            "runtime.running",
            "runtime.stopping",
            "runtime.stopped"
        ]
    );
    println!("real lifecycle: start={start_elapsed:?} stop={stop_elapsed:?} cdp_port={cdp_port}");

    let crash = options(&binary, &root, "crash");
    supervisor.start(crash).await.expect("crash scenario start");
    supervisor.force_kill("crash").expect("SIGKILL kernel");
    wait_until(|| {
        supervisor
            .state("crash")
            .is_some_and(|snapshot| snapshot.state == RuntimeState::Crashed)
    })
    .await;
    let crashed = supervisor.state("crash").expect("crashed snapshot");
    assert_eq!(crashed.state, RuntimeState::Crashed);
    assert!(
        supervisor
            .events()
            .iter()
            .any(|event| event.kind == "runtime.crashed" && event.payload.runtime_id == "crash")
    );
    wait_until(|| pgrep_path(&binary).is_empty()).await;
    assert!(pgrep_path(&binary).is_empty());
    println!(
        "forced crash: state={:?} exit_code={:?}",
        crashed.state, crashed.exit_code
    );

    let loop_started = Instant::now();
    for index in 0..20 {
        let id = format!("cycle-{index}");
        let runtime = supervisor
            .start(options(&binary, &root, &id))
            .await
            .expect("cycle start");
        assert_eq!(runtime.state, RuntimeState::Running);
        let port = runtime.cdp_port.expect("cycle CDP port");
        assert!(TcpStream::connect(("127.0.0.1", port)).is_ok());
        let stopped = supervisor.stop(&id).await.expect("cycle stop");
        assert_eq!(stopped.state, RuntimeState::Stopped);
        wait_until(|| pgrep_path(&binary).is_empty()).await;
        assert!(pgrep_path(&binary).is_empty(), "orphan after cycle {index}");
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

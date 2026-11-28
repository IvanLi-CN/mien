//! 真机验收：创建 → 启动有头内核 → 停止 → 再启动。
//!
//! 只有设置 `MIEN_KERNEL_BIN` 时才执行；未设置时打印显式跳过原因，不允许
//! 静默通过。数据根用临时目录，绝不触碰 `~/Library/Application Support/Mien`。

#[path = "../src/state.rs"]
#[allow(dead_code)]
mod state;

use mien_core::runtime::RuntimeState;
use state::{AppState, CreateProfileInput};

const SKIP_MESSAGE: &str = "skipped: MIEN_KERNEL_BIN not set";

fn kernel() -> Option<std::path::PathBuf> {
    match std::env::var_os("MIEN_KERNEL_BIN") {
        None => {
            eprintln!("{SKIP_MESSAGE}");
            None
        }
        Some(value) => {
            let path = std::path::PathBuf::from(value);
            assert!(path.is_file(), "MIEN_KERNEL_BIN is not a file: {path:?}");
            Some(path)
        }
    }
}

fn process_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn mien_runtime_processes(runtime_id: &str) -> Vec<u32> {
    let output = std::process::Command::new("pgrep")
        .args(["-f", "--", &format!("--mien-runtime-id={runtime_id}")])
        .output()
        .expect("pgrep");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .collect()
}

#[test]
fn create_start_stop_start_through_the_host_state() {
    let Some(binary) = kernel() else {
        return;
    };
    assert_eq!(std::env::consts::OS, "macos");
    let directory = tempfile::tempdir().expect("temporary directory");
    let state = AppState::open_at_with_kernel(directory.path().join("data"), Some(binary))
        .expect("host state");
    let runtime = tokio::runtime::Runtime::new().expect("runtime");

    runtime.block_on(async {
        let created = state
            .create_profile(CreateProfileInput {
                name: "acceptance".into(),
                fingerprint: None,
                mode: None,
                start_url: None,
                notes: None,
                automation_api: None,
                group_id: None,
                tags: Vec::new(),
                proxy_id: None,
                kernel_version: None,
            })
            .await
            .expect("create");
        let profile_id = created.profile.id.0.clone();

        let first = state.start_runtime(&profile_id).await.expect("first start");
        assert_eq!(first.state, RuntimeState::Running);
        assert!(first.reduced_assurance, "direct mode is reduced assurance");
        assert!(
            first
                .disclosures
                .iter()
                .any(|disclosure| disclosure.contains("direct mode")),
            "direct mode must be disclosed: {:?}",
            first.disclosures
        );
        assert!(
            !first.disclosures.iter().any(|_| false),
            "disclosures are strings"
        );
        let status = state.runtime_status(&profile_id).await.expect("status");
        let pid = status.pid.expect("running kernel pid");
        assert!(process_alive(pid), "the headful kernel must be running");
        println!(
            "kernel pid={pid} pgrep={:?}",
            mien_runtime_processes(&first.runtime_id)
        );
        println!(
            "first start: state={:?} isolation={:?} reduced_assurance={} disclosures={:?}",
            first.state, first.isolation, first.reduced_assurance, first.disclosures
        );

        let processes = mien_runtime_processes(&first.runtime_id);
        assert!(
            !processes.is_empty(),
            "kernel children must be attributable"
        );

        let stopped = state.stop_runtime(&profile_id).await.expect("first stop");
        assert_eq!(stopped.state, RuntimeState::Stopped);
        assert!(!process_alive(pid), "stop must reap the kernel process");
        assert!(
            mien_runtime_processes(&first.runtime_id).is_empty(),
            "stop must reap the whole process group"
        );

        let second = state
            .start_runtime(&profile_id)
            .await
            .expect("second start");
        assert_eq!(second.state, RuntimeState::Running);
        println!(
            "second start: state={:?} isolation={:?} disclosures={:?}",
            second.state, second.isolation, second.disclosures
        );

        let stopped = state.stop_runtime(&profile_id).await.expect("second stop");
        assert_eq!(stopped.state, RuntimeState::Stopped);
    });
}

//! 命令层验收（PRD-ACC-001 的真实产品路径）：创建 → 启动 → 状态 → 停止 → 再启动。
//!
//! 这里调用的是 `generate_handler!` 注册的同一批命令函数，断言 UI 实际收到的
//! JSON 载荷，而不是 `AppState` 的 Rust 结构。ACL 与参数反序列化由 `ipc.rs` 覆盖；
//! 本文件补的是命令函数本身的契约与真实内核生命周期。
//!
//! `tauri::State` 没有公开构造函数，所以用 `tauri::test` 的 mock 组件只为取出
//! `State<'_, AppState>`；命令体与发布构建逐字相同，没有替代实现。注意不使用
//! `generate_context!`：它每个 crate 只能展开一次，装配点留给 `ipc.rs`。
//!
//! 只有设置 `MIEN_KERNEL_BIN` 时才执行；未设置时打印显式跳过原因，不静默通过。
//! 数据根用临时目录，绝不触碰 `~/Library/Application Support/Mien`。

#[path = "../src/commands.rs"]
#[allow(dead_code)]
mod commands;
#[path = "../src/state.rs"]
#[allow(dead_code)]
mod state;

use mien_core::domain::profile::ProfileMode;
use tauri::Manager;
use tauri::test::{mock_builder, mock_context, noop_assets};

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

/// 命令返回值按 IPC 序列化后的形态断言，避免直接比对 Rust 结构。
fn json<T: serde::Serialize>(value: &T) -> serde_json::Value {
    serde_json::to_value(value).expect("serialize command result")
}

#[test]
fn create_start_status_stop_start_through_the_command_layer() {
    let Some(binary) = kernel() else {
        return;
    };
    assert_eq!(std::env::consts::OS, "macos");
    let directory = tempfile::tempdir().expect("temporary directory");
    let state = state::AppState::open_at_with_kernel(directory.path().join("data"), Some(binary))
        .expect("host state");
    // mock 组件只提供 `State`；命令函数本身是注册到 `generate_handler!` 的同一批。
    let app = mock_builder()
        .manage(state)
        .build(mock_context(noop_assets()))
        .expect("mock app");
    let host = app.state::<state::AppState>();
    let runtime = tokio::runtime::Runtime::new().expect("runtime");

    runtime.block_on(async {
        // 1. 创建：默认指纹必须通过 manifest 准入。
        let created = json(
            &commands::profiles_create(
                host.clone(),
                "command-acceptance".into(),
                None,
                Some(ProfileMode::Headful),
                None,
                None,
            )
            .await
            .expect("create"),
        );
        let profile_id = created["id"].as_str().expect("profile id").to_owned();
        assert_eq!(created["name"], "command-acceptance");
        assert_eq!(created["mode"], "headful");
        assert_eq!(created["direct"], true);
        assert_eq!(created["isolation"], "unknown");
        assert_eq!(
            created["reduced_assurance"], true,
            "a direct-mode profile is reduced assurance before it ever starts"
        );
        assert!(
            created["runtime"].is_null(),
            "a fresh profile has no runtime: {created:?}"
        );

        // 2. 启动：有头内核真的起来；disclosures 必须带 direct 与 unverified 说明。
        let started = json(
            &commands::runtime_start(host.clone(), profile_id.clone())
                .await
                .expect("first start"),
        );
        assert_eq!(started["state"], "running");
        assert_eq!(started["mode"], "headful");
        assert_eq!(
            started["reducedAssurance"], true,
            "direct mode is reduced assurance"
        );
        let runtime_id = started["runtimeId"]
            .as_str()
            .expect("runtime id")
            .to_owned();
        let disclosures = started["disclosures"]
            .as_array()
            .expect("disclosures array");
        assert!(
            disclosures.iter().all(serde_json::Value::is_string),
            "disclosures are strings: {disclosures:?}"
        );
        assert!(
            disclosures.iter().any(|value| value
                .as_str()
                .is_some_and(|text| text.contains("direct mode"))),
            "direct mode must be disclosed: {disclosures:?}"
        );
        assert!(
            disclosures.iter().any(|value| value
                .as_str()
                .is_some_and(|text| text.contains("unverified"))),
            "unverified network controls must be disclosed: {disclosures:?}"
        );
        println!(
            "start: state={} isolation={} reducedAssurance={} runtimeId={runtime_id} disclosures={disclosures:?}",
            started["state"], started["isolation"], started["reducedAssurance"]
        );

        // 3. 状态：与上一帧 start 一致，且 pid 是一个真实存活的进程。
        let running = json(
            &commands::runtime_status(host.clone(), profile_id.clone())
                .await
                .expect("status"),
        );
        assert_eq!(running["state"], "running");
        assert_eq!(running["isolation"], started["isolation"]);
        let pid = running["pid"].as_u64().expect("running pid") as u32;
        assert!(process_alive(pid), "the headful kernel must be running");
        let processes = mien_runtime_processes(&runtime_id);
        assert!(
            !processes.is_empty(),
            "kernel children must be attributable: runtimeId={runtime_id}"
        );
        println!("status: pid={pid} pgrep={processes:?}");

        // 4. 停止：状态回到 stopped，进程被回收，不留孤儿。
        let stopped = json(
            &commands::runtime_stop(host.clone(), profile_id.clone())
                .await
                .expect("first stop"),
        );
        assert_eq!(stopped["state"], "stopped");
        assert!(!process_alive(pid), "stop must reap the kernel process");
        assert!(
            mien_runtime_processes(&runtime_id).is_empty(),
            "stop must reap the whole process group"
        );

        // 5. 再次启动：同一档案必须能重新拉起一条全新的内核。
        let restarted = json(
            &commands::runtime_start(host.clone(), profile_id.clone())
                .await
                .expect("second start"),
        );
        assert_eq!(restarted["state"], "running");
        assert_eq!(restarted["runtimeId"], runtime_id.as_str());
        let restarted_status = json(
            &commands::runtime_status(host.clone(), profile_id.clone())
                .await
                .expect("status after restart"),
        );
        let restarted_pid = restarted_status["pid"].as_u64().expect("restarted pid") as u32;
        assert_ne!(restarted_pid, pid, "a restart needs a new kernel process");
        assert!(process_alive(restarted_pid));

        let stopped = json(
            &commands::runtime_stop(host.clone(), profile_id.clone())
                .await
                .expect("second stop"),
        );
        assert_eq!(stopped["state"], "stopped");
        assert!(!process_alive(restarted_pid), "second stop must reap too");

        // 列表命令必须反映最终状态，UI 刷新后不会停在 running。
        let listed = json(
            &commands::profiles_list(host.clone(), 10, None, None)
                .await
                .expect("list"),
        );
        let items = listed["items"].as_array().expect("items");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["id"], profile_id.as_str());
        assert_eq!(items[0]["runtime"]["state"], "stopped");
    });
}

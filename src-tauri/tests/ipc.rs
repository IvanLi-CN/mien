//! IPC 端到端测试：真实 ACL 授权、命令名与 camelCase 参数映射。
//!
//! 用 Tauri 的 mock runtime 走完整 IPC 链路（`InvokeRequest` → 权限解析 →
//! `#[tauri::command]` 参数反序列化 → `AppState`），因此能力清单未授权的命令会
//! 在这里失败，参数名写错也会失败。

#[path = "../src/commands.rs"]
#[allow(dead_code)]
mod commands;
#[path = "../src/state.rs"]
#[allow(dead_code)]
mod state;

use serde_json::json;
use tauri::WebviewWindowBuilder;
use tauri::test::{INVOKE_KEY, mock_builder};
use tauri::webview::InvokeRequest;
use tempfile::tempdir;

fn app(directory: &tempfile::TempDir) -> tauri::App<tauri::test::MockRuntime> {
    let state = state::AppState::open_at_with_kernel(directory.path().join("data"), None)
        .expect("host state");
    build_app(state)
}

/// `generate_context!` 每个 crate 只能展开一次（它定义 `_EMBED_INFO_PLIST`），
/// 所以所有用例共享这一个装配点。
fn build_app(state: state::AppState) -> tauri::App<tauri::test::MockRuntime> {
    mock_builder()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::profiles_list,
            commands::profiles_get,
            commands::profiles_create,
            commands::profiles_update,
            commands::runtime_start,
            commands::runtime_stop,
            commands::runtime_status,
        ])
        // 使用真实 `tauri.conf.json` 与 `capabilities/default.json`，所以未授权命令
        // 在这里和发布构建中一样被拒绝。
        .build(tauri::generate_context!())
        .expect("mock app")
}

fn invoke(
    webview: &tauri::WebviewWindow<tauri::test::MockRuntime>,
    command: &str,
    body: serde_json::Value,
) -> serde_json::Value {
    invoke_from(webview, command, body, "tauri://localhost")
}

fn invoke_from(
    webview: &tauri::WebviewWindow<tauri::test::MockRuntime>,
    command: &str,
    body: serde_json::Value,
    url: &str,
) -> serde_json::Value {
    let response = tauri::test::get_ipc_response(
        webview,
        InvokeRequest {
            cmd: command.into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: url.parse().expect("invoke url"),
            body: tauri::ipc::InvokeBody::Json(body),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_string(),
        },
    );
    match response {
        Ok(body) => body.deserialize::<serde_json::Value>().expect("json body"),
        Err(error) => panic!("{command} rejected: {error:?}"),
    }
}

#[test]
fn authorized_commands_create_list_and_report_runtime_state() {
    let directory = tempdir().expect("temporary directory");
    let app = app(&directory);
    let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("webview");

    let created = invoke(
        &webview,
        "profiles_create",
        json!({ "name": "ipc-profile", "mode": "headful" }),
    );
    let id = created["id"].as_str().expect("profile id").to_owned();
    assert_eq!(created["name"], "ipc-profile");
    assert_eq!(created["direct"], true);
    assert_eq!(created["isolation"], "unknown");

    let listed = invoke(
        &webview,
        "profiles_list",
        json!({ "limit": 10, "status": "active" }),
    );
    assert_eq!(listed["items"].as_array().expect("items").len(), 1);
    assert_eq!(listed["items"][0]["id"], id);

    let fetched = invoke(&webview, "profiles_get", json!({ "id": id }));
    assert_eq!(fetched["id"], id);

    let status = invoke(&webview, "runtime_status", json!({ "profileId": id }));
    assert_eq!(status["state"], "stopped");
    assert_eq!(status["isolation"], "unknown");

    let stopped = invoke(&webview, "runtime_stop", json!({ "profileId": id }));
    assert_eq!(stopped["state"], "stopped");

    let info = invoke(&webview, "app_info", json!({}));
    assert_eq!(info["schemaVersion"], 8);
}

/// 未授权命令必须在 IPC 层被拒绝；错误码契约也必须在同一条链路上成立。
#[test]
fn command_errors_carry_the_stable_code() {
    let directory = tempdir().expect("temporary directory");
    let app = app(&directory);
    let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("webview");

    let response = tauri::test::get_ipc_response(
        &webview,
        InvokeRequest {
            cmd: "profiles_get".into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: "tauri://localhost".parse().expect("local url"),
            body: tauri::ipc::InvokeBody::Json(json!({ "id": "01J0000000000000000000000" })),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_string(),
        },
    )
    .expect_err("missing profile must fail");
    let text = format!("{response:?}");
    assert!(
        text.contains("PROFILE_NOT_FOUND"),
        "error payload must carry the stable code: {text}"
    );
}

/// `cargo run` 的 dev 外壳来自 `devUrl`，Tauri 同样视为 local；授权必须覆盖它，
/// 否则本机验收会只有空态。
#[test]
fn dev_origin_is_authorized_like_the_packaged_origin() {
    let directory = tempdir().expect("temporary directory");
    let app = app(&directory);
    let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("webview");

    let listed = invoke_from(
        &webview,
        "profiles_list",
        json!({ "limit": 10 }),
        "http://localhost:5173/profiles",
    );
    assert_eq!(listed["items"].as_array().expect("items").len(), 0);
}

/// 真机验收：用 UI 实际发送的 IPC 载荷走完整链路（ACL → 命令 → `StartProfile` →
/// 真内核）。`MIEN_KERNEL_BIN` 未设置时显式跳过。
#[test]
fn ui_payloads_drive_the_real_kernel_lifecycle() {
    let Some(binary) = std::env::var_os("MIEN_KERNEL_BIN").map(std::path::PathBuf::from) else {
        eprintln!("skipped: MIEN_KERNEL_BIN not set");
        return;
    };
    let directory = tempdir().expect("temporary directory");
    let state = state::AppState::open_at_with_kernel(directory.path().join("data"), Some(binary))
        .expect("host state");
    let app = build_app(state);
    let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("webview");

    // 1. 创建：表单默认指纹（含 spoof 字段）必须能通过 manifest 准入。
    let created = invoke(
        &webview,
        "profiles_create",
        json!({
            "name": "ui-acceptance",
            "fingerprint": {
                "schema_version": 1,
                "platform": "macos",
                "platform_version": "14.5",
                "brand": "Edge",
                "brand_version": "148.0.0.0",
                "hardware_concurrency": 12,
                "languages": ["en-US", "en"],
                "timezone": "America/Los_Angeles",
                "window": { "width": 1440, "height": 900 },
                "webrtc": { "disable_non_proxied_udp": true },
                "network": { "doh_mode": "disabled", "ipv6_mode": "block" }
            },
            "mode": "headful"
        }),
    );
    let id = created["id"].as_str().expect("profile id").to_owned();
    assert_eq!(created["direct"], true);

    // 2. 启动：有头窗口出现，disclosures 必须包含 direct 与 unverified 网络控制。
    let started = invoke(&webview, "runtime_start", json!({ "profileId": id }));
    assert_eq!(started["state"], "running");
    let disclosures = started["disclosures"].as_array().expect("disclosures");
    assert!(
        disclosures.iter().any(|value| value
            .as_str()
            .is_some_and(|text| text.contains("direct mode"))),
        "direct mode must be disclosed: {disclosures:?}"
    );
    assert!(
        disclosures.iter().any(|value| value
            .as_str()
            .is_some_and(|text| text.contains("disable_non_proxied_udp is unverified"))),
        "unverified network control must be disclosed: {disclosures:?}"
    );
    println!(
        "start: state={} isolation={} reduced_assurance={} disclosures={disclosures:?}",
        started["state"], started["isolation"], started["reducedAssurance"]
    );

    let status = invoke(&webview, "runtime_status", json!({ "profileId": id }));
    assert_eq!(status["state"], "running");
    let pid = status["pid"].as_u64().expect("running pid") as u32;
    assert!(!status["isolation"].is_null());

    // 3. 停止：进程消失，状态回到 stopped。
    let stopped = invoke(&webview, "runtime_stop", json!({ "profileId": id }));
    assert_eq!(stopped["state"], "stopped");
    let alive = std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    assert!(!alive, "stop must reap the kernel process");

    // 4. 再次启动成功。
    let restarted = invoke(&webview, "runtime_start", json!({ "profileId": id }));
    assert_eq!(restarted["state"], "running");
    let stopped = invoke(&webview, "runtime_stop", json!({ "profileId": id }));
    assert_eq!(stopped["state"], "stopped");

    // 列表也要反映最终状态。
    let listed = invoke(&webview, "profiles_list", json!({ "limit": 10 }));
    assert_eq!(listed["items"][0]["id"], id);
    assert_eq!(listed["items"][0]["runtime"]["state"], "stopped");
}

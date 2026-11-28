//! Mien 桌面宿主：窗口、IPC、钥匙串、自动更新的装配点（`docs/ARCHITECTURE.md` §4）。
//!
//! 启动时按 `MIEN_DATA_ROOT`（或 `~/Library/Application Support/Mien`）打开数据库、
//! 跑迁移并装配运行时监督器；任何一个失败都阻止窗口出现，避免半装配状态。
//! 命令适配（`<domain>_<action>`）在 `commands.rs`，权限在 `capabilities/default.json`。

// 发布构建后不弹出 Windows 控制台窗口。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod state;

use tauri::Manager;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "mien_desktop=info,tauri=warn".into()),
        )
        .init();

    tauri::Builder::default()
        .setup(|app| {
            let state = state::AppState::open()?;
            // Reconciliation must run before the first start request: only rows
            // whose command line is attributable are terminated, everything else
            // keeps its port lease (REQ-RUNTIME-012).
            let holder = app.handle().clone();
            let data_root = state.data_root().display().to_string();
            let schema_version = state.schema_version();
            tauri::async_runtime::spawn(async move {
                let state = holder.state::<state::AppState>();
                match state.reconcile().await {
                    Ok(rows) => tracing::info!(
                        data_root,
                        schema_version,
                        reconciled = rows.len(),
                        "runtime reconciliation finished"
                    ),
                    Err(error) => {
                        tracing::warn!(data_root, %error, "runtime reconciliation failed")
                    }
                }
            });
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::kernel_install,
            commands::profiles_list,
            commands::profiles_get,
            commands::profiles_create,
            commands::profiles_update,
            commands::runtime_start,
            commands::runtime_stop,
            commands::runtime_status,
        ])
        .run(tauri::generate_context!())
        .expect("mien-desktop 启动失败");
}

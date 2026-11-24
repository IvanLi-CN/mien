//! Mien 桌面宿主：窗口、IPC、钥匙串、自动更新的装配点（`docs/ARCHITECTURE.md` §4）。
//!
//! 骨架：只起一个加载 Web 壳的主窗口；没有任何业务命令。
//! 命令适配（`<domain>_<action>`）由 T9-A/D/E 按 ADR-0008 追加，并在
//! `capabilities/default.json` 中逐个授权。

// 发布构建后不弹出 Windows 控制台窗口。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod state;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "mien_desktop=info,tauri=warn".into()),
        )
        .init();

    tauri::Builder::default()
        .manage(state::AppState::new())
        .invoke_handler(tauri::generate_handler![commands::app_info])
        .run(tauri::generate_context!())
        .expect("mien-desktop 启动失败");
}

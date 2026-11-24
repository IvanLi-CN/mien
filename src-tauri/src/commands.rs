//! Tauri 命令注册表。每个命令是薄适配器，直接进入 `mien-core` 用例。
//!
//! 命名固定为 `<domain>_<action>`，参数用 `rename_all = "camelCase"`（ADR-0008）。
//! 占位归属：真实命令按域由 T9-A（档案）、T9-C（代理）、T9-D（运行时）、T9-E（首屏）。

use serde::Serialize;

/// 应用版本信息，供壳层首帧展示。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub name: &'static str,
    pub version: &'static str,
    pub schema_version: u32,
}

/// 返回应用名与版本；`schemaVersion` 在未接通数据库时为 0。
#[tauri::command]
pub fn app_info() -> AppInfo {
    AppInfo {
        name: env!("CARGO_PKG_NAME"),
        version: env!("CARGO_PKG_VERSION"),
        schema_version: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::app_info;

    #[test]
    fn app_info_carries_build_identity() {
        let info = app_info();
        assert_eq!(info.name, "mien-desktop");
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(info.schema_version, 0);
    }
}

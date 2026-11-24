//! 宿主状态。核心用例对象在 T9 装配，暂不挂入 Tauri（首个用到它的命令出现时再 `manage`）。

use mien_core::CoreError;
use mien_core::storage::migrations;

/// 进程内共享状态。
#[allow(dead_code, reason = "T9 装配命令后开始读取")]
pub struct AppState {
    /// 数据根目录；尚未确定前为 `None`。
    data_root: Option<std::path::PathBuf>,
}

impl AppState {
    pub fn new() -> Self {
        Self { data_root: None }
    }

    /// 数据根目录。
    #[allow(dead_code, reason = "T9 装配命令后开始读取")]
    pub fn data_root(&self) -> Option<&std::path::Path> {
        self.data_root.as_deref()
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

/// 迁移版本查询，由 `app_info` 返回给壳层；接 T9-A 后换成真实迁移。
///
/// # Errors
/// 数据库不可读或迁移记录损坏。
#[allow(dead_code, reason = "T9-A 起由 app_info 返回")]
pub fn migration_version(database_url: &str) -> Result<migrations::SchemaVersion, CoreError> {
    migrations::current_version(database_url)
}

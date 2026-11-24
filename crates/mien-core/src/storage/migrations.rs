//! SQLite 迁移执行。迁移文件在仓库根 `migrations/`，按整数版本递增。
//!
//! 骨架：`xtask migrate` 已可执行空迁移 `0001_init.sql`；连接模式（WAL、外键、`busy_timeout`）
//! 与备份策略由 T9-A 按 `docs/DATABASE-SCHEMA.md` §1、§5 实现。

use serde::Serialize;

use crate::error::CoreError;

/// 已提交迁移的最大版本号；空库为 0。目前恒返回 [`CoreError::NotImplemented`]，T9-A 接入 sqlx。
///
/// # Errors
/// 数据库不可读或迁移记录损坏。
pub fn current_version(database_url: &str) -> Result<SchemaVersion, CoreError> {
    let _ = database_url;
    Err(CoreError::NotImplemented)
}

/// 迁移版本号；接 T9-A 后由 `schema_migrations` 读出。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SchemaVersion(pub u32);

//! 存储端口与迁移。具体驱动只在 `mien-store-sqlite` 出现。
//!
//! 占位归属：T9-A（仓储实现与事务）。

use crate::CoreError;

pub mod audit;
pub mod migrations;
pub mod profile;
pub mod proxy;
pub mod sqlite;
pub use migrations::{current_version, open_and_migrate};

/// SQLite 错误到领域错误的统一映射。
///
/// `5` 是 `SQLITE_BUSY`、`6` 是 `SQLITE_LOCKED`；两者都是可重试的并发冲突，
/// 其余一律按不透明存储错误上报。此前每个存储模块各复制一份，其中一份漏了 `6`，
/// 同一类锁冲突在档案与代理之间会映射成不同错误码。
pub(crate) fn map_sqlx_error(error: sqlx::Error) -> CoreError {
    match &error {
        sqlx::Error::Database(database)
            if matches!(database.code().as_deref(), Some("5") | Some("6")) =>
        {
            CoreError::StorageBusy
        }
        _ => CoreError::Storage,
    }
}

pub fn now_utc() -> String {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
    let days = secs / 86_400;
    let mut year = 1970i64;
    let mut remaining = days as i64;
    while remaining >= if is_leap(year) { 366 } else { 365 } {
        remaining -= if is_leap(year) { 366 } else { 365 };
        year += 1;
    }
    let month_lengths = [
        31,
        if is_leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 1;
    while remaining >= month_lengths[(month - 1) as usize] {
        remaining -= month_lengths[(month - 1) as usize];
        month += 1;
    }
    let hour = (secs % 86_400) / 3_600;
    let minute = (secs % 3_600) / 60;
    let second = secs % 60;
    format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{:03}Z",
        duration.subsec_millis(),
        day = remaining + 1
    )
}

fn is_leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

//! 存储端口与迁移。具体驱动只在 `mien-store-sqlite` 出现。
//!
//! 占位归属：T9-A（仓储实现与事务）。

pub mod migrations;
pub mod sqlite;

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

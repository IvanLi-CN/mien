//! `event_log` 写入、保留裁剪与回放快照。
//!
//! 契约：`docs/specs/control-api/SPEC.md` `REQ-API-011`、`docs/DATABASE-SCHEMA.md`
//! §2/§3。`sequence` 是 `INTEGER PRIMARY KEY AUTOINCREMENT` 回放游标，`event_id`
//! 只用于去重；保留窗口固定为 7 天且 `payload_json` UTF-8 总量至多 100 MiB，
//! 按最旧 `sequence` 前缀裁剪。
//!
//! `high_water = max(sqlite_sequence.seq, max(event_log.sequence))`，空库为 0；
//! `floor = 最早保留 sequence - 1`，保留集为空时等于 `high_water`。删除行不会
//! 重置 `sqlite_sequence`，因此裁剪与重启后 `sequence` 永不复用，过期游标仍可区分。

use std::time::Duration;

use mien_core::domain::profile::ProfileId;
use serde_json::Value;
use sqlx::{Row, SqlitePool};

use crate::auth::session;

/// 保留窗口：7 天。
pub const RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// 保留总量：`payload_json` 的 UTF-8 字节上限（100 MiB）。
pub const MAX_BYTES: u64 = 100 * 1024 * 1024;
/// 回放分页大小；所有页都在同一个读快照内读取。
const PAGE: i64 = 256;

/// 契约事件类型（`docs/contracts/events.schema.json#/properties/type`）。
pub const EVENT_TYPES: &[&str] = &[
    "runtime.starting",
    "runtime.running",
    "runtime.stopping",
    "runtime.stopped",
    "runtime.crashed",
    "runtime.failed",
    "proxy.engine.failed",
    "proxy.engine.rebuilt",
    "proxy.check.completed",
    "profile.created",
    "profile.updated",
    "profile.archived",
    "profile.restored",
    "profile.deleted",
    "profile.purged",
    "profile.duplicated",
    "kernel.installed",
    "kernel.activated",
    "engine.installed",
    "engine.activated",
    "audit.recorded",
    "auth.login",
    "auth.logout",
    "token.created",
    "token.revoked",
];

/// 保留策略；生产默认 7 天/100 MiB，测试可注入更小的窗口。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventLimits {
    pub retention: Duration,
    pub max_bytes: u64,
}

impl Default for EventLimits {
    fn default() -> Self {
        Self {
            retention: RETENTION,
            max_bytes: MAX_BYTES,
        }
    }
}

/// 一条已提交事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRecord {
    pub sequence: i64,
    pub event_id: String,
    pub kind: String,
    pub payload: Value,
    pub created_at: String,
}

impl EventRecord {
    /// WebSocket 文本帧内容：`docs/contracts/events.schema.json` 的信封。
    pub fn envelope_json(&self) -> String {
        serde_json::json!({
            "schema_version": "1",
            "id": &self.event_id,
            "sequence": self.sequence,
            "type": &self.kind,
            "ts": &self.created_at,
            "payload": &self.payload,
        })
        .to_string()
    }
}

/// 回放快照：`floor`/`high_water` 与 `after` 之后的保留事件来自同一个读事务。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub floor: i64,
    pub high_water: i64,
    pub replay: Vec<EventRecord>,
}

/// 存储层失败；由传输层映射为稳定错误码。
#[derive(Debug)]
pub enum StoreError {
    /// 单条事件超过保留预算。
    TooLarge {
        bytes: u64,
        limit: u64,
    },
    /// 类型不在契约枚举中。
    InvalidKind,
    /// payload 不是 JSON 对象或已损坏。
    InvalidPayload,
    /// `after` 早于保留窗口。
    ReplayExpired {
        floor: i64,
        high_water: i64,
    },
    /// `after` 大于 `high_water`。
    FutureCursor {
        high_water: i64,
    },
    Sqlx(sqlx::Error),
}

impl From<sqlx::Error> for StoreError {
    fn from(error: sqlx::Error) -> Self {
        Self::Sqlx(error)
    }
}

/// 追加一条事件：写入后立即裁剪最旧前缀，返回已提交记录。
///
/// # Errors
/// 类型不在契约枚举、payload 非对象、单条超过预算或数据库写入失败。
pub async fn append(
    pool: &SqlitePool,
    limits: &EventLimits,
    kind: &str,
    payload: &Value,
) -> Result<EventRecord, StoreError> {
    if !EVENT_TYPES.contains(&kind) {
        return Err(StoreError::InvalidKind);
    }
    if !payload.is_object() {
        return Err(StoreError::InvalidPayload);
    }
    let payload_json = payload.to_string();
    let bytes = payload_json.len() as u64;
    if bytes > limits.max_bytes {
        return Err(StoreError::TooLarge {
            bytes,
            limit: limits.max_bytes,
        });
    }
    let event_id = ProfileId::new().0;
    let created_at = session::now_timestamp();
    let inserted = sqlx::query(
        "INSERT INTO event_log (event_id, type, payload_json, created_at) \
         VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(&event_id)
    .bind(kind)
    .bind(&payload_json)
    .bind(&created_at)
    .execute(pool)
    .await?;
    let sequence = inserted.last_insert_rowid();
    prune(pool, limits).await?;
    Ok(EventRecord {
        sequence,
        event_id,
        kind: kind.to_owned(),
        payload: payload.clone(),
        created_at,
    })
}

/// 按最旧 `sequence` 前缀裁剪，直到 7 天与字节预算同时成立。
///
/// 删除行不会重置 `sqlite_sequence`；保留集为空时 `high_water` 仍保留最后序号。
///
/// # Errors
/// 数据库读写失败。
pub async fn prune(pool: &SqlitePool, limits: &EventLimits) -> Result<u64, StoreError> {
    let mut transaction = pool.begin().await?;
    let cutoff: String = sqlx::query_scalar("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now', ?1)")
        .bind(format!("-{} seconds", limits.retention.as_secs()))
        .fetch_one(&mut *transaction)
        .await?;
    let rows = sqlx::query(
        "SELECT sequence, created_at, length(CAST(payload_json AS BLOB)) AS bytes \
         FROM event_log ORDER BY sequence ASC",
    )
    .fetch_all(&mut *transaction)
    .await?;

    let mut entries = Vec::with_capacity(rows.len());
    let mut total = 0u64;
    for row in &rows {
        let bytes = row.get::<i64, _>("bytes").max(0) as u64;
        total += bytes;
        entries.push((
            row.get::<i64, _>("sequence"),
            row.get::<String, _>("created_at"),
            bytes,
        ));
    }

    let mut first_kept = 0;
    while first_kept < entries.len() {
        let (_, created_at, bytes) = &entries[first_kept];
        let expired = created_at.as_str() < cutoff.as_str();
        if expired || total > limits.max_bytes {
            total = total.saturating_sub(*bytes);
            first_kept += 1;
        } else {
            break;
        }
    }

    let deleted = if first_kept == 0 {
        0
    } else if first_kept == entries.len() {
        sqlx::query("DELETE FROM event_log")
            .execute(&mut *transaction)
            .await?
            .rows_affected()
    } else {
        sqlx::query("DELETE FROM event_log WHERE sequence < ?1")
            .bind(entries[first_kept].0)
            .execute(&mut *transaction)
            .await?
            .rows_affected()
    };
    transaction.commit().await?;
    Ok(deleted)
}

/// 读取 `floor`/`high_water` 与 `after` 之后的全部保留事件。
///
/// 快照之前先裁剪；`floor`/`high_water` 与分页读取在同一个只读事务中完成，
/// 因此并发写入不会让回放与实时衔接出现空洞。
///
/// # Errors
/// `after` 早于 `floor`、大于 `high_water`，或数据库读写失败。
pub async fn snapshot(
    pool: &SqlitePool,
    limits: &EventLimits,
    after: Option<i64>,
) -> Result<Snapshot, StoreError> {
    prune(pool, limits).await?;
    let mut transaction = pool.begin().await?;
    let sequence: i64 = sqlx::query_scalar(
        "SELECT COALESCE((SELECT seq FROM sqlite_sequence WHERE name = 'event_log'), 0)",
    )
    .fetch_one(&mut *transaction)
    .await?;
    let highest: Option<i64> = sqlx::query_scalar("SELECT max(sequence) FROM event_log")
        .fetch_one(&mut *transaction)
        .await?;
    let high_water = sequence.max(highest.unwrap_or(0));
    let earliest: Option<i64> = sqlx::query_scalar("SELECT min(sequence) FROM event_log")
        .fetch_one(&mut *transaction)
        .await?;
    let floor = earliest.map_or(high_water, |value| value - 1);

    let mut replay = Vec::new();
    if let Some(after) = after {
        if after > high_water {
            return Err(StoreError::FutureCursor { high_water });
        }
        if after < floor {
            return Err(StoreError::ReplayExpired { floor, high_water });
        }
        let mut cursor = after;
        loop {
            let rows = sqlx::query(
                "SELECT sequence, event_id, type, payload_json, created_at \
                 FROM event_log WHERE sequence > ?1 ORDER BY sequence ASC LIMIT ?2",
            )
            .bind(cursor)
            .bind(PAGE)
            .fetch_all(&mut *transaction)
            .await?;
            if rows.is_empty() {
                break;
            }
            for row in rows {
                let record = record_from_row(&row)?;
                cursor = record.sequence;
                replay.push(record);
            }
        }
    }
    transaction.commit().await?;
    Ok(Snapshot {
        floor,
        high_water,
        replay,
    })
}

/// 实时页；`floor` 与事件来自同一个读事务。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LivePage {
    pub floor: i64,
    pub events: Vec<EventRecord>,
}

impl LivePage {
    /// 游标与保留前缀是否已断开：`floor > cursor` 表示 `cursor+1` 已被裁剪。
    ///
    /// 调用方必须据此让客户端重新同步，而不是静默跳过保留窗口外的事件。
    pub fn has_gap(&self, cursor: i64) -> bool {
        self.floor > cursor
    }
}

/// 实时衔接：读取 `sequence > after` 的一页事件，同时给出当前 `floor`。
///
/// # Errors
/// 数据库读取失败或行损坏。
pub async fn live_page(pool: &SqlitePool, after: i64, page: i64) -> Result<LivePage, StoreError> {
    let mut transaction = pool.begin().await?;
    let sequence: i64 = sqlx::query_scalar(
        "SELECT COALESCE((SELECT seq FROM sqlite_sequence WHERE name = 'event_log'), 0)",
    )
    .fetch_one(&mut *transaction)
    .await?;
    let earliest: Option<i64> = sqlx::query_scalar("SELECT min(sequence) FROM event_log")
        .fetch_one(&mut *transaction)
        .await?;
    let floor = earliest.map_or(sequence, |value| value - 1);
    let rows = sqlx::query(
        "SELECT sequence, event_id, type, payload_json, created_at \
         FROM event_log WHERE sequence > ?1 ORDER BY sequence ASC LIMIT ?2",
    )
    .bind(after)
    .bind(page.max(1))
    .fetch_all(&mut *transaction)
    .await?;
    let events = rows
        .iter()
        .map(record_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    transaction.commit().await?;
    Ok(LivePage { floor, events })
}

fn record_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<EventRecord, StoreError> {
    let payload_json: String = row.get("payload_json");
    Ok(EventRecord {
        sequence: row.get("sequence"),
        event_id: row.get("event_id"),
        kind: row.get("type"),
        payload: serde_json::from_str(&payload_json).map_err(|_| StoreError::InvalidPayload)?,
        created_at: row.get("created_at"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_types_match_the_contract_catalogue() {
        // 顺序与 `docs/contracts/events.schema.json#/properties/type/enum` 一致。
        assert_eq!(EVENT_TYPES.len(), 25);
        assert!(EVENT_TYPES.contains(&"runtime.starting"));
        assert!(EVENT_TYPES.contains(&"token.revoked"));
    }

    #[test]
    fn default_limits_match_the_contract_window() {
        let limits = EventLimits::default();
        assert_eq!(limits.retention, Duration::from_secs(7 * 24 * 60 * 60));
        assert_eq!(limits.max_bytes, 100 * 1024 * 1024);
    }
}

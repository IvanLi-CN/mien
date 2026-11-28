//! 审计事件读取（`REQ-API-009`）。
//!
//! 写入在服务端（`mien_server::auth::record_audit`），读取放在这里是因为列表端点
//! 需要**稳定排序 + 二元组游标**，那是存储层语义而不是传输层语义。
//!
//! 排序固定为 `created_at DESC, id DESC`：`created_at` 是毫秒级 UTC 文本，
//! 同一毫秒内可能有多行；只按时间分页会重复或丢行，所以 `id` 必须做决胜键，
//! 游标也必须携带两者。

use serde_json::Value;
use sqlx::{Row as _, SqlitePool};

use crate::error::ErrorCode;
use crate::{CoreError, storage::map_sqlx_error};

/// 审计列表筛选；`None` 表示不限制。
#[derive(Debug, Clone, Default)]
pub struct AuditFilter {
    pub action: Option<String>,
    pub event_type: Option<String>,
    pub profile_id: Option<String>,
    /// 含边界（`>=`），RFC3339 UTC 文本。
    pub from: Option<String>,
    /// 含边界（`<=`），RFC3339 UTC 文本。
    pub to: Option<String>,
}

/// 分页游标：上一页最后一行的 `created_at` 与 `id`。
#[derive(Debug, Clone)]
pub struct AuditCursor {
    pub created_at: String,
    pub id: String,
}

/// 一行审计事件；`metadata` 已解析，损坏的历史行退化为空对象。
#[derive(Debug, Clone)]
pub struct AuditRow {
    pub id: String,
    pub actor: String,
    pub event_type: String,
    pub profile_id: Option<String>,
    pub action: String,
    pub target: String,
    pub result: String,
    pub request_id: String,
    pub metadata: Value,
    pub created_at: String,
}

/// 一页结果与是否还有更多。
#[derive(Debug, Clone)]
pub struct AuditPage {
    pub rows: Vec<AuditRow>,
    pub has_more: bool,
}

/// 把一行解成 [`AuditRow`]。
///
/// `metadata_json` 保持字符串直读，避免一行坏数据让整页 500；契约要求
/// `metadata` 是对象，历史坏行按空对象处理。
fn decode_row(row: &sqlx::sqlite::SqliteRow) -> AuditRow {
    let metadata = serde_json::from_str::<Value>(&row.get::<String, _>("metadata_json"))
        .ok()
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    AuditRow {
        id: row.get("id"),
        actor: row.get("actor"),
        event_type: row.get("event_type"),
        profile_id: row.get("profile_id"),
        action: row.get("action"),
        target: row.get("target"),
        result: row.get("result"),
        request_id: row.get("request_id"),
        metadata,
        created_at: row.get("created_at"),
    }
}

/// 读取一页审计事件。
///
/// `limit` 由调用方校验（1..=200）；这里仍然读 `limit + 1` 行以判定 `has_more`，
/// 因此不需要 `COUNT(*)`，翻页成本与页大小成正比。
pub async fn list(
    pool: &SqlitePool,
    filter: &AuditFilter,
    cursor: Option<&AuditCursor>,
    limit: u32,
) -> Result<AuditPage, CoreError> {
    if limit == 0 {
        return Err(CoreError::validation(
            ErrorCode::ValidationFailed,
            "limit must be at least 1",
        ));
    }
    let cursor_created = cursor.map(|value| value.created_at.clone());
    let cursor_id = cursor.map(|value| value.id.clone());
    let rows = sqlx::query(
        "SELECT id, actor, event_type, profile_id, action, target, result, request_id, \
                metadata_json, created_at \
         FROM audit_events \
         WHERE (?1 IS NULL OR action = ?1) \
           AND (?2 IS NULL OR event_type = ?2) \
           AND (?3 IS NULL OR profile_id = ?3) \
           AND (?4 IS NULL OR created_at >= ?4) \
           AND (?5 IS NULL OR created_at <= ?5) \
           AND (?6 IS NULL OR (created_at, id) < (?6, ?7)) \
         ORDER BY created_at DESC, id DESC \
         LIMIT ?8",
    )
    .bind(filter.action.clone())
    .bind(filter.event_type.clone())
    .bind(filter.profile_id.clone())
    .bind(filter.from.clone())
    .bind(filter.to.clone())
    .bind(cursor_created)
    .bind(cursor_id)
    .bind(i64::from(limit) + 1)
    .fetch_all(pool)
    .await
    .map_err(map_sqlx_error)?;

    let has_more = rows.len() > limit as usize;
    Ok(AuditPage {
        rows: rows.iter().take(limit as usize).map(decode_row).collect(),
        has_more,
    })
}

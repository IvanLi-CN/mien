//! 审计事件列表（`REQ-API-009`、`auditList`）。
//!
//! 排序固定 `created_at DESC, id DESC`；游标携带二元组，因此翻页无重复、无遗漏。
//! 筛选字段名与 UI 活动页共用同一套名字：`event_type`/`profile_id`/`action`/`from`/`to`。
//!
//! 读取实现在 `mien_core::storage::audit`；本模块只做传输层：参数解析、scope 断言、
//! 响应形状。`AuditEvent` 必须同时含 `event_type` 与 `profile_id`（可为 null），
//! `action` 仅为兼容保留。

use std::collections::BTreeMap;

use axum::Router;
use axum::extract::{Extension, State};
use axum::http::Uri;
use axum::response::Response;
use axum::routing::get;
use serde::Serialize;
use serde_json::Value;

use crate::RequestId;
use crate::auth::Principal;
use crate::profiles::{ApiError, ok};
use crate::query::{self, Cursor};
use crate::state::AppState;

/// 审计读操作 scope（`openapi.yaml#/x-required-scope`）。
pub const SCOPE_READ: &str = "audit.read";

/// 页大小上限，与档案/代理列表一致。
const MAX_LIMIT: u32 = 200;
const DEFAULT_LIMIT: u32 = 50;

/// 支持的筛选字段；未知字段是 422 而不是静默全量。
const FILTERS: &[&str] = &[
    "cursor",
    "limit",
    "action",
    "event_type",
    "profile_id",
    "from",
    "to",
];

/// `GET /api/v1/audit/events`。
pub fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/audit/events", get(list))
}

/// 契约 `AuditEvent`。
#[derive(Debug, Serialize)]
pub struct AuditEvent {
    id: String,
    actor: String,
    action: String,
    target: String,
    result: String,
    request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<Value>,
    created_at: String,
    event_type: String,
    profile_id: Option<String>,
}

/// 契约 `auditList` 响应。
#[derive(Debug, Serialize)]
pub struct AuditPage {
    items: Vec<AuditEvent>,
    next_cursor: Option<String>,
}

/// 解析后的查询。
#[derive(Debug)]
struct Query {
    limit: u32,
    cursor: Option<Cursor>,
    action: Option<String>,
    event_type: Option<String>,
    profile_id: Option<String>,
    from: Option<String>,
    to: Option<String>,
}

async fn list(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Extension(request_id): Extension<RequestId>,
    uri: Uri,
) -> Response {
    if let Err(error) = require_scope(&principal) {
        return error.into_response(&request_id);
    }
    let params = match query::query_map(&uri) {
        Ok(params) => params,
        Err(error) => return error.into_response(&request_id),
    };
    let query = match parse(&params) {
        Ok(query) => query,
        Err(error) => return error.into_response(&request_id),
    };
    let Some(pool) = state.db() else {
        return ApiError::internal().into_response(&request_id);
    };
    let filter = mien_core::storage::audit::AuditFilter {
        action: query.action,
        event_type: query.event_type,
        profile_id: query.profile_id,
        from: query.from,
        to: query.to,
    };
    let cursor = query
        .cursor
        .as_ref()
        .map(|cursor| mien_core::storage::audit::AuditCursor {
            created_at: cursor.key.clone(),
            id: cursor.id.clone(),
        });
    match mien_core::storage::audit::list(pool, &filter, cursor.as_ref(), query.limit).await {
        Ok(page) => ok(&AuditPage {
            next_cursor: page
                .has_more
                .then(|| {
                    page.rows
                        .last()
                        .map(|row| query::encode_cursor(&row.created_at, &row.id))
                })
                .flatten(),
            items: page
                .rows
                .into_iter()
                .map(|row| AuditEvent {
                    id: row.id,
                    actor: row.actor,
                    action: row.action,
                    target: row.target,
                    result: row.result,
                    request_id: row.request_id,
                    metadata: Some(row.metadata),
                    created_at: row.created_at,
                    event_type: row.event_type,
                    profile_id: row.profile_id,
                })
                .collect(),
        }),
        Err(error) => ApiError::from_core(error).into_response(&request_id),
    }
}

fn require_scope(principal: &Principal) -> Result<(), ApiError> {
    principal
        .require_scope(SCOPE_READ)
        .map_err(|error| ApiError {
            status: error.status,
            code: error.code,
            message: error.message.to_owned(),
            retryable: error.retryable,
            details: error.details,
        })
}

fn parse(params: &BTreeMap<String, String>) -> Result<Query, ApiError> {
    query::reject_unknown(params, FILTERS, "audit")?;
    let limit = query::parse_limit(params, DEFAULT_LIMIT, MAX_LIMIT)?;
    let cursor = match params.get("cursor") {
        Some(raw) if !raw.is_empty() => Some(query::decode_cursor(raw, is_ulid)?),
        _ => None,
    };
    let profile_id = non_empty(params, "profile_id");
    if let Some(id) = &profile_id {
        if !is_ulid(id) {
            return Err(ApiError::validation("profile_id", "must be a valid ULID"));
        }
    }
    let from = non_empty(params, "from");
    if let Some(value) = &from {
        validate_timestamp(value, "from")?;
    }
    let to = non_empty(params, "to");
    if let Some(value) = &to {
        validate_timestamp(value, "to")?;
    }
    if let (Some(from), Some(to)) = (&from, &to)
        && from > to
    {
        return Err(ApiError::validation("from", "must not be later than `to`"));
    }
    Ok(Query {
        limit,
        cursor,
        action: non_empty(params, "action"),
        event_type: non_empty(params, "event_type"),
        profile_id,
        from,
        to,
    })
}

fn non_empty(params: &BTreeMap<String, String>, key: &str) -> Option<String> {
    params.get(key).cloned().filter(|value| !value.is_empty())
}

fn is_ulid(value: &str) -> bool {
    mien_core::domain::profile::ProfileId::from_string(value).is_valid()
}

/// RFC 3339 UTC 时间戳的**可比较**形态校验。
///
/// 库里 `created_at` 是定宽 UTC 文本，字典序即时间序；因此这里既校验形状，
/// 也要求定宽 `YYYY-MM-DDTHH:MM:SS[.fff]Z`，避免客户端拿到"能解析但排序不同"
/// 的偏移量形态（`+08:00`）后筛出错误区间。
fn validate_timestamp(value: &str, field: &str) -> Result<(), ApiError> {
    let bytes = value.as_bytes();
    let shaped = bytes.len() >= 20
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && matches!(bytes[10], b'T' | b't')
        && bytes[13] == b':'
        && bytes[16] == b':'
        && value.ends_with('Z')
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[8..10].iter().all(u8::is_ascii_digit)
        && bytes[11..13].iter().all(u8::is_ascii_digit)
        && bytes[14..16].iter().all(u8::is_ascii_digit)
        && bytes[17..19].iter().all(u8::is_ascii_digit)
        && (bytes.len() == 20
            || (bytes[19] == b'.' && bytes[20..bytes.len() - 1].iter().all(u8::is_ascii_digit)));
    if !shaped {
        return Err(ApiError::validation(
            field,
            "must be an RFC 3339 UTC timestamp ending in Z",
        ));
    }
    let month = value[5..7].parse::<u32>().unwrap_or(0);
    let day = value[8..10].parse::<u32>().unwrap_or(0);
    let hour = value[11..13].parse::<u32>().unwrap_or(0);
    let minute = value[14..16].parse::<u32>().unwrap_or(0);
    let second = value[17..19].parse::<u32>().unwrap_or(0);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return Err(ApiError::validation(field, "is not a real UTC timestamp"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn unknown_filters_and_bad_limits_are_rejected() {
        assert!(parse(&params(&[("foo", "1")])).is_err());
        let error = parse(&params(&[("limit", "201")])).expect_err("cap");
        assert!(format!("{error:?}").contains("200"));
        assert!(parse(&params(&[("limit", "0")])).is_err());
    }

    #[test]
    fn profile_ids_and_ranges_are_validated() {
        assert!(parse(&params(&[("profile_id", "nope")])).is_err());
        assert!(parse(&params(&[("from", "2026-13-01T00:00:00Z")])).is_err());
        assert!(parse(&params(&[("from", "2026-01-01T00:00:00+08:00")])).is_err());
        assert!(
            parse(&params(&[
                ("from", "2026-11-27T10:00:00Z"),
                ("to", "2026-11-27T09:00:00Z"),
            ]))
            .is_err()
        );
        assert!(
            parse(&params(&[
                ("from", "2026-11-27T09:00:00Z"),
                ("to", "2026-11-27T10:00:00.123Z"),
            ]))
            .is_ok()
        );
    }

    #[test]
    fn cursors_must_carry_a_ulid() {
        let encoded = query::encode_cursor("2026-11-27T10:00:00Z", "01JC5W3X6Z8N4M2P9QR7STVWXY");
        assert!(parse(&params(&[("cursor", encoded.as_str())])).is_ok());
        assert!(parse(&params(&[("cursor", "zz")])).is_err());
    }
}

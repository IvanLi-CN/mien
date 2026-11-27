//! `Idempotency-Key` 缓存（`REQ-API-010`）。
//!
//! 语义：
//! - 同一凭据、同一键、同一方法/路径/请求体：回放原始状态码与响应体。
//! - 同一键但请求不同：`409 IDEMPOTENCY_CONFLICT`。
//! - 过期键（`expires_at` 已过）可以重新使用；读取时顺带清理。
//! - 只有成功响应被缓存；执行中的请求用短认领行占位，避免并发重复执行。
//!   `tokensCreate` 这类含一次性秘密的响应不经过本模块：它们必须永远 409。

use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};

use super::ApiError;
use crate::RequestId;

/// 幂等键头部（`docs/contracts/openapi.yaml#/components/parameters/IdempotencyKey`）。
pub const IDEMPOTENCY_HEADER: &str = "idempotency-key";
/// 键长度下限。
pub const MIN_KEY_LEN: usize = 8;
/// 键长度上限。
pub const MAX_KEY_LEN: usize = 128;
/// 完成后的保留时长（秒）；过期键可重新使用。
pub const RETENTION_SECONDS: u64 = 24 * 60 * 60;
/// 执行中认领的时长（秒）；进程崩溃后最多阻塞这么久。
const CLAIM_SECONDS: u64 = 60;
/// 认领行的状态码哨兵；真实响应状态码不会是 0。
///
/// `0006_api_tokens.sql` 的 `status` 列可为 NULL（T23 的 tokensCreate 只用
/// `(key, scope, request_hash)` 占位），因此认领写 NULL，完成时才写真实状态码。
const CLAIMED_STATUS: Option<i64> = None;

/// 一个已校验的幂等键。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key(String);

impl Key {
    /// 校验客户端给出的键；缺失时返回 `None`。
    ///
    /// # Errors
    /// 长度不在 8..=128 内或包含非可打印 ASCII。
    pub fn parse(raw: Option<&str>) -> Result<Option<Self>, ApiError> {
        let Some(raw) = raw else {
            return Ok(None);
        };
        if raw.len() < MIN_KEY_LEN
            || raw.len() > MAX_KEY_LEN
            || !raw
                .bytes()
                .all(|byte| byte.is_ascii_graphic() && byte != b' ')
        {
            return Err(ApiError::validation(
                "Idempotency-Key",
                "must be 8-128 printable ASCII characters",
            ));
        }
        Ok(Some(Self(raw.to_owned())))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 一次请求的身份：方法、路径（含查询串）与请求体摘要。
#[derive(Debug, Clone)]
pub struct RequestFingerprint {
    pub method: String,
    pub path: String,
    pub hash: String,
}

impl RequestFingerprint {
    /// 从请求要素计算摘要；不记录请求体原文。
    pub fn new(method: &str, path: &str, body: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(body);
        Self {
            method: method.to_owned(),
            path: path.to_owned(),
            hash: hex(&hasher.finalize()),
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

/// 一次幂等查询的结果。
#[derive(Debug)]
pub enum Outcome {
    /// 首次执行；调用方需在完成后调用 [`complete`]。
    Execute,
    /// 命中缓存；原样回放。
    Replay { status: u16, body: String },
    /// 同一键仍在执行中；客户端稍后重试。
    InFlight,
}

/// 认领键并判定是否回放。
///
/// # Errors
/// 存储不可读，或同一键携带了不同请求。
pub async fn begin(
    pool: &SqlitePool,
    scope: &str,
    key: &Key,
    request: &RequestFingerprint,
) -> Result<Outcome, ApiError> {
    // 先清理过期行：过期键可以重新使用。
    sqlx::query(
        "DELETE FROM idempotency_keys WHERE expires_at <= strftime('%Y-%m-%dT%H:%M:%fZ','now')",
    )
    .execute(pool)
    .await
    .map_err(|error| ApiError::storage(&error))?;

    let existing = sqlx::query(
        "SELECT method,path,request_hash,status,response_json FROM idempotency_keys WHERE scope=?1 AND key=?2",
    )
    .bind(scope)
    .bind(key.as_str())
    .fetch_optional(pool)
    .await
    .map_err(|error| ApiError::storage(&error))?;

    let Some(row) = existing else {
        claim(pool, scope, key, request).await?;
        return Ok(Outcome::Execute);
    };

    // `method`/`path` 是 0007 新增列：T23 的 tokensCreate 占位行没有它们。
    // 那些行属于不同 scope，正常不会命中这里；若命中，视为不同的请求。
    let method: Option<String> = row
        .try_get("method")
        .map_err(|error| ApiError::storage(&error))?;
    let path: Option<String> = row
        .try_get("path")
        .map_err(|error| ApiError::storage(&error))?;
    let hash: String = row
        .try_get("request_hash")
        .map_err(|error| ApiError::storage(&error))?;
    if method.as_deref() != Some(request.method.as_str())
        || path.as_deref() != Some(request.path.as_str())
        || hash != request.hash
    {
        return Err(ApiError::idempotency_conflict());
    }
    let status: Option<i64> = row
        .try_get("status")
        .map_err(|error| ApiError::storage(&error))?;
    let Some(status) = status else {
        return Ok(Outcome::InFlight);
    };
    let body: Option<String> = row
        .try_get("response_json")
        .map_err(|error| ApiError::storage(&error))?;
    Ok(Outcome::Replay {
        status: u16::try_from(status).unwrap_or(500),
        body: body.unwrap_or_default(),
    })
}

async fn claim(
    pool: &SqlitePool,
    scope: &str,
    key: &Key,
    request: &RequestFingerprint,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO idempotency_keys (key,scope,method,path,request_hash,status,response_json,created_at,expires_at) \
         VALUES (?1,?2,?3,?4,?5,?6,NULL,strftime('%Y-%m-%dT%H:%M:%fZ','now'), \
                 strftime('%Y-%m-%dT%H:%M:%fZ','now','+'||?7||' seconds'))",
    )
    .bind(key.as_str())
    .bind(scope)
    .bind(&request.method)
    .bind(&request.path)
    .bind(&request.hash)
    .bind(CLAIMED_STATUS)
    .bind(CLAIM_SECONDS as i64)
    .execute(pool)
    .await
    .map_err(|error| ApiError::storage(&error))?;
    Ok(())
}

/// 记录成功响应，释放认领并开始保留窗口。
///
/// 只缓存 2xx；失败响应不缓存，客户端可以用同一键重试。
///
/// # Errors
/// 存储不可读。
pub async fn complete(
    pool: &SqlitePool,
    scope: &str,
    key: &Key,
    status: u16,
    body: &str,
) -> Result<(), ApiError> {
    if !(200..300).contains(&status) {
        release(pool, scope, key).await?;
        return Ok(());
    }
    sqlx::query(
        "UPDATE idempotency_keys SET status=?3, response_json=?4, \
         expires_at=strftime('%Y-%m-%dT%H:%M:%fZ','now','+'||?5||' seconds') \
         WHERE scope=?1 AND key=?2",
    )
    .bind(scope)
    .bind(key.as_str())
    .bind(i64::from(status))
    .bind(body)
    .bind(RETENTION_SECONDS as i64)
    .execute(pool)
    .await
    .map_err(|error| ApiError::storage(&error))?;
    Ok(())
}

/// 放弃认领；失败响应不占用键。
///
/// # Errors
/// 存储不可读。
pub async fn release(pool: &SqlitePool, scope: &str, key: &Key) -> Result<(), ApiError> {
    sqlx::query("DELETE FROM idempotency_keys WHERE scope=?1 AND key=?2 AND status IS NULL")
        .bind(scope)
        .bind(key.as_str())
        .execute(pool)
        .await
        .map_err(|error| ApiError::storage(&error))?;
    Ok(())
}

/// 回放缓存响应；`request_id` 使用当前请求，便于关联重试。
pub fn replay(request_id: &RequestId, status: u16, body: &str) -> axum::response::Response {
    let status = axum::http::StatusCode::from_u16(status)
        .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    let mut response = axum::response::Response::new(axum::body::Body::from(body.to_owned()));
    *response.status_mut() = status;
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/json"),
    );
    if let Ok(value) = axum::http::HeaderValue::from_str(request_id.as_str()) {
        response.headers_mut().insert(
            axum::http::HeaderName::from_static(crate::REQUEST_ID_HEADER),
            value,
        );
    }
    response
}

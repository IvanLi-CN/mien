//! API Token 端点：列表、创建与撤销（`REQ-API-005`、`REQ-API-010`）。
//!
//! 认证门禁（`crate::auth::enforce`）负责 401/403 与 `tokens.manage` scope 检查；本模块
//! 只处理请求体校验、权限授予上限、一次性明文返回与审计。响应中除创建响应的
//! `token` 字段外不含任何秘密；日志与错误消息同样不记录明文、哈希或 pepper。

pub mod scope;
pub mod store;

use std::sync::Arc;

use axum::Json;
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::RequestId;
use crate::auth::{AuditEvent, AuthError, Principal, record_audit};
use crate::state::AppState;
use crate::tokens::scope::TokenScope;
use crate::tokens::store::{Issued, TokenRecord, TokenStore};

/// 请求体：`TokenInput`，`additionalProperties: false`。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenInput {
    pub name: String,
    pub scopes: Vec<TokenScope>,
}

/// 幂等键请求头；与 OpenAPI `IdempotencyKey` 一致。
pub const IDEMPOTENCY_KEY_HEADER: &str = "idempotency-key";
/// 幂等键长度下限。
const IDEMPOTENCY_KEY_MIN: usize = 8;
/// 幂等键长度上限。
const IDEMPOTENCY_KEY_MAX: usize = 128;
/// Token 名称长度上限。
const NAME_MAX_CHARS: usize = 120;
/// Token 行标识最大长度（ULID 为 26）。
const TOKEN_ID_MAX: usize = 64;

/// 取已挂载的 Token 存储。
///
/// 主密钥不可用时返回 `None`，调用方统一回 `500 INTERNAL_UNEXPECTED`；日志与错误
/// 消息都不含主密钥路径或密钥材料。
fn store_or_internal(state: &AppState, request_id: &RequestId) -> Option<Arc<TokenStore>> {
    let store = state.tokens().map(Arc::clone);
    if store.is_none() {
        tracing::error!(
            request_id = request_id.as_str(),
            "token store is unavailable: the master key could not be loaded"
        );
    }
    store
}

/// `GET /api/v1/tokens`：列表，不返回明文。
pub async fn list(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let Some(store) = store_or_internal(&state, &request_id) else {
        return AuthError::internal().into_response(&request_id);
    };
    match store.list().await {
        Ok(records) => no_store(Json(records).into_response()),
        Err(error) => error.into_response(&request_id),
    }
}

/// `POST /api/v1/tokens`：创建并返回一次性明文。
///
/// 授予的 scope 必须是创建者已持有的子集；幂等键重复一律 409，不重放明文。
pub async fn create(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Extension(principal): Extension<Principal>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let Some(store) = store_or_internal(&state, &request_id) else {
        return AuthError::internal().into_response(&request_id);
    };
    let input: TokenInput = match store::parse_body(&body) {
        Ok(input) => input,
        Err(error) => return error.into_response(&request_id),
    };
    if input.name.trim().is_empty() || input.name.chars().count() > NAME_MAX_CHARS {
        return AuthError::validation("name", "must be 1..=120 characters")
            .into_response(&request_id);
    }
    if input.scopes.is_empty() {
        return AuthError::validation("scopes", "must contain at least one scope")
            .into_response(&request_id);
    }
    let mut unique = input.scopes.clone();
    unique.sort_unstable();
    unique.dedup();
    if unique.len() != input.scopes.len() {
        return AuthError::validation("scopes", "must not contain duplicates")
            .into_response(&request_id);
    }
    // 不得授予创建者没有的 scope：在写库之前判定。
    for scope in &unique {
        if !principal.has_scope(scope.as_str()) {
            return AuthError::forbidden_scope(scope.as_str()).into_response(&request_id);
        }
    }
    let idempotency_key = match idempotency_key(&headers) {
        Ok(key) => key,
        Err(error) => return error.into_response(&request_id),
    };

    let issued = match store
        .create(input.name.trim(), &unique, idempotency_key.as_deref())
        .await
    {
        Ok(issued) => issued,
        Err(error) => return error.into_response(&request_id),
    };

    record_audit(
        store.pool(),
        AuditEvent {
            actor: &principal.subject,
            event_type: "token.created",
            action: "token_create",
            target: &format!("api_tokens:{}", issued.record.id),
            result: "success",
            request_id: request_id.as_str(),
            metadata: serde_json::json!({
                "scopes": unique.iter().map(|scope| scope.as_str()).collect::<Vec<_>>(),
            }),
        },
    )
    .await;

    no_store((StatusCode::CREATED, Json(IssuedBody::from(issued))).into_response())
}

/// `DELETE /api/v1/tokens/{token_id}`：撤销；撤销后 Bearer 立即失效。
pub async fn revoke(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Extension(principal): Extension<Principal>,
    Path(token_id): Path<String>,
) -> Response {
    let Some(store) = store_or_internal(&state, &request_id) else {
        return AuthError::internal().into_response(&request_id);
    };
    if token_id.is_empty() || token_id.len() > TOKEN_ID_MAX {
        return AuthError::validation("token_id", "must be 1..=64 characters")
            .into_response(&request_id);
    }
    if let Err(error) = store.revoke(&token_id).await {
        return error.into_response(&request_id);
    }
    record_audit(
        store.pool(),
        AuditEvent {
            actor: &principal.subject,
            event_type: "token.revoked",
            action: "token_revoke",
            target: &format!("api_tokens:{token_id}"),
            result: "success",
            request_id: request_id.as_str(),
            metadata: serde_json::json!({}),
        },
    )
    .await;
    StatusCode::NO_CONTENT.into_response()
}

/// 创建响应体：`TokenCreated`，明文只在本次响应出现。
#[derive(Debug, Serialize)]
struct IssuedBody {
    id: String,
    name: String,
    prefix: Option<String>,
    scopes: Vec<TokenScope>,
    created_at: String,
    last_used_at: Option<String>,
    revoked_at: Option<String>,
    token: String,
}

impl From<Issued> for IssuedBody {
    fn from(issued: Issued) -> Self {
        let TokenRecord {
            id,
            name,
            prefix,
            scopes,
            created_at,
            last_used_at,
            revoked_at,
        } = issued.record;
        Self {
            id,
            name,
            prefix,
            scopes,
            created_at,
            last_used_at,
            revoked_at,
            token: issued.token,
        }
    }
}

fn no_store(response: Response) -> Response {
    let mut response = response;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().expect("static"));
    response
}

/// 解析并校验 `Idempotency-Key`；缺省返回 `None`。
fn idempotency_key(headers: &HeaderMap) -> Result<Option<String>, AuthError> {
    let Some(value) = headers.get(IDEMPOTENCY_KEY_HEADER) else {
        return Ok(None);
    };
    let key = value
        .to_str()
        .map_err(|_| AuthError::validation("Idempotency-Key", "must be visible ASCII"))?;
    if key.len() < IDEMPOTENCY_KEY_MIN || key.len() > IDEMPOTENCY_KEY_MAX {
        return Err(AuthError::validation(
            "Idempotency-Key",
            "must be 8..=128 characters",
        ));
    }
    Ok(Some(key.to_owned()))
}

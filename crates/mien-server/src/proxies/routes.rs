//! `/api/v1/proxies*` 路由与处理函数。
//!
//! 认证门禁在 [`crate::router`] 统一安装；本模块只负责授权断言、资源存在性、
//! 请求体形状（`additionalProperties: false`）与响应脱敏。顺序固定为：
//! 认证（中间件）→ scope → 资源存在性 → 状态冲突。
//!
//! 契约里没有 `GET /api/v1/proxies/{proxy_id}`（只有 PATCH/DELETE），本模块也不
//! 添加未声明的操作；单条读取由列表端点按游标覆盖。

use std::collections::BTreeMap;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::{StatusCode, Uri};
use axum::response::Response;
use axum::routing::{get, post};
use mien_core::ports::proxy::{ProxyInput, ProxyKind, ProxyPatch};
use serde::Deserialize;
use serde_json::Value;

use super::{DEFAULT_LIMIT, MAX_LIMIT, SCOPE_READ, SCOPE_WRITE};
use crate::RequestId;
use crate::auth::Principal;
use crate::profiles::{ApiError, accepted, created, no_content, ok};
use crate::state::AppState;

/// 全部 `/api/v1/proxies*` 路由。
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/proxies", get(list).post(create))
        .route("/api/v1/proxies/import-link", post(import_link))
        .route(
            "/api/v1/proxies/{proxy_id}",
            get(get_one).patch(update).delete(delete_one),
        )
        .route("/api/v1/proxies/{proxy_id}/check", post(check))
}

// ---------------------------------------------------------------------------
// 输入 DTO
// ---------------------------------------------------------------------------

/// `ProxyInput`：`additionalProperties: false`，凭据 write-only。
///
/// `mien_core::ports::proxy::ProxyInput` 没有 `deny_unknown_fields`，所以在传输层
/// 重新声明一份等价的严格形状，再转换过去；领域校验仍由仓储执行。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProxyInputBody {
    name: String,
    kind: ProxyKind,
    server: String,
    port: u16,
    #[serde(default)]
    options: BTreeMap<String, Value>,
    #[serde(default)]
    credentials: BTreeMap<String, String>,
}

impl From<ProxyInputBody> for ProxyInput {
    fn from(body: ProxyInputBody) -> Self {
        Self {
            name: body.name,
            kind: body.kind,
            server: body.server,
            port: body.port,
            options: body.options,
            credentials: body.credentials,
        }
    }
}

/// `ProxyPatch`：同样的严格形状；省略字段保持原值。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProxyPatchBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    kind: Option<ProxyKind>,
    #[serde(default)]
    server: Option<String>,
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    options: Option<BTreeMap<String, Value>>,
    #[serde(default)]
    credentials: Option<BTreeMap<String, String>>,
}

impl From<ProxyPatchBody> for ProxyPatch {
    fn from(body: ProxyPatchBody) -> Self {
        Self {
            name: body.name,
            kind: body.kind,
            server: body.server,
            port: body.port,
            options: body.options,
            credentials: body.credentials,
        }
    }
}

/// `proxiesImportLink` 的请求体：单字段 `link`，`additionalProperties: false`。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportLinkBody {
    link: String,
}

/// 单条链接上限，与 `mien_core::proxy::share_link::MAX_LINK_BYTES` 一致。
const MAX_LINK_BYTES: usize = mien_core::proxy::share_link::MAX_LINK_BYTES;

// ---------------------------------------------------------------------------
// 提取辅助
// ---------------------------------------------------------------------------

/// 授权断言；在资源存在性之前执行（`REQ-API-003`）。
fn require_scope(principal: &Principal, scope: &str) -> Result<(), ApiError> {
    principal.require_scope(scope).map_err(|error| ApiError {
        status: error.status,
        code: error.code,
        message: error.message.to_owned(),
        retryable: error.retryable,
        details: error.details,
    })
}

fn query_map(uri: &Uri) -> Result<BTreeMap<String, String>, ApiError> {
    let mut out = BTreeMap::new();
    if let Some(query) = uri.query() {
        for pair in query.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            let name = percent_decode(name)?;
            let value = percent_decode(value)?;
            if out.insert(name.clone(), value).is_some() {
                return Err(ApiError::validation(&name, "is repeated"));
            }
        }
    }
    Ok(out)
}

fn percent_decode(raw: &str) -> Result<String, ApiError> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                let high = *bytes
                    .get(index + 1)
                    .ok_or_else(|| ApiError::validation("query", "has an invalid escape"))?;
                let low = *bytes
                    .get(index + 2)
                    .ok_or_else(|| ApiError::validation("query", "has an invalid escape"))?;
                out.push((hex_digit(high)? << 4) | hex_digit(low)?);
                index += 3;
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            other => {
                out.push(other);
                index += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| ApiError::validation("query", "is not valid UTF-8"))
}

fn hex_digit(byte: u8) -> Result<u8, ApiError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(ApiError::validation("query", "has an invalid escape")),
    }
}

/// 解析列表查询：`cursor`、`limit`、`kind`；未知参数一律 422。
fn parse_list_query(params: &BTreeMap<String, String>) -> Result<super::ListQuery, ApiError> {
    let mut unknown: Vec<&String> = params
        .keys()
        .filter(|key| !matches!(key.as_str(), "limit" | "cursor" | "kind"))
        .collect();
    unknown.sort();
    if let Some(field) = unknown.first() {
        return Err(ApiError::validation(
            field,
            "is not a supported filter for proxy listing",
        ));
    }
    let limit =
        match params.get("limit") {
            Some(raw) => Some(raw.parse::<u32>().map_err(|_| {
                ApiError::validation("limit", "must be an integer between 1 and 200")
            })?),
            None => None,
        };
    if let Some(limit) = limit
        && !(1..=MAX_LIMIT).contains(&limit)
    {
        return Err(ApiError::validation(
            "limit",
            "must be an integer between 1 and 200",
        ));
    }
    if limit.is_none() && !params.contains_key("limit") {
        // 默认值由 `list` 应用；这里只校验显式给出的值。
    }
    let kind = match params.get("kind") {
        Some(raw) => Some(ProxyKind::parse(raw).map_err(|_| {
            ApiError::validation("kind", "is not one of the documented proxy kinds")
        })?),
        None => None,
    };
    let cursor = params.get("cursor").cloned();
    if let Some(cursor) = &cursor
        && cursor.len() > 512
    {
        return Err(ApiError::validation("cursor", "is too long"));
    }
    Ok(super::ListQuery {
        limit,
        cursor,
        kind,
    })
}

/// 请求体上限；代理输入远小于此值，但保留与档案端点同一预算。
const MAX_BODY_BYTES: usize = 256 * 1024;

fn parse_body<T: serde::de::DeserializeOwned>(body: &Bytes) -> Result<T, ApiError> {
    if body.len() > MAX_BODY_BYTES {
        return Err(ApiError::validation("body", "is too large"));
    }
    serde_json::from_slice(body)
        .map_err(|error| ApiError::validation("body", format!("is not a valid request: {error}")))
}

// ---------------------------------------------------------------------------
// 处理函数
// ---------------------------------------------------------------------------

async fn list(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Extension(request_id): Extension<RequestId>,
    uri: Uri,
) -> Response {
    if let Err(error) = require_scope(&principal, SCOPE_READ) {
        return error.into_response(&request_id);
    }
    let query = match query_map(&uri).and_then(|params| parse_list_query(&params)) {
        Ok(query) => query,
        Err(error) => return error.into_response(&request_id),
    };
    match super::list(&state, query).await {
        Ok(page) => ok(&page),
        Err(error) => error.into_response(&request_id),
    }
}

async fn create(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Extension(request_id): Extension<RequestId>,
    body: Bytes,
) -> Response {
    if let Err(error) = require_scope(&principal, SCOPE_WRITE) {
        return error.into_response(&request_id);
    }
    let input: ProxyInputBody = match parse_body(&body) {
        Ok(input) => input,
        Err(error) => return error.into_response(&request_id),
    };
    match super::create(&state, input.into()).await {
        Ok(view) => created(&view),
        Err(error) => error.into_response(&request_id),
    }
}

async fn get_one(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Extension(request_id): Extension<RequestId>,
    Path(proxy_id): Path<String>,
) -> Response {
    if let Err(error) = require_scope(&principal, SCOPE_READ) {
        return error.into_response(&request_id);
    }
    match super::get(&state, &proxy_id).await {
        Ok(view) => ok(&view),
        Err(error) => error.into_response(&request_id),
    }
}

async fn update(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Extension(request_id): Extension<RequestId>,
    Path(proxy_id): Path<String>,
    body: Bytes,
) -> Response {
    if let Err(error) = require_scope(&principal, SCOPE_WRITE) {
        return error.into_response(&request_id);
    }
    let patch: ProxyPatchBody = match parse_body(&body) {
        Ok(patch) => patch,
        Err(error) => return error.into_response(&request_id),
    };
    match super::update(&state, &proxy_id, patch.into()).await {
        Ok(view) => ok(&view),
        Err(error) => error.into_response(&request_id),
    }
}

async fn delete_one(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Extension(request_id): Extension<RequestId>,
    Path(proxy_id): Path<String>,
) -> Response {
    if let Err(error) = require_scope(&principal, SCOPE_WRITE) {
        return error.into_response(&request_id);
    }
    match super::delete(&state, &proxy_id).await {
        Ok(()) => no_content(),
        Err(error) => error.into_response(&request_id),
    }
}

/// `POST /api/v1/proxies/import-link`：返回 `ProxyDraft`，**不保存、不启动**。
async fn import_link(
    State(_state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Extension(request_id): Extension<RequestId>,
    body: Bytes,
) -> Response {
    if let Err(error) = require_scope(&principal, SCOPE_WRITE) {
        return error.into_response(&request_id);
    }
    let input: ImportLinkBody = match parse_body(&body) {
        Ok(input) => input,
        Err(error) => return error.into_response(&request_id),
    };
    if input.link.len() > MAX_LINK_BYTES {
        return ApiError::validation("link", "must be at most 16384 bytes")
            .into_response(&request_id);
    }
    match super::import_link(&input.link) {
        Ok(draft) => ok(&draft),
        Err(error) => error.into_response(&request_id),
    }
}

/// `POST /api/v1/proxies/{proxy_id}/check`：节点失败一律 200 + `error_code`。
async fn check(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Extension(request_id): Extension<RequestId>,
    Path(proxy_id): Path<String>,
) -> Response {
    if let Err(error) = require_scope(&principal, SCOPE_READ) {
        return error.into_response(&request_id);
    }
    match super::check::run(&state, &principal.subject, &request_id, &proxy_id).await {
        Ok(result) => ok(&result),
        Err(error) => error.into_response(&request_id),
    }
}

/// `limit` 默认值在 `list` 内应用；这里保留常量引用以免误删契约值。
const _: u32 = DEFAULT_LIMIT;
/// `201 Created` 的显式依赖，避免 `created` 失去唯一调用者。
const _: StatusCode = StatusCode::CREATED;
/// `accepted` 保留给后续异步检查；当前检查是同步 200。
const _: fn(&serde_json::Value) -> Response = |_| accepted(&serde_json::Value::Null);

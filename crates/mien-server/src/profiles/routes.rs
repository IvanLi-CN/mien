//! `/api/v1/profiles*` 路由与处理函数。
//!
//! 认证门禁在 [`crate::router`] 统一安装；本模块只负责授权断言、资源存在性
//! 与幂等包装。顺序固定为：认证（中间件）→ scope（[`require_scope`]）→
//! 资源存在性 → 状态冲突。

use std::collections::BTreeMap;
use std::future::Future;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::Response;
use axum::routing::{get, post};

use super::idempotency::{self, Key, Outcome, RequestFingerprint};
use super::{
    ApiError, DuplicateInput, ProfileInput, ProfilePatchInput, accepted, created, no_content, ok,
    parse_body, parse_list_query, parse_permanent,
};
use crate::RequestId;
use crate::auth::Principal;
use crate::state::AppState;

/// 档案读操作 scope。
pub const SCOPE_READ: &str = "profiles.read";
/// 档案写操作 scope。
pub const SCOPE_WRITE: &str = "profiles.write";
/// 运行时控制 scope。
pub const SCOPE_RUNTIME: &str = "runtime.control";

/// 全部 `/api/v1/profiles*` 路由。
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/profiles", get(list).post(create))
        .route(
            "/api/v1/profiles/{profile_id}",
            get(get_one).patch(update).delete(delete_one),
        )
        .route("/api/v1/profiles/{profile_id}/duplicate", post(duplicate))
        .route("/api/v1/profiles/{profile_id}/archive", post(archive))
        .route("/api/v1/profiles/{profile_id}/restore", post(restore))
        .route("/api/v1/profiles/{profile_id}/start", post(start))
        .route("/api/v1/profiles/{profile_id}/stop", post(stop))
        .route("/api/v1/profiles/{profile_id}/runtime", get(runtime))
}

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
                let value = (hex_digit(high)? << 4) | hex_digit(low)?;
                out.push(value);
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
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(error) = require_scope(&principal, SCOPE_WRITE) {
        return error.into_response(&request_id);
    }
    let input: ProfileInput = match parse_body(&body) {
        Ok(input) => input,
        Err(error) => return error.into_response(&request_id),
    };
    let key = match Key::parse(idempotency_header(&headers)) {
        Ok(key) => key,
        Err(error) => return error.into_response(&request_id),
    };
    let fingerprint = RequestFingerprint::new("POST", "/api/v1/profiles", &body);
    execute_idempotent(
        &state,
        &principal,
        key,
        fingerprint,
        &request_id,
        |state| async move {
            super::create(&state, input)
                .await
                .map(|view| created(&view))
        },
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn get_one(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Extension(request_id): Extension<RequestId>,
    Path(profile_id): Path<String>,
) -> Response {
    if let Err(error) = require_scope(&principal, SCOPE_READ) {
        return error.into_response(&request_id);
    }
    match super::get(&state, &profile_id).await {
        Ok(profile) => ok(&profile),
        Err(error) => error.into_response(&request_id),
    }
}

async fn update(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Extension(request_id): Extension<RequestId>,
    Path(profile_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(error) = require_scope(&principal, SCOPE_WRITE) {
        return error.into_response(&request_id);
    }
    let patch: ProfilePatchInput = match parse_body(&body) {
        Ok(patch) => patch,
        Err(error) => return error.into_response(&request_id),
    };
    let key = match Key::parse(idempotency_header(&headers)) {
        Ok(key) => key,
        Err(error) => return error.into_response(&request_id),
    };
    let path = format!("/api/v1/profiles/{profile_id}");
    let fingerprint = RequestFingerprint::new("PATCH", &path, &body);
    execute_idempotent(
        &state,
        &principal,
        key,
        fingerprint,
        &request_id,
        |state| async move {
            super::update(&state, &profile_id, patch)
                .await
                .map(|view| ok(&view))
        },
    )
    .await
}

async fn delete_one(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Extension(request_id): Extension<RequestId>,
    Path(profile_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Err(error) = require_scope(&principal, SCOPE_WRITE) {
        return error.into_response(&request_id);
    }
    let permanent = match query_map(&uri).and_then(|params| parse_permanent(&params)) {
        Ok(permanent) => permanent,
        Err(error) => return error.into_response(&request_id),
    };
    let key = match Key::parse(idempotency_header(&headers)) {
        Ok(key) => key,
        Err(error) => return error.into_response(&request_id),
    };
    let path = format!(
        "/api/v1/profiles/{profile_id}{}",
        if permanent { "?permanent=true" } else { "" }
    );
    let fingerprint = RequestFingerprint::new("DELETE", &path, b"");
    execute_idempotent(
        &state,
        &principal,
        key,
        fingerprint,
        &request_id,
        |state| async move {
            super::delete(&state, &profile_id, permanent)
                .await
                .map(|()| no_content())
        },
    )
    .await
}

async fn duplicate(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Extension(request_id): Extension<RequestId>,
    Path(profile_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(error) = require_scope(&principal, SCOPE_WRITE) {
        return error.into_response(&request_id);
    }
    let input: DuplicateInput = if body.is_empty() {
        DuplicateInput {
            copy_fingerprint: false,
            copy_proxy: true,
        }
    } else {
        match parse_body(&body) {
            Ok(input) => input,
            Err(error) => return error.into_response(&request_id),
        }
    };
    let key = match Key::parse(idempotency_header(&headers)) {
        Ok(key) => key,
        Err(error) => return error.into_response(&request_id),
    };
    let path = format!("/api/v1/profiles/{profile_id}/duplicate");
    let fingerprint = RequestFingerprint::new("POST", &path, &body);
    execute_idempotent(
        &state,
        &principal,
        key,
        fingerprint,
        &request_id,
        |state| async move {
            super::duplicate(&state, &profile_id, input)
                .await
                .map(|view| created(&view))
        },
    )
    .await
}

async fn archive(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Extension(request_id): Extension<RequestId>,
    Path(profile_id): Path<String>,
) -> Response {
    if let Err(error) = require_scope(&principal, SCOPE_WRITE) {
        return error.into_response(&request_id);
    }
    match super::archive(&state, &profile_id).await {
        Ok(()) => no_content(),
        Err(error) => error.into_response(&request_id),
    }
}

async fn restore(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Extension(request_id): Extension<RequestId>,
    Path(profile_id): Path<String>,
) -> Response {
    if let Err(error) = require_scope(&principal, SCOPE_WRITE) {
        return error.into_response(&request_id);
    }
    match super::restore(&state, &profile_id).await {
        Ok(()) => no_content(),
        Err(error) => error.into_response(&request_id),
    }
}

async fn start(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Extension(request_id): Extension<RequestId>,
    Path(profile_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(error) = require_scope(&principal, SCOPE_RUNTIME) {
        return error.into_response(&request_id);
    }
    let key = match Key::parse(idempotency_header(&headers)) {
        Ok(key) => key,
        Err(error) => return error.into_response(&request_id),
    };
    let path = format!("/api/v1/profiles/{profile_id}/start");
    let fingerprint = RequestFingerprint::new("POST", &path, &body);
    execute_idempotent(
        &state,
        &principal,
        key,
        fingerprint,
        &request_id,
        |state| async move {
            super::start(&state, &profile_id)
                .await
                .map(|runtime| accepted(&runtime))
        },
    )
    .await
}

async fn stop(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Extension(request_id): Extension<RequestId>,
    Path(profile_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(error) = require_scope(&principal, SCOPE_RUNTIME) {
        return error.into_response(&request_id);
    }
    let key = match Key::parse(idempotency_header(&headers)) {
        Ok(key) => key,
        Err(error) => return error.into_response(&request_id),
    };
    let path = format!("/api/v1/profiles/{profile_id}/stop");
    let fingerprint = RequestFingerprint::new("POST", &path, &body);
    execute_idempotent(
        &state,
        &principal,
        key,
        fingerprint,
        &request_id,
        |state| async move {
            super::stop(&state, &profile_id)
                .await
                .map(|runtime| accepted(&runtime))
        },
    )
    .await
}

async fn runtime(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Extension(request_id): Extension<RequestId>,
    Path(profile_id): Path<String>,
) -> Response {
    if let Err(error) = require_scope(&principal, SCOPE_READ) {
        return error.into_response(&request_id);
    }
    match super::runtime(&state, &profile_id).await {
        Ok(runtime) => ok(&runtime),
        Err(error) => error.into_response(&request_id),
    }
}

// ---------------------------------------------------------------------------
// 幂等包装
// ---------------------------------------------------------------------------

fn idempotency_header(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(idempotency::IDEMPOTENCY_HEADER)
        .and_then(|value| value.to_str().ok())
}

/// 带 `Idempotency-Key` 时缓存成功响应；无键时直接执行。
async fn execute_idempotent<F, Fut>(
    state: &AppState,
    principal: &Principal,
    key: Option<Key>,
    fingerprint: RequestFingerprint,
    request_id: &RequestId,
    run: F,
) -> Response
where
    F: FnOnce(AppState) -> Fut,
    Fut: Future<Output = Result<Response, ApiError>>,
{
    let Some(key) = key else {
        return match run(state.clone()).await {
            Ok(response) => response,
            Err(error) => error.into_response(request_id),
        };
    };
    let Some(pool) = state.db() else {
        return ApiError::internal().into_response(request_id);
    };
    let scope = principal.subject.clone();
    match idempotency::begin(pool, &scope, &key, &fingerprint).await {
        Ok(Outcome::Execute) => {}
        Ok(Outcome::Replay { status, body }) => {
            return idempotency::replay(request_id, status, &body);
        }
        Ok(Outcome::InFlight) => {
            let mut error = ApiError::new(
                StatusCode::CONFLICT,
                mien_core::ErrorCode::IdempotencyConflict,
                "The same Idempotency-Key is still being processed.",
            );
            error.retryable = true;
            return error.into_response(request_id);
        }
        Err(error) => return error.into_response(request_id),
    }
    match run(state.clone()).await {
        Ok(response) => {
            let status = response.status().as_u16();
            let body = read_body(response).await;
            if let Err(error) = idempotency::complete(pool, &scope, &key, status, &body).await {
                return error.into_response(request_id);
            }
            idempotency::replay(request_id, status, &body)
        }
        Err(error) => {
            let _ = idempotency::release(pool, &scope, &key).await;
            error.into_response(request_id)
        }
    }
}

async fn read_body(response: Response) -> String {
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap_or_default();
    if status == StatusCode::NO_CONTENT || bytes.is_empty() {
        return String::new();
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

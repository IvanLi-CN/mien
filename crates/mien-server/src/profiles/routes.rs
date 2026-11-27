//! `/api/v1/profiles*` 路由与处理函数。
//!
//! 认证门禁在 [`crate::router`] 统一安装；本模块只负责授权断言、资源存在性
//! 与幂等包装。顺序固定为：认证（中间件）→ scope（[`require_scope`]）→
//! 资源存在性 → 状态冲突。

use std::future::Future;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::Response;
use axum::routing::{get, post};
use serde_json::{Value, json};

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
        .route(
            "/api/v1/profiles/{profile_id}/export",
            post(super::export_import::export_handler),
        )
        .route(
            "/api/v1/profiles/import",
            post(super::export_import::import_handler),
        )
        .route(
            "/api/v1/profiles/import/validate",
            post(super::export_import::validate_handler),
        )
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

/// 写一条档案生命周期审计：`target` 固定为 `profile:<ULID>`（`REQ-API-009`）。
///
/// 只在领域函数成功后调用；`metadata` 只放非秘密标量。
async fn audit_profile(
    state: &AppState,
    event_type: &str,
    profile_id: &str,
    request_id: &RequestId,
    metadata: Value,
) {
    super::audit(
        state,
        event_type,
        &format!("profile:{profile_id}"),
        request_id.as_str(),
        metadata,
    )
    .await;
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
    let query = match crate::query::query_map(&uri).and_then(|params| parse_list_query(&params)) {
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
    let audit_request_id = request_id.clone();
    execute_idempotent(
        &state,
        &principal,
        key,
        fingerprint,
        &request_id,
        move |state| async move {
            let view = super::create(&state, input).await?;
            audit_profile(
                &state,
                "profile.created",
                &view.profile.id.0,
                &audit_request_id,
                json!({}),
            )
            .await;
            Ok(created(&view))
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
    let audit_request_id = request_id.clone();
    execute_idempotent(
        &state,
        &principal,
        key,
        fingerprint,
        &request_id,
        |state| async move {
            let view = super::update(&state, &profile_id, patch).await?;
            audit_profile(
                &state,
                "profile.updated",
                &view.profile.id.0,
                &audit_request_id,
                json!({}),
            )
            .await;
            Ok(ok(&view))
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
    let permanent = match crate::query::query_map(&uri).and_then(|params| parse_permanent(&params))
    {
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
    let audit_request_id = request_id.clone();
    execute_idempotent(
        &state,
        &principal,
        key,
        fingerprint,
        &request_id,
        |state| async move {
            let changed = super::delete(&state, &profile_id, permanent).await?;
            if changed {
                let (event_type, metadata) = if permanent {
                    ("profile.purged", json!({ "permanent": true }))
                } else {
                    ("profile.deleted", json!({ "permanent": false }))
                };
                audit_profile(&state, event_type, &profile_id, &audit_request_id, metadata).await;
            }
            Ok(no_content())
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
    let audit_request_id = request_id.clone();
    let audit_source_id = profile_id.clone();
    execute_idempotent(
        &state,
        &principal,
        key,
        fingerprint,
        &request_id,
        |state| async move {
            let view = super::duplicate(&state, &profile_id, input).await?;
            audit_profile(
                &state,
                "profile.duplicated",
                &view.profile.id.0,
                &audit_request_id,
                json!({ "source_id": audit_source_id }),
            )
            .await;
            Ok(created(&view))
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
        Ok(()) => {
            audit_profile(
                &state,
                "profile.archived",
                &profile_id,
                &request_id,
                json!({}),
            )
            .await;
            no_content()
        }
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
        Ok(changed) => {
            if changed {
                audit_profile(
                    &state,
                    "profile.restored",
                    &profile_id,
                    &request_id,
                    json!({}),
                )
                .await;
            }
            no_content()
        }
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
pub(super) async fn execute_idempotent<F, Fut>(
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

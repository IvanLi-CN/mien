//! 认证：管理员会话、CSRF、登录限流与强制改密。
//!
//! 契约：`docs/specs/control-api/SPEC.md`（`REQ-API-003` 状态码优先级、`REQ-API-006`
//! CSRF/Origin、`REQ-API-007` 登录限流、`REQ-API-018` 强制改密）与 `docs/SECURITY.md`
//! §5.1。认证是**唯一**的门禁入口：所有 `/api/v1/*` 受保护路由由 [`enforce`] 中间件
//! 按「认证 → 授权（CSRF/Origin/强制改密）→ 资源存在性」的顺序判定，因此 401 永远
//! 先于 403，403 永远先于路由产生的 404。
//!
//! Cookie 会话是浏览器 UI 的唯一凭据；Bearer Token 由后续 Task 接入 token 存储，
//! 本模块只提供 [`BearerAuthenticator`] 挂载点：没有挂载真实校验器时，任何 Bearer
//! 请求都以 `401 AUTH_INVALID` 拒绝，并且**从不**参与 Cookie 的 CSRF 校验。

pub mod csrf;
pub mod password;
pub mod ratelimit;
pub mod session;

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, Extension, Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use mien_core::ErrorCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::SqlitePool;

use crate::RequestId;
use crate::state::AppState;

/// 唯一管理员账户标识（v1 不实现多用户 RBAC）。
pub const ADMIN_ACCOUNT: &str = "admin";
/// Cookie 会话持有的 scope 全集；与 `docs/contracts/openapi.yaml#/components/schemas/TokenScope` 一致。
pub const SESSION_SCOPES: &[&str] = &[
    "profiles.read",
    "profiles.write",
    "runtime.control",
    "proxies.read",
    "proxies.write",
    "audit.read",
    "events.read",
    "tokens.manage",
    "kernels.manage",
    "engines.manage",
    "viewer.access",
];
/// 强制改密期间仍允许的写路径：改密与登出。
const PASSWORD_CHANGE_EXEMPT_PATHS: &[&str] = &["/api/v1/auth/password", "/api/v1/auth/logout"];
/// 请求体上限：登录与改密都只接受很小的 JSON。
const MAX_AUTH_BODY_BYTES: usize = 8 * 1024;

/// 认证失败；每种失败都携带稳定错误码与 HTTP 状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthError {
    pub status: StatusCode,
    pub code: ErrorCode,
    pub message: &'static str,
    pub retryable: bool,
    pub retry_after: Option<u64>,
    pub details: Option<Value>,
}

impl AuthError {
    fn new(status: StatusCode, code: ErrorCode, message: &'static str) -> Self {
        Self {
            status,
            code,
            message,
            retryable: false,
            retry_after: None,
            details: None,
        }
    }

    /// 缺少凭据：`401 AUTH_REQUIRED`。
    pub fn required() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            ErrorCode::AuthRequired,
            "Sign-in is required.",
        )
    }

    /// 凭据无效或已过期：`401 AUTH_INVALID`；不区分具体原因。
    pub fn invalid() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            ErrorCode::AuthInvalid,
            "Invalid credentials.",
        )
    }

    /// 缺少或错误的 CSRF Token：`403 AUTH_CSRF_INVALID`。
    pub fn csrf_invalid() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            ErrorCode::AuthCsrfInvalid,
            "The request is missing a valid CSRF token.",
        )
    }

    /// 跨源写请求：`403 AUTH_ORIGIN_INVALID`。
    pub fn origin_invalid() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            ErrorCode::AuthOriginInvalid,
            "The request origin is not allowed.",
        )
    }

    /// 未完成首次改密：`403 AUTH_PASSWORD_CHANGE_REQUIRED`。
    pub fn password_change_required() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            ErrorCode::AuthPasswordChangeRequired,
            "Change the bootstrap password before using other write operations.",
        )
    }

    /// 登录失败次数超限：`429 AUTH_RATE_LIMITED`，带 `Retry-After`。
    pub fn rate_limited(retry_after: u64) -> Self {
        let mut error = Self::new(
            StatusCode::TOO_MANY_REQUESTS,
            ErrorCode::AuthRateLimited,
            "Too many failed sign-in attempts. Try again later.",
        );
        error.retryable = true;
        error.retry_after = Some(retry_after.max(1));
        error
    }

    /// 缺少 scope：`403 FORBIDDEN_SCOPE`；在资源存在性判定之前返回。
    pub fn forbidden_scope(scope: &str) -> Self {
        let mut error = Self::new(
            StatusCode::FORBIDDEN,
            ErrorCode::ForbiddenScope,
            "The credential does not hold the required scope.",
        );
        error.details = Some(serde_json::json!({ "required_scope": scope }));
        error
    }

    /// 请求体校验失败：`422 VALIDATION_FAILED`，`details.fields` 给出字段级原因。
    pub fn validation(field: &str, reason: &'static str) -> Self {
        let mut error = Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationFailed,
            "The request body is not valid.",
        );
        error.details = Some(serde_json::json!({
            "fields": { field: reason },
        }));
        error
    }

    /// 引导秘密不可用（文件缺失、权限过宽或内容为空）。
    pub fn bootstrap_secret_unavailable() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::InternalUnexpected,
            "The administrator bootstrap secret is unavailable.",
        )
    }

    /// 内部错误；不泄露底层原因。
    pub fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::InternalUnexpected,
            "The server could not complete the request.",
        )
    }

    /// 渲染统一错误信封。
    pub fn into_response(self, request_id: &RequestId) -> Response {
        let mut response = crate::error_response_with(
            self.status,
            self.code,
            self.message,
            request_id.as_str(),
            self.retryable,
            self.details,
        );
        if let Some(seconds) = self.retry_after {
            if let Ok(value) = axum::http::HeaderValue::from_str(&seconds.to_string()) {
                response.headers_mut().insert(header::RETRY_AFTER, value);
            }
        }
        response
    }
}

/// 已认证者：Cookie 会话或 Bearer Token。
#[derive(Debug, Clone)]
pub struct Principal {
    pub subject: String,
    pub scopes: Vec<String>,
}

impl Principal {
    /// 管理员会话持有全部 scope。
    pub fn admin() -> Self {
        Self {
            subject: ADMIN_ACCOUNT.to_owned(),
            scopes: SESSION_SCOPES
                .iter()
                .map(|scope| (*scope).to_owned())
                .collect(),
        }
    }

    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|held| held == scope)
    }

    /// 授权检查；必须在任何资源存在性查询之前调用（`REQ-API-003`）。
    ///
    /// # Errors
    /// 缺少所需 scope 时返回 `403 FORBIDDEN_SCOPE`。
    pub fn require_scope(&self, scope: &str) -> Result<(), AuthError> {
        if self.has_scope(scope) {
            Ok(())
        } else {
            Err(AuthError::forbidden_scope(scope))
        }
    }
}

/// Cookie 会话上下文；由 [`enforce`] 注入请求扩展。
#[derive(Debug, Clone)]
pub struct CurrentSession {
    pub id: String,
    /// Cookie 值；仅用于派生 CSRF Token 与登出，不写日志。
    pub token: String,
    pub csrf_token: String,
    pub must_change_password: bool,
}

/// Bearer 校验结果。
#[derive(Debug, Clone)]
pub struct BearerIdentity {
    pub subject: String,
    pub scopes: Vec<String>,
}

/// Bearer Token 校验器挂载点。
///
/// token 表随 tokens.manage 任务落地；在挂载真实实现之前，Bearer 请求一律
/// `401 AUTH_INVALID`，因此不存在「未校验却放行」的路径。
pub trait BearerAuthenticator: Send + Sync {
    fn authenticate<'a>(
        &'a self,
        token: &'a str,
    ) -> Pin<Box<dyn Future<Output = Option<BearerIdentity>> + Send + 'a>>;
}

/// 一个请求需要的凭据类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Requirement {
    /// 公开白名单。
    Public,
    /// 仅 Cookie 会话（`authSession`、`authPassword`、`authLogout`）。
    SessionOnly,
    /// Cookie 会话或 Bearer Token。
    SessionOrBearer,
}

fn requirement(method: &Method, path: &str) -> Requirement {
    match (method, path) {
        (&Method::GET, "/healthz") | (&Method::GET, "/readyz") | (&Method::GET, "/api/v1") => {
            Requirement::Public
        }
        (&Method::POST, "/api/v1/auth/login") => Requirement::Public,
        (&Method::GET, "/api/v1/openapi.json") => Requirement::Public,
        _ if path != "/api/v1" && !path.starts_with("/api/v1/") => Requirement::Public,
        (_, "/api/v1/auth/session") | (_, "/api/v1/auth/password") | (_, "/api/v1/auth/logout") => {
            Requirement::SessionOnly
        }
        _ => Requirement::SessionOrBearer,
    }
}

fn request_id_of(request: &Request) -> RequestId {
    request
        .extensions()
        .get::<RequestId>()
        .cloned()
        .unwrap_or_else(|| RequestId::from_static("unknown"))
}

/// 认证门禁：`401` → `403`（CSRF/Origin/强制改密/scope）→ 路由自身的 `404`。
pub async fn enforce(State(state): State<AppState>, mut request: Request, next: Next) -> Response {
    let request_id = request_id_of(&request);
    let path = request.uri().path().to_owned();
    let method = request.method().clone();

    match requirement(&method, &path) {
        Requirement::Public => next.run(request).await,
        Requirement::SessionOnly => {
            let current = match cookie_session(&state, request.headers()).await {
                Ok(current) => current,
                Err(error) => return error.into_response(&request_id),
            };
            if let Err(error) = write_guards(&state, &method, &path, &current, request.headers()) {
                return error.into_response(&request_id);
            }
            request.extensions_mut().insert(Principal::admin());
            request.extensions_mut().insert(current);
            next.run(request).await
        }
        Requirement::SessionOrBearer => {
            // Bearer 不依赖 Cookie，因此也不参与 CSRF/Origin 校验。
            if request.headers().contains_key(header::AUTHORIZATION) {
                let Some(token) = bearer_token(request.headers()) else {
                    return AuthError::invalid().into_response(&request_id);
                };
                let Some(verifier) = state.bearer_verifier() else {
                    return AuthError::invalid().into_response(&request_id);
                };
                let Some(identity) = verifier.authenticate(&token).await else {
                    return AuthError::invalid().into_response(&request_id);
                };
                let principal = Principal {
                    subject: identity.subject,
                    scopes: identity.scopes,
                };
                if let Err(error) = authorize(&principal, &method, &path) {
                    return error.into_response(&request_id);
                }
                request.extensions_mut().insert(principal);
                return next.run(request).await;
            }
            let current = match cookie_session(&state, request.headers()).await {
                Ok(current) => current,
                Err(error) => return error.into_response(&request_id),
            };
            if let Err(error) = write_guards(&state, &method, &path, &current, request.headers()) {
                return error.into_response(&request_id);
            }
            let principal = Principal::admin();
            if let Err(error) = authorize(&principal, &method, &path) {
                return error.into_response(&request_id);
            }
            request.extensions_mut().insert(principal);
            request.extensions_mut().insert(current);
            next.run(request).await
        }
    }
}

/// 端点所需 scope（`x-required-scope`）；未登记的方法/路径不额外授权，由路由给 404。
fn authorize(principal: &Principal, method: &Method, path: &str) -> Result<(), AuthError> {
    match crate::tokens::scope::required(method, path) {
        Some(scope) => principal.require_scope(scope.as_str()),
        None => Ok(()),
    }
}

/// Cookie 写请求的 CSRF/Origin 与强制改密门禁；顺序固定为 403 先于 404。
fn write_guards(
    state: &AppState,
    method: &Method,
    path: &str,
    current: &CurrentSession,
    headers: &HeaderMap,
) -> Result<(), AuthError> {
    if !csrf::is_write_method(method) {
        return Ok(());
    }
    csrf::verify_write(
        headers,
        state.security().public_url.as_deref(),
        &current.token,
    )?;
    if current.must_change_password && !PASSWORD_CHANGE_EXEMPT_PATHS.contains(&path) {
        return Err(AuthError::password_change_required());
    }
    Ok(())
}

async fn cookie_session(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<CurrentSession, AuthError> {
    let Some(pool) = state.db() else {
        return Err(AuthError::internal());
    };
    let Some(token) = session::parse_cookie(headers) else {
        return Err(AuthError::required());
    };
    let Some(found) = session::find(pool, &token).await? else {
        return Err(AuthError::invalid());
    };
    let must_change = password::must_change_password(pool).await?;
    Ok(CurrentSession {
        id: found.id,
        token,
        csrf_token: found.csrf_token,
        must_change_password: must_change,
    })
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = raw.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_owned())
    }
}

/// `POST /api/v1/auth/login`：公开端点，成功后设置会话 Cookie。
pub async fn login(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    body: Bytes,
) -> Response {
    let Some(pool) = state.db() else {
        return AuthError::internal().into_response(&request_id);
    };
    let input: LoginInput = match parse_json(&body) {
        Ok(input) => input,
        Err(error) => return error.into_response(&request_id),
    };
    if input.password.is_empty() || input.password.len() > 1024 {
        return AuthError::validation("password", "must be 1..=1024 characters")
            .into_response(&request_id);
    }

    let address = session::peer_ip(peer.map(|peer| peer.0.0));
    let key = ratelimit::Key::new(address, ADMIN_ACCOUNT);
    if let ratelimit::Verdict::Locked { retry_after } = state.auth().limiter().check(&key) {
        return AuthError::rate_limited(retry_after).into_response(&request_id);
    }

    let credential = match ensure_credential(&state, pool).await {
        Ok(credential) => credential,
        Err(error) => return error.into_response(&request_id),
    };
    // 没有凭据行时仍然做一次等价开销的哈希校验，避免用响应时间区分“账户不存在”。
    let verified = match &credential {
        Some(credential) => password::verify_password(
            &input.password,
            &credential.password_hash,
            &credential.params_json,
        ),
        None => password::verify_password(&input.password, dummy_hash(), "{}"),
    };
    if !verified {
        state.auth().limiter().record_failure(&key);
        record_audit(
            pool,
            AuditEvent {
                actor: ADMIN_ACCOUNT,
                event_type: "auth.login",
                action: "login",
                target: "admin_credentials:1",
                result: "failure",
                request_id: request_id.as_str(),
                metadata: serde_json::json!({}),
            },
        )
        .await;
        return AuthError::invalid().into_response(&request_id);
    }
    state.auth().limiter().record_success(&key);

    let issued = match session::create(pool, state.auth().session_ttl()).await {
        Ok(issued) => issued,
        Err(error) => return error.into_response(&request_id),
    };
    let must_change = credential.as_ref().is_some_and(|value| value.must_change);
    record_audit(
        pool,
        AuditEvent {
            actor: ADMIN_ACCOUNT,
            event_type: "auth.login",
            action: "login",
            target: "admin_credentials:1",
            result: "success",
            request_id: request_id.as_str(),
            metadata: serde_json::json!({}),
        },
    )
    .await;

    session_response(
        must_change,
        issued.session.csrf_token,
        session::cookie_header(
            &issued.token,
            state.auth().session_ttl(),
            state.security().secure_cookies,
        ),
    )
}

/// `GET /api/v1/auth/session`：当前会话与 scope。
pub async fn session_info(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentSession>,
) -> Response {
    let mut response = Json(SessionBody {
        authenticated: true,
        csrf_token: current.csrf_token,
        must_change_password: current.must_change_password,
        user: ADMIN_ACCOUNT,
        scopes: SESSION_SCOPES,
    })
    .into_response();
    // 会话 Cookie 的有效期由服务端会话表控制，这里不重发 Cookie。
    if let Some(pool) = state.db() {
        let _ = session::purge_expired(pool).await;
    }
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().expect("static"));
    response
}

/// `POST /api/v1/auth/logout`：撤销当前会话并清除 Cookie。
pub async fn logout(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Extension(current): Extension<CurrentSession>,
) -> Response {
    if let Some(pool) = state.db() {
        if let Err(error) = session::revoke(pool, &current.id).await {
            return error.into_response(&request_id);
        }
        record_audit(
            pool,
            AuditEvent {
                actor: ADMIN_ACCOUNT,
                event_type: "auth.logout",
                action: "logout",
                target: "admin_credentials:1",
                result: "success",
                request_id: request_id.as_str(),
                metadata: serde_json::json!({}),
            },
        )
        .await;
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    insert_set_cookie(
        &mut response,
        session::clear_cookie_header(state.security().secure_cookies),
    );
    response
}

/// `POST /api/v1/auth/password`：改密并让其他会话失效。
pub async fn change_password(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Extension(current): Extension<CurrentSession>,
    body: Bytes,
) -> Response {
    let Some(pool) = state.db() else {
        return AuthError::internal().into_response(&request_id);
    };
    let input: PasswordChangeInput = match parse_json(&body) {
        Ok(input) => input,
        Err(error) => return error.into_response(&request_id),
    };
    if input.current_password.is_empty() || input.current_password.len() > 1024 {
        return AuthError::validation("current_password", "must be 1..=1024 characters")
            .into_response(&request_id);
    }
    if input.new_password.chars().count() < password::MIN_NEW_PASSWORD_LEN
        || input.new_password.len() > 1024
    {
        return AuthError::validation("new_password", "must be at least 12 characters")
            .into_response(&request_id);
    }
    if input.new_password == input.current_password {
        return AuthError::validation("new_password", "must differ from the current password")
            .into_response(&request_id);
    }

    let credential = match password::load(pool).await {
        Ok(Some(credential)) => credential,
        Ok(None) => return AuthError::invalid().into_response(&request_id),
        Err(error) => return error.into_response(&request_id),
    };
    if !password::verify_password(
        &input.current_password,
        &credential.password_hash,
        &credential.params_json,
    ) {
        return AuthError::invalid().into_response(&request_id);
    }

    if let Err(error) = password::set_password(pool, &input.new_password).await {
        return error.into_response(&request_id);
    }
    let revoked = session::revoke_others(pool, &current.id)
        .await
        .unwrap_or_default();
    let could_be_generated = credential.bootstrap_pending
        || credential.source.is_none()
        || credential.source == Some(password::BootstrapSource::Generated);
    let removed = if could_be_generated {
        password::remove_generated_bootstrap(state.data_root(), credential.source)
    } else {
        Ok(())
    };
    let bootstrap_file_removed = removed.is_ok()
        && password::generated_bootstrap_path(state.data_root()).exists()
        && credential.source == Some(password::BootstrapSource::Generated);
    if let Err(error) = removed {
        tracing::warn!(
            request_id = request_id.as_str(),
            error = %error,
            "failed to remove the generated bootstrap secret"
        );
    }
    record_audit(
        pool,
        AuditEvent {
            actor: ADMIN_ACCOUNT,
            event_type: "auth.password_changed",
            action: "password_change",
            target: "admin_credentials:1",
            result: "success",
            request_id: request_id.as_str(),
            metadata: serde_json::json!({
                "sessions_revoked": revoked,
                "bootstrap_source": credential.source.map_or("unknown", password::BootstrapSource::as_str),
                "bootstrap_file_removed": bootstrap_file_removed,
            }),
        },
    )
    .await;

    StatusCode::NO_CONTENT.into_response()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginInput {
    password: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PasswordChangeInput {
    current_password: String,
    new_password: String,
}

#[derive(Debug, Serialize)]
struct SessionBody {
    authenticated: bool,
    csrf_token: String,
    must_change_password: bool,
    user: &'static str,
    scopes: &'static [&'static str],
}

fn session_response(must_change_password: bool, csrf_token: String, cookie: String) -> Response {
    let mut response = Json(SessionBody {
        authenticated: true,
        csrf_token,
        must_change_password,
        user: ADMIN_ACCOUNT,
        scopes: SESSION_SCOPES,
    })
    .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().expect("static"));
    insert_set_cookie(&mut response, cookie);
    response
}

fn insert_set_cookie(response: &mut Response, cookie: String) {
    match axum::http::HeaderValue::from_str(&cookie) {
        Ok(value) => {
            response.headers_mut().append(header::SET_COOKIE, value);
        }
        Err(_) => tracing::error!("session cookie value is not a valid header value"),
    }
}

/// 解析认证请求体：上限、未知字段与一次性秘密字段统一按 422 拒绝。
///
/// `REQ-API-005` 要求任何请求体都不接受 `token`/`ticket`，因此登录与改密也走同一
/// 校验路径，而不是只依赖各自的 `deny_unknown_fields`。
fn parse_json<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, AuthError> {
    if body.len() > MAX_AUTH_BODY_BYTES {
        return Err(AuthError::validation("body", "is too large"));
    }
    crate::tokens::store::parse_body(body)
}

/// 账户不存在时的等价开销哈希；只用于让失败路径的耗时一致。
fn dummy_hash() -> &'static str {
    static DUMMY: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        password::hash_password("mien-placeholder-password")
            .map(|(phc, _)| phc)
            .unwrap_or_else(|_| {
                // 静态参数必定有效；退回到一个不可校验的 PHC 前缀也不泄露信息。
                "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHRzYWx0c2E$invalid".to_owned()
            })
    });
    DUMMY.as_str()
}

/// 首次登录时把启动引导口令写成 Argon2id 哈希；已有凭据行时原样返回。
///
/// 这样 `admin_credentials` 只在服务自己的异步运行时里写入，启动路径不必为了
/// 哈希而创建额外运行时。
async fn ensure_credential(
    state: &AppState,
    pool: &SqlitePool,
) -> Result<Option<password::Credential>, AuthError> {
    let existing = password::load(pool).await?;
    if existing.is_some() {
        return Ok(existing);
    }
    let Some(bootstrap) = state.auth().bootstrap() else {
        return Ok(None);
    };
    password::ensure_admin(pool, bootstrap).await?;
    if bootstrap.source() == password::BootstrapSource::Generated {
        tracing::info!(
            path = %password::generated_bootstrap_path(state.data_root()).display(),
            "wrote the administrator credential from the generated bootstrap file"
        );
    }
    password::load(pool).await
}

/// 一条待写入的审计事件。
pub(crate) struct AuditEvent<'a> {
    pub actor: &'a str,
    pub event_type: &'a str,
    pub action: &'a str,
    pub target: &'a str,
    pub result: &'a str,
    pub request_id: &'a str,
    pub metadata: Value,
}

/// 写一条审计事件；失败只记日志，不阻断已完成的认证动作。
pub(crate) async fn record_audit(pool: &SqlitePool, event: AuditEvent<'_>) {
    let AuditEvent {
        actor,
        event_type,
        action,
        target,
        result,
        request_id,
        metadata,
    } = event;
    let id = mien_core::domain::profile::ProfileId::new().0;
    let written = sqlx::query(
        "INSERT INTO audit_events \
         (id, actor, event_type, profile_id, action, target, result, request_id, metadata_json, created_at) \
         VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6, ?7, ?8, ?9)",
    )
    .bind(id)
    .bind(actor)
    .bind(event_type)
    .bind(action)
    .bind(target)
    .bind(result)
    .bind(request_id)
    .bind(metadata.to_string())
    .bind(session::now_timestamp())
    .execute(pool)
    .await;
    if let Err(error) = written {
        tracing::warn!(
            request_id,
            event_type,
            error = %error,
            "cannot record the audit event"
        );
    }
}

/// 供测试与宿主注入的 Bearer 校验器类型别名。
pub type SharedBearerAuthenticator = Arc<dyn BearerAuthenticator>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requirement_covers_the_public_allowlist() {
        assert_eq!(requirement(&Method::GET, "/healthz"), Requirement::Public);
        assert_eq!(requirement(&Method::GET, "/readyz"), Requirement::Public);
        assert_eq!(requirement(&Method::GET, "/api/v1"), Requirement::Public);
        assert_eq!(
            requirement(&Method::POST, "/api/v1/auth/login"),
            Requirement::Public
        );
        assert_eq!(
            requirement(&Method::GET, "/api/v1/openapi.json"),
            Requirement::Public
        );
        assert_eq!(
            requirement(&Method::GET, "/api/v1/auth/session"),
            Requirement::SessionOnly
        );
        assert_eq!(
            requirement(&Method::POST, "/api/v1/auth/password"),
            Requirement::SessionOnly
        );
        assert_eq!(
            requirement(&Method::POST, "/api/v1/auth/logout"),
            Requirement::SessionOnly
        );
        assert_eq!(
            requirement(&Method::POST, "/api/v1/profiles"),
            Requirement::SessionOrBearer
        );
        assert_eq!(
            requirement(&Method::GET, "/api/v1/profiles/01ARZ3NDEKTSV4RRFFQ69G5FAV"),
            Requirement::SessionOrBearer
        );
        // 未知 API 版本与静态资源不受认证门禁影响，保持 404 语义。
        assert_eq!(requirement(&Method::GET, "/api/v2"), Requirement::Public);
        assert_eq!(requirement(&Method::GET, "/nope"), Requirement::Public);
    }

    #[test]
    fn bearer_credentials_are_parsed_case_insensitively() {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, "Bearer abc".parse().expect("value"));
        assert_eq!(bearer_token(&headers).as_deref(), Some("abc"));
        headers.insert(
            header::AUTHORIZATION,
            "bearer  xyz ".parse().expect("value"),
        );
        assert_eq!(bearer_token(&headers).as_deref(), Some("xyz"));
        headers.insert(header::AUTHORIZATION, "Basic abc".parse().expect("value"));
        assert_eq!(bearer_token(&headers), None);
        headers.insert(header::AUTHORIZATION, "Bearer ".parse().expect("value"));
        assert_eq!(bearer_token(&headers), None);
    }

    #[test]
    fn scope_checks_report_the_missing_scope() {
        let principal = Principal::admin();
        assert!(principal.require_scope("profiles.write").is_ok());
        let limited = Principal {
            subject: "automation".to_owned(),
            scopes: vec!["profiles.read".to_owned()],
        };
        let error = limited.require_scope("profiles.write").expect_err("denied");
        assert_eq!(error.code, ErrorCode::ForbiddenScope);
        assert_eq!(error.status, StatusCode::FORBIDDEN);
        assert_eq!(
            error.details,
            Some(serde_json::json!({ "required_scope": "profiles.write" }))
        );
    }
}

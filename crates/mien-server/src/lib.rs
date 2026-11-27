//! `mien-server`：HTTP 宿主骨架。
//!
//! 本 crate 只提供健康检查、版本发现、统一错误信封、安全头与优雅关闭；
//! 认证、档案、代理等业务由后续任务逐个接入。路由与错误结构以
//! `docs/contracts/openapi.yaml` 与 `docs/specs/control-api/SPEC.md` 为准。

pub mod auth;
pub mod discovery;
pub mod health;
pub mod profiles;
pub mod routes;
pub mod state;
pub mod tokens;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Extension, Request, State};
use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router, routing::get};
use serde::Serialize;
use tower_http::set_header::SetResponseHeaderLayer;

use crate::state::AppState;

/// 请求标识响应头。
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// `REQ-API-015` 的内容安全策略：同源资源、禁止内联脚本、禁止被嵌入。
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; \
     connect-src 'self'; object-src 'none'; base-uri 'self'; form-action 'self'; \
     frame-ancestors 'none'";

/// 组合完整路由。请求标识与安全头在全局层生效，包括 404 兜底。
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(health::healthz))
        .route("/readyz", get(health::readyz))
        .route("/api/v1", get(discovery::discover))
        .merge(routes::auth_routes())
        .merge(routes::token_routes())
        .merge(routes::profile_routes())
        .fallback(not_found)
        .with_state(state.clone())
        .layer(middleware::from_fn_with_state(state, auth::enforce))
        .layer(middleware::from_fn(request_id))
        .layer(SetResponseHeaderLayer::overriding(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(CONTENT_SECURITY_POLICY),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_FRAME_OPTIONS,
            HeaderValue::from_static("DENY"),
        ))
}

/// 请求标识；中间件生成后同时写入响应头与错误信封。
#[derive(Clone, Debug)]
pub struct RequestId(Arc<str>);

impl RequestId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 静态请求标识；仅用于中间件之外的内部构造路径。
    pub(crate) fn from_static(value: &'static str) -> Self {
        Self(Arc::from(value))
    }
}

impl Serialize for RequestId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

/// 生成请求标识：纳秒时间戳 + 进程内单调计数，不引入额外依赖。
fn next_request_id() -> RequestId {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or_default();
    let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
    RequestId(Arc::from(format!("{nanos:016x}{sequence:08x}")))
}

/// 补请求标识并记录一行访问日志；不记录请求体与任何请求头。
async fn request_id(mut request: Request, next: Next) -> Response {
    let id = next_request_id();
    request.extensions_mut().insert(id.clone());

    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let mut response = next.run(request).await;

    if let Ok(value) = HeaderValue::from_str(id.as_str()) {
        response
            .headers_mut()
            .insert(HeaderName::from_static(REQUEST_ID_HEADER), value);
    }
    tracing::info!(
        request_id = id.as_str(),
        method = %method,
        path = %path,
        status = response.status().as_u16(),
        "request"
    );
    response
}

/// 统一错误信封。
///
/// `code` 恒为 [`mien_core::ErrorCode`] 成员。契约使用 `request_id` 与
/// `retryable`（`docs/contracts/openapi.yaml#/components/schemas/Error`），
/// 同一标识同时以 `requestId` 别名返回，客户端两种写法都能读到。
#[derive(Debug, Serialize)]
struct ErrorBody<'a> {
    code: &'a str,
    message: &'a str,
    request_id: &'a str,
    #[serde(rename = "requestId")]
    request_id_camel: &'a str,
    retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<serde_json::Value>,
}

fn error_response(
    status: StatusCode,
    code: mien_core::ErrorCode,
    message: &str,
    request_id: &str,
    details: Option<serde_json::Value>,
) -> Response {
    error_response_with(status, code, message, request_id, false, details)
}

/// 同 [`error_response`]，但由调用方决定 `retryable`（例如限流 429）。
pub(crate) fn error_response_with(
    status: StatusCode,
    code: mien_core::ErrorCode,
    message: &str,
    request_id: &str,
    retryable: bool,
    details: Option<serde_json::Value>,
) -> Response {
    let body = ErrorBody {
        code: code.as_str(),
        message,
        request_id,
        request_id_camel: request_id,
        retryable,
        details,
    };
    let mut response = (status, Json(body)).into_response();
    if let Ok(value) = HeaderValue::from_str(request_id) {
        response
            .headers_mut()
            .insert(HeaderName::from_static(REQUEST_ID_HEADER), value);
    }
    response
}

/// 未知路由与未知 API 版本统一返回 404 + 错误信封。
async fn not_found(
    State(_state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    request: Request,
) -> Response {
    let requested = unsupported_version(request.uri().path());
    let (message, details) = match requested {
        Some(version) => (
            "That API version is not served.",
            Some(serde_json::json!({
                "requested_version": version,
                "supported_versions": discovery::SUPPORTED_VERSIONS,
            })),
        ),
        None => ("No such route.", None),
    };
    error_response(
        StatusCode::NOT_FOUND,
        mien_core::ErrorCode::ValidationFailed,
        message,
        request_id.as_str(),
        details,
    )
}

/// `/api/<segment>` 中不受支持的数字版本段，例如 `v2`。
fn unsupported_version(path: &str) -> Option<&str> {
    let segment = path.strip_prefix("/api/")?.split('/').next()?;
    let version = segment.strip_prefix('v')?;
    if version.is_empty() || !version.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(segment)
}

/// SIGINT/SIGTERM 后停止接收新连接。
///
/// # Panics
/// 信号处理器无法注册时 panic：静默继续会失去优雅关闭保证。
pub async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install SIGINT handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
    tracing::info!("shutdown signal received");
}

/// 用给定监听器启动 HTTP 服务，直到收到关闭信号。
///
/// # Errors
/// 接受连接失败。
pub async fn serve(listener: tokio::net::TcpListener, state: AppState) -> std::io::Result<()> {
    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_api_version_segment_is_detected() {
        assert_eq!(unsupported_version("/api/v2"), Some("v2"));
        assert_eq!(unsupported_version("/api/v2/profiles"), Some("v2"));
        assert_eq!(unsupported_version("/api/v1"), Some("v1"));
        assert_eq!(unsupported_version("/api/vX"), None);
        assert_eq!(unsupported_version("/api/"), None);
        assert_eq!(unsupported_version("/healthz"), None);
    }
}

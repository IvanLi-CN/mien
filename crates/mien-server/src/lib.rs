//! `mien-server`：HTTP 宿主骨架。
//!
//! 本 crate 只提供健康检查、版本发现、统一错误信封、安全头与优雅关闭；
//! 认证、档案、代理等业务由后续任务逐个接入。路由与错误结构以
//! `docs/contracts/openapi.yaml` 与 `docs/specs/control-api/SPEC.md` 为准。

pub mod audit;
pub mod auth;
pub mod discovery;
pub mod events;
pub mod health;
pub mod profiles;
pub mod proxies;
pub mod query;
pub mod routes;
pub mod shutdown;
pub mod state;
pub mod tokens;
pub mod web;

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::Request;
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
        .merge(routes::proxy_routes())
        .merge(routes::event_routes())
        .merge(audit::routes())
        // 兜底先判定 API 面，再尝试静态文件与 SPA 入口（`REQ-API-004`）。
        .fallback(web::fallback)
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
///
/// 日志目标会先脱敏：`ticket` 与任何 URL 长效 Token 参数都替换为 `redacted`
/// （`docs/SECURITY.md` §5.2、`REQ-API-011`），因此票据值不会进入访问日志。
async fn request_id(mut request: Request, next: Next) -> Response {
    let id = next_request_id();
    request.extensions_mut().insert(id.clone());

    let method = request.method().clone();
    let target = redacted_request_target(request.uri());
    let mut response = next.run(request).await;

    if let Ok(value) = HeaderValue::from_str(id.as_str()) {
        response
            .headers_mut()
            .insert(HeaderName::from_static(REQUEST_ID_HEADER), value);
    }
    tracing::info!(
        request_id = id.as_str(),
        method = %method,
        path = %target,
        status = response.status().as_u16(),
        "request"
    );
    response
}

/// 查询串中不得进入日志的参数名；命中时只保留名称。
const SENSITIVE_QUERY_PARAMS: &[&str] = &[
    "ticket",
    "token",
    "access_token",
    "id_token",
    "api_token",
    "api_key",
    "apikey",
    "authorization",
    "bearer",
    "session",
    "cookie",
    "password",
    "passphrase",
];

/// 访问日志与诊断使用的请求目标：路径不变，敏感查询值替换为 `redacted`。
pub(crate) fn redacted_request_target(uri: &axum::http::Uri) -> String {
    let path = uri.path();
    let Some(query) = uri.query() else {
        return path.to_owned();
    };
    let mut kept = Vec::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (name, _) = pair.split_once('=').unwrap_or((pair, ""));
        if SENSITIVE_QUERY_PARAMS
            .iter()
            .any(|sensitive| name.eq_ignore_ascii_case(sensitive))
        {
            kept.push(format!("{name}=redacted"));
        } else {
            kept.push(pair.to_owned());
        }
    }
    if kept.is_empty() {
        path.to_owned()
    } else {
        format!("{path}?{}", kept.join("&"))
    }
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

/// 未命中路由的 JSON 404；未知 API 版本额外给出请求的版本段。
///
/// 静态 fallback 与 API 兜底共用这一实现，保证 `/api/*` 的未知路径永远不会
/// 变成 HTML。
pub(crate) fn not_found_response(request_id: &RequestId, path: &str) -> Response {
    let requested = unsupported_version(path);
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

/// API 面：健康检查与 `/api` 前缀由 API 独占，静态 fallback 不得接管。
pub(crate) fn is_api_surface(path: &str) -> bool {
    matches!(path, "/healthz" | "/readyz" | "/api") || path.starts_with("/api/")
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

/// SIGINT/SIGTERM 监听器。
///
/// 一个进程只注册一次；它把每个信号累加到 [`SignalCount`]，因此「第一次」与
/// 「第二次」是可区分的。两个独立的 `tokio::signal::unix::Signal` 不做这件事：
/// 两者都会收到同一个信号，第二次监听会立刻完成，关闭遍历就永远被中止。
pub struct ShutdownSignals {
    #[cfg(unix)]
    interrupt: tokio::signal::unix::Signal,
    #[cfg(unix)]
    terminate: tokio::signal::unix::Signal,
}

impl ShutdownSignals {
    /// 注册 SIGINT 与 SIGTERM。
    ///
    /// # Panics
    /// 信号处理器无法注册时 panic：静默继续会失去优雅关闭保证。
    pub fn install() -> Self {
        #[cfg(unix)]
        {
            let interrupt = || {
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
                    .expect("failed to install SIGINT handler")
            };
            let terminate = || {
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("failed to install SIGTERM handler")
            };
            Self {
                interrupt: interrupt(),
                terminate: terminate(),
            }
        }
        #[cfg(not(unix))]
        {
            Self {}
        }
    }

    /// 等待下一个 SIGINT 或 SIGTERM。
    ///
    /// # Panics
    /// 非 Unix 宿主上无法注册 Ctrl-C 处理器时 panic。
    pub async fn recv(&mut self) {
        #[cfg(unix)]
        tokio::select! {
            _ = self.interrupt.recv() => {}
            _ = self.terminate.recv() => {}
        }
        #[cfg(not(unix))]
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install SIGINT handler");
    }
}

/// 已收到的关闭信号数；`watch` 让每个等待者独立判断阈值。
type SignalCount = tokio::sync::watch::Sender<u64>;

/// 等到第 `n` 个关闭信号。
///
/// 接收者被 drop（发送端仍在）时立刻返回，不阻塞关闭。
async fn signal_waiter(mut signals: tokio::sync::watch::Receiver<u64>, n: u64) {
    let _ = signals.wait_for(move |count| *count >= n).await;
}

/// 排空在途连接的上限：预算的四分之一，落在 250ms..=3s 内。
///
/// 上限必须小于预算：排空吃掉的每一秒都是停止内核少掉的一秒。默认 10s 预算
/// 留 2.5s 给排空、至少 7.5s 给停止序列。
fn drain_grace(budget: Duration) -> Duration {
    (budget / 4).clamp(Duration::from_millis(250), Duration::from_secs(3))
}

/// 用给定监听器启动 HTTP 服务，直到收到关闭信号。
///
/// # Errors
/// 接受连接失败。
pub async fn serve(listener: tokio::net::TcpListener, state: AppState) -> std::io::Result<()> {
    serve_with(listener, state, shutdown::shutdown_budget()).await
}

/// 生产信号接线：一个监听任务累加信号，[`serve_with_shutdown`] 消费前两个。
///
/// 公开是为了让测试能对真实进程发 SIGTERM：信号注册、计数与关闭触发是一整条
/// 链路，把 `axum` 的关闭未来与真实信号拆开测就会漏掉「注册了处理器却不唤醒」
/// 这类缺陷。
///
/// # Errors
/// 接受连接失败。
pub async fn serve_with_signals(
    listener: tokio::net::TcpListener,
    state: AppState,
    budget: Duration,
) -> std::io::Result<()> {
    let (counts, receivers): (SignalCount, _) = tokio::sync::watch::channel(0u64);
    let mut signals = ShutdownSignals::install();
    tokio::spawn(async move {
        loop {
            signals.recv().await;
            // `send_modify` takes the write lock; `send(*counts.borrow() + 1)`
            // would hold the read guard across it and deadlock this task, leaving
            // the installed handler to swallow SIGTERM silently.
            counts.send_modify(|count| *count = count.saturating_add(1));
        }
    });
    serve_with_shutdown(
        listener,
        state,
        budget,
        signal_waiter(receivers.clone(), 1),
        signal_waiter(receivers, 2),
    )
    .await
}

/// 同 [`serve`]，但关闭预算显式给出。
///
/// 关闭顺序固定，且每一段都必须在前一段返回之后才开始：
///
/// 1. 信号到达 → 落下监督器闸门（拒绝新启动）→ `axum` 停止接新连接并排空；
/// 2. 停止全部活动运行时（逐个 `RuntimeSupervisor::stop`），每个结果按 HTTP
///    `stop` 的语义写回 `runtimes`；
/// 3. 关闭数据库池；
/// 4. 返回，进程退出。
///
/// 第二段的总预算为 `budget`；超时或单行失败不阻断其余行，最终 `warn` 报告
/// 未停干净的行数。第二次信号立即放弃关闭遍历并返回：容器编排的 `SIGKILL`
/// 宽限期不必被一个卡住的停止序列耗尽。
///
/// # Errors
/// 接受连接失败。
pub async fn serve_with(
    listener: tokio::net::TcpListener,
    state: AppState,
    budget: Duration,
) -> std::io::Result<()> {
    serve_with_signals(listener, state, budget).await
}

/// [`serve_with`] 的触发接缝：`shutdown` 触发关闭，`interrupt` 放弃停止遍历。
///
/// 生产路径传入两个真实的信号监听器；测试传入一次性触发器，因此走的是与生产
/// 完全相同的顺序（闸门 → 排空 → 停止 → 落库 → 关池）。
///
/// # Errors
/// 接受连接失败。
pub async fn serve_with_shutdown<F, G>(
    listener: tokio::net::TcpListener,
    state: AppState,
    budget: Duration,
    shutdown: F,
    interrupt: G,
) -> std::io::Result<()>
where
    F: Future<Output = ()> + Send + 'static,
    G: Future<Output = ()> + Send + 'static,
{
    // 预算从信号到达时开始计：排空与停止运行时共用同一份，而不是各自一份。
    let (signal_tx, mut signal_rx) = tokio::sync::watch::channel(None::<tokio::time::Instant>);

    // 闸门在排空之前落下：排空期间 keep-alive 连接仍可能送达启动请求。
    let supervisor = state.supervisor().clone();
    let gate = {
        let supervisor = supervisor.clone();
        async move {
            shutdown.await;
            supervisor.begin_shutdown();
            let _ = signal_tx.send(Some(tokio::time::Instant::now()));
            tracing::info!("shutdown signal received; draining connections");
        }
    };

    let serving = axum::serve(listener, router(state.clone()))
        .with_graceful_shutdown(gate)
        .into_future();
    let mut serving = std::pin::pin!(serving);
    // 排空上限：一个卡住的请求不得吃掉整个关闭预算。容器编排在宽限期结束后
    // SIGKILL 整个容器，那样连“写回真实状态”都来不及。
    let drain_grace = drain_grace(budget);
    let served = tokio::select! {
        result = &mut serving => result,
        () = async {
            let _ = signal_rx.wait_for(Option::is_some).await;
            tokio::time::sleep(drain_grace).await;
        } => {
            tracing::warn!(
                grace_secs = drain_grace.as_secs(),
                "shutdown: connection drain exceeded its grace; closing the listener"
            );
            Ok(())
        }
    };

    // 第二段：axum 已不再接新连接。停止运行时，同时监听第二次信号。
    //
    // 只有真的收到过信号才扣掉排空耗时：服务因接受连接失败而返回时没有信号，
    // 此时仍应给出完整预算（那种情况下通常也没有活动运行时）。
    let remaining = match *signal_rx.borrow() {
        Some(at) => budget.saturating_sub(at.elapsed()),
        None => budget,
    };
    let pool = state.db().cloned();
    let report =
        shutdown::stop_runtimes_unless(&supervisor, pool.as_ref(), remaining, interrupt).await;
    tracing::info!(
        attempted = report.attempted,
        stopped = report.stopped.len(),
        unfinished = report.unfinished.len(),
        aborted = report.aborted,
        remaining_secs = remaining.as_secs(),
        "shutdown runtime sweep finished"
    );

    // 第三段：刷新并关闭持久化。关闭池会唤醒在途查询，进程退出不必再等后台
    // 任务超时。
    if let Some(pool) = pool {
        pool.close().await;
    }
    tracing::info!("shutdown complete");
    served
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

    #[test]
    fn api_surface_is_reserved_before_static_files() {
        for path in [
            "/api",
            "/api/",
            "/api/v1",
            "/api/v1/nope",
            "/healthz",
            "/readyz",
        ] {
            assert!(is_api_surface(path), "{path} must stay on the API side");
        }
        for path in ["/", "/apiary", "/api-docs", "/profiles/abc", "/healthzz"] {
            assert!(!is_api_surface(path), "{path} may be served by the UI");
        }
    }
}

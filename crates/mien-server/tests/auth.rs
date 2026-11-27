//! 认证验收（T22）：会话、CSRF、登录限流与强制改密。
//!
//! 覆盖 `REQ-API-003`/`006`/`007`/`018` 与 `VER-API-003`/`006`/`007`/`018`：
//! 正确/错误口令同码同状态、首次登录 `must_change_password`、改密后其他会话失效、
//! 未改密写操作 `403 AUTH_PASSWORD_CHANGE_REQUIRED`、缺失 CSRF 被拒、跨源 Origin
//! 被拒、Bearer 不依赖 CSRF、连续失败 429 带 `Retry-After`、改密后删除应用生成的
//! `admin-bootstrap` 文件，以及 401 → 403 → 404 的优先级。

use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use axum::response::Response;
use mien_server::auth::password::{
    ADMIN_PASSWORD_ENV, ADMIN_PASSWORD_FILE_ENV, BOOTSTRAP_FILE, BootstrapSource, Credential,
};
use mien_server::auth::ratelimit::Policy;
use mien_server::router;
use mien_server::state::{AppState, AuthState, SecuritySettings};
use tempfile::TempDir;
use tower::ServiceExt;

const ADMIN_PASSWORD: &str = "correct horse battery staple";

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

/// 回环开发模式状态（Cookie 无 `Secure`），带内联引导口令。
fn state_with_bootstrap() -> (TempDir, AppState) {
    state_with(SecuritySettings::loopback())
}

/// 服务端模式状态：`MIEN_PUBLIC_URL` 已配置，Cookie 带 `Secure`。
fn public_server_state() -> (TempDir, AppState) {
    state_with(SecuritySettings {
        public_url: Some("https://mien.example.com".to_owned()),
        secure_cookies: true,
    })
}

fn state_with(security: SecuritySettings) -> (TempDir, AppState) {
    let dir = TempDir::new().expect("temp dir");
    let pool = mien_core::storage::open_and_migrate(&dir.path().join("mien.db")).expect("migrate");
    let auth = AuthState::default().with_bootstrap(mien_server::auth::password::Bootstrap::inline(
        ADMIN_PASSWORD,
    ));
    let state = AppState::new(dir.path(), Some(pool))
        .with_security(security)
        .with_auth(auth);
    (dir, state)
}

struct Call {
    method: Method,
    path: &'static str,
    headers: HeaderMap,
    body: Option<serde_json::Value>,
}

impl Call {
    fn new(method: Method, path: &'static str) -> Self {
        Self {
            method,
            path,
            headers: HeaderMap::new(),
            body: None,
        }
    }

    fn header(mut self, name: header::HeaderName, value: impl AsRef<str>) -> Self {
        self.headers
            .insert(name, value.as_ref().parse().expect("header value"));
        self
    }

    fn cookie(self, token: &str) -> Self {
        self.header(
            header::COOKIE,
            format!("{}={token}", mien_server::auth::session::COOKIE_NAME),
        )
    }

    fn origin(self, value: &str) -> Self {
        self.header(header::ORIGIN, value)
    }

    fn csrf(self, token: &str) -> Self {
        self.header(
            header::HeaderName::from_static(mien_server::auth::csrf::CSRF_HEADER),
            token,
        )
    }

    fn bearer(self, token: &str) -> Self {
        self.header(header::AUTHORIZATION, format!("Bearer {token}"))
    }

    fn json(mut self, body: serde_json::Value) -> Self {
        self.body = Some(body);
        self
    }

    async fn send(self, app: axum::Router) -> Response {
        let mut builder = Request::builder().method(self.method).uri(self.path);
        for (name, value) in &self.headers {
            builder = builder.header(name, value);
        }
        let body = match self.body {
            Some(value) => Body::from(value.to_string()),
            None => Body::empty(),
        };
        app.oneshot(
            builder
                .header(header::CONTENT_TYPE, "application/json")
                .body(body)
                .expect("request"),
        )
        .await
        .expect("response")
    }
}

async fn body_json(response: Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("body");
    serde_json::from_slice(&bytes).expect("json body")
}

fn set_cookie(response: &Response) -> String {
    response
        .headers()
        .get(header::SET_COOKIE)
        .expect("set-cookie")
        .to_str()
        .expect("ascii")
        .to_owned()
}

fn cookie_value(set_cookie: &str) -> String {
    set_cookie
        .split(';')
        .next()
        .and_then(|pair| pair.split_once('='))
        .map(|(_, value)| value.to_owned())
        .expect("session cookie value")
}

/// 登录并返回（会话 Token、CSRF Token、登录响应体）。
async fn login(app: axum::Router) -> (String, String, serde_json::Value) {
    let response = Call::new(Method::POST, "/api/v1/auth/login")
        .origin("http://127.0.0.1:8787")
        .json(serde_json::json!({ "password": ADMIN_PASSWORD }))
        .send(app)
        .await;
    assert_eq!(response.status(), StatusCode::OK, "login must succeed");
    let token = cookie_value(&set_cookie(&response));
    let body = body_json(response).await;
    let csrf = body["csrf_token"].as_str().expect("csrf token").to_owned();
    (token, csrf, body)
}

#[test]
fn login_sets_a_hardened_cookie_and_reports_the_first_login_flag() {
    let (_dir, state) = state_with_bootstrap();
    let rt = runtime();
    rt.block_on(async move {
        let response = Call::new(Method::POST, "/api/v1/auth/login")
            .origin("http://127.0.0.1:8787")
            .json(serde_json::json!({ "password": ADMIN_PASSWORD }))
            .send(router(state))
            .await;
        assert_eq!(response.status(), StatusCode::OK);

        let cookie = set_cookie(&response);
        assert!(cookie.starts_with("mien_session="), "cookie: {cookie}");
        assert!(cookie.contains("HttpOnly"), "cookie: {cookie}");
        assert!(cookie.contains("SameSite=Strict"), "cookie: {cookie}");
        assert!(cookie.contains("Path=/"), "cookie: {cookie}");
        assert!(
            !cookie.contains("Secure"),
            "loopback development must not mark the cookie Secure: {cookie}"
        );
        assert_eq!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|v| v.to_str().ok()),
            Some("no-store")
        );

        let body = body_json(response).await;
        assert_eq!(body["authenticated"], true);
        assert_eq!(
            body["must_change_password"], true,
            "first login must change the password"
        );
        assert_eq!(body["user"], "admin");
        assert!(
            body["scopes"]
                .as_array()
                .expect("scopes array")
                .iter()
                .any(|scope| scope == "profiles.write"),
            "admin session holds every scope: {body}"
        );
        assert!(
            body["csrf_token"]
                .as_str()
                .is_some_and(|token| token.len() == 64),
            "csrf token is 64 hex characters: {body}"
        );
    });
}

#[test]
fn a_public_server_marks_the_session_cookie_secure() {
    let (_dir, state) = public_server_state();
    let rt = runtime();
    rt.block_on(async move {
        let response = Call::new(Method::POST, "/api/v1/auth/login")
            .origin("https://mien.example.com")
            .json(serde_json::json!({ "password": ADMIN_PASSWORD }))
            .send(router(state))
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        let cookie = set_cookie(&response);
        assert!(cookie.contains("; Secure"), "cookie: {cookie}");
        assert!(cookie.contains("HttpOnly"), "cookie: {cookie}");
        assert!(cookie.contains("SameSite=Strict"), "cookie: {cookie}");
    });
}

#[test]
fn wrong_password_and_unknown_account_share_one_code_and_status() {
    let (_dir, state) = state_with_bootstrap();
    let (_empty_dir, empty_state) = {
        let dir = TempDir::new().expect("temp dir");
        let pool =
            mien_core::storage::open_and_migrate(&dir.path().join("mien.db")).expect("migrate");
        // 没有引导口令、也没有 admin_credentials 行：等价于“用户不存在”。
        (dir, AppState::new("/nonexistent", Some(pool)))
    };
    let rt = runtime();
    rt.block_on(async move {
        let wrong = Call::new(Method::POST, "/api/v1/auth/login")
            .origin("http://127.0.0.1:8787")
            .json(serde_json::json!({ "password": "wrong password entirely" }))
            .send(router(state.clone()))
            .await;
        assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
        assert!(
            set_cookie_opt(&wrong).is_none(),
            "failure must not set a session cookie"
        );
        let wrong_body = body_json(wrong).await;

        let missing = Call::new(Method::POST, "/api/v1/auth/login")
            .origin("http://127.0.0.1:8787")
            .json(serde_json::json!({ "password": "wrong password entirely" }))
            .send(router(empty_state))
            .await;
        assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);
        let missing_body = body_json(missing).await;

        assert_eq!(wrong_body["code"], "AUTH_INVALID");
        assert_eq!(missing_body["code"], "AUTH_INVALID");
        assert_eq!(wrong_body["message"], missing_body["message"]);
        assert_eq!(wrong_body["retryable"], false);
        assert_eq!(missing_body["retryable"], false);
        // 尚未认证的请求带 request_id，便于关联审计。
        assert!(
            wrong_body["request_id"]
                .as_str()
                .is_some_and(|id| !id.is_empty())
        );
    });
}

fn set_cookie_opt(response: &Response) -> Option<&str> {
    response
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
}

#[test]
fn session_endpoint_returns_the_current_session_and_401_without_a_cookie() {
    let (_dir, state) = state_with_bootstrap();
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state.clone());
        let (token, csrf, _) = login(app.clone()).await;

        let response = Call::new(Method::GET, "/api/v1/auth/session")
            .cookie(&token)
            .send(app.clone())
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["authenticated"], true);
        assert_eq!(body["must_change_password"], true);
        assert_eq!(body["csrf_token"], csrf);
        assert_eq!(body["user"], "admin");

        let anonymous = Call::new(Method::GET, "/api/v1/auth/session")
            .send(app)
            .await;
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(body_json(anonymous).await["code"], "AUTH_REQUIRED");
    });
}

#[test]
fn changing_the_password_clears_the_flag_and_revokes_other_sessions() {
    let (_dir, state) = state_with_bootstrap();
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state.clone());
        let (first_token, first_csrf, _) = login(app.clone()).await;
        let (second_token, _, _) = login(app.clone()).await;
        assert_ne!(first_token, second_token);

        let changed = Call::new(Method::POST, "/api/v1/auth/password")
            .cookie(&first_token)
            .origin("http://127.0.0.1:8787")
            .csrf(&first_csrf)
            .json(serde_json::json!({
                "current_password": ADMIN_PASSWORD,
                "new_password": "a brand new passphrase",
            }))
            .send(app.clone())
            .await;
        assert_eq!(changed.status(), StatusCode::NO_CONTENT);

        // 当前会话仍然有效，且强制改密标记已清除。
        let current = Call::new(Method::GET, "/api/v1/auth/session")
            .cookie(&first_token)
            .send(app.clone())
            .await;
        assert_eq!(current.status(), StatusCode::OK);
        assert_eq!(body_json(current).await["must_change_password"], false);

        // 其他会话失效。
        let other = Call::new(Method::GET, "/api/v1/auth/session")
            .cookie(&second_token)
            .send(app.clone())
            .await;
        assert_eq!(other.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(body_json(other).await["code"], "AUTH_INVALID");

        // 旧口令不能再登录，新口令可以。
        let old = Call::new(Method::POST, "/api/v1/auth/login")
            .origin("http://127.0.0.1:8787")
            .json(serde_json::json!({ "password": ADMIN_PASSWORD }))
            .send(app.clone())
            .await;
        assert_eq!(old.status(), StatusCode::UNAUTHORIZED);

        let new = Call::new(Method::POST, "/api/v1/auth/login")
            .origin("http://127.0.0.1:8787")
            .json(serde_json::json!({ "password": "a brand new passphrase" }))
            .send(app)
            .await;
        assert_eq!(new.status(), StatusCode::OK);
        assert_eq!(body_json(new).await["must_change_password"], false);
    });
}

#[test]
fn a_write_before_the_password_change_is_403_with_the_required_code() {
    let (_dir, state) = state_with_bootstrap();
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state.clone());
        let (token, csrf, _) = login(app.clone()).await;

        // 一个受保护写路径：未实现业务路由也会先经过认证门禁。
        let denied = Call::new(Method::POST, "/api/v1/profiles")
            .cookie(&token)
            .origin("http://127.0.0.1:8787")
            .csrf(&csrf)
            .json(serde_json::json!({}))
            .send(app.clone())
            .await;
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        let body = body_json(denied).await;
        assert_eq!(body["code"], "AUTH_PASSWORD_CHANGE_REQUIRED");
        assert_eq!(body["retryable"], false);

        // 改密后同一请求不再被强制改密拦截，落到路由的 404。
        let changed = Call::new(Method::POST, "/api/v1/auth/password")
            .cookie(&token)
            .origin("http://127.0.0.1:8787")
            .csrf(&csrf)
            .json(serde_json::json!({
                "current_password": ADMIN_PASSWORD,
                "new_password": "a brand new passphrase",
            }))
            .send(app.clone())
            .await;
        assert_eq!(changed.status(), StatusCode::NO_CONTENT);

        let allowed = Call::new(Method::POST, "/api/v1/profiles")
            .cookie(&token)
            .origin("http://127.0.0.1:8787")
            .csrf(&csrf)
            .json(serde_json::json!({}))
            .send(app)
            .await;
        assert_eq!(allowed.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_json(allowed).await["code"], "VALIDATION_FAILED");
    });
}

#[test]
fn a_cookie_write_without_csrf_is_rejected() {
    let (_dir, state) = state_with_bootstrap();
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state.clone());
        let (token, _csrf, _) = login(app.clone()).await;

        let missing = Call::new(Method::POST, "/api/v1/auth/password")
            .cookie(&token)
            .origin("http://127.0.0.1:8787")
            .json(serde_json::json!({
                "current_password": ADMIN_PASSWORD,
                "new_password": "a brand new passphrase",
            }))
            .send(app.clone())
            .await;
        assert_eq!(missing.status(), StatusCode::FORBIDDEN);
        assert_eq!(body_json(missing).await["code"], "AUTH_CSRF_INVALID");

        let wrong = Call::new(Method::POST, "/api/v1/auth/password")
            .cookie(&token)
            .origin("http://127.0.0.1:8787")
            .csrf("00")
            .json(serde_json::json!({
                "current_password": ADMIN_PASSWORD,
                "new_password": "a brand new passphrase",
            }))
            .send(app.clone())
            .await;
        assert_eq!(wrong.status(), StatusCode::FORBIDDEN);
        assert_eq!(body_json(wrong).await["code"], "AUTH_CSRF_INVALID");

        // 读请求不需要 CSRF。
        let read = Call::new(Method::GET, "/api/v1/auth/session")
            .cookie(&token)
            .send(app)
            .await;
        assert_eq!(read.status(), StatusCode::OK);
    });
}

#[test]
fn a_cross_origin_cookie_write_is_rejected() {
    let (_dir, state) = state_with_bootstrap();
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state.clone());
        let (token, csrf, _) = login(app.clone()).await;

        let response = Call::new(Method::POST, "/api/v1/auth/password")
            .cookie(&token)
            .origin("https://evil.example")
            .csrf(&csrf)
            .json(serde_json::json!({
                "current_password": ADMIN_PASSWORD,
                "new_password": "a brand new passphrase",
            }))
            .send(app.clone())
            .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(body_json(response).await["code"], "AUTH_ORIGIN_INVALID");

        // 缺失 Origin/Referer 的 Cookie 写请求同样被拒。
        let anonymous = Call::new(Method::POST, "/api/v1/auth/password")
            .cookie(&token)
            .csrf(&csrf)
            .json(serde_json::json!({
                "current_password": ADMIN_PASSWORD,
                "new_password": "a brand new passphrase",
            }))
            .send(app)
            .await;
        assert_eq!(anonymous.status(), StatusCode::FORBIDDEN);
        assert_eq!(body_json(anonymous).await["code"], "AUTH_ORIGIN_INVALID");
    });
}

#[test]
fn bearer_requests_do_not_need_csrf_but_must_authenticate() {
    let (_dir, state) = state_with_bootstrap();
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state.clone());
        let (token, csrf, _) = login(app.clone()).await;

        // 无 CSRF 头的 Bearer 写请求：先认证；未挂载 token 校验器时是 401，而不是 CSRF 403。
        let bearer = Call::new(Method::POST, "/api/v1/profiles")
            .bearer("some-automation-token")
            .json(serde_json::json!({}))
            .send(app.clone())
            .await;
        assert_eq!(bearer.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(body_json(bearer).await["code"], "AUTH_INVALID");

        // 同一端点用 Cookie 且缺 CSRF 时是 403 AUTH_CSRF_INVALID —— 两条路径确实不同。
        let cookie = Call::new(Method::POST, "/api/v1/profiles")
            .cookie(&token)
            .origin("http://127.0.0.1:8787")
            .json(serde_json::json!({}))
            .send(app.clone())
            .await;
        assert_eq!(cookie.status(), StatusCode::FORBIDDEN);
        assert_eq!(body_json(cookie).await["code"], "AUTH_CSRF_INVALID");

        // 挂载校验器后，Bearer 写请求通过认证与授权，且不需要 CSRF 头。
        let app = router(
            state.with_auth(AuthState::default().with_bearer(std::sync::Arc::new(StaticBearer))),
        );
        let allowed = Call::new(Method::POST, "/api/v1/profiles")
            .bearer("any-token")
            .origin("https://evil.example")
            .json(serde_json::json!({}))
            .send(app)
            .await;
        assert_eq!(
            allowed.status(),
            StatusCode::NOT_FOUND,
            "authenticated Bearer write reaches the route layer"
        );
        let _ = csrf;
    });
}

/// 固定 Bearer 校验器：把任意非空 Token 映射到持有全部 scope 的自动化主体。
struct StaticBearer;

impl mien_server::auth::BearerAuthenticator for StaticBearer {
    fn authenticate<'a>(
        &'a self,
        token: &'a str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Option<mien_server::auth::BearerIdentity>> + Send + 'a,
        >,
    > {
        let identity = if token.is_empty() {
            None
        } else {
            Some(mien_server::auth::BearerIdentity {
                subject: "automation".to_owned(),
                scopes: mien_server::auth::SESSION_SCOPES
                    .iter()
                    .map(|scope| (*scope).to_owned())
                    .collect(),
            })
        };
        Box::pin(async move { identity })
    }
}

#[test]
fn repeated_failures_return_429_with_retry_after() {
    let (_dir, state) = state_with_bootstrap();
    let state = state.with_auth(AuthState::default().with_limiter_policy(Policy {
        window: std::time::Duration::from_secs(60),
        max_failures: 3,
        base_lockout: std::time::Duration::from_secs(30),
        max_lockout: std::time::Duration::from_secs(60),
    }));
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state.clone());
        for _ in 0..3 {
            let response = Call::new(Method::POST, "/api/v1/auth/login")
                .origin("http://127.0.0.1:8787")
                .json(serde_json::json!({ "password": "definitely wrong" }))
                .send(app.clone())
                .await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }

        // 第 4 次（即使口令正确）也是 429，并带 Retry-After。
        let limited = Call::new(Method::POST, "/api/v1/auth/login")
            .origin("http://127.0.0.1:8787")
            .json(serde_json::json!({ "password": ADMIN_PASSWORD }))
            .send(app.clone())
            .await;
        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
        let retry_after = limited
            .headers()
            .get(header::RETRY_AFTER)
            .expect("retry-after")
            .to_str()
            .expect("ascii")
            .parse::<u64>()
            .expect("seconds");
        assert!(retry_after >= 1, "retry_after: {retry_after}");
        let body = body_json(limited).await;
        assert_eq!(body["code"], "AUTH_RATE_LIMITED");
        assert_eq!(body["retryable"], true);
    });
}

#[test]
fn the_generated_bootstrap_file_is_removed_after_the_password_change() {
    // 没有 MIEN_ADMIN_PASSWORD/_FILE 时启动引导生成 0600 文件。
    let dir = TempDir::new().expect("temp dir");
    let pool = mien_core::storage::open_and_migrate(&dir.path().join("mien.db")).expect("migrate");
    let bootstrap = mien_server::auth::password::resolve_bootstrap(dir.path(), |_| None)
        .expect("generated bootstrap");
    assert_eq!(bootstrap.source(), BootstrapSource::Generated);
    let password = bootstrap.expose().to_owned();
    let state = AppState::new(dir.path(), Some(pool))
        .with_auth(AuthState::default().with_bootstrap(bootstrap));
    let path = dir.path().join(BOOTSTRAP_FILE);
    assert!(path.is_file(), "generated bootstrap file must exist");
    let rt = runtime();
    rt.block_on(async {
        let app = router(state.clone());
        let response = Call::new(Method::POST, "/api/v1/auth/login")
            .origin("http://127.0.0.1:8787")
            .json(serde_json::json!({ "password": password }))
            .send(app.clone())
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        let token = cookie_value(&set_cookie(&response));
        let body = body_json(response).await;
        assert_eq!(body["must_change_password"], true);
        let csrf = body["csrf_token"].as_str().expect("csrf").to_owned();

        let changed = Call::new(Method::POST, "/api/v1/auth/password")
            .cookie(&token)
            .origin("http://127.0.0.1:8787")
            .csrf(&csrf)
            .json(serde_json::json!({
                "current_password": password,
                "new_password": "a brand new passphrase",
            }))
            .send(app.clone())
            .await;
        assert_eq!(changed.status(), StatusCode::NO_CONTENT);
    });
    assert!(
        !path.exists(),
        "the app-generated bootstrap file must be deleted after the password change"
    );

    // 审计事件确实落库，且不含口令。
    let credential: Option<Credential> = rt
        .block_on(mien_server::auth::password::load(state.db().expect("pool")))
        .expect("load");
    assert!(!credential.expect("credential").must_change);
    let events: Vec<(String, String, String)> = rt
        .block_on(
            sqlx::query_as(
                "SELECT event_type, result, metadata_json FROM audit_events ORDER BY created_at",
            )
            .fetch_all(state.db().expect("pool")),
        )
        .expect("audit rows");
    let types: Vec<&str> = events.iter().map(|(kind, _, _)| kind.as_str()).collect();
    assert!(types.contains(&"auth.login"), "events: {types:?}");
    assert!(
        types.contains(&"auth.password_changed"),
        "events: {types:?}"
    );
    for (_, _, metadata) in &events {
        assert!(
            !metadata.contains(&password),
            "audit metadata must never contain the password"
        );
    }

    // 环境变量来源不会被应用删除；此处只确认删除逻辑只对生成来源生效。
    let dir2 = TempDir::new().expect("temp dir");
    let fake = dir2.path().join(BOOTSTRAP_FILE);
    std::fs::create_dir_all(fake.parent().expect("parent")).expect("secrets dir");
    std::fs::write(&fake, "external\n").expect("write");
    mien_server::auth::password::remove_generated_bootstrap(
        dir2.path(),
        Some(BootstrapSource::Environment),
    )
    .expect("no-op removal");
    assert!(fake.is_file(), "external secrets must not be deleted");
}

#[test]
fn authentication_precedes_authorization_and_existence() {
    let (_dir, state) = state_with_bootstrap();
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state.clone());
        // 1. 无凭据访问一个不存在的资源：401 而不是 404。
        let anonymous = Call::new(Method::GET, "/api/v1/profiles/01ARZ3NDEKTSV4RRFFQ69G5FAV")
            .send(app.clone())
            .await;
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(body_json(anonymous).await["code"], "AUTH_REQUIRED");

        // 2. 已认证但缺少 scope：403 在资源存在性之前返回。
        let mut limited = mien_server::auth::Principal::admin();
        limited.scopes.retain(|scope| scope != "profiles.read");
        let denied = limited
            .require_scope("profiles.read")
            .expect_err("scope must be required");
        assert_eq!(denied.code, mien_core::ErrorCode::ForbiddenScope);
        assert_eq!(denied.status, StatusCode::FORBIDDEN);

        // 3. 已认证且已授权，但资源不存在：路由层 404。
        let (token, csrf, _) = login(app.clone()).await;
        let changed = Call::new(Method::POST, "/api/v1/auth/password")
            .cookie(&token)
            .origin("http://127.0.0.1:8787")
            .csrf(&csrf)
            .json(serde_json::json!({
                "current_password": ADMIN_PASSWORD,
                "new_password": "a brand new passphrase",
            }))
            .send(app.clone())
            .await;
        assert_eq!(changed.status(), StatusCode::NO_CONTENT);
        let missing = Call::new(Method::GET, "/api/v1/profiles/01ARZ3NDEKTSV4RRFFQ69G5FAV")
            .cookie(&token)
            .send(app)
            .await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    });
}

#[test]
fn the_bootstrap_environment_variables_are_honoured() {
    // MIEN_ADMIN_PASSWORD 优先于 MIEN_ADMIN_PASSWORD_FILE。
    let dir = TempDir::new().expect("temp dir");
    let bootstrap = mien_server::auth::password::resolve_bootstrap(dir.path(), |key| match key {
        ADMIN_PASSWORD_ENV => Some("env-secret".to_owned()),
        ADMIN_PASSWORD_FILE_ENV => Some("/nonexistent".to_owned()),
        _ => None,
    })
    .expect("inline bootstrap");
    assert_eq!(bootstrap.source(), BootstrapSource::Environment);
    assert_eq!(bootstrap.expose(), "env-secret");

    // MIEN_ADMIN_PASSWORD_FILE 必须是 0600；内容尾部换行被去除。
    let file = dir.path().join("admin-password");
    std::fs::write(&file, "file-secret\n").expect("write");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).expect("chmod");
    }
    let from_file = mien_server::auth::password::resolve_bootstrap(dir.path(), |key| match key {
        ADMIN_PASSWORD_FILE_ENV => Some(file.display().to_string()),
        _ => None,
    })
    .expect("file bootstrap");
    assert_eq!(from_file.source(), BootstrapSource::File);
    assert_eq!(from_file.expose(), "file-secret");
}

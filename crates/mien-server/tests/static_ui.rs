//! 同源静态 Web UI 验收（`REQ-API-004`、`REQ-API-015`）。
//!
//! 每个用例用 `TempDir` 自造静态根，不依赖仓库根的 `dist/`：那里是否构建过
//! 不能决定验收结果。覆盖存在与缺失两种启动形态、SPA 深链接、API 面隔离、
//! 路径穿越与安全头。

use std::path::Path;

use axum::body::Body;
use axum::http::{HeaderValue, Request, StatusCode, header};
use axum::response::Response;
use mien_server::router;
use mien_server::state::{AppState, AuthState, SecuritySettings};
use tempfile::TempDir;
use tower::ServiceExt;

const INDEX_HTML: &str = "<!doctype html><html><body><div id=\"root\"></div></body></html>";
const APP_JS: &str = "console.log('mien');";
const APP_CSS: &str = "body{margin:0}";
const ADMIN_PASSWORD: &str = "correct horse battery staple";

/// 与真实服务一致的多线程运行时；静态读取走 `tokio::fs`。
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime")
}

/// 一个已构建好的最小静态根，布局与 Vite 产物一致。
fn web_dist() -> TempDir {
    let dir = TempDir::new().expect("temp dir");
    let root = dir.path();
    std::fs::write(root.join("index.html"), INDEX_HTML).expect("index");
    std::fs::create_dir_all(root.join("assets")).expect("assets");
    std::fs::write(root.join("assets/index-abc123.js"), APP_JS).expect("js");
    std::fs::write(root.join("assets/index-abc123.css"), APP_CSS).expect("css");
    std::fs::write(root.join("manifest.webmanifest"), "{}").expect("manifest");
    dir
}

/// 数据根与静态根分离：静态根缺失也不能影响数据库与健康检查。
fn state(web: Option<&Path>) -> (TempDir, AppState) {
    let dir = TempDir::new().expect("temp dir");
    let pool = mien_core::storage::open_and_migrate(&dir.path().join("mien.db")).expect("migrate");
    let auth = AuthState::default().with_bootstrap(mien_server::auth::password::Bootstrap::inline(
        ADMIN_PASSWORD,
    ));
    let state = AppState::new(dir.path(), Some(pool))
        .with_security(SecuritySettings::loopback())
        .with_auth(auth)
        .with_web_dist(web.map(Path::to_path_buf));
    (dir, state)
}

/// 登录并完成强制改密，返回可直接使用的会话 Cookie。
async fn login(app: axum::Router) -> String {
    let response = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/login")
        .header(header::ORIGIN, "http://127.0.0.1:8787")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(format!(r#"{{"password":"{ADMIN_PASSWORD}"}}"#)))
        .expect("request");
    let response = app.clone().oneshot(response).await.expect("login");
    assert_eq!(response.status(), StatusCode::OK, "login must succeed");
    let cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .expect("set-cookie")
        .to_str()
        .expect("ascii")
        .split(';')
        .next()
        .and_then(|pair| pair.split_once('='))
        .map(|(_, value)| value.to_owned())
        .expect("session cookie");
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("body");
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("login body");
    let csrf = body["csrf_token"].as_str().expect("csrf token");
    let changed = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/password")
        .header(header::ORIGIN, "http://127.0.0.1:8787")
        .header(header::CONTENT_TYPE, "application/json")
        .header(
            header::COOKIE,
            format!("{}={cookie}", mien_server::auth::session::COOKIE_NAME),
        )
        .header(
            header::HeaderName::from_static(mien_server::auth::csrf::CSRF_HEADER),
            csrf,
        )
        .body(Body::from(format!(
            r#"{{"current_password":"{ADMIN_PASSWORD}","new_password":"a brand new passphrase"}}"#
        )))
        .expect("request");
    let changed = app.oneshot(changed).await.expect("password change");
    assert_eq!(changed.status(), StatusCode::NO_CONTENT, "password change");
    cookie
}

struct Call {
    method: &'static str,
    path: &'static str,
    accept: Option<HeaderValue>,
    cookie: Option<String>,
}

impl Call {
    fn get(path: &'static str) -> Self {
        Self {
            method: "GET",
            path,
            accept: None,
            cookie: None,
        }
    }

    fn accept(mut self, accept: &'static str) -> Self {
        self.accept = Some(HeaderValue::from_static(accept));
        self
    }

    fn cookie(mut self, session: &str) -> Self {
        self.cookie = Some(session.to_owned());
        self
    }

    async fn send(self, app: axum::Router) -> Response {
        let mut builder = Request::builder().method(self.method).uri(self.path);
        if let Some(accept) = self.accept {
            builder = builder.header(header::ACCEPT, accept);
        }
        if let Some(cookie) = self.cookie {
            builder = builder.header(
                header::COOKIE,
                format!("{}={cookie}", mien_server::auth::session::COOKIE_NAME),
            );
        }
        app.oneshot(builder.body(Body::empty()).expect("request"))
            .await
            .expect("response")
    }
}

async fn body_text(response: Response) -> (StatusCode, String) {
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("body");
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

fn header_value(response: &Response, name: header::HeaderName) -> &str {
    response
        .headers()
        .get(&name)
        .unwrap_or_else(|| panic!("missing {name}"))
        .to_str()
        .expect("ascii")
}

/// 安全头逐项断言：静态响应必须与 API 响应同样受保护。
fn assert_security_headers(response: &Response) {
    let csp = header_value(response, header::CONTENT_SECURITY_POLICY);
    for directive in [
        "default-src 'self'",
        "script-src 'self'",
        "style-src 'self'",
        "connect-src 'self'",
        "object-src 'none'",
        "base-uri 'self'",
        "form-action 'self'",
        "frame-ancestors 'none'",
    ] {
        assert!(csp.contains(directive), "csp misses {directive}: {csp}");
    }
    assert!(
        !csp.contains("'unsafe-inline'"),
        "inline is not allowed: {csp}"
    );
    assert_eq!(
        header_value(response, header::CONTENT_SECURITY_POLICY),
        mien_server::CONTENT_SECURITY_POLICY
    );
    assert_eq!(
        header_value(response, header::X_CONTENT_TYPE_OPTIONS),
        "nosniff"
    );
    assert_eq!(
        header_value(response, header::REFERRER_POLICY),
        "no-referrer"
    );
    assert_eq!(header_value(response, header::X_FRAME_OPTIONS), "DENY");
}

#[test]
fn index_is_served_as_html_with_a_strict_csp() {
    let web = web_dist();
    let (_data, state) = state(Some(web.path()));
    let rt = runtime();
    rt.block_on(async move {
        let response = Call::get("/").send(router(state)).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            header_value(&response, header::CONTENT_TYPE),
            "text/html; charset=utf-8"
        );
        assert_eq!(header_value(&response, header::CACHE_CONTROL), "no-cache");
        assert_security_headers(&response);
        let (_, text) = body_text(response).await;
        assert_eq!(text, INDEX_HTML);
    });
}

#[test]
fn hashed_assets_are_immutable_and_typed() {
    let web = web_dist();
    let (_data, state) = state(Some(web.path()));
    let rt = runtime();
    rt.block_on(async move {
        let response = Call::get("/assets/index-abc123.js")
            .send(router(state.clone()))
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            header_value(&response, header::CONTENT_TYPE),
            "text/javascript; charset=utf-8"
        );
        let cache = header_value(&response, header::CACHE_CONTROL);
        assert!(cache.contains("immutable"), "cache: {cache}");
        assert!(cache.contains("max-age=31536000"), "cache: {cache}");
        assert_security_headers(&response);
        let (_, text) = body_text(response).await;
        assert_eq!(text, APP_JS);

        let css = Call::get("/assets/index-abc123.css")
            .send(router(state.clone()))
            .await;
        assert_eq!(css.status(), StatusCode::OK);
        assert_eq!(
            header_value(&css, header::CONTENT_TYPE),
            "text/css; charset=utf-8"
        );
        let (_, text) = body_text(css).await;
        assert_eq!(text, APP_CSS);

        let manifest = Call::get("/manifest.webmanifest").send(router(state)).await;
        assert_eq!(manifest.status(), StatusCode::OK);
        assert_eq!(
            header_value(&manifest, header::CACHE_CONTROL),
            "no-cache",
            "only assets/* may be cached immutably"
        );
    });
}

#[test]
fn spa_deep_links_fall_back_to_index_for_navigations() {
    let web = web_dist();
    let (_data, state) = state(Some(web.path()));
    let rt = runtime();
    rt.block_on(async move {
        let response = Call::get("/profiles/abc")
            .accept("text/html")
            .send(router(state.clone()))
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            header_value(&response, header::CONTENT_TYPE),
            "text/html; charset=utf-8"
        );
        assert_eq!(header_value(&response, header::CACHE_CONTROL), "no-cache");
        assert_security_headers(&response);
        let (_, text) = body_text(response).await;
        assert_eq!(text, INDEX_HTML);

        // 非导航请求（例如取数据）不得拿到 HTML。
        let api_shaped = Call::get("/profiles/abc")
            .accept("application/json")
            .send(router(state))
            .await;
        assert_eq!(api_shaped.status(), StatusCode::NOT_FOUND);
    });
}

#[test]
fn unknown_api_paths_stay_json_even_with_an_html_accept() {
    let web = web_dist();
    let (_data, app_state) = state(Some(web.path()));
    let rt = runtime();
    rt.block_on(async move {
        let app = router(app_state);
        let session = login(app.clone()).await;
        for path in [
            "/api/v1/nope",
            "/api/v1/tokens/nope/extra",
            "/api/nope",
            "/api%2fv1/nope",
        ] {
            let response = Call::get(path)
                .accept("text/html,application/xhtml+xml")
                .cookie(&session)
                .send(app.clone())
                .await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
            let content_type = header_value(&response, header::CONTENT_TYPE);
            assert!(
                content_type.starts_with("application/json"),
                "{path} must not return HTML: {content_type}"
            );
            assert_security_headers(&response);
            let (_, text) = body_text(response).await;
            assert!(
                !text.contains("<!doctype html") && !text.contains("<html"),
                "{path} returned the SPA shell: {text}"
            );
            let body: serde_json::Value =
                serde_json::from_str(&text).expect("error envelope must be JSON");
            assert_eq!(body["code"], "VALIDATION_FAILED");
            assert!(body["requestId"].as_str().is_some_and(|id| !id.is_empty()));
        }

        // 匿名请求先被认证门禁拦下：仍是 JSON，不能变成 UI。
        let anonymous = Call::get("/api/v1/nope")
            .accept("text/html")
            .send(app.clone())
            .await;
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
        assert!(header_value(&anonymous, header::CONTENT_TYPE).starts_with("application/json"));

        // 缺失的静态资源也不能伪装成 SPA 页面。
        let missing_asset = Call::get("/assets/missing-abc.js")
            .accept("text/html")
            .send(app)
            .await;
        assert_eq!(missing_asset.status(), StatusCode::NOT_FOUND);
    });
}

#[test]
fn path_traversal_never_escapes_the_static_root() {
    let web = web_dist();
    // 静态根外放一个可识别的文件：穿越必须 404 且正文不得泄露其内容。
    let outside = TempDir::new().expect("outside dir");
    std::fs::write(
        outside.path().join("secret.txt"),
        "[package]\nname = \"outside\"\n",
    )
    .expect("outside file");
    let (_data, state) = state(Some(web.path()));
    let rt = runtime();
    rt.block_on(async move {
        for path in [
            "/../Cargo.toml",
            "/assets/%2e%2e/Cargo.toml",
            "/%2e%2e/README.md",
            "/assets/..%2f..%2fCargo.toml",
            "/..%2fCargo.toml",
            "/%2e%2e%2fsecret.txt",
        ] {
            let response = Call::get(path)
                .accept("text/html")
                .send(router(state.clone()))
                .await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
            let (_, text) = body_text(response).await;
            for needle in ["[package]", "name = \"outside\"", "README"] {
                assert!(
                    !text.contains(needle),
                    "{path} leaked file content through: {text}"
                );
            }
        }
    });
}

#[test]
fn a_missing_dist_still_starts_and_keeps_the_api_alive() {
    let missing = TempDir::new().expect("temp dir");
    let absent = missing.path().join("not-built");
    let (_data, state) = state(Some(&absent));
    assert!(
        state.web_dist().is_none(),
        "missing dist must disable the UI"
    );

    // 未显式配置的构造路径也必须能启动，快照不因缺 dist 失败。
    let plain = TempDir::new().expect("temp dir");
    let pool =
        mien_core::storage::open_and_migrate(&plain.path().join("mien.db")).expect("migrate");
    let default_state = AppState::new(plain.path(), Some(pool))
        .with_web_dist(None)
        .with_auth(AuthState::default().with_bootstrap(
            mien_server::auth::password::Bootstrap::inline(ADMIN_PASSWORD),
        ));

    let rt = runtime();
    rt.block_on(async move {
        let app = router(state.clone());
        let index = Call::get("/").send(app.clone()).await;
        assert_eq!(index.status(), StatusCode::NOT_FOUND);

        let health = Call::get("/healthz").send(app.clone()).await;
        assert_eq!(health.status(), StatusCode::OK);
        assert_security_headers(&health);

        let discovery = Call::get("/api/v1").send(app.clone()).await;
        assert_eq!(discovery.status(), StatusCode::OK);

        let session = login(app.clone()).await;
        let unknown = Call::get("/api/v1/nope")
            .accept("text/html")
            .cookie(&session)
            .send(app)
            .await;
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
        assert!(header_value(&unknown, header::CONTENT_TYPE).starts_with("application/json"));

        let response = Call::get("/healthz").send(router(default_state)).await;
        assert_eq!(response.status(), StatusCode::OK);
    });
}

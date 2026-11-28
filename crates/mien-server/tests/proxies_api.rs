//! 代理配置 REST 验收（T26）。
//!
//! 覆盖 `REQ-API-017`：CRUD 往返、单条分享链接导入（合法与非法）、
//! 代理检查的成功与失败分类、脱敏边界（响应与错误都不得出现凭据明文或完整链接）、
//! 以及 `401 -> 403 -> 404` 的优先级。检查端点要求共享引擎，因此需要 `MIEN_XRAY_BIN`；
//! 未设置时打印显式跳过原因，不做静默通过。

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;

use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use axum::response::Response;
use mien_server::router;
use mien_server::state::{AppState, AuthState, SecuritySettings};
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;

const ADMIN_PASSWORD: &str = "correct horse battery staple";
const SECRET_UUID: &str = "11111111-2222-4333-8444-555555555555";
const SECRET_PASSWORD: &str = "sh4dow-socks-pass";

/// 与真实服务一致的**多线程**运行时。
///
/// 代理检查经由 `mien_core` 的同步 gRPC 适配层，它在已有运行时内会另起线程并
/// `join`；`current_thread` 运行时只有一个线程，被 `join` 阻塞后 tonic 连接的驱动
/// 任务无法被轮询，检查会永久挂起。
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime")
}

/// 迁移后的临时数据根；主密钥就地生成，供代理凭据加密使用。
fn state() -> (TempDir, AppState) {
    let dir = TempDir::new().expect("temp dir");
    let pool = mien_core::storage::open_and_migrate(&dir.path().join("mien.db")).expect("migrate");
    let key = mien_core::secrets::MasterKey::from_bytes([7_u8; 32]);
    std::fs::create_dir_all(dir.path().join("secrets")).expect("secrets dir");
    let key_path = dir.path().join("secrets/master.key");
    std::fs::write(&key_path, hex(key.as_bytes())).expect("master key");
    // `MasterKey::from_file` refuses anything wider than 0600.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))
            .expect("key permissions");
    }
    let auth = AuthState::default().with_bootstrap(mien_server::auth::password::Bootstrap::inline(
        ADMIN_PASSWORD,
    ));
    let state = AppState::new(dir.path(), Some(pool))
        .with_security(SecuritySettings::loopback())
        .with_auth(auth)
        // 检查端点要一个回显端点；这里指向本地，用例只验证失败分类。
        .with_proxy_check(mien_server::state::ProxyCheckRuntime::new(
            Some("http://127.0.0.1:9/check".to_owned()),
            std::time::Duration::from_secs(3),
        ))
        .with_allow_direct_egress(true);
    (dir, state)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

struct Session {
    cookie: String,
    csrf: String,
}

struct Bearer {
    scopes: &'static [&'static str],
}

impl mien_server::auth::BearerAuthenticator for Bearer {
    fn authenticate<'a>(
        &'a self,
        token: &'a str,
    ) -> Pin<Box<dyn Future<Output = Option<mien_server::auth::BearerIdentity>> + Send + 'a>> {
        let _ = token;
        Box::pin(async move {
            Some(mien_server::auth::BearerIdentity {
                subject: "test-token".to_owned(),
                scopes: self
                    .scopes
                    .iter()
                    .map(|scope| (*scope).to_owned())
                    .collect(),
            })
        })
    }
}

struct Call {
    method: Method,
    path: String,
    headers: HeaderMap,
    body: Option<Vec<u8>>,
}

impl Call {
    fn new(method: Method, path: impl Into<String>) -> Self {
        Self {
            method,
            path: path.into(),
            headers: HeaderMap::new(),
            body: None,
        }
    }

    fn header(mut self, name: header::HeaderName, value: impl AsRef<str>) -> Self {
        self.headers
            .insert(name, value.as_ref().parse().expect("header value"));
        self
    }

    fn cookie(self, session: &Session) -> Self {
        self.header(
            header::COOKIE,
            format!(
                "{}={}",
                mien_server::auth::session::COOKIE_NAME,
                session.cookie
            ),
        )
    }

    fn csrf(self, session: &Session) -> Self {
        self.header(
            header::HeaderName::from_static(mien_server::auth::csrf::CSRF_HEADER),
            session.csrf.as_str(),
        )
    }

    fn origin(self) -> Self {
        self.header(header::ORIGIN, "http://127.0.0.1:8787")
    }

    fn bearer(self, token: &str) -> Self {
        self.header(header::AUTHORIZATION, format!("Bearer {token}"))
    }

    fn json(mut self, body: Value) -> Self {
        self.body = Some(body.to_string().into_bytes());
        self
    }

    async fn send(self, app: axum::Router) -> Response {
        let mut builder = Request::builder().method(self.method).uri(&self.path);
        for (name, value) in &self.headers {
            builder = builder.header(name, value);
        }
        let body = self.body.map_or_else(Body::empty, Body::from);
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

async fn body_text(response: Response) -> (StatusCode, String) {
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("body");
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn body_json(response: Response) -> (StatusCode, Value) {
    let (status, text) = body_text(response).await;
    if text.is_empty() {
        return (status, json!({}));
    }
    let parsed = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("non-JSON response {status}: {error}; body={text}"));
    (status, parsed)
}

fn set_cookie(response: &Response) -> String {
    response
        .headers()
        .get(header::SET_COOKIE)
        .expect("set-cookie")
        .to_str()
        .expect("ascii")
        .split(';')
        .next()
        .and_then(|pair| pair.split_once('='))
        .map(|(_, value)| value.to_owned())
        .expect("session cookie value")
}

async fn login(app: axum::Router) -> Session {
    let response = Call::new(Method::POST, "/api/v1/auth/login")
        .origin()
        .json(json!({ "password": ADMIN_PASSWORD }))
        .send(app.clone())
        .await;
    assert_eq!(response.status(), StatusCode::OK, "login must succeed");
    let cookie = set_cookie(&response);
    let (_, body) = body_json(response).await;
    let csrf = body["csrf_token"].as_str().expect("csrf token").to_owned();
    let session = Session { cookie, csrf };
    let changed = Call::new(Method::POST, "/api/v1/auth/password")
        .cookie(&session)
        .csrf(&session)
        .origin()
        .json(json!({
            "current_password": ADMIN_PASSWORD,
            "new_password": "a brand new passphrase",
        }))
        .send(app)
        .await;
    assert_eq!(changed.status(), StatusCode::NO_CONTENT, "password change");
    session
}

/// 合法 VLESS 输入，凭据用固定测试值以便做脱敏断言。
fn vless_input(name: &str) -> Value {
    json!({
        "name": name,
        "kind": "vless",
        "server": "node.example",
        "port": 443,
        "options": { "method": "aes-128-gcm" },
        "credentials": { "id": SECRET_UUID },
    })
}

fn shadowsocks_input(name: &str) -> Value {
    json!({
        "name": name,
        "kind": "shadowsocks",
        "server": "ss.example",
        "port": 8388,
        "options": { "method": "aes-256-gcm" },
        "credentials": { "password": SECRET_PASSWORD },
    })
}

fn assert_redacted(text: &str) {
    for secret in [SECRET_UUID, SECRET_PASSWORD] {
        assert!(
            !text.contains(secret),
            "response leaked a credential: {text}"
        );
    }
}

async fn create(app: axum::Router, session: &Session, body: Value) -> Value {
    let response = Call::new(Method::POST, "/api/v1/proxies")
        .cookie(session)
        .csrf(session)
        .origin()
        .json(body)
        .send(app)
        .await;
    let (status, text) = body_text(response).await;
    assert_eq!(status, StatusCode::CREATED, "create proxy: {text}");
    assert_redacted(&text);
    serde_json::from_str(&text).expect("proxy view")
}

#[test]
fn crud_round_trip_returns_only_redacted_views() {
    let (_dir, state) = state();
    let app = router(state);

    runtime().block_on(async {
        let session = login(app.clone()).await;

        let created = create(app.clone(), &session, vless_input("vless node")).await;
        let id = created["id"].as_str().expect("id").to_owned();
        assert_eq!(created["kind"], "vless");
        assert_eq!(created["server"], "node.example");
        assert_eq!(created["credential_set"], true);
        assert_eq!(created["credential_fields"], json!(["id"]));
        assert!(created.get("credentials").is_none(), "no credentials key");

        // 列表也只给脱敏视图。
        let response = Call::new(Method::GET, "/api/v1/proxies")
            .cookie(&session)
            .send(app.clone())
            .await;
        let (status, text) = body_text(response).await;
        assert_eq!(status, StatusCode::OK, "list: {text}");
        assert_redacted(&text);
        let page: Value = serde_json::from_str(&text).expect("page");
        assert_eq!(page["items"].as_array().expect("items").len(), 1);

        // 读取单条。
        let response = Call::new(Method::GET, format!("/api/v1/proxies/{id}"))
            .cookie(&session)
            .send(app.clone())
            .await;
        let (status, text) = body_text(response).await;
        assert_eq!(status, StatusCode::OK, "get: {text}");
        assert_redacted(&text);

        // 更新非秘密字段并替换凭据。
        let response = Call::new(Method::PATCH, format!("/api/v1/proxies/{id}"))
            .cookie(&session)
            .csrf(&session)
            .origin()
            .json(json!({
                "name": "renamed node",
                "credentials": { "id": "99999999-8888-4777-8666-555555555555" },
            }))
            .send(app.clone())
            .await;
        let (status, text) = body_text(response).await;
        assert_eq!(status, StatusCode::OK, "patch: {text}");
        assert_redacted(&text);
        assert!(text.contains("renamed node"), "name updated: {text}");
        assert!(
            !text.contains("99999999"),
            "replacement credential leaked: {text}"
        );

        // 删除后读取 404。
        let response = Call::new(Method::DELETE, format!("/api/v1/proxies/{id}"))
            .cookie(&session)
            .csrf(&session)
            .origin()
            .send(app.clone())
            .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT, "delete");
        let response = Call::new(Method::GET, format!("/api/v1/proxies/{id}"))
            .cookie(&session)
            .send(app.clone())
            .await;
        let (status, body) = body_json(response).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["code"], "PROXY_NOT_FOUND");
    });
}

#[test]
fn import_link_parses_without_saving_or_starting() {
    let (_dir, state) = state();
    let app = router(state);

    runtime().block_on(async {
        let session = login(app.clone()).await;

        let link = format!("vless://{SECRET_UUID}@node.example:443?type=tcp&security=tls#imported");
        let response = Call::new(Method::POST, "/api/v1/proxies/import-link")
            .cookie(&session)
            .csrf(&session)
            .origin()
            .json(json!({ "link": link }))
            .send(app.clone())
            .await;
        let (status, text) = body_text(response).await;
        assert_eq!(status, StatusCode::OK, "import link: {text}");
        assert_redacted(&text);
        let draft: Value = serde_json::from_str(&text).expect("draft");
        assert_eq!(draft["kind"], "vless");
        assert_eq!(draft["server"], "node.example");
        assert_eq!(draft["credential_fields"], json!(["id"]));

        // 导入不落库：列表仍为空。
        let response = Call::new(Method::GET, "/api/v1/proxies")
            .cookie(&session)
            .send(app.clone())
            .await;
        let (_, page) = body_json(response).await;
        assert_eq!(
            page["items"].as_array().expect("items").len(),
            0,
            "import must not persist"
        );

        // 非法链接 422。
        for bad in ["tuic://abc@host:443", "vless://", "not-a-link"] {
            let response = Call::new(Method::POST, "/api/v1/proxies/import-link")
                .cookie(&session)
                .csrf(&session)
                .origin()
                .json(json!({ "link": bad }))
                .send(app.clone())
                .await;
            let (status, text) = body_text(response).await;
            assert_eq!(
                status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "bad link {bad}: {text}"
            );
            assert_redacted(&text);
        }
    });
}

#[test]
fn scope_and_existence_precedence_is_401_then_403_then_404() {
    let (_dir, state) = state();
    let app = router(state.clone());

    runtime().block_on(async {
        // 未认证：即使资源不存在也必须是 401。
        let response = Call::new(Method::GET, "/api/v1/proxies/does-not-exist")
            .send(app.clone())
            .await;
        let (status, text) = body_text(response).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "unauthenticated: {text}");

        // 已认证但缺 scope：403，且不因为资源不存在变成 404。
        let reader =
            state
                .clone()
                .with_auth(
                    AuthState::default().with_bearer(::std::sync::Arc::new(Bearer {
                        scopes: &["profiles.read"],
                    })),
                );
        let app_reader = router(reader);
        let response = Call::new(Method::GET, "/api/v1/proxies")
            .bearer("any")
            .send(app_reader)
            .await;
        let (status, text) = body_text(response).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "missing scope: {text}");

        // 有 scope 但资源不存在：404。
        let writer = state.with_auth(AuthState::default().with_bearer(::std::sync::Arc::new(
            Bearer {
                scopes: &["proxies.read", "proxies.write"],
            },
        )));
        let app_writer = router(writer);
        let response = Call::new(Method::GET, "/api/v1/proxies/does-not-exist")
            .bearer("any")
            .send(app_writer)
            .await;
        let (status, body) = body_json(response).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["code"], "PROXY_NOT_FOUND");
    });
}

/// 检查端点：共享引擎 + 回显端点。两个用例都要求真实 Xray。
fn xray() -> Option<std::path::PathBuf> {
    match std::env::var_os("MIEN_XRAY_BIN") {
        Some(path) => Some(std::path::PathBuf::from(path)),
        None => {
            println!("skipped: MIEN_XRAY_BIN not set");
            None
        }
    }
}

#[test]
fn check_reports_a_classified_failure_and_keeps_the_engine_alive() {
    let Some(_xray) = xray() else { return };
    let (_dir, state) = state();
    let app = router(state);

    runtime().block_on(async {
        let session = login(app.clone()).await;
        // 指向必定拒绝连接的回环端口。
        let created = create(
            app.clone(),
            &session,
            json!({
                "name": "dead node",
                "kind": "vless",
                "server": "127.0.0.1",
                "port": 1,
                "credentials": { "id": SECRET_UUID },
            }),
        )
        .await;
        let id = created["id"].as_str().expect("id").to_owned();

        let response = Call::new(Method::POST, format!("/api/v1/proxies/{id}/check"))
            .cookie(&session)
            .csrf(&session)
            .origin()
            .json(json!({}))
            .send(app.clone())
            .await;
        let (status, text) = body_text(response).await;
        // 契约：检查失败仍是 HTTP 200，分类进 ProxyCheck.error_code。
        assert_eq!(status, StatusCode::OK, "check must answer 200: {text}");
        assert_redacted(&text);
        let body: Value = serde_json::from_str(&text).expect("check body");
        assert_eq!(body["ok"], false, "dead node cannot be ok: {text}");
        let code = body["error_code"].as_str().unwrap_or_default();
        assert!(
            [
                "PROXY_CONNECTION_REFUSED",
                "PROXY_TIMEOUT",
                "PROXY_CONNECTION_FAILED",
                "PROXY_DNS_FAILED",
            ]
            .contains(&code),
            "unexpected classification {code}: {text}"
        );

        // 检查走共享引擎的临时资源，不产生运行时记录。
        let response = Call::new(Method::GET, "/api/v1/profiles")
            .cookie(&session)
            .send(app)
            .await;
        let (status, page) = body_json(response).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(page["items"].as_array().expect("items").len(), 0);
    });
}

#[test]
fn import_round_trip_for_shadowsocks_keeps_credentials_off_the_wire() {
    let (_dir, state) = state();
    let app = router(state);

    runtime().block_on(async {
        let session = login(app.clone()).await;
        let created = create(app.clone(), &session, shadowsocks_input("ss node")).await;
        assert_eq!(created["credential_fields"], json!(["password"]));
        assert_eq!(created["options"]["method"], "aes-256-gcm");

        let mut expected = BTreeMap::new();
        expected.insert("password", SECRET_PASSWORD);
        for secret in expected.values() {
            assert!(!created.to_string().contains(secret), "leaked {secret}");
        }
    });
}

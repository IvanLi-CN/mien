//! API Token 验收（T23）：哈希存储、scope 强制、一次性明文与幂等冲突。
//!
//! 覆盖 `REQ-API-005`/`010` 与 `VER-API-005`/`010`：创建→使用→撤销→失效全链路、
//! `last_used_at` 刷新、越权 403、`token`/`ticket` 请求体字段 422、重复幂等键 409
//! 且不返回明文、常量时间路径对任意伪造值只返回 401，以及数据库文件字节中不存在
//! Token 明文。`x-required-scope` 与 OpenAPI 的逐项比对也在本文件内完成。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use axum::response::Response;
use mien_core::secrets::MasterKey;
use mien_server::auth::password::Bootstrap;
use mien_server::auth::session::COOKIE_NAME;
use mien_server::router;
use mien_server::state::{AppState, AuthState, SecuritySettings};
use mien_server::tokens::scope::{ROUTES, TokenScope};
use mien_server::tokens::store::TokenStore;
use tempfile::TempDir;
use tower::ServiceExt;

const ADMIN_PASSWORD: &str = "correct horse battery staple";
const MASTER_KEY: [u8; 32] = [7u8; 32];

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

/// 迁移后的临时数据根，带引导口令与从固定主密钥派生的 Token 存储。
fn state() -> (TempDir, AppState) {
    let dir = TempDir::new().expect("temp dir");
    let pool = mien_core::storage::open_and_migrate(&dir.path().join("mien.db")).expect("migrate");
    let store = TokenStore::from_master_key(pool.clone(), &MasterKey::from_bytes(MASTER_KEY));
    let auth = AuthState::default().with_bootstrap(Bootstrap::inline(ADMIN_PASSWORD));
    let state = AppState::new(dir.path(), Some(pool))
        .with_security(SecuritySettings::loopback())
        .with_auth(auth)
        .with_tokens(Some(Arc::new(store)));
    (dir, state)
}

struct Call {
    method: Method,
    path: String,
    headers: HeaderMap,
    body: Option<serde_json::Value>,
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

    fn cookie(self, token: &str) -> Self {
        self.header(header::COOKIE, format!("{COOKIE_NAME}={token}"))
    }

    fn origin(self) -> Self {
        self.header(header::ORIGIN, "http://127.0.0.1:8787")
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

    fn idempotency(self, key: &str) -> Self {
        self.header(
            header::HeaderName::from_static(mien_server::tokens::IDEMPOTENCY_KEY_HEADER),
            key,
        )
    }

    fn json(mut self, body: serde_json::Value) -> Self {
        self.body = Some(body);
        self
    }

    async fn send(self, app: axum::Router) -> Response {
        let mut builder = Request::builder().method(self.method).uri(&self.path);
        for (name, value) in &self.headers {
            builder = builder.header(name, value);
        }
        let body = match &self.body {
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
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("body");
    if bytes.is_empty() {
        return serde_json::json!({ "status": status.as_u16() });
    }
    serde_json::from_slice(&bytes).expect("json body")
}

/// 登录、完成引导改密并返回 (会话 Cookie 值, CSRF Token)。
///
/// 引导口令首次登录后 `must_change_password=true`，未改密前除改密与登出外的写操作
/// 都是 `403 AUTH_PASSWORD_CHANGE_REQUIRED`，因此 Token 创建必须先完成改密。
async fn login(app: axum::Router) -> (String, String) {
    let response = Call::new(Method::POST, "/api/v1/auth/login")
        .json(serde_json::json!({ "password": ADMIN_PASSWORD }))
        .send(app.clone())
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .expect("set-cookie")
        .to_str()
        .expect("ascii");
    let token = cookie
        .split(';')
        .next()
        .and_then(|pair| pair.split_once('='))
        .map(|(_, value)| value.to_owned())
        .expect("cookie value");
    let csrf = body_json(response).await["csrf_token"]
        .as_str()
        .expect("csrf token")
        .to_owned();
    let changed = Call::new(Method::POST, "/api/v1/auth/password")
        .cookie(&token)
        .origin()
        .csrf(&csrf)
        .json(serde_json::json!({
            "current_password": ADMIN_PASSWORD,
            "new_password": "a brand new passphrase",
        }))
        .send(app)
        .await;
    assert_eq!(changed.status(), StatusCode::NO_CONTENT);
    let _ = csrf;
    // 改密使其他会话失效，当前会话保留；CSRF Token 由 Cookie 派生，保持不变。
    let csrf = mien_server::auth::csrf::derive(&token);
    (token, csrf)
}

/// 用会话创建 Token，返回创建响应。
async fn create_with_session(
    app: axum::Router,
    cookie: &str,
    csrf: &str,
    body: serde_json::Value,
) -> Response {
    Call::new(Method::POST, "/api/v1/tokens")
        .cookie(cookie)
        .origin()
        .csrf(csrf)
        .json(body)
        .send(app)
        .await
}

#[test]
fn create_use_revoke_and_fail_closed() {
    let (_dir, state) = state();
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state.clone());
        let (cookie, csrf) = login(app.clone()).await;

        let created = create_with_session(
            app.clone(),
            &cookie,
            &csrf,
            serde_json::json!({
                "name": "  automation  ",
                "scopes": ["profiles.read", "tokens.manage"],
            }),
        )
        .await;
        assert_eq!(created.status(), StatusCode::CREATED);
        assert_eq!(
            created
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-store")
        );
        let created = body_json(created).await;
        let plaintext = created["token"]
            .as_str()
            .expect("one-time token")
            .to_owned();
        assert!(plaintext.starts_with("mien_"), "token: {plaintext}");
        assert_eq!(plaintext.len(), "mien_".len() + 64);
        assert_eq!(created["name"], "automation");
        assert_eq!(
            created["scopes"],
            serde_json::json!(["profiles.read", "tokens.manage"])
        );
        let prefix = created["prefix"].as_str().expect("prefix").to_owned();
        assert_eq!(prefix, &plaintext[..13]);
        assert!(plaintext.starts_with(&prefix));
        let id = created["id"].as_str().expect("id").to_owned();
        assert!(created["last_used_at"].is_null());
        assert!(created["revoked_at"].is_null());

        // 前缀不能用于认证；完整 Token 通过 Bearer 认证，并刷新 last_used_at。
        let with_prefix = Call::new(Method::GET, "/api/v1/tokens")
            .bearer(&prefix)
            .send(app.clone())
            .await;
        assert_eq!(with_prefix.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(body_json(with_prefix).await["code"], "AUTH_INVALID");

        let before = sqlx::query_scalar::<_, Option<String>>(
            "SELECT last_used_at FROM api_tokens WHERE id = ?1",
        )
        .bind(&id)
        .fetch_one(state.db().expect("pool"))
        .await
        .expect("row");
        assert!(before.is_none(), "last_used_at starts empty");

        let used = Call::new(Method::GET, "/api/v1/tokens")
            .bearer(&plaintext)
            .send(app.clone())
            .await;
        assert_eq!(used.status(), StatusCode::OK);
        let listed = body_json(used).await;
        let items = listed.as_array().expect("array");
        assert_eq!(items.len(), 1);
        assert!(
            items[0].get("token").is_none(),
            "list must not leak plaintext"
        );
        assert!(items[0].get("token_hash").is_none());

        let after = sqlx::query_scalar::<_, Option<String>>(
            "SELECT last_used_at FROM api_tokens WHERE id = ?1",
        )
        .bind(&id)
        .fetch_one(state.db().expect("pool"))
        .await
        .expect("row");
        assert!(
            after.is_some(),
            "last_used_at is refreshed by authentication"
        );
        assert_eq!(after.as_deref().map(str::len), Some(24));

        // 撤销后立即 401；重复撤销仍为 204；未知标识 404。
        let revoked = Call::new(Method::DELETE, format!("/api/v1/tokens/{id}"))
            .bearer(&plaintext)
            .send(app.clone())
            .await;
        assert_eq!(revoked.status(), StatusCode::NO_CONTENT);
        let after_revoke = Call::new(Method::GET, "/api/v1/tokens")
            .bearer(&plaintext)
            .send(app.clone())
            .await;
        assert_eq!(after_revoke.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(body_json(after_revoke).await["code"], "AUTH_INVALID");

        let again = Call::new(Method::DELETE, format!("/api/v1/tokens/{id}"))
            .cookie(&cookie)
            .origin()
            .csrf(&csrf)
            .send(app.clone())
            .await;
        assert_eq!(again.status(), StatusCode::NO_CONTENT);
        let missing = Call::new(Method::DELETE, "/api/v1/tokens/01ARZ3NDEKTSV4RRFFQ69G5FAV")
            .cookie(&cookie)
            .origin()
            .csrf(&csrf)
            .send(app.clone())
            .await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_json(missing).await["code"], "TOKEN_NOT_FOUND");
    });
}

#[test]
fn scopes_are_enforced_before_existence_and_cannot_escalate() {
    let (_dir, state) = state();
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state.clone());
        let (cookie, csrf) = login(app.clone()).await;

        let reader = body_json(
            create_with_session(
                app.clone(),
                &cookie,
                &csrf,
                serde_json::json!({ "name": "reader", "scopes": ["profiles.read"] }),
            )
            .await,
        )
        .await;
        let reader_token = reader["token"].as_str().expect("token").to_owned();

        // 缺少 profiles.write：403 在路由 404 之前返回（档案端点尚未实现）。
        let denied = Call::new(Method::POST, "/api/v1/profiles")
            .bearer(&reader_token)
            .json(serde_json::json!({}))
            .send(app.clone())
            .await;
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        let denied = body_json(denied).await;
        assert_eq!(denied["code"], "FORBIDDEN_SCOPE");
        assert_eq!(denied["details"]["required_scope"], "profiles.write");

        // 缺少 tokens.manage：不能列出、创建或撤销 Token。
        let list_denied = Call::new(Method::GET, "/api/v1/tokens")
            .bearer(&reader_token)
            .send(app.clone())
            .await;
        assert_eq!(list_denied.status(), StatusCode::FORBIDDEN);
        let create_denied = Call::new(Method::POST, "/api/v1/tokens")
            .bearer(&reader_token)
            .json(serde_json::json!({ "name": "x", "scopes": ["profiles.read"] }))
            .send(app.clone())
            .await;
        assert_eq!(create_denied.status(), StatusCode::FORBIDDEN);

        // 持有 profiles.write 的 Token 可以到达路由层（未实现的档案端点给 404）。
        let writer = body_json(
            create_with_session(
                app.clone(),
                &cookie,
                &csrf,
                serde_json::json!({ "name": "writer", "scopes": ["profiles.write"] }),
            )
            .await,
        )
        .await;
        let writer_token = writer["token"].as_str().expect("token").to_owned();
        let reached = Call::new(Method::POST, "/api/v1/profiles")
            .bearer(&writer_token)
            .json(serde_json::json!({}))
            .send(app.clone())
            .await;
        assert_eq!(
            reached.status(),
            StatusCode::NOT_FOUND,
            "authorized bearer request reaches routing"
        );

        // 不能授予创建者没有的 scope：受限 Token 尝试签发 profiles.write 时 403。
        let limited = body_json(
            create_with_session(
                app.clone(),
                &cookie,
                &csrf,
                serde_json::json!({ "name": "manager", "scopes": ["tokens.manage"] }),
            )
            .await,
        )
        .await;
        let limited_token = limited["token"].as_str().expect("token").to_owned();
        let escalated = Call::new(Method::POST, "/api/v1/tokens")
            .bearer(&limited_token)
            .json(serde_json::json!({ "name": "escalated", "scopes": ["profiles.write"] }))
            .send(app.clone())
            .await;
        assert_eq!(escalated.status(), StatusCode::FORBIDDEN);
        assert_eq!(body_json(escalated).await["code"], "FORBIDDEN_SCOPE");

        let granted = Call::new(Method::POST, "/api/v1/tokens")
            .bearer(&limited_token)
            .json(serde_json::json!({ "name": "subset", "scopes": ["tokens.manage"] }))
            .send(app.clone())
            .await;
        assert_eq!(granted.status(), StatusCode::CREATED);
    });
}

#[test]
fn one_time_secrets_are_rejected_as_request_fields() {
    let (_dir, state) = state();
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state.clone());
        let (cookie, csrf) = login(app.clone()).await;

        for field in ["token", "ticket"] {
            let response = create_with_session(
                app.clone(),
                &cookie,
                &csrf,
                serde_json::json!({
                    "name": "bad",
                    "scopes": ["profiles.read"],
                    field: "mien_smuggled",
                }),
            )
            .await;
            assert_eq!(
                response.status(),
                StatusCode::UNPROCESSABLE_ENTITY,
                "{field}"
            );
            let body = body_json(response).await;
            assert_eq!(body["code"], "VALIDATION_FAILED");
            assert_eq!(
                body["details"]["fields"][field],
                serde_json::json!(
                    "one-time secrets are response-only and not accepted in request bodies"
                )
            );
        }

        // 未知字段同样 422：`TokenInput` 是 `additionalProperties: false`。
        let unknown = create_with_session(
            app.clone(),
            &cookie,
            &csrf,
            serde_json::json!({ "name": "bad", "scopes": ["profiles.read"], "extra": true }),
        )
        .await;
        assert_eq!(unknown.status(), StatusCode::UNPROCESSABLE_ENTITY);

        // 空 scope 列表、重复 scope、超长名称与非法 scope 都是 422。
        for body in [
            serde_json::json!({ "name": "x", "scopes": [] }),
            serde_json::json!({ "name": "x", "scopes": ["profiles.read", "profiles.read"] }),
            serde_json::json!({ "name": "", "scopes": ["profiles.read"] }),
            serde_json::json!({ "name": "x".repeat(121), "scopes": ["profiles.read"] }),
            serde_json::json!({ "name": "x", "scopes": ["profiles.delete"] }),
        ] {
            let response = create_with_session(app.clone(), &cookie, &csrf, body.clone()).await;
            assert_eq!(
                response.status(),
                StatusCode::UNPROCESSABLE_ENTITY,
                "body: {body}"
            );
        }
    });
}

#[test]
fn a_repeated_idempotency_key_is_409_without_plaintext() {
    let (_dir, state) = state();
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state.clone());
        let (cookie, csrf) = login(app.clone()).await;
        let key = "token-create-key-0001";

        let first = Call::new(Method::POST, "/api/v1/tokens")
            .cookie(&cookie)
            .origin()
            .csrf(&csrf)
            .idempotency(key)
            .json(serde_json::json!({ "name": "idem", "scopes": ["profiles.read"] }))
            .send(app.clone())
            .await;
        assert_eq!(first.status(), StatusCode::CREATED);
        let first = body_json(first).await;
        assert!(first["token"].is_string());

        // 相同请求体、相同键：409 且不含明文、不含缓存的 201 响应体。
        let repeated = Call::new(Method::POST, "/api/v1/tokens")
            .cookie(&cookie)
            .origin()
            .csrf(&csrf)
            .idempotency(key)
            .json(serde_json::json!({ "name": "idem", "scopes": ["profiles.read"] }))
            .send(app.clone())
            .await;
        assert_eq!(repeated.status(), StatusCode::CONFLICT);
        let repeated_body = body_json(repeated).await;
        assert_eq!(repeated_body["code"], "IDEMPOTENCY_CONFLICT");
        assert!(repeated_body.get("token").is_none());

        // 不同请求体、相同键：同样 409（不区分内容），也不返回明文。
        let different = Call::new(Method::POST, "/api/v1/tokens")
            .cookie(&cookie)
            .origin()
            .csrf(&csrf)
            .idempotency(key)
            .json(serde_json::json!({ "name": "other", "scopes": ["profiles.write"] }))
            .send(app.clone())
            .await;
        assert_eq!(different.status(), StatusCode::CONFLICT);
        assert!(body_json(different).await.get("token").is_none());

        // 只落了一条 Token 行。
        let count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM api_tokens")
            .fetch_one(state.db().expect("pool"))
            .await
            .expect("count");
        assert_eq!(count, 1);
        // 幂等缓存不保存秘密。
        let cached = sqlx::query_scalar::<_, Option<String>>(
            "SELECT response_json FROM idempotency_keys WHERE key = ?1",
        )
        .bind(key)
        .fetch_one(state.db().expect("pool"))
        .await
        .expect("row");
        assert!(cached.is_none());

        // 另一个键可以正常创建；长度越界的键是 422。
        let fresh = Call::new(Method::POST, "/api/v1/tokens")
            .cookie(&cookie)
            .origin()
            .csrf(&csrf)
            .idempotency("token-create-key-0002")
            .json(serde_json::json!({ "name": "fresh", "scopes": ["profiles.read"] }))
            .send(app.clone())
            .await;
        assert_eq!(fresh.status(), StatusCode::CREATED);
        let short = Call::new(Method::POST, "/api/v1/tokens")
            .cookie(&cookie)
            .origin()
            .csrf(&csrf)
            .idempotency("short")
            .json(serde_json::json!({ "name": "fresh", "scopes": ["profiles.read"] }))
            .send(app.clone())
            .await;
        assert_eq!(short.status(), StatusCode::UNPROCESSABLE_ENTITY);
    });
}

#[test]
fn forged_tokens_of_any_shape_are_401_without_panicking() {
    let (_dir, state) = state();
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state.clone());
        let (cookie, csrf) = login(app.clone()).await;
        let created = body_json(
            create_with_session(
                app.clone(),
                &cookie,
                &csrf,
                serde_json::json!({ "name": "real", "scopes": ["profiles.read"] }),
            )
            .await,
        )
        .await;
        let real = created["token"].as_str().expect("token").to_owned();
        let prefix = created["prefix"].as_str().expect("prefix").to_owned();

        let cases = vec![
            String::new(),
            "mien_".to_owned(),
            prefix.clone(),
            format!("{real}x"),
            real.to_uppercase(),
            "mien_".to_owned() + &"0".repeat(64),
            "ü".repeat(64),
            "mien_".to_owned() + &"f".repeat(512),
            real[..real.len() - 1].to_owned(),
        ];
        for candidate in cases {
            let response = Call::new(Method::GET, "/api/v1/tokens")
                .bearer(&candidate)
                .send(app.clone())
                .await;
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "candidate length {}",
                candidate.len()
            );
            let body = body_json(response).await;
            assert_eq!(body["code"], "AUTH_INVALID");
            // 错误信封不得回显候选值。
            assert!(!body.to_string().contains(&candidate) || candidate.is_empty());
        }

        // 缺失 Bearer 前缀（只有 Scheme 或非 Bearer）也是 401。
        for raw in ["Bearer ", "Basic abc", "Token abc"] {
            let response = Call::new(Method::GET, "/api/v1/tokens")
                .header(header::AUTHORIZATION, raw)
                .send(app.clone())
                .await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
    });
}

#[test]
fn the_database_files_never_contain_the_plaintext_token() {
    let (dir, state) = state();
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state.clone());
        let (cookie, csrf) = login(app.clone()).await;
        let created = body_json(
            create_with_session(
                app.clone(),
                &cookie,
                &csrf,
                serde_json::json!({ "name": "leak-check", "scopes": ["tokens.manage"] }),
            )
            .await,
        )
        .await;
        let plaintext = created["token"].as_str().expect("token").to_owned();

        // 通过 Bearer 使用一次，确保 WAL 中也留下本记录的写入（last_used_at）。
        let used = Call::new(Method::GET, "/api/v1/tokens")
            .bearer(&plaintext)
            .send(app.clone())
            .await;
        assert_eq!(used.status(), StatusCode::OK);

        // 直接扫描数据目录下的全部文件字节（含 mien.db 与 -wal/-shm）。
        let mut files = 0usize;
        for entry in walk(dir.path()) {
            let bytes = std::fs::read(&entry).expect("read database file");
            assert!(
                !contains(&bytes, plaintext.as_bytes()),
                "{} contains the plaintext token",
                entry.display()
            );
            files += 1;
        }
        assert!(files >= 1, "data root has files to scan");

        // 正对照：同一扫描能发现真正存在的字符串（证明扫描方法有效）。
        let marker = b"mien_server_test_marker";
        let probe = dir.path().join("probe.bin");
        std::fs::write(&probe, marker).expect("probe");
        let bytes = std::fs::read(&probe).expect("probe");
        assert!(contains(&bytes, marker));
        std::fs::remove_file(&probe).expect("cleanup probe");

        // 数据库里存的是 64 个十六进制字符的 HMAC，而不是明文。
        let stored: String = sqlx::query_scalar("SELECT token_hash FROM api_tokens LIMIT 1")
            .fetch_one(state.db().expect("pool"))
            .await
            .expect("hash");
        assert_eq!(stored.len(), 64);
        assert!(stored.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(stored, plaintext);
    });
}

fn walk(directory: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(directory) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// `x-required-scope` 与 `TokenScope` 的双向一致性：仓库内的 OpenAPI 文件是契约源。
#[test]
fn route_scopes_match_the_openapi_contract() {
    let document = include_str!("../../../docs/contracts/openapi.yaml");
    let contracted = openapi_scopes(document);
    assert!(!contracted.is_empty(), "contract must declare scopes");
    assert_eq!(
        contracted.len(),
        ROUTES.len(),
        "route table and OpenAPI differ in size"
    );
    for (index, (operation, path, method, scope)) in contracted.iter().enumerate() {
        let route = &ROUTES[index];
        assert_eq!(route.operation, *operation, "operation order drift");
        assert_eq!(route.path, *path, "{operation} path drift");
        assert_eq!(route.method, method.as_str(), "{operation} method drift");
        assert_eq!(route.scope.as_str(), *scope, "{operation} scope drift");
    }

    // 无 scope 的操作必须是明确的 allowlist（仅 Cookie/ticket 或公开）。
    let unscoped = openapi_unscoped_operations(document);
    let mut expected = vec![
        "apiVersion",
        "authChangePassword",
        "authLogin",
        "authLogout",
        "authSession",
        "eventsWebSocket",
        "healthz",
        "openapiDocument",
        "readyz",
    ];
    expected.sort_unstable();
    let mut actual = unscoped;
    actual.sort_unstable();
    assert_eq!(actual, expected);
}

/// 解析 `  "/path":` → method → operationId/x-required-scope 的三级结构。
fn openapi_scopes(document: &str) -> Vec<(String, String, String, String)> {
    let mut rows = Vec::new();
    let mut path: Option<String> = None;
    let mut method: Option<String> = None;
    let mut operation: Option<String> = None;
    for line in document.lines() {
        if line == "components:" {
            break;
        }
        if let Some(rest) = line.strip_prefix("  \"") {
            if let Some((value, _)) = rest.split_once("\":") {
                path = Some(value.to_owned());
                method = None;
                operation = None;
                continue;
            }
        }
        if line.starts_with("    ") && !line.starts_with("     ") {
            let trimmed = line.trim();
            if let Some(value) = trimmed.strip_suffix(':') {
                if matches!(value, "get" | "post" | "put" | "patch" | "delete" | "head") {
                    method = Some(value.to_ascii_uppercase());
                    operation = None;
                    continue;
                }
            }
        }
        if let Some(value) = line.trim().strip_prefix("operationId: ") {
            operation = Some(value.trim().to_owned());
            continue;
        }
        if let Some(value) = line.trim().strip_prefix("x-required-scope: ") {
            rows.push((
                operation.clone().expect("operationId before scope"),
                path.clone().expect("path before scope"),
                method.clone().expect("method before scope"),
                value.trim().to_owned(),
            ));
        }
    }
    rows
}

/// 所有没有 `x-required-scope` 的 operationId。
fn openapi_unscoped_operations(document: &str) -> Vec<String> {
    let mut rows = Vec::new();
    let mut operation: Option<String> = None;
    let mut scoped = false;
    let finish = |rows: &mut Vec<String>, operation: &mut Option<String>, scoped: &mut bool| {
        if let Some(name) = operation.take() {
            if !*scoped {
                rows.push(name);
            }
        }
        *scoped = false;
    };
    for line in document.lines() {
        if line == "components:" {
            break;
        }
        if line.starts_with("    ") && !line.starts_with("     ") {
            if let Some(value) = line.trim().strip_suffix(':') {
                if matches!(value, "get" | "post" | "put" | "patch" | "delete" | "head") {
                    finish(&mut rows, &mut operation, &mut scoped);
                    continue;
                }
            }
        }
        if let Some(value) = line.trim().strip_prefix("operationId: ") {
            finish(&mut rows, &mut operation, &mut scoped);
            operation = Some(value.trim().to_owned());
            continue;
        }
        if line.trim().starts_with("x-required-scope: ") {
            scoped = true;
        }
    }
    rows
}

/// OpenAPI `TokenScope` 枚举与 [`TokenScope::ALL`] 一致（顺序与集合）。
#[test]
fn token_scope_enum_matches_the_contract() {
    let document = include_str!("../../../docs/contracts/openapi.yaml");
    let mut values = Vec::new();
    let mut inside = false;
    for line in document.lines() {
        if line == "    TokenScope:" {
            inside = true;
            continue;
        }
        if inside {
            if let Some(value) = line.trim().strip_prefix("- ") {
                if !value.trim().is_empty() && line.starts_with("      - ") {
                    values.push(value.trim().to_owned());
                }
                continue;
            }
            if !line.trim().is_empty() && !line.starts_with("      ") {
                break;
            }
        }
    }
    let expected: Vec<&str> = TokenScope::ALL.iter().map(|scope| scope.as_str()).collect();
    assert_eq!(values, expected);
}

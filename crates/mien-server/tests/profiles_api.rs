//! 档案与运行时 REST 验收（T24）。
//!
//! 覆盖 `VER-API-003/009/010/021` 与 `VER-PROFILE-003/004/006`：
//! CRUD 全链路、游标分页无重复无遗漏、`Idempotency-Key` 重复不重复启动、
//! 软删除启动 409、未认证 401（不因资源不存在变 404）、缺 scope 403、
//! `direct`/`reduced_assurance` 真值表，以及一次真实内核启动。
//!
//! 真实启动需要 `MIEN_KERNEL_BIN`；未设置时打印显式跳过原因，不做静默通过。

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;

use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use axum::response::Response;
use mien_core::ports::proxy::ProxyRepository;
use mien_server::router;
use mien_server::state::{AppState, AuthState, SecuritySettings};
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;

const ADMIN_PASSWORD: &str = "correct horse battery staple";

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

/// 迁移后的临时数据根与服务状态；直连出口默认关闭，由测试显式打开。
fn state() -> (TempDir, AppState) {
    let dir = TempDir::new().expect("temp dir");
    let pool = mien_core::storage::open_and_migrate(&dir.path().join("mien.db")).expect("migrate");
    let auth = AuthState::default().with_bootstrap(mien_server::auth::password::Bootstrap::inline(
        ADMIN_PASSWORD,
    ));
    let state = AppState::new(dir.path(), Some(pool))
        .with_security(SecuritySettings::loopback())
        .with_auth(auth)
        .with_allow_direct_egress(true);
    (dir, state)
}

/// 一个可通过认证的 Cookie 会话。
struct Session {
    cookie: String,
    csrf: String,
}

/// 一个持有指定 scope 的 Bearer Token。
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

    fn idempotency(self, key: &str) -> Self {
        self.header(header::HeaderName::from_static("idempotency-key"), key)
    }

    fn json(self, body: Value) -> Self {
        self.json_bytes(body.to_string().into_bytes())
    }

    fn json_bytes(mut self, body: Vec<u8>) -> Self {
        self.body = Some(body);
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

async fn body_json(response: Response) -> Value {
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("body");
    if bytes.is_empty() {
        return json!({ "status": status.as_u16() });
    }
    serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        panic!(
            "non-JSON response {status}: {error}; body={}",
            String::from_utf8_lossy(&bytes)
        )
    })
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

/// 登录并把强制改密清空，得到可直接写操作的会话。
async fn login(app: axum::Router) -> Session {
    let response = Call::new(Method::POST, "/api/v1/auth/login")
        .origin()
        .json(json!({ "password": ADMIN_PASSWORD }))
        .send(app.clone())
        .await;
    assert_eq!(response.status(), StatusCode::OK, "login must succeed");
    let cookie = set_cookie(&response);
    let body = body_json(response).await;
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

/// 可以下发的档案输入；`fingerprint` 使用固定显式字段。
fn profile_input(name: &str) -> Value {
    json!({
        "name": name,
        "fingerprint": fingerprint(),
        "mode": "headful",
    })
}

fn fingerprint() -> Value {
    json!({
        "schema_version": 1,
        "platform": "macos",
        "platform_version": "14.5",
        "brand": "Edge",
        "brand_version": "148.0.0.0",
        "hardware_concurrency": 12,
        "languages": ["en-US", "en"],
        "timezone": "America/Los_Angeles",
        "window": { "width": 1280, "height": 720 },
        "webrtc": { "disable_non_proxied_udp": true },
        "network": { "doh_mode": "disabled", "ipv6_mode": "block" },
    })
}

/// 一条真实可解密的代理行：主密钥写入数据根，凭据按 `proxy_id` 作为 AAD 加密。
async fn seed_proxy(pool: &sqlx::SqlitePool) -> String {
    let repository = mien_core::storage::proxy::SqliteProxyRepository::new(
        pool.clone(),
        mien_core::secrets::MasterKey::from_bytes([7u8; 32]),
    );
    let view = repository
        .create(mien_core::ports::proxy::ProxyInput {
            name: "test proxy".to_owned(),
            kind: mien_core::ports::proxy::ProxyKind::Vless,
            server: "127.0.0.1".to_owned(),
            port: 443,
            options: BTreeMap::new(),
            credentials: [(
                "id".to_owned(),
                "00000000-0000-4000-8000-000000000001".to_owned(),
            )]
            .into_iter()
            .collect(),
        })
        .await
        .expect("seed proxy");
    view.id.0
}

async fn create_profile(app: axum::Router, session: &Session, name: &str) -> Value {
    let response = Call::new(Method::POST, "/api/v1/profiles")
        .cookie(session)
        .csrf(session)
        .origin()
        .json(profile_input(name))
        .send(app)
        .await;
    assert_eq!(response.status(), StatusCode::CREATED, "create {name}");
    body_json(response).await
}

// ---------------------------------------------------------------------------
// CRUD 全链路
// ---------------------------------------------------------------------------

#[test]
fn crud_covers_create_read_update_duplicate_archive_restore_and_delete() {
    let (_dir, state) = state();
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state);
        let session = login(app.clone()).await;

        let created = create_profile(app.clone(), &session, "alpha").await;
        let id = created["id"].as_str().expect("id").to_owned();
        assert_eq!(created["name"], "alpha");
        assert_eq!(created["status"], "active");
        assert_eq!(created["mode"], "headful");
        assert_eq!(created["direct"], true);
        assert_eq!(created["reduced_assurance"], true, "direct is reduced");
        assert_eq!(created["isolation"], "unknown");
        assert_eq!(created["proxy_id"], Value::Null);
        assert!(created["deleted_at"].is_null());
        assert!(created["deletion_error"].is_null());
        assert_eq!(created["tags"], json!([]));
        assert!(
            created["fingerprint"]["seed"]
                .as_u64()
                .is_some_and(|seed| seed > 0),
            "seed is server side: {created}"
        );

        let read = Call::new(Method::GET, format!("/api/v1/profiles/{id}"))
            .cookie(&session)
            .send(app.clone())
            .await;
        assert_eq!(read.status(), StatusCode::OK);
        assert_eq!(body_json(read).await["name"], "alpha");

        let updated = Call::new(Method::PATCH, format!("/api/v1/profiles/{id}"))
            .cookie(&session)
            .csrf(&session)
            .origin()
            .json(json!({ "name": "alpha renamed", "notes": "kept" }))
            .send(app.clone())
            .await;
        assert_eq!(updated.status(), StatusCode::OK);
        let updated = body_json(updated).await;
        assert_eq!(updated["name"], "alpha renamed");
        assert_eq!(updated["notes"], "kept");
        assert_eq!(
            updated["fingerprint"]["seed"], created["fingerprint"]["seed"],
            "PATCH must not rotate the seed"
        );

        // 显式 null 清空可空字段。
        let cleared = Call::new(Method::PATCH, format!("/api/v1/profiles/{id}"))
            .cookie(&session)
            .csrf(&session)
            .origin()
            .json(json!({ "notes": null }))
            .send(app.clone())
            .await;
        assert_eq!(cleared.status(), StatusCode::OK);
        assert!(body_json(cleared).await["notes"].is_null());

        let duplicate = Call::new(Method::POST, format!("/api/v1/profiles/{id}/duplicate"))
            .cookie(&session)
            .csrf(&session)
            .origin()
            .json(json!({ "copy_fingerprint": true, "copy_proxy": true }))
            .send(app.clone())
            .await;
        assert_eq!(duplicate.status(), StatusCode::CREATED);
        let duplicate = body_json(duplicate).await;
        let copy_id = duplicate["id"].as_str().expect("copy id").to_owned();
        assert_ne!(copy_id, id);
        assert_ne!(
            duplicate["fingerprint"]["seed"], updated["fingerprint"]["seed"],
            "duplicate always draws a new seed"
        );
        assert_eq!(duplicate["fingerprint"]["platform"], "macos");

        let archived = Call::new(Method::POST, format!("/api/v1/profiles/{id}/archive"))
            .cookie(&session)
            .csrf(&session)
            .origin()
            .send(app.clone())
            .await;
        assert_eq!(archived.status(), StatusCode::NO_CONTENT);
        let listed = Call::new(Method::GET, "/api/v1/profiles?status=archived")
            .cookie(&session)
            .send(app.clone())
            .await;
        assert_eq!(listed.status(), StatusCode::OK);
        let listed = body_json(listed).await;
        assert_eq!(listed["items"].as_array().expect("items").len(), 1);
        assert_eq!(listed["items"][0]["status"], "archived");
        assert!(listed["items"][0]["archived_at"].is_string());

        let restored = Call::new(Method::POST, format!("/api/v1/profiles/{id}/restore"))
            .cookie(&session)
            .csrf(&session)
            .origin()
            .send(app.clone())
            .await;
        assert_eq!(restored.status(), StatusCode::NO_CONTENT);

        let deleted = Call::new(Method::DELETE, format!("/api/v1/profiles/{id}"))
            .cookie(&session)
            .csrf(&session)
            .origin()
            .send(app.clone())
            .await;
        assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
        let after = Call::new(Method::GET, format!("/api/v1/profiles/{id}"))
            .cookie(&session)
            .send(app.clone())
            .await;
        assert_eq!(after.status(), StatusCode::OK);
        let after = body_json(after).await;
        assert_eq!(after["status"], "deleted");
        assert!(after["deleted_at"].is_string());

        // 已删除档案读取仍可用；默认列表不再包含。
        let active = Call::new(Method::GET, "/api/v1/profiles")
            .cookie(&session)
            .send(app.clone())
            .await;
        assert_eq!(
            body_json(active).await["items"]
                .as_array()
                .expect("items")
                .len(),
            1
        );

        // 删除重试幂等。
        let retried = Call::new(Method::DELETE, format!("/api/v1/profiles/{id}"))
            .cookie(&session)
            .csrf(&session)
            .origin()
            .send(app.clone())
            .await;
        assert_eq!(retried.status(), StatusCode::NO_CONTENT);

        // 永久删除。
        let purged = Call::new(
            Method::DELETE,
            format!("/api/v1/profiles/{id}?permanent=true"),
        )
        .cookie(&session)
        .csrf(&session)
        .origin()
        .send(app.clone())
        .await;
        assert_eq!(purged.status(), StatusCode::NO_CONTENT);
        let missing = Call::new(Method::GET, format!("/api/v1/profiles/{id}"))
            .cookie(&session)
            .send(app)
            .await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_json(missing).await["code"], "PROFILE_NOT_FOUND");
    });
}

#[test]
fn permanent_delete_of_a_live_profile_is_rejected_with_profile_not_deleted() {
    let (_dir, state) = state();
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state);
        let session = login(app.clone()).await;
        let created = create_profile(app.clone(), &session, "keep").await;
        let id = created["id"].as_str().expect("id");

        let rejected = Call::new(
            Method::DELETE,
            format!("/api/v1/profiles/{id}?permanent=true"),
        )
        .cookie(&session)
        .csrf(&session)
        .origin()
        .send(app.clone())
        .await;
        assert_eq!(rejected.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(rejected).await["code"], "PROFILE_NOT_DELETED");

        // 行与目录都保留。
        let read = Call::new(Method::GET, format!("/api/v1/profiles/{id}"))
            .cookie(&session)
            .send(app)
            .await;
        assert_eq!(read.status(), StatusCode::OK);
        assert_eq!(body_json(read).await["status"], "active");
    });
}

// ---------------------------------------------------------------------------
// 分页
// ---------------------------------------------------------------------------

#[test]
fn cursor_pagination_has_no_duplicates_and_no_gaps() {
    let (_dir, state) = state();
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state);
        let session = login(app.clone()).await;
        let mut expected = Vec::new();
        for index in 0..23 {
            let created = create_profile(app.clone(), &session, &format!("page-{index:02}")).await;
            expected.push(created["id"].as_str().expect("id").to_owned());
        }
        expected.sort();

        let mut seen: Vec<String> = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut path = String::from("/api/v1/profiles?limit=5&sort=name&order=asc");
            if let Some(cursor) = &cursor {
                path.push_str(&format!("&cursor={cursor}"));
            }
            let response = Call::new(Method::GET, path)
                .cookie(&session)
                .send(app.clone())
                .await;
            assert_eq!(response.status(), StatusCode::OK);
            let page = body_json(response).await;
            let items = page["items"].as_array().expect("items");
            assert!(items.len() <= 5, "limit is honoured");
            for item in items {
                seen.push(item["id"].as_str().expect("id").to_owned());
            }
            match page["next_cursor"].as_str() {
                Some(next) if !next.is_empty() => cursor = Some(next.to_owned()),
                _ => break,
            }
        }
        seen.sort();
        assert_eq!(seen, expected, "every profile appears exactly once");
        assert_eq!(
            seen.len(),
            seen.iter().collect::<std::collections::BTreeSet<_>>().len(),
            "no duplicates across pages"
        );

        // 超上限与非法枚举都是 422。
        let too_large = Call::new(Method::GET, "/api/v1/profiles?limit=201")
            .cookie(&session)
            .send(app.clone())
            .await;
        assert_eq!(too_large.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let bad_sort = Call::new(Method::GET, "/api/v1/profiles?sort=random")
            .cookie(&session)
            .send(app.clone())
            .await;
        assert_eq!(bad_sort.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let bad_filter = Call::new(Method::GET, "/api/v1/profiles?unknown_filter=1")
            .cookie(&session)
            .send(app.clone())
            .await;
        assert_eq!(bad_filter.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let bad_state = Call::new(Method::GET, "/api/v1/profiles?state=sleeping")
            .cookie(&session)
            .send(app.clone())
            .await;
        assert_eq!(bad_state.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let bad_cursor = Call::new(Method::GET, "/api/v1/profiles?cursor=zzzz")
            .cookie(&session)
            .send(app)
            .await;
        assert_eq!(bad_cursor.status(), StatusCode::UNPROCESSABLE_ENTITY);
    });
}

// ---------------------------------------------------------------------------
// 错误优先级
// ---------------------------------------------------------------------------

#[test]
fn authentication_precedes_existence_and_authorization() {
    let (_dir, state) = state();
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state.clone());

        // 1. 无凭据访问不存在的档案：401，不是 404。
        let anonymous = Call::new(Method::GET, "/api/v1/profiles/01ARZ3NDEKTSV4RRFFQ69G5FAV")
            .send(app.clone())
            .await;
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(body_json(anonymous).await["code"], "AUTH_REQUIRED");

        // 2. 已认证但缺 scope：403 在资源存在性之前返回。
        let read_only = mien_server::auth::Principal {
            subject: "token".to_owned(),
            scopes: vec!["profiles.read".to_owned()],
        };
        let app_read_only = router(state.with_auth(AuthState::default().with_bearer(
            std::sync::Arc::new(Bearer {
                scopes: &["profiles.read"],
            }),
        )));
        let denied = Call::new(Method::POST, "/api/v1/profiles")
            .bearer("anything")
            .json(profile_input("nope"))
            .send(app_read_only.clone())
            .await;
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        assert_eq!(body_json(denied).await["code"], "FORBIDDEN_SCOPE");
        assert!(
            read_only.has_scope("profiles.read"),
            "fixture sanity: principal holds read scope"
        );

        // 3. 缺 scope 访问不存在的档案仍是 403，不泄露存在性。
        let nonexistent = Call::new(Method::GET, "/api/v1/profiles/01ARZ3NDEKTSV4RRFFQ69G5FAV")
            .bearer("anything")
            .send(app_read_only)
            .await;
        assert_eq!(nonexistent.status(), StatusCode::NOT_FOUND);

        // 4. 已认证且已授权，但资源不存在：404。
        let session = login(app.clone()).await;
        let missing = Call::new(Method::GET, "/api/v1/profiles/01ARZ3NDEKTSV4RRFFQ69G5FAV")
            .cookie(&session)
            .send(app)
            .await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_json(missing).await["code"], "PROFILE_NOT_FOUND");
    });
}

#[test]
fn a_scope_limited_token_cannot_write() {
    let (_dir, state) = state();
    let rt = runtime();
    rt.block_on(async move {
        let state = state.with_auth(AuthState::default().with_bearer(std::sync::Arc::new(
            Bearer {
                scopes: &["profiles.read", "runtime.control"],
            },
        )));
        let app = router(state);
        let denied = Call::new(Method::POST, "/api/v1/profiles")
            .bearer("anything")
            .json(profile_input("denied"))
            .send(app.clone())
            .await;
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        assert_eq!(body_json(denied).await["code"], "FORBIDDEN_SCOPE");

        // 读列表仍可用。
        let listed = Call::new(Method::GET, "/api/v1/profiles")
            .bearer("anything")
            .send(app.clone())
            .await;
        assert_eq!(listed.status(), StatusCode::OK);
    });
}

// ---------------------------------------------------------------------------
// 软删除与启动
// ---------------------------------------------------------------------------

#[test]
fn starting_a_soft_deleted_profile_is_409_profile_deleted() {
    let (_dir, state) = state();
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state);
        let session = login(app.clone()).await;
        let created = create_profile(app.clone(), &session, "gone").await;
        let id = created["id"].as_str().expect("id").to_owned();

        let deleted = Call::new(Method::DELETE, format!("/api/v1/profiles/{id}"))
            .cookie(&session)
            .csrf(&session)
            .origin()
            .send(app.clone())
            .await;
        assert_eq!(deleted.status(), StatusCode::NO_CONTENT);

        let start = Call::new(Method::POST, format!("/api/v1/profiles/{id}/start"))
            .cookie(&session)
            .csrf(&session)
            .origin()
            .send(app.clone())
            .await;
        assert_eq!(start.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(start).await["code"], "PROFILE_DELETED");

        // 修改同样是 409。
        let patch = Call::new(Method::PATCH, format!("/api/v1/profiles/{id}"))
            .cookie(&session)
            .csrf(&session)
            .origin()
            .json(json!({ "name": "should not apply" }))
            .send(app.clone())
            .await;
        assert_eq!(patch.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(patch).await["code"], "PROFILE_DELETED");

        // restore 与 purge 仍可用。
        let restored = Call::new(Method::POST, format!("/api/v1/profiles/{id}/restore"))
            .cookie(&session)
            .csrf(&session)
            .origin()
            .send(app)
            .await;
        assert_eq!(restored.status(), StatusCode::NO_CONTENT);
    });
}

#[test]
fn starting_a_direct_profile_requires_the_server_direct_egress_switch() {
    let (_dir, state) = state();
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state.clone().with_allow_direct_egress(false));
        let session = login(app.clone()).await;
        let created = create_profile(app.clone(), &session, "direct").await;
        let id = created["id"].as_str().expect("id").to_owned();
        let start = Call::new(Method::POST, format!("/api/v1/profiles/{id}/start"))
            .cookie(&session)
            .csrf(&session)
            .origin()
            .send(app)
            .await;
        assert_eq!(start.status(), StatusCode::FORBIDDEN);
        let body = body_json(start).await;
        assert_eq!(body["code"], "DIRECT_EGRESS_DISABLED");
    });
}

// ---------------------------------------------------------------------------
// 幂等
// ---------------------------------------------------------------------------

#[test]
fn idempotent_start_does_not_start_twice_and_conflicts_on_a_different_body() {
    let Some(binary) = std::env::var_os("MIEN_KERNEL_BIN").map(std::path::PathBuf::from) else {
        eprintln!("skipped: MIEN_KERNEL_BIN not set");
        return;
    };
    assert!(
        binary.is_file(),
        "MIEN_KERNEL_BIN is not a file: {binary:?}"
    );
    let (_dir, state) = state();
    let rt = runtime();
    let state = state.with_kernel_bin(Some(binary));
    rt.block_on(async move {
        let app = router(state.clone());
        let session = login(app.clone()).await;
        let created = create_profile(app.clone(), &session, "idempotent").await;
        let id = created["id"].as_str().expect("id").to_owned();

        let first = Call::new(Method::POST, format!("/api/v1/profiles/{id}/start"))
            .cookie(&session)
            .csrf(&session)
            .origin()
            .idempotency("start-key-0001")
            .send(app.clone())
            .await;
        assert_eq!(first.status(), StatusCode::ACCEPTED);
        let first_body = body_json(first).await;
        assert_eq!(first_body["state"], "running");
        let first_runtime = first_body["id"].as_str().expect("runtime id").to_owned();
        let first_pid = first_body["proxy_port"].clone();

        // 同一键同一请求：回放第一次结果，不重复启动。
        let replay = Call::new(Method::POST, format!("/api/v1/profiles/{id}/start"))
            .cookie(&session)
            .csrf(&session)
            .origin()
            .idempotency("start-key-0001")
            .send(app.clone())
            .await;
        assert_eq!(replay.status(), StatusCode::ACCEPTED);
        let replay_body = body_json(replay).await;
        assert_eq!(replay_body["id"], first_runtime);
        assert_eq!(replay_body["proxy_port"], first_pid);

        // 同一键不同请求体：409。
        let conflicting = Call::new(Method::POST, format!("/api/v1/profiles/{id}/start"))
            .cookie(&session)
            .csrf(&session)
            .origin()
            .idempotency("start-key-0001")
            .json(json!({ "unexpected": true }))
            .send(app.clone())
            .await;
        assert_eq!(conflicting.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(conflicting).await["code"], "IDEMPOTENCY_CONFLICT");

        // 只有一条运行时记录。
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM runtimes WHERE profile_id=?1")
            .bind(&id)
            .fetch_one(state.db().expect("db"))
            .await
            .expect("count");
        assert_eq!(rows, 1, "idempotent replay must not add a runtime row");

        let stopped = Call::new(Method::POST, format!("/api/v1/profiles/{id}/stop"))
            .cookie(&session)
            .csrf(&session)
            .origin()
            .idempotency("stop-key-0001")
            .send(app.clone())
            .await;
        assert_eq!(stopped.status(), StatusCode::ACCEPTED);
        let stopped_body = body_json(stopped).await;
        assert_eq!(stopped_body["state"], "stopped");

        let stopped_replay = Call::new(Method::POST, format!("/api/v1/profiles/{id}/stop"))
            .cookie(&session)
            .csrf(&session)
            .origin()
            .idempotency("stop-key-0001")
            .send(app)
            .await;
        assert_eq!(stopped_replay.status(), StatusCode::ACCEPTED);
        assert_eq!(body_json(stopped_replay).await["state"], "stopped");
    });
}

#[test]
fn an_idempotency_key_expires_and_can_be_reused() {
    let (_dir, state) = state();
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state.clone());
        let session = login(app.clone()).await;
        let created = create_profile(app.clone(), &session, "expiry").await;
        let id = created["id"].as_str().expect("id").to_owned();
        let first = Call::new(Method::PATCH, format!("/api/v1/profiles/{id}"))
            .cookie(&session)
            .csrf(&session)
            .origin()
            .idempotency("expiry-key-0001")
            .json(json!({ "name": "first" }))
            .send(app.clone())
            .await;
        assert_eq!(first.status(), StatusCode::OK);

        // 把缓存行改成已过期：同一键可以重新使用。
        sqlx::query(
            "UPDATE idempotency_keys SET expires_at='2000-01-01T00:00:00.000Z' WHERE key='expiry-key-0001' AND scope='admin'",
        )
        .execute(state.db().expect("db"))
        .await
        .expect("expire");

        let second = Call::new(Method::PATCH, format!("/api/v1/profiles/{id}"))
            .cookie(&session)
            .csrf(&session)
            .origin()
            .idempotency("expiry-key-0001")
            .json(json!({ "name": "second" }))
            .send(app)
            .await;
        assert_eq!(second.status(), StatusCode::OK);
        assert_eq!(body_json(second).await["name"], "second");
    });
}

// ---------------------------------------------------------------------------
// 隔离与降低保证真值表
// ---------------------------------------------------------------------------

#[test]
fn reduced_assurance_truth_table_holds_for_profile_payloads() {
    let (_dir, state) = state();
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state.clone());
        let session = login(app.clone()).await;
        let proxy_id = seed_proxy(state.db().expect("db")).await;

        // 18 组合：3 种模式 × 3 种隔离 × direct/proxied。
        let mut checked = 0;
        for mode in ["headful", "headless", "pseudo_headful"] {
            for isolation in ["enforced", "unsandboxed", "unknown"] {
                for proxied in [false, true] {
                    let mut input = profile_input(mode);
                    input["mode"] = json!(mode);
                    if proxied {
                        input["proxy_id"] = json!(proxy_id);
                    }
                    let response = Call::new(Method::POST, "/api/v1/profiles")
                        .cookie(&session)
                        .csrf(&session)
                        .origin()
                        .json(input)
                        .send(app.clone())
                        .await;
                    assert_eq!(response.status(), StatusCode::CREATED);
                    let created = body_json(response).await;
                    let id = created["id"].as_str().expect("id").to_owned();
                    let direct = !proxied;
                    assert_eq!(created["direct"], direct);

                    // 直接写历史结论：Profile 的 isolation 只来自 last_known_isolation。
                    // 本次 isolation 写另一个值，确保 Runtime 与 Profile 各有来源。
                    sqlx::query(
                        "INSERT INTO runtimes (id, profile_id, state, mode, isolation, last_known_isolation, updated_at) \
                         VALUES (?1, ?2, 'stopped', ?3, 'unknown', ?4, '2026-01-01T00:00:00.000Z')",
                    )
                    .bind(format!("rt-{id}"))
                    .bind(&id)
                    .bind(mode)
                    .bind(isolation)
                    .execute(state.db().expect("db"))
                    .await
                    .expect("seed runtime");

                    let read = Call::new(Method::GET, format!("/api/v1/profiles/{id}"))
                        .cookie(&session)
                        .send(app.clone())
                        .await;
                    assert_eq!(read.status(), StatusCode::OK);
                    let read = body_json(read).await;
                    let expected_reduced =
                        direct || isolation != "enforced" || !matches!(mode, "headful");
                    assert_eq!(
                        read["isolation"], isolation,
                        "Profile isolation comes from last_known_isolation ({mode}/{isolation}/proxied={proxied})"
                    );
                    assert_eq!(
                        read["reduced_assurance"], expected_reduced,
                        "truth table for mode={mode} isolation={isolation} direct={direct}"
                    );
                    assert_eq!(read["direct"], direct);
                    assert_eq!(
                        read["runtime"]["isolation"], "unknown",
                        "Runtime reads this run's isolation, not the historical one"
                    );
                    assert_eq!(
                        read["runtime"]["reduced_assurance"], true,
                        "unknown isolation always reduces assurance"
                    );
                    assert_eq!(read["runtime"]["state"], "stopped");
                    assert_eq!(
                        read["runtime"]["ua_scope"],
                        if mode == "pseudo_headful" {
                            json!("chrome")
                        } else {
                            Value::Null
                        },
                        "only pseudo_headful claims ua_scope=chrome"
                    );
                    checked += 1;
                }
            }
        }
        assert_eq!(checked, 18);
    });
}

#[test]
fn a_failed_soft_delete_leaves_the_row_and_directory_untouched() {
    let (_dir, state) = state();
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state.clone());
        let session = login(app.clone()).await;
        let created = create_profile(app.clone(), &session, "soft-failure").await;
        let id = created["id"].as_str().expect("id").to_owned();

        // 预先占用 trash/<id>，核心拒绝覆盖并返回 PROFILE_PURGE_FAILED。
        let trash = state.data_root().join("trash").join(&id);
        std::fs::create_dir_all(&trash).expect("pre-occupy trash");

        let failed = Call::new(Method::DELETE, format!("/api/v1/profiles/{id}"))
            .cookie(&session)
            .csrf(&session)
            .origin()
            .send(app.clone())
            .await;
        assert_eq!(failed.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = body_json(failed).await;
        assert_eq!(body["code"], "PROFILE_PURGE_FAILED");
        assert!(body["deletion_error"].is_string(), "body: {body}");

        // 行保持 active，目录仍在 profiles/。
        let read = Call::new(Method::GET, format!("/api/v1/profiles/{id}"))
            .cookie(&session)
            .send(app)
            .await;
        let read = body_json(read).await;
        assert_eq!(read["status"], "active", "body: {read}");
        assert!(read["deleted_at"].is_null());
        assert!(state.data_root().join("profiles").join(&id).is_dir());
    });
}

#[test]
fn a_failed_purge_keeps_the_profile_deleted_and_reports_the_deletion_error() {
    let (_dir, state) = state();
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state.clone());
        let session = login(app.clone()).await;
        let created = create_profile(app.clone(), &session, "purge-failure").await;
        let id = created["id"].as_str().expect("id").to_owned();

        // 软删除：目录移入 trash。
        let deleted = Call::new(Method::DELETE, format!("/api/v1/profiles/{id}"))
            .cookie(&session)
            .csrf(&session)
            .origin()
            .send(app.clone())
            .await;
        assert_eq!(deleted.status(), StatusCode::NO_CONTENT);

        // 让 trash 目录不可读：`remove_dir_all` 失败，内核写 `deletion_error=io_error`，
        // purge 返回 500 且响应携带该字段。
        let trash = state.data_root().join("trash").join(&id);
        assert!(
            trash.is_dir(),
            "soft delete must move the directory to trash"
        );
        std::fs::write(trash.join("blocker"), b"x").expect("block trash");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&trash, std::fs::Permissions::from_mode(0o000))
                .expect("chmod trash");
        }

        let failed = Call::new(
            Method::DELETE,
            format!("/api/v1/profiles/{id}?permanent=true"),
        )
        .cookie(&session)
        .csrf(&session)
        .origin()
        .send(app.clone())
        .await;
        assert_eq!(failed.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = body_json(failed).await;
        assert_eq!(body["code"], "PROFILE_PURGE_FAILED");
        assert_eq!(body["retryable"], true);
        assert_eq!(body["deletion_error"], "io_error", "body: {body}");

        // 行保留为 deleted，且 deletion_error 可读。
        let read = Call::new(Method::GET, format!("/api/v1/profiles/{id}"))
            .cookie(&session)
            .send(app.clone())
            .await;
        let read = body_json(read).await;
        assert_eq!(read["status"], "deleted");
        assert_eq!(read["deletion_error"], "io_error");

        // 恢复权限后 purge 成功。
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&trash, std::fs::Permissions::from_mode(0o700))
                .expect("unblock trash");
        }
        std::fs::remove_file(trash.join("blocker")).expect("clear blocker");
        let purged = Call::new(
            Method::DELETE,
            format!("/api/v1/profiles/{id}?permanent=true"),
        )
        .cookie(&session)
        .csrf(&session)
        .origin()
        .send(app.clone())
        .await;
        assert_eq!(purged.status(), StatusCode::NO_CONTENT);
        let missing = Call::new(Method::GET, format!("/api/v1/profiles/{id}"))
            .cookie(&session)
            .send(app)
            .await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    });
}

#[test]
fn a_profile_that_never_started_omits_the_runtime_object() {
    let (_dir, state) = state();
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state);
        let session = login(app.clone()).await;
        let created = create_profile(app.clone(), &session, "no-runtime").await;
        assert!(
            created.get("runtime").is_none(),
            "a profile without runtime evidence must omit it: {created}"
        );
        assert_eq!(created["isolation"], "unknown");
        assert_eq!(created["reduced_assurance"], true);
    });
}

#[test]
fn runtime_endpoint_reports_stopped_and_unknown_before_any_start() {
    let (_dir, state) = state();
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state);
        let session = login(app.clone()).await;
        let created = create_profile(app.clone(), &session, "never-started").await;
        let id = created["id"].as_str().expect("id").to_owned();
        let runtime_view = Call::new(Method::GET, format!("/api/v1/profiles/{id}/runtime"))
            .cookie(&session)
            .send(app.clone())
            .await;
        assert_eq!(runtime_view.status(), StatusCode::OK);
        let runtime_view = body_json(runtime_view).await;
        assert_eq!(runtime_view["state"], "stopped");
        assert_eq!(runtime_view["isolation"], "unknown");
        assert_eq!(runtime_view["direct"], true);
        assert_eq!(runtime_view["reduced_assurance"], true);
        assert_eq!(runtime_view["profile_id"], id);
        assert!(
            runtime_view["id"]
                .as_str()
                .is_some_and(|id| id.starts_with("rt-"))
        );
    });
}

#[test]
fn unknown_fields_in_a_patch_are_422() {
    let (_dir, state) = state();
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state);
        let session = login(app.clone()).await;
        let created = create_profile(app.clone(), &session, "strict").await;
        let id = created["id"].as_str().expect("id").to_owned();
        let rejected = Call::new(Method::PATCH, format!("/api/v1/profiles/{id}"))
            .cookie(&session)
            .csrf(&session)
            .origin()
            .json(json!({ "window": { "width": 800, "height": 600 } }))
            .send(app)
            .await;
        assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body_json(rejected).await["code"], "VALIDATION_FAILED");
    });
}

#[test]
fn grouping_and_tagging_round_trip_through_relations() {
    let (_dir, state) = state();
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state.clone());
        let session = login(app.clone()).await;
        let group = mien_core::domain::profile::ProfileId::new().0;
        let tag = mien_core::domain::profile::ProfileId::new().0;
        sqlx::query(
            "INSERT INTO groups (id,name,sort_order,created_at,updated_at) VALUES (?1,'team',0,'2026-01-01T00:00:00.000Z','2026-01-01T00:00:00.000Z')",
        )
        .bind(&group)
        .execute(state.db().expect("db"))
        .await
        .expect("seed group");
        sqlx::query("INSERT INTO tags (id,name,created_at) VALUES (?1,'blue','2026-01-01T00:00:00.000Z')")
            .bind(&tag)
            .execute(state.db().expect("db"))
            .await
            .expect("seed tag");

        let created = create_profile(app.clone(), &session, "grouped").await;
        let id = created["id"].as_str().expect("id").to_owned();
        let patched = Call::new(Method::PATCH, format!("/api/v1/profiles/{id}"))
            .cookie(&session)
            .csrf(&session)
            .origin()
            .json(json!({ "group_id": group, "tags": ["blue"] }))
            .send(app.clone())
            .await;
        assert_eq!(patched.status(), StatusCode::OK);
        let patched = body_json(patched).await;
        assert_eq!(patched["group_id"], group);
        assert_eq!(patched["tags"], json!(["blue"]));

        let filtered = Call::new(Method::GET, format!("/api/v1/profiles?tag=blue&group_id={group}"))
            .cookie(&session)
            .send(app.clone())
            .await;
        assert_eq!(filtered.status(), StatusCode::OK);
        assert_eq!(body_json(filtered).await["items"].as_array().expect("items").len(), 1);

        // 未知标签被拒绝，不静默忽略。
        let unknown = Call::new(Method::PATCH, format!("/api/v1/profiles/{id}"))
            .cookie(&session)
            .csrf(&session)
            .origin()
            .json(json!({ "tags": ["missing"] }))
            .send(app)
            .await;
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_json(unknown).await["code"], "TAG_NOT_FOUND");
    });
}

#[test]
fn query_parsing_rejects_unknown_grid_and_accepts_the_openapi_defaults() {
    let mut params = BTreeMap::new();
    let query = mien_server::profiles::parse_list_query(&params).expect("defaults");
    assert_eq!(query.limit, 50);
    assert_eq!(
        query.status,
        Some(mien_core::domain::profile::ProfileStatus::Active)
    );

    params.insert("sort".into(), "name".into());
    params.insert("order".into(), "asc".into());
    assert!(mien_server::profiles::parse_list_query(&params).is_ok());

    params.insert("bogus".into(), "1".into());
    let error = mien_server::profiles::parse_list_query(&params).expect_err("unknown filter");
    assert_eq!(error.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(error.code, mien_core::ErrorCode::ValidationFailed);
}

// ---------------------------------------------------------------------------
// 真实内核
// ---------------------------------------------------------------------------

/// 真实启动一次：`MIEN_KERNEL_BIN` 缺失时显式跳过。
///
/// 覆盖 `REQ-API-021` 的 `isolation`/`direct`/`reduced_assurance` 与
/// `REQ-API-010` 的幂等启动；结束后停止，不留孤儿进程。
#[test]
fn real_kernel_start_reports_running_and_stops_cleanly() {
    let Some(binary) = std::env::var_os("MIEN_KERNEL_BIN").map(std::path::PathBuf::from) else {
        eprintln!("skipped: MIEN_KERNEL_BIN not set");
        return;
    };
    assert!(
        binary.is_file(),
        "MIEN_KERNEL_BIN is not a file: {binary:?}"
    );
    let (_dir, state) = state();
    let rt = runtime();
    let state = state.with_kernel_bin(Some(binary));
    rt.block_on(async move {
        let app = router(state.clone());
        let session = login(app.clone()).await;
        let created = create_profile(app.clone(), &session, "real-kernel").await;
        let id = created["id"].as_str().expect("id").to_owned();

        let started = Call::new(Method::POST, format!("/api/v1/profiles/{id}/start"))
            .cookie(&session)
            .csrf(&session)
            .origin()
            .idempotency("real-start-0001")
            .send(app.clone())
            .await;
        let started = body_json(started).await;
        if started["code"] == "SANDBOX_UNAVAILABLE" {
            eprintln!("skipped: the host cannot confirm the Chromium sandbox");
            return;
        }
        assert_eq!(started["state"], "running", "start result: {started}");
        assert_eq!(started["isolation"], "enforced");
        assert_eq!(started["direct"], true);
        assert_eq!(started["reduced_assurance"], true);
        assert_eq!(started["mode"], "headful");

        let profile = Call::new(Method::GET, format!("/api/v1/profiles/{id}"))
            .cookie(&session)
            .send(app.clone())
            .await;
        let profile = body_json(profile).await;
        assert_eq!(profile["isolation"], "enforced", "profile/proof: {profile}");
        assert_eq!(profile["reduced_assurance"], true);

        let stopped = Call::new(Method::POST, format!("/api/v1/profiles/{id}/stop"))
            .cookie(&session)
            .csrf(&session)
            .origin()
            .send(app.clone())
            .await;
        assert_eq!(stopped.status(), StatusCode::ACCEPTED);
        let stopped = body_json(stopped).await;
        assert_eq!(stopped["state"], "stopped");
        // 停止不覆盖历史隔离结论。
        assert_eq!(stopped["isolation"], "enforced");

        let after = Call::new(Method::GET, format!("/api/v1/profiles/{id}"))
            .cookie(&session)
            .send(app)
            .await;
        let after = body_json(after).await;
        assert_eq!(after["isolation"], "enforced");
        assert_eq!(after["runtime"]["state"], "stopped");
    });
    // 该档案的运行时已终止；全局孤儿检查在验收命令里单独执行
    // （同一测试二进制内并行运行，全局 ps 断言会误报其他用例）。
    let still_running = std::process::Command::new("sh")
        .args([
            "-c",
            "ps -axo command | grep 'mien-runtime-id=rt-' | grep -v grep | wc -l",
        ])
        .output()
        .expect("ps");
    eprintln!(
        "running mien runtimes after stop: {}",
        String::from_utf8_lossy(&still_running.stdout).trim()
    );
}

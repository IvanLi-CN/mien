//! 审计事件验收（`REQ-API-009`）。
//!
//! 读端点覆盖：稳定排序与二元组游标（同一毫秒不重复不遗漏）、五个筛选、
//! 未知参数/越界页大小/非法游标/非法 ULID/反向区间的 422、
//! 401 先于 403 先于 422，以及历史坏 `metadata_json` 行不放大成 500。
//!
//! 排序类用例直接写 `audit_events`：读端点的职责是"忠实读出存储里的行"，
//! 走认证路径生成每一条会让同毫秒断言不可控。写路径用例（T31 档案生命周期）
//! 反过来必须走真实路由，因为要验证的正是"领域函数成功后落一行审计"，
//! 并且只用 `profile_id`/`event_type` 筛选，不靠全量计数——登录与改密也写审计。

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
const PROFILE_A: &str = "01JC5W3X6Z8N4M2P9QR7STVWXY";
const PROFILE_B: &str = "01JC5W3X6Z8N4M2P9QR7STVWXZ";

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime")
}

fn state() -> (TempDir, AppState) {
    let dir = TempDir::new().expect("temp dir");
    let pool = mien_core::storage::open_and_migrate(&dir.path().join("mien.db")).expect("migrate");
    let auth = AuthState::default().with_bootstrap(mien_server::auth::password::Bootstrap::inline(
        ADMIN_PASSWORD,
    ));
    let state = AppState::new(dir.path(), Some(pool))
        .with_security(SecuritySettings::loopback())
        .with_auth(auth);
    (dir, state)
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
                subject: "audit-reader".to_owned(),
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
        self.headers.insert(
            header::CONTENT_TYPE,
            "application/json".parse().expect("content type"),
        );
        self.body = Some(body.to_string().into_bytes());
        self
    }

    async fn send(self, app: axum::Router) -> Response {
        let mut builder = Request::builder().method(self.method).uri(&self.path);
        for (name, value) in &self.headers {
            builder = builder.header(name, value);
        }
        let body = self.body.map_or_else(Body::empty, Body::from);
        app.oneshot(builder.body(body).expect("request"))
            .await
            .expect("response")
    }
}

async fn body_json(response: Response) -> (StatusCode, Value) {
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("body");
    if bytes.is_empty() {
        return (status, json!({}));
    }
    let parsed = serde_json::from_str(&String::from_utf8_lossy(&bytes)).unwrap_or_else(|error| {
        panic!(
            "non-JSON response {status}: {error}; body={}",
            String::from_utf8_lossy(&bytes)
        )
    });
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
    assert_eq!(response.status(), StatusCode::OK, "login");
    let cookie = set_cookie(&response);
    let (_, body) = body_json(response).await;
    let csrf = body["csrf_token"].as_str().expect("csrf").to_owned();
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

/// 插一行审计事件；`created_at` 由调用方给定以控制排序。
async fn insert(
    pool: &sqlx::SqlitePool,
    id: &str,
    event_type: &str,
    action: &str,
    profile_id: Option<&str>,
    created_at: &str,
) {
    insert_with_metadata(pool, id, event_type, action, profile_id, created_at, "{}").await;
}

async fn insert_with_metadata(
    pool: &sqlx::SqlitePool,
    id: &str,
    event_type: &str,
    action: &str,
    profile_id: Option<&str>,
    created_at: &str,
    metadata_json: &str,
) {
    sqlx::query(
        "INSERT INTO audit_events \
         (id, actor, event_type, profile_id, action, target, result, request_id, metadata_json, created_at) \
         VALUES (?1, 'admin', ?2, ?3, ?4, 'profiles', 'success', 'req-test', ?5, ?6)",
    )
    .bind(id)
    .bind(event_type)
    .bind(profile_id)
    .bind(action)
    .bind(metadata_json)
    .bind(created_at)
    .execute(pool)
    .await
    .expect("insert audit row");
}

/// 生成 26 位 Crockford Base32 形态的测试 id。
fn audit_id(index: usize) -> String {
    // 固定前缀保证字典序稳定；索引用 Crockford 字符集编码。
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut tail = [b'0'; 16];
    let mut value = index as u64;
    for slot in tail.iter_mut().rev() {
        *slot = ALPHABET[(value % 32) as usize];
        value /= 32;
    }
    // 10 + 16 = 26 位，首字符 `0` 满足 ULID 的 48 位时间戳上界。
    format!("01JC5W3X6Z{}", String::from_utf8_lossy(&tail))
}

async fn list(app: axum::Router, session: &Session, query: &str) -> (StatusCode, Value) {
    let path = if query.is_empty() {
        "/api/v1/audit/events".to_owned()
    } else {
        format!("/api/v1/audit/events?{query}")
    };
    body_json(Call::new(Method::GET, path).cookie(session).send(app).await).await
}

fn items(body: &Value) -> &Vec<Value> {
    body["items"].as_array().expect("items array")
}

#[test]
fn an_empty_audit_log_returns_an_empty_page() {
    let (_dir, state) = state();
    let app = router(state);

    runtime().block_on(async {
        // 登录与强制改密本身会写认证审计行，因此这里断言"没有任何业务行"，
        // 而不是"表为空"。
        let session = login(app.clone()).await;
        let (status, body) = list(app, &session, "event_type=profile.start").await;
        assert_eq!(status, StatusCode::OK, "empty page: {body}");
        assert!(items(&body).is_empty(), "no business rows expected: {body}");
        assert!(body["next_cursor"].is_null(), "no cursor: {body}");
    });
}

#[test]
fn every_row_is_returned_once_across_pages_with_stable_order() {
    let (_dir, state) = state();
    let pool = state.db().expect("pool").clone();
    let app = router(state);

    runtime().block_on(async {
        // 250 行，跨 25 个毫秒；每毫秒 10 行，因此同一毫秒必然多行。
        for index in 0..250_usize {
            let millis = index / 10;
            let created = format!(
                "2026-11-27T10:00:{:02}.{:03}Z",
                millis / 1000,
                millis % 1000
            );
            insert(
                &pool,
                &audit_id(index),
                "profile.start",
                "start",
                Some(PROFILE_A),
                &created,
            )
            .await;
        }
        let session = login(app.clone()).await;

        let mut seen: Vec<String> = Vec::new();
        let mut cursor: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &cursor {
                Some(value) if !value.is_empty() => {
                    format!("limit=200&to=2026-11-27T11:00:00Z&cursor={value}")
                }
                _ => "limit=200&to=2026-11-27T11:00:00Z".to_owned(),
            };
            let (status, body) = list(app.clone(), &session, &query).await;
            assert_eq!(status, StatusCode::OK, "page: {body}");
            let page_items = items(&body);
            assert!(page_items.len() <= 200, "page size cap");
            for item in page_items {
                seen.push(item["id"].as_str().expect("id").to_owned());
            }
            pages += 1;
            assert!(pages < 10, "pagination must terminate");
            match body["next_cursor"].as_str() {
                Some(next) => cursor = Some(next.to_owned()),
                None => break,
            }
        }

        assert_eq!(seen.len(), 250, "no duplicates and nothing missing");
        let mut unique = seen.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), 250, "every id appears exactly once");

        // 排序：`created_at DESC, id DESC`。
        let (_, first) = list(app, &session, "limit=200&to=2026-11-27T11:00:00Z").await;
        let first_items = items(&first);
        for pair in first_items.windows(2) {
            let left = (
                pair[0]["created_at"].as_str().expect("created_at"),
                pair[0]["id"].as_str().expect("id"),
            );
            let right = (
                pair[1]["created_at"].as_str().expect("created_at"),
                pair[1]["id"].as_str().expect("id"),
            );
            assert!(left >= right, "order broke at {left:?} vs {right:?}");
        }
    });
}

#[test]
fn rows_sharing_one_millisecond_appear_exactly_once_each() {
    let (_dir, state) = state();
    let pool = state.db().expect("pool").clone();
    let app = router(state);

    runtime().block_on(async {
        let created = "2026-11-27T10:00:00.500Z";
        for index in 0..5_usize {
            insert(
                &pool,
                &audit_id(index),
                "profile.stop",
                "stop",
                Some(PROFILE_A),
                created,
            )
            .await;
        }
        let session = login(app.clone()).await;

        // 页大小 2：同一毫秒的 5 行必须跨 3 页全部出现一次。
        // `to` 把登录产生的认证行排除在本用例之外。
        let mut seen = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let query = match &cursor {
                Some(value) => format!("limit=2&to=2026-11-27T11:00:00Z&cursor={value}"),
                None => "limit=2&to=2026-11-27T11:00:00Z".to_owned(),
            };
            let (status, body) = list(app.clone(), &session, &query).await;
            assert_eq!(status, StatusCode::OK, "page: {body}");
            for item in items(&body) {
                seen.push(item["id"].as_str().expect("id").to_owned());
            }
            match body["next_cursor"].as_str() {
                Some(next) => cursor = Some(next.to_owned()),
                None => break,
            }
        }
        seen.sort();
        seen.dedup();
        assert_eq!(
            seen.len(),
            5,
            "a shared millisecond must not drop or repeat rows"
        );
    });
}

#[test]
fn each_filter_narrows_the_page() {
    let (_dir, state) = state();
    let pool = state.db().expect("pool").clone();
    let app = router(state);

    runtime().block_on(async {
        insert(
            &pool,
            &audit_id(0),
            "profile.start",
            "start",
            Some(PROFILE_A),
            "2026-11-27T10:00:00Z",
        )
        .await;
        insert(
            &pool,
            &audit_id(1),
            "profile.stop",
            "stop",
            Some(PROFILE_A),
            "2026-11-27T10:01:00Z",
        )
        .await;
        insert(
            &pool,
            &audit_id(2),
            "profile.start",
            "start",
            Some(PROFILE_B),
            "2026-11-27T10:02:00Z",
        )
        .await;
        insert(
            &pool,
            &audit_id(3),
            "token.revoke",
            "revoke",
            None,
            "2026-11-27T10:03:00Z",
        )
        .await;
        let session = login(app.clone()).await;

        // 登录与改密会写认证审计行（当前时刻），因此每个断言都限定在造数据的
        // 10:xx 窗口内，否则计数会被认证事件污染。
        const WINDOW: &str = "to=2026-11-27T11:00:00Z";

        let (status, body) = list(
            app.clone(),
            &session,
            &format!("event_type=profile.start&{WINDOW}"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(items(&body).len(), 2, "event_type filter: {body}");

        let (_, body) = list(app.clone(), &session, &format!("action=stop&{WINDOW}")).await;
        assert_eq!(items(&body).len(), 1, "action filter: {body}");

        let (_, body) = list(
            app.clone(),
            &session,
            &format!("profile_id={PROFILE_A}&{WINDOW}"),
        )
        .await;
        assert_eq!(items(&body).len(), 2, "profile_id filter: {body}");

        let (_, body) = list(
            app.clone(),
            &session,
            "from=2026-11-27T10:01:00Z&to=2026-11-27T11:00:00Z",
        )
        .await;
        assert_eq!(items(&body).len(), 3, "from is inclusive: {body}");

        let (_, body) = list(app, &session, "to=2026-11-27T10:01:00Z").await;
        assert_eq!(items(&body).len(), 2, "to is inclusive: {body}");
    });
}

#[test]
fn invalid_filters_and_cursors_are_rejected() {
    let (_dir, state) = state();
    let pool = state.db().expect("pool").clone();
    let app = router(state);

    runtime().block_on(async {
        insert(
            &pool,
            &audit_id(0),
            "profile.start",
            "start",
            Some(PROFILE_A),
            "2026-11-27T10:00:00Z",
        )
        .await;
        let session = login(app.clone()).await;

        for query in [
            // 未知字段不得静默变成全量结果。
            "foo=1",
            // 页大小上限；消息必须点名 200。
            "limit=201",
            "limit=0",
            // 非法 ULID。
            "profile_id=nope",
            // 非法游标。
            "cursor=zz",
            // 反向区间。
            "from=2026-11-27T10:00:00Z&to=2026-11-27T09:00:00Z",
            // 带时区偏移的形态不可比较。
            "from=2026-11-27T10:00:00%2B08:00",
        ] {
            let (status, body) = list(app.clone(), &session, query).await;
            assert_eq!(
                status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "`{query}`: {body}"
            );
        }
        let (_, body) = list(app.clone(), &session, "limit=201").await;
        assert!(
            body.to_string().contains("200"),
            "the limit error must name the cap: {body}"
        );
    });
}

#[test]
fn authentication_precedes_authorization_and_validation() {
    let (_dir, state) = state();

    runtime().block_on(async {
        // 未认证：即使游标非法也必须是 401。
        let app = router(state.clone());
        let (status, body) = body_json(
            Call::new(Method::GET, "/api/v1/audit/events?cursor=zz")
                .send(app)
                .await,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "unauthenticated: {body}");

        // 有身份但缺 `audit.read`：403，且不因参数非法变成 422。
        let reader =
            state
                .clone()
                .with_auth(
                    AuthState::default().with_bearer(std::sync::Arc::new(Bearer {
                        scopes: &["profiles.read"],
                    })),
                );
        let app = router(reader);
        let (status, body) = body_json(
            Call::new(Method::GET, "/api/v1/audit/events?cursor=zz")
                .bearer("any")
                .send(app)
                .await,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "missing scope: {body}");

        // 有 scope 后非法参数才是 422。
        let owner = state.with_auth(AuthState::default().with_bearer(std::sync::Arc::new(
            Bearer {
                scopes: &["audit.read"],
            },
        )));
        let app = router(owner);
        let (status, body) = body_json(
            Call::new(Method::GET, "/api/v1/audit/events?cursor=zz")
                .bearer("any")
                .send(app)
                .await,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "bad cursor: {body}"
        );
    });
}

#[test]
fn events_carry_both_event_type_and_profile_id() {
    let (_dir, state) = state();
    let pool = state.db().expect("pool").clone();
    let app = router(state);

    runtime().block_on(async {
        insert(
            &pool,
            &audit_id(0),
            "proxy.check.completed",
            "check",
            Some(PROFILE_B),
            "2026-11-27T10:00:00Z",
        )
        .await;
        insert(
            &pool,
            &audit_id(1),
            "token.revoke",
            "revoke",
            None,
            "2026-11-27T10:01:00Z",
        )
        .await;
        let session = login(app.clone()).await;

        let (status, body) = list(app, &session, "to=2026-11-27T11:00:00Z").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(items(&body).len(), 2, "only the seeded rows: {body}");
        for item in items(&body) {
            assert!(item["event_type"].is_string(), "event_type missing: {item}");
            assert!(
                item.get("profile_id").is_some(),
                "profile_id key must be present even when null: {item}"
            );
            assert!(item["action"].is_string(), "action kept for compatibility");
        }
    });
}

#[test]
fn a_corrupt_metadata_row_does_not_break_the_page() {
    let (_dir, state) = state();
    let pool = state.db().expect("pool").clone();
    let app = router(state);

    runtime().block_on(async {
        insert(
            &pool,
            &audit_id(0),
            "profile.start",
            "start",
            Some(PROFILE_A),
            "2026-11-27T10:00:00Z",
        )
        .await;
        insert_with_metadata(
            &pool,
            &audit_id(1),
            "profile.stop",
            "stop",
            Some(PROFILE_A),
            "2026-11-27T10:01:00Z",
            "not-json",
        )
        .await;
        let session = login(app.clone()).await;

        let (status, body) = list(app, &session, "to=2026-11-27T11:00:00Z").await;
        assert_eq!(status, StatusCode::OK, "a corrupt row must not 500: {body}");
        assert_eq!(items(&body).len(), 2, "only the seeded rows: {body}");
        let corrupt = items(&body)
            .iter()
            .find(|item| item["event_type"] == "profile.stop")
            .expect("corrupt row present");
        assert_eq!(
            corrupt["metadata"],
            json!({}),
            "fallback to an empty object"
        );
    });
}

/// 导出会写 `profile:<id>` 审计，且 `target` 能被 `profile_id` 筛出来。
///
/// 导出是档案级审计的最早实现；此处保留它作为独立回归点，确保后来加入的
/// CRUD/生命周期事件没有改掉既有 target 约定。
#[test]
fn records_written_through_the_api_carry_their_profile_id() {
    let (_dir, state) = state();
    let app = router(state);

    runtime().block_on(async {
        let session = login(app.clone()).await;
        let created = Call::new(Method::POST, "/api/v1/profiles")
            .cookie(&session)
            .csrf(&session)
            .origin()
            .json(json!({
                "name": "audit seed",
                "fingerprint": {
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
                },
                "mode": "headful",
                "start_url": "about:blank",
            }))
            .send(app.clone())
            .await;
        let (status, body) = body_json(created).await;
        assert_eq!(status, StatusCode::CREATED, "seed profile: {body}");
        let profile_id = body["id"].as_str().expect("profile id").to_owned();

        let exported = Call::new(
            Method::POST,
            format!("/api/v1/profiles/{profile_id}/export"),
        )
        .cookie(&session)
        .csrf(&session)
        .origin()
        .json(json!({ "format": "plain" }))
        .send(app.clone())
        .await;
        let (status, body) = body_json(exported).await;
        assert_eq!(status, StatusCode::OK, "export: {body}");

        let (status, page) = list(app.clone(), &session, &format!("profile_id={profile_id}")).await;
        assert_eq!(status, StatusCode::OK, "filtered page: {page}");
        let rows = items(&page);
        assert!(
            !rows.is_empty(),
            "an audit row targeting a profile must be filterable by profile_id: {page}"
        );
        for row in rows {
            assert_eq!(row["profile_id"], profile_id.as_str());
        }
        assert!(
            rows.iter()
                .any(|row| row["event_type"] == "profile.export.plain"),
            "the export event must be the one found: {page}"
        );

        // 反向确认：筛选不会带出其他档案的行。
        let (_, all) = list(app, &session, "").await;
        for row in items(&all) {
            if let Some(id) = row["profile_id"].as_str() {
                assert_eq!(id, profile_id, "unrelated profile leaked into the filter");
            }
        }
    });
}

// ---------------------------------------------------------------------------
// 档案生命周期审计（T31）
// ---------------------------------------------------------------------------

/// 固定的档案创建请求体；`notes`/`start_url` 用独特值以便做泄漏负例断言。
fn profile_input(name: &str) -> Value {
    json!({
        "name": name,
        "notes": "NOTE-LEAK-CANARY-9f3a",
        "start_url": "https://CANARY-URL-7b21.example.com/landing",
        "fingerprint": {
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
        },
        "mode": "headful",
    })
}

async fn create_profile(
    app: axum::Router,
    session: &Session,
    name: &str,
    key: Option<&str>,
) -> Value {
    let mut call = Call::new(Method::POST, "/api/v1/profiles")
        .cookie(session)
        .csrf(session)
        .origin()
        .json(profile_input(name));
    if let Some(key) = key {
        call = call.header(header::HeaderName::from_static("idempotency-key"), key);
    }
    let (status, body) = body_json(call.send(app).await).await;
    assert_eq!(status, StatusCode::CREATED, "create {name}: {body}");
    body
}

/// 一次写操作的响应断言：只关心状态，正文供失败信息使用。
async fn send(app: axum::Router, call: Call) -> (StatusCode, Value) {
    body_json(call.send(app).await).await
}

/// 直接读该档案的审计行：`(event_type, target, result, metadata_json)`。
async fn rows_for(
    pool: &sqlx::SqlitePool,
    profile_id: &str,
) -> Vec<(String, String, String, String)> {
    sqlx::query_as(
        "SELECT event_type, target, result, metadata_json FROM audit_events \
         WHERE profile_id=?1 ORDER BY created_at, id",
    )
    .bind(profile_id)
    .fetch_all(pool)
    .await
    .expect("audit rows")
}

fn event_types(rows: &[(String, String, String, String)]) -> Vec<&str> {
    rows.iter().map(|row| row.0.as_str()).collect()
}

#[test]
fn every_lifecycle_endpoint_writes_its_event_with_the_profile_target() {
    let (_dir, state) = state();
    let pool = state.db().expect("pool").clone();
    let app = router(state);

    runtime().block_on(async {
        let session = login(app.clone()).await;
        let created = create_profile(app.clone(), &session, "lifecycle", None).await;
        let source = created["id"].as_str().expect("id").to_owned();

        // update
        let (status, body) = send(
            app.clone(),
            Call::new(Method::PATCH, format!("/api/v1/profiles/{source}"))
                .cookie(&session)
                .csrf(&session)
                .origin()
                .json(json!({ "name": "lifecycle renamed" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "update: {body}");

        // duplicate：新 id 是 target，来源 id 进 metadata
        let (status, copy) = send(
            app.clone(),
            Call::new(Method::POST, format!("/api/v1/profiles/{source}/duplicate"))
                .cookie(&session)
                .csrf(&session)
                .origin()
                .json(json!({ "copy_fingerprint": false, "copy_proxy": false })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "duplicate: {copy}");
        let copy_id = copy["id"].as_str().expect("copy id").to_owned();
        assert_ne!(copy_id, source, "duplicate must mint a new id");

        let copy_rows = rows_for(&pool, &copy_id).await;
        assert_eq!(
            event_types(&copy_rows),
            vec!["profile.duplicated"],
            "only the duplicate event targets the copy: {copy_rows:?}"
        );
        assert_eq!(copy_rows[0].1, format!("profile:{copy_id}"));
        let copy_metadata: Value =
            serde_json::from_str(&copy_rows[0].3).expect("duplicate metadata");
        assert_eq!(copy_metadata["source_id"], source.as_str());
        assert_eq!(
            copy_metadata.as_object().expect("object").len(),
            1,
            "duplicate metadata is just the source id: {copy_metadata}"
        );

        // archive，然后重复归档：领域函数是 409，不得再写事件
        let (status, body) = send(
            app.clone(),
            Call::new(Method::POST, format!("/api/v1/profiles/{source}/archive"))
                .cookie(&session)
                .csrf(&session)
                .origin(),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "archive: {body}");
        let (status, body) = send(
            app.clone(),
            Call::new(Method::POST, format!("/api/v1/profiles/{source}/archive"))
                .cookie(&session)
                .csrf(&session)
                .origin(),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "second archive: {body}");
        assert_eq!(body["code"], "PROFILE_ARCHIVED");

        // restore，以及对已 active 的档案再次 restore：幂等空操作，不写事件
        let (status, body) = send(
            app.clone(),
            Call::new(Method::POST, format!("/api/v1/profiles/{source}/restore"))
                .cookie(&session)
                .csrf(&session)
                .origin(),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "restore: {body}");
        let (status, _) = send(
            app.clone(),
            Call::new(Method::POST, format!("/api/v1/profiles/{source}/restore"))
                .cookie(&session)
                .csrf(&session)
                .origin(),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "restoring an active profile is an idempotent no-op"
        );

        // 软删除，重复软删除：幂等空操作
        let (status, body) = send(
            app.clone(),
            Call::new(Method::DELETE, format!("/api/v1/profiles/{source}"))
                .cookie(&session)
                .csrf(&session)
                .origin(),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "soft delete: {body}");
        let (status, _) = send(
            app.clone(),
            Call::new(Method::DELETE, format!("/api/v1/profiles/{source}"))
                .cookie(&session)
                .csrf(&session)
                .origin(),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "deleting an already deleted profile is an idempotent no-op"
        );

        // 永久清理
        let (status, body) = send(
            app.clone(),
            Call::new(
                Method::DELETE,
                format!("/api/v1/profiles/{source}?permanent=true"),
            )
            .cookie(&session)
            .csrf(&session)
            .origin(),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "purge: {body}");

        let rows = rows_for(&pool, &source).await;
        assert_eq!(
            event_types(&rows),
            vec![
                "profile.created",
                "profile.updated",
                "profile.archived",
                "profile.restored",
                "profile.deleted",
                "profile.purged",
            ],
            "exactly one event per real transition: {rows:?}"
        );
        for row in &rows {
            assert_eq!(row.1, format!("profile:{source}"), "target: {row:?}");
            assert_eq!(row.2, "success", "result: {row:?}");
        }

        let soft_delete: Value = serde_json::from_str(&rows[4].3).expect("delete metadata");
        assert_eq!(soft_delete, json!({ "permanent": false }));
        let purge: Value = serde_json::from_str(&rows[5].3).expect("purge metadata");
        assert_eq!(purge, json!({ "permanent": true }));
    });
}

#[test]
fn action_mirrors_event_type_and_the_profile_filter_finds_every_event() {
    let (_dir, state) = state();
    let pool = state.db().expect("pool").clone();
    let app = router(state);

    runtime().block_on(async {
        let session = login(app.clone()).await;
        let created = create_profile(app.clone(), &session, "filtered", None).await;
        let id = created["id"].as_str().expect("id").to_owned();

        let other = create_profile(app.clone(), &session, "bystander", None).await;
        let other_id = other["id"].as_str().expect("id").to_owned();

        for (method, path) in [
            (Method::POST, format!("/api/v1/profiles/{id}/archive")),
            (Method::POST, format!("/api/v1/profiles/{id}/restore")),
        ] {
            let (status, body) = send(
                app.clone(),
                Call::new(method, path)
                    .cookie(&session)
                    .csrf(&session)
                    .origin(),
            )
            .await;
            assert_eq!(status, StatusCode::NO_CONTENT, "transition: {body}");
        }

        let actions: Vec<(String, String)> =
            sqlx::query_as("SELECT event_type, action FROM audit_events WHERE profile_id=?1")
                .bind(&id)
                .fetch_all(&pool)
                .await
                .expect("event/action pairs");
        for (event_type, action) in &actions {
            assert_eq!(event_type, action, "action must mirror event_type");
        }

        let (status, page) =
            list(app.clone(), &session, &format!("profile_id={id}&limit=200")).await;
        assert_eq!(status, StatusCode::OK, "filtered page: {page}");
        let found: Vec<&str> = items(&page)
            .iter()
            .map(|item| item["event_type"].as_str().expect("event_type"))
            .collect();
        for expected in ["profile.created", "profile.archived", "profile.restored"] {
            assert!(
                found.contains(&expected),
                "`{expected}` must be filterable by profile_id: {page}"
            );
        }
        for item in items(&page) {
            assert_eq!(item["profile_id"], id.as_str(), "no foreign rows: {page}");
        }

        // 反向确认：另一个档案的筛选页里没有本档案的事件。
        let (_, other_page) = list(
            app,
            &session,
            &format!("profile_id={other_id}&event_type=profile.archived"),
        )
        .await;
        assert!(
            items(&other_page).is_empty(),
            "the filter must not leak across profiles: {other_page}"
        );
    });
}

#[test]
fn replayed_idempotent_create_writes_one_audit_row() {
    let (_dir, state) = state();
    let pool = state.db().expect("pool").clone();
    let app = router(state);

    runtime().block_on(async {
        let session = login(app.clone()).await;
        let first =
            create_profile(app.clone(), &session, "replay", Some("audit-replay-key-1")).await;
        let id = first["id"].as_str().expect("id").to_owned();

        let (status, replay) = body_json(
            Call::new(Method::POST, "/api/v1/profiles")
                .cookie(&session)
                .csrf(&session)
                .origin()
                .header(
                    header::HeaderName::from_static("idempotency-key"),
                    "audit-replay-key-1",
                )
                .json(profile_input("replay"))
                .send(app.clone())
                .await,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "replay: {replay}");
        assert_eq!(
            replay["id"],
            id.as_str(),
            "replay returns the first profile"
        );

        let rows = rows_for(&pool, &id).await;
        assert_eq!(
            event_types(&rows),
            vec!["profile.created"],
            "a cached replay must not append a second event: {rows:?}"
        );
    });
}

#[test]
fn lifecycle_metadata_carries_no_seed_credential_notes_or_url() {
    let (_dir, state) = state();
    let pool = state.db().expect("pool").clone();
    let app = router(state);

    runtime().block_on(async {
        let session = login(app.clone()).await;
        let created = create_profile(app.clone(), &session, "canary", None).await;
        let id = created["id"].as_str().expect("id").to_owned();
        let seed = created["fingerprint"]["seed"]
            .as_u64()
            .expect("server-side seed");
        assert!(seed > 0, "seed must exist to make the negative test real");

        let (status, body) = send(
            app.clone(),
            Call::new(
                Method::POST,
                format!("/api/v1/profiles/{id}/duplicate"),
            )
            .cookie(&session)
            .csrf(&session)
            .origin(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "duplicate: {body}");
        let copy_id = body["id"].as_str().expect("copy id");

        let metadata: Vec<String> = sqlx::query_scalar(
            "SELECT metadata_json FROM audit_events WHERE event_type LIKE 'profile.%'",
        )
        .fetch_all(&pool)
        .await
        .expect("lifecycle metadata");

        for row in &metadata {
            assert!(!row.contains("NOTE-LEAK-CANARY"), "notes leaked: {row}");
            assert!(!row.contains("CANARY-URL-7b21"), "full URL leaked: {row}");
            assert!(!row.contains("MIEN_"), "server config leaked: {row}");
            let parsed: Value = serde_json::from_str(row).expect("metadata object");
            let object = parsed.as_object().expect("object");
            for (key, value) in object {
                assert!(
                    matches!(key.as_str(), "source_id" | "permanent"),
                    "unexpected lifecycle metadata key `{key}` in {row}"
                );
                let forbidden = ["seed", "url", "note", "credential", "password"];
                for term in forbidden {
                    assert!(
                        !key.to_ascii_lowercase().contains(term),
                        "forbidden metadata key `{key}`: {row}"
                    );
                    assert!(
                        !value
                            .as_str()
                            .is_some_and(|text| text.to_ascii_lowercase().contains(term)),
                        "forbidden metadata value at `{key}`: {row}"
                    );
                }
                assert!(
                    value.as_u64() != Some(seed),
                    "fingerprint seed leaked at `{key}`: {row}"
                );
            }
        }

        let copy_metadata: String = sqlx::query_scalar(
            "SELECT metadata_json FROM audit_events WHERE profile_id=?1 AND event_type='profile.duplicated'",
        )
        .bind(copy_id)
        .fetch_one(&pool)
        .await
        .expect("duplicate metadata");
        assert!(
            !copy_metadata.contains(&seed.to_string()),
            "duplicate must record ids, not fingerprint material: {copy_metadata}"
        );
    });
}

#[test]
fn export_still_writes_its_own_events_after_the_lifecycle_change() {
    let (_dir, state) = state();
    let pool = state.db().expect("pool").clone();
    let app = router(state);

    runtime().block_on(async {
        let session = login(app.clone()).await;
        let created = create_profile(app.clone(), &session, "exportable", None).await;
        let id = created["id"].as_str().expect("id").to_owned();

        for (format, expected) in [
            ("plain", "profile.export.plain"),
            ("encrypted", "profile.export.encrypted"),
        ] {
            // `passphrase` 只在加密导出时允许，明文导出带它是 422。
            let payload = if format == "encrypted" {
                json!({ "format": format, "passphrase": "audit-test-passphrase" })
            } else {
                json!({ "format": format })
            };
            let (status, body) = send(
                app.clone(),
                Call::new(Method::POST, format!("/api/v1/profiles/{id}/export"))
                    .cookie(&session)
                    .csrf(&session)
                    .origin()
                    .json(payload),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "export {format}: {body}");
            let rows = rows_for(&pool, &id).await;
            assert!(
                event_types(&rows).contains(&expected),
                "`{expected}` must still be written: {rows:?}"
            );
        }
    });
}

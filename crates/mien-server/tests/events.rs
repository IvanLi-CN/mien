//! 事件流 WebSocket 与票据验收（T25）。
//!
//! 覆盖 `VER-API-002` 的事件部分与 `REQ-API-011`：票据一次性/过期、Origin 校验、
//! URL 长效 Token 拒绝、`after` 全部边界、`sequence > after` 回放与实时衔接、
//! 7 天/100 MiB 最旧前缀裁剪后的 `floor` 前进、单条超大事件拒绝，以及访问日志
//! 对 `ticket` 参数脱敏。
//!
//! WebSocket 用真实监听器与 `tokio-tungstenite` 客户端验证，错误响应走同一路由的
//! `oneshot` 调用；两者共享同一个 `AppState` 数据库。迁移在 `#[test]` 里同步完成，
//! 异步断言集中在 `runtime()` 的 `block_on` 内（`open_and_migrate` 自带运行时）。

use std::io::Write;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;

use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use mien_server::events::store::{self, EventLimits};
use mien_server::events::{EventRuntime, ticket};
use mien_server::router;
use mien_server::state::{AppState, AuthState, SecuritySettings};
use mien_server::tokens::store::TokenStore;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::Message;
use tower::ServiceExt;

const ADMIN_PASSWORD: &str = "correct horse battery staple";
const NEW_PASSWORD: &str = "a brand new passphrase";
/// 回环开发模式的合法同源。
const ORIGIN: &str = "http://127.0.0.1:8787";
const EVENT_TYPE: &str = "profile.updated";

/// 多线程运行时：每个用例都在同一个 runtime 里跑服务端 accept 循环、WebSocket
/// 任务与客户端读取。`current_thread` 只有一个线程，客户端在自己的
/// `client.next()` 上忙等时，服务端任务的 live-gap 检查与数据库 await 点拿不到
/// 调度，`retention_advancing_past_a_live_cursor_closes_the_socket` 会一路等满
/// 30 秒超时（整文件从 ~5s 变成 ~34s 并失败）。与 `proxies_api.rs` 同一原因。
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime")
}

/// 迁移后的临时数据根与服务状态；保留参数按测试注入。
fn state_with(events: EventRuntime) -> (TempDir, AppState) {
    let dir = TempDir::new().expect("temp dir");
    let pool = mien_core::storage::open_and_migrate(&dir.path().join("mien.db")).expect("migrate");
    let auth = AuthState::default().with_bootstrap(mien_server::auth::password::Bootstrap::inline(
        ADMIN_PASSWORD,
    ));
    let state = AppState::new(dir.path(), Some(pool))
        .with_security(SecuritySettings::loopback())
        .with_auth(auth)
        .with_events(events);
    (dir, state)
}

struct Session {
    cookie: String,
    csrf: String,
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

    fn origin(self) -> Self {
        self.header(header::ORIGIN, ORIGIN)
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

    fn json(self, body: Value) -> Self {
        let mut call = self;
        call.body = Some(body.to_string().into_bytes());
        call
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

/// 登录并清空强制改密，得到可直接签发票据的会话。
async fn login(app: axum::Router) -> Session {
    let response = Call::new(Method::POST, "/api/v1/auth/login")
        .origin()
        .json(json!({ "password": ADMIN_PASSWORD }))
        .send(app.clone())
        .await;
    assert_eq!(response.status(), StatusCode::OK, "login");
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
        .expect("session cookie value");
    let body = body_json(response).await;
    let csrf = body["csrf_token"].as_str().expect("csrf").to_owned();
    let session = Session { cookie, csrf };
    let changed = Call::new(Method::POST, "/api/v1/auth/password")
        .cookie(&session)
        .csrf(&session)
        .origin()
        .json(json!({
            "current_password": ADMIN_PASSWORD,
            "new_password": NEW_PASSWORD,
        }))
        .send(app)
        .await;
    assert_eq!(changed.status(), StatusCode::NO_CONTENT, "password change");
    session
}

/// 签发一张票据；明文只在响应中出现一次。
async fn issue_ticket(app: axum::Router, session: &Session) -> Value {
    let response = Call::new(Method::POST, "/api/v1/events/ticket")
        .cookie(session)
        .csrf(session)
        .origin()
        .send(app)
        .await;
    assert_eq!(response.status(), StatusCode::CREATED, "ticket endpoint");
    let body = body_json(response).await;
    assert!(
        body["ticket"]
            .as_str()
            .is_some_and(|value| value.starts_with("mien_t_")),
        "ticket body: {body}"
    );
    assert!(
        body["expires_at"]
            .as_str()
            .is_some_and(|value| value.ends_with('Z')),
        "ticket body: {body}"
    );
    body
}

// ---------------------------------------------------------------------------
// WebSocket 客户端
// ---------------------------------------------------------------------------

type Client =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// 启动真实监听器；服务随测试运行时结束而停止。
async fn serve(app: axum::Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    address
}

enum Handshake {
    Open(Box<Client>),
    Rejected { status: StatusCode, body: Value },
}

/// 建立一次升级：`query` 不含前导 `?`；`cookie` 可选。
async fn connect(
    address: SocketAddr,
    query: &str,
    origin: &str,
    cookie: Option<&str>,
) -> Handshake {
    let url = if query.is_empty() {
        format!("ws://{address}/api/v1/events")
    } else {
        format!("ws://{address}/api/v1/events?{query}")
    };
    let mut request = url.into_client_request().expect("client request");
    request
        .headers_mut()
        .insert(header::ORIGIN, origin.parse().expect("origin"));
    if let Some(cookie) = cookie {
        request.headers_mut().insert(
            header::COOKIE,
            format!("{}={cookie}", mien_server::auth::session::COOKIE_NAME)
                .parse()
                .expect("cookie"),
        );
    }
    match tokio_tungstenite::connect_async(request).await {
        Ok((socket, _response)) => Handshake::Open(Box::new(socket)),
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
            let status = response.status();
            let body = response
                .body()
                .clone()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or(Value::Null);
            Handshake::Rejected { status, body }
        }
        Err(error) => panic!("handshake failed: {error}"),
    }
}

async fn expect_open(address: SocketAddr, query: &str, cookie: Option<&str>) -> Client {
    match connect(address, query, ORIGIN, cookie).await {
        Handshake::Open(socket) => *socket,
        Handshake::Rejected { status, body } => panic!("expected 101, got {status}: {body}"),
    }
}

async fn expect_rejected(
    address: SocketAddr,
    query: &str,
    origin: &str,
    cookie: Option<&str>,
    status: StatusCode,
    code: &str,
) -> Value {
    match connect(address, query, origin, cookie).await {
        Handshake::Open(_) => panic!("expected {status} {code}, got 101"),
        Handshake::Rejected {
            status: actual,
            body,
        } => {
            assert_eq!(actual, status, "status for {query}: {body}");
            assert_eq!(body["code"], code, "code for {query}: {body}");
            body
        }
    }
}

async fn next_event(client: &mut Client) -> Value {
    let message = tokio::time::timeout(Duration::from_secs(5), client.next())
        .await
        .expect("event timeout")
        .expect("stream ended")
        .expect("websocket error");
    match message {
        Message::Text(text) => serde_json::from_str(&text).expect("event JSON"),
        other => panic!("unexpected message: {other:?}"),
    }
}

async fn expect_no_event(client: &mut Client, within: Duration) {
    match tokio::time::timeout(within, client.next()).await {
        Err(_) => {}
        Ok(Some(Ok(Message::Ping(_) | Message::Pong(_)))) => {}
        Ok(Some(Ok(other))) => panic!("unexpected message: {other:?}"),
        Ok(Some(Err(error))) => panic!("websocket error: {error}"),
        Ok(None) => panic!("stream ended"),
    }
}

fn sequence(event: &Value) -> i64 {
    event["sequence"].as_i64().expect("sequence")
}

// ---------------------------------------------------------------------------
// 票据
// ---------------------------------------------------------------------------

#[test]
fn ticket_is_single_use_and_bound_to_a_live_session() {
    let (_dir, state) = state_with(EventRuntime::default());
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state);
        let session = login(app.clone()).await;
        let body = issue_ticket(app.clone(), &session).await;
        let raw_ticket = body["ticket"].as_str().expect("ticket").to_owned();

        let address = serve(app).await;
        let client = expect_open(address, &format!("ticket={raw_ticket}"), None).await;
        drop(client);

        // 同一票据第二次使用：401 EVENT_TICKET_INVALID。
        expect_rejected(
            address,
            &format!("ticket={raw_ticket}"),
            ORIGIN,
            None,
            StatusCode::UNAUTHORIZED,
            "EVENT_TICKET_INVALID",
        )
        .await;

        // 无票据且无 Cookie 也是 401；伪造票据同理。
        expect_rejected(
            address,
            "",
            ORIGIN,
            None,
            StatusCode::UNAUTHORIZED,
            "EVENT_TICKET_INVALID",
        )
        .await;
        expect_rejected(
            address,
            "ticket=mien_t_forged",
            ORIGIN,
            None,
            StatusCode::UNAUTHORIZED,
            "EVENT_TICKET_INVALID",
        )
        .await;

        // 有效会话 Cookie 单独认证，不需要票据。
        let client = expect_open(address, "", Some(&session.cookie)).await;
        drop(client);
    });
}

#[test]
fn expired_ticket_is_rejected() {
    let (_dir, state) = state_with(EventRuntime::default().with_ticket_ttl(Duration::ZERO));
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state);
        let session = login(app.clone()).await;
        let body = issue_ticket(app.clone(), &session).await;
        let raw_ticket = body["ticket"].as_str().expect("ticket").to_owned();
        let address = serve(app).await;

        expect_rejected(
            address,
            &format!("ticket={raw_ticket}"),
            ORIGIN,
            None,
            StatusCode::UNAUTHORIZED,
            "EVENT_TICKET_INVALID",
        )
        .await;
    });
}

#[test]
fn cross_origin_upgrade_is_forbidden_and_does_not_burn_the_ticket() {
    let (_dir, state) = state_with(EventRuntime::default());
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state);
        let session = login(app.clone()).await;
        let body = issue_ticket(app.clone(), &session).await;
        let raw_ticket = body["ticket"].as_str().expect("ticket").to_owned();
        let address = serve(app).await;

        // 票据有效也无济于事：跨源升级 403。
        expect_rejected(
            address,
            &format!("ticket={raw_ticket}"),
            "http://evil.example",
            None,
            StatusCode::FORBIDDEN,
            "AUTH_ORIGIN_INVALID",
        )
        .await;
        expect_rejected(
            address,
            "",
            "http://evil.example",
            Some(&session.cookie),
            StatusCode::FORBIDDEN,
            "AUTH_ORIGIN_INVALID",
        )
        .await;
        // 票据仍然可用：403 不消费票据。
        let client = expect_open(address, &format!("ticket={raw_ticket}"), None).await;
        drop(client);
    });
}

#[test]
fn long_lived_tokens_in_the_url_are_rejected() {
    let (_dir, state) = state_with(EventRuntime::default());
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state);
        let session = login(app.clone()).await;
        let address = serve(app).await;

        // 即使带着有效 Cookie，URL 长效 Token 也按 EVENT_TICKET_INVALID 拒绝。
        for query in [
            "token=mien_abc",
            "access_token=mien_abc",
            "api_key=mien_abc",
        ] {
            expect_rejected(
                address,
                query,
                ORIGIN,
                Some(&session.cookie),
                StatusCode::UNAUTHORIZED,
                "EVENT_TICKET_INVALID",
            )
            .await;
        }
    });
}

#[test]
fn ticket_bound_to_a_revoked_session_is_rejected() {
    let (_dir, state) = state_with(EventRuntime::default());
    let pool = state.db().cloned().expect("pool");
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state);
        let _session = login(app.clone()).await;
        let issued = ticket::issue(
            &pool,
            Duration::from_secs(60),
            &ticket::Binding::Session(
                sqlx::query_scalar::<_, String>("SELECT id FROM sessions LIMIT 1")
                    .fetch_one(&pool)
                    .await
                    .expect("session id"),
            ),
        )
        .await
        .expect("issue");
        sqlx::query("UPDATE sessions SET revoked_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')")
            .execute(&pool)
            .await
            .expect("revoke");

        let address = serve(app).await;
        expect_rejected(
            address,
            &format!("ticket={}", issued.ticket),
            ORIGIN,
            None,
            StatusCode::UNAUTHORIZED,
            "EVENT_TICKET_INVALID",
        )
        .await;
    });
}

// ---------------------------------------------------------------------------
// after 边界
// ---------------------------------------------------------------------------

#[test]
fn after_boundaries_and_replay_semantics() {
    let (_dir, state) = state_with(EventRuntime::default());
    let pool = state.db().cloned().expect("pool");
    let limits = state.events().limits().clone();
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state);
        let session = login(app.clone()).await;

        // 空库新流：high_water = 0，after=0 可连接且无回放。
        let address = serve(app.clone()).await;
        let mut client = expect_open(address, "after=0", Some(&session.cookie)).await;
        expect_no_event(&mut client, Duration::from_millis(200)).await;
        drop(client);

        for index in 1..=5 {
            store::append(
                &pool,
                &limits,
                EVENT_TYPE,
                &json!({ "profile_id": "p1", "name": format!("event-{index}") }),
            )
            .await
            .expect("append");
        }

        // 回放只返回 sequence > after。
        let mut client = expect_open(address, "after=2", Some(&session.cookie)).await;
        for expected in 3..=5 {
            let event = next_event(&mut client).await;
            assert_eq!(sequence(&event), expected);
            assert_eq!(event["type"], EVENT_TYPE);
            assert_eq!(event["schema_version"], "1");
        }
        drop(client);

        // after = high_water：无回放，但新事件能实时衔接。
        let mut client = expect_open(address, "after=5", Some(&session.cookie)).await;
        expect_no_event(&mut client, Duration::from_millis(200)).await;
        store::append(
            &pool,
            &limits,
            EVENT_TYPE,
            &json!({ "profile_id": "p1", "name": "live" }),
        )
        .await
        .expect("append");
        let live = next_event(&mut client).await;
        assert_eq!(sequence(&live), 6);
        drop(client);

        // 负数、非整数、空值、显式 `+`、未知参数 → 422。
        for query in ["after=-1", "after=abc", "after=1.5", "after=", "after=%2B1"] {
            expect_rejected(
                address,
                query,
                ORIGIN,
                Some(&session.cookie),
                StatusCode::UNPROCESSABLE_ENTITY,
                "VALIDATION_FAILED",
            )
            .await;
        }
        expect_rejected(
            address,
            "after=1&limit=10",
            ORIGIN,
            Some(&session.cookie),
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_FAILED",
        )
        .await;

        // 未来游标 → 422；high_water 当前为 6。
        expect_rejected(
            address,
            "after=7",
            ORIGIN,
            Some(&session.cookie),
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_FAILED",
        )
        .await;
    });
}

// ---------------------------------------------------------------------------
// 保留窗口
// ---------------------------------------------------------------------------

/// 每条事件约 100 字节 payload_json；用于构造超出注入预算的序列。
async fn append_sized(
    pool: &sqlx::SqlitePool,
    limits: &EventLimits,
    index: usize,
) -> store::EventRecord {
    let payload = json!({
        "profile_id": "p1",
        "name": format!("event-{index:02}-{}", "x".repeat(60)),
    });
    store::append(pool, limits, EVENT_TYPE, &payload)
        .await
        .expect("append")
}

#[test]
fn retention_prunes_the_oldest_prefix_and_advances_the_floor() {
    // 预算只够保留约两条事件，避免测试依赖真实 100 MiB。
    let limits = EventLimits {
        retention: Duration::from_secs(7 * 24 * 60 * 60),
        max_bytes: 220,
    };
    let (_dir, state) = state_with(EventRuntime::default().with_limits(limits.clone()));
    let pool = state.db().cloned().expect("pool");
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state);
        let session = login(app.clone()).await;

        for index in 1..=4 {
            append_sized(&pool, &limits, index).await;
        }
        // 每条 98 字节 payload；220 字节预算只能保留两条，floor 前进到 2。
        let snapshot = store::snapshot(&pool, &limits, None)
            .await
            .expect("snapshot");
        assert_eq!(snapshot.high_water, 4);
        assert_eq!(snapshot.floor, 2, "pruning must advance the floor");
        let floor = snapshot.floor;
        let earliest = floor + 1;
        assert_eq!(earliest, 3, "retention keeps sequences 3 and 4");

        let address = serve(app).await;
        // after < floor → 409 EVENT_REPLAY_WINDOW_EXPIRED。
        let body = expect_rejected(
            address,
            &format!("after={}", floor - 1),
            ORIGIN,
            Some(&session.cookie),
            StatusCode::CONFLICT,
            "EVENT_REPLAY_WINDOW_EXPIRED",
        )
        .await;
        assert_eq!(body["details"]["floor"], floor);
        assert_eq!(body["details"]["high_water"], 4);

        // after = floor → 101，且完整回放保留前缀（无空洞、无跳过）。
        let mut client =
            expect_open(address, &format!("after={floor}"), Some(&session.cookie)).await;
        for expected in earliest..=4 {
            let event = next_event(&mut client).await;
            assert_eq!(sequence(&event), expected);
        }
        drop(client);

        // after = 0（新流值）但序列已被裁掉 → 409，不静默跳过。
        expect_rejected(
            address,
            "after=0",
            ORIGIN,
            Some(&session.cookie),
            StatusCode::CONFLICT,
            "EVENT_REPLAY_WINDOW_EXPIRED",
        )
        .await;
    });
}

#[test]
fn time_based_pruning_keeps_high_water_and_rejects_stale_cursors() {
    let (_dir, state) = state_with(EventRuntime::default());
    let pool = state.db().cloned().expect("pool");
    let limits = state.events().limits().clone();
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state);
        let session = login(app.clone()).await;

        for index in 1..=3 {
            append_sized(&pool, &limits, index).await;
        }
        // 把所有行推到 8 天前：7 天窗口必须清空保留集，但 sqlite_sequence 不回退。
        sqlx::query(
            "UPDATE event_log SET created_at = strftime('%Y-%m-%dT%H:%M:%fZ','now','-8 days')",
        )
        .execute(&pool)
        .await
        .expect("age rows");

        let snapshot = store::snapshot(&pool, &limits, None)
            .await
            .expect("snapshot");
        assert_eq!(snapshot.high_water, 3);
        assert_eq!(snapshot.floor, 3, "empty retention: floor == high_water");
        assert!(snapshot.replay.is_empty());

        let address = serve(app).await;
        // after = high_water 仍可接续实时流。
        let mut client = expect_open(address, "after=3", Some(&session.cookie)).await;
        expect_no_event(&mut client, Duration::from_millis(200)).await;
        drop(client);
        // after = high_water - 1 已过期：409，而不是静默跳过。
        expect_rejected(
            address,
            "after=2",
            ORIGIN,
            Some(&session.cookie),
            StatusCode::CONFLICT,
            "EVENT_REPLAY_WINDOW_EXPIRED",
        )
        .await;
    });
}

#[test]
fn retention_advancing_past_a_live_cursor_closes_the_socket() {
    // 极小预算：连接建立后继续写入，保留窗口很快越过实时游标。
    let limits = EventLimits {
        retention: Duration::from_secs(7 * 24 * 60 * 60),
        max_bytes: 150,
    };
    let (_dir, state) = state_with(
        EventRuntime::default()
            .with_limits(limits.clone())
            .with_live_poll(Duration::from_millis(20)),
    );
    let pool = state.db().cloned().expect("pool");
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state);
        let session = login(app.clone()).await;
        let address = serve(app).await;

        let mut client = expect_open(address, "", Some(&session.cookie)).await;
        // 两次写入即超出预算：保留集只剩最后一条，floor 越过实时游标 0。
        for index in 1..=2 {
            append_sized(&pool, &limits, index).await;
        }
        // 唤醒服务端循环：`stream` 每轮先等 `poll` 时长的 `recv`，在并行测试的
        // 负载下那一轮可能迟迟不返回，30 秒超时就会先到。一次 Ping 让循环立刻
        // 进入本轮，随后必定执行 gap 检查，与调度时机无关。
        client
            .send(Message::Ping(axum::body::Bytes::from_static(b"wake")))
            .await
            .expect("wake the server loop");
        // 连接必须显式关闭并带上稳定错误码，而不是静默跳过被裁掉的事件。
        let closed = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                match client.next().await {
                    Some(Ok(Message::Close(frame))) => break frame,
                    Some(Ok(Message::Text(_))) => continue,
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                    Some(Ok(_)) => continue,
                    Some(Err(error)) => panic!("websocket error: {error}"),
                    None => break None,
                }
            }
        })
        .await
        .expect("the gap must close the socket");
        let frame = closed.expect("a close frame with the stable code");
        assert_eq!(
            frame.code,
            tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Library(
                mien_server::events::ws::GAP_CLOSE_CODE
            )
        );
        assert_eq!(frame.reason, "EVENT_REPLAY_WINDOW_EXPIRED");
    });
}

#[test]
fn a_single_event_larger_than_the_budget_is_rejected() {
    let limits = EventLimits {
        retention: Duration::from_secs(7 * 24 * 60 * 60),
        max_bytes: 512,
    };
    let (_dir, state) = state_with(EventRuntime::default().with_limits(limits.clone()));
    let pool = state.db().cloned().expect("pool");
    let rt = runtime();
    rt.block_on(async move {
        let big = json!({ "profile_id": "p1", "detail": "x".repeat(1024) });
        let error = store::append(&pool, &limits, EVENT_TYPE, &big)
            .await
            .expect_err("oversized event must be rejected");
        assert!(
            matches!(error, store::StoreError::TooLarge { bytes, limit } if bytes > limit && limit == 512),
            "unexpected error: {error:?}"
        );
        // 拒绝后没有写入任何行，high_water 仍为 0。
        let snapshot = store::snapshot(&pool, &limits, None)
            .await
            .expect("snapshot");
        assert_eq!(snapshot.high_water, 0);
        assert_eq!(snapshot.floor, 0);

        // 恰好低于预算的事件可以写入。
        let fits = json!({ "profile_id": "p1", "detail": "y" });
        assert!(store::append(&pool, &limits, EVENT_TYPE, &fits).await.is_ok());
    });
}

#[test]
fn a_bearer_token_issues_a_ticket_bound_to_that_token() {
    let dir = TempDir::new().expect("temp dir");
    let pool = mien_core::storage::open_and_migrate(&dir.path().join("mien.db")).expect("migrate");
    let store = TokenStore::from_master_key(
        pool.clone(),
        &mien_core::secrets::MasterKey::from_bytes([7u8; 32]),
    );
    let auth = AuthState::default().with_bootstrap(mien_server::auth::password::Bootstrap::inline(
        ADMIN_PASSWORD,
    ));
    let state = AppState::new(dir.path(), Some(pool))
        .with_security(SecuritySettings::loopback())
        .with_auth(auth)
        .with_tokens(Some(Arc::new(store)));
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state);
        let session = login(app.clone()).await;
        let created = Call::new(Method::POST, "/api/v1/tokens")
            .cookie(&session)
            .csrf(&session)
            .origin()
            .json(json!({ "name": "events-reader", "scopes": ["events.read"] }))
            .send(app.clone())
            .await;
        assert_eq!(created.status(), StatusCode::CREATED, "create token");
        let created_body = body_json(created).await;
        let token = created_body["token"].as_str().expect("token").to_owned();
        let token_id = created_body["id"].as_str().expect("token id").to_owned();

        // Bearer（非 Cookie）也能签发票据：`events.read` scope 足够。
        let response = Call::new(Method::POST, "/api/v1/events/ticket")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .send(app.clone())
            .await;
        assert_eq!(response.status(), StatusCode::CREATED, "bearer ticket");
        let raw_ticket = body_json(response).await["ticket"]
            .as_str()
            .expect("ticket")
            .to_owned();

        let address = serve(app.clone()).await;
        let client = expect_open(address, &format!("ticket={raw_ticket}"), None).await;
        drop(client);

        // 撤销 Token 后，未使用的票据随绑定对象一起失效。
        let issued = Call::new(Method::POST, "/api/v1/events/ticket")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .send(app.clone())
            .await;
        assert_eq!(issued.status(), StatusCode::CREATED);
        let unused = body_json(issued).await["ticket"]
            .as_str()
            .expect("ticket")
            .to_owned();
        // 撤销需要 `tokens.manage`：用完整 scope 的会话完成，再验证票据已失效。
        let revoke = Call::new(Method::DELETE, format!("/api/v1/tokens/{token_id}"))
            .cookie(&session)
            .csrf(&session)
            .origin()
            .send(app)
            .await;
        assert_eq!(revoke.status(), StatusCode::NO_CONTENT, "revoke token");
        expect_rejected(
            address,
            &format!("ticket={unused}"),
            ORIGIN,
            None,
            StatusCode::UNAUTHORIZED,
            "EVENT_TICKET_INVALID",
        )
        .await;
    });
}

// ---------------------------------------------------------------------------
// 访问日志脱敏
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct LogSink(Arc<Mutex<Vec<u8>>>);

impl LogSink {
    fn contents(&self) -> String {
        String::from_utf8(self.0.lock().expect("log sink").clone()).expect("utf-8 logs")
    }
}

struct LogWriter(Arc<Mutex<Vec<u8>>>);

impl Write for LogWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("log sink").extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogSink {
    type Writer = LogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        LogWriter(Arc::clone(&self.0))
    }
}

#[test]
fn access_log_redacts_the_ticket_query_parameter() {
    static INIT: Once = Once::new();
    static SINK: std::sync::LazyLock<LogSink> = std::sync::LazyLock::new(LogSink::default);
    INIT.call_once(|| {
        let _ = tracing_subscriber::fmt()
            .with_writer(SINK.clone())
            .with_ansi(false)
            .with_target(false)
            .try_init();
    });

    let (_dir, state) = state_with(EventRuntime::default());
    let rt = runtime();
    rt.block_on(async move {
        let app = router(state);
        let session = login(app.clone()).await;

        // 唯一的票据样式秘密：访问日志里只能看到 redacted。
        let secret = format!("mien_t_{}", "0123456789abcdef".repeat(4));
        let response = Call::new(
            Method::GET,
            format!("/api/v1/events?ticket={secret}&after=0"),
        )
        .cookie(&session)
        .origin()
        .send(app)
        .await;
        // `oneshot` 没有 `Connection: upgrade`，提取器返回 4xx；认证与访问日志都已走完。
        assert!(
            response.status().is_client_error(),
            "expected a 4xx rejection, got {}",
            response.status()
        );
    });

    let logs = SINK.contents();
    assert!(
        logs.contains("path=/api/v1/events?ticket=redacted"),
        "access log must redact the ticket: {logs}"
    );
    assert!(
        logs.contains("after=0"),
        "non-sensitive parameters stay visible: {logs}"
    );
    assert!(
        !logs.contains(&"0123456789abcdef".repeat(4)),
        "the ticket value leaked into the access log"
    );
}

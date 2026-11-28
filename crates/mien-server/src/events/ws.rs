//! `GET /api/v1/events`：WebSocket 升级、票据/Cookie 认证、回放与实时推送。
//!
//! 契约：`REQ-API-011`、`REQ-API-006`、`docs/contracts/openapi.yaml`
//! （`eventsTicket`/`eventsWebSocket`）与 `docs/contracts/events.schema.json`。
//!
//! 顺序固定为 `401`（票据/Cookie）→ `403`（Origin）→ `422`/`409`（`after`）→ `101`，
//! 与 `REQ-API-003` 的 `401 -> 403 -> 422` 优先级一致。票据在认证阶段只校验、
//! 不消费；等 `after` 边界通过后才原子消费，因此客户端不会因一个可修正的游标错误
//! 白白损失票据，跨源请求也无法把票据有效性当作 oracle。
//!
//! 升级只接受有效会话 Cookie 或一次性票据；Bearer/长效 Token 从不参与本端点认证，
//! 出现在查询串的 `token`/`access_token` 等参数按 `EVENT_TICKET_INVALID` 拒绝。

use std::collections::BTreeMap;
use std::time::Duration;

use axum::Extension;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use mien_core::ErrorCode;
use sqlx::SqlitePool;

use super::{EventRuntime, store, ticket};
use crate::RequestId;
use crate::auth::{AuthError, csrf, session};
use crate::state::AppState;

/// 契约事件流路径。
pub const EVENTS_PATH: &str = "/api/v1/events";
/// 一次性票据的查询参数名（`ticketAuth`）。
pub const TICKET_PARAM: &str = "ticket";
/// 回放游标查询参数。
pub const AFTER_PARAM: &str = "after";
/// 实时页大小。
const LIVE_PAGE: i64 = 128;
/// 保留窗口越过实时游标时的关闭码（RFC 6455 私有段）；reason 为稳定错误码。
pub const GAP_CLOSE_CODE: u16 = 4001;

/// 查询串里出现即视为“把长效 Token 放进 URL”，按 `EVENT_TICKET_INVALID` 拒绝。
const URL_TOKEN_PARAMS: &[&str] = &[
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
];

/// 查询串的原始形态：认证只需要 `ticket` 与“是否出现 URL Token”。
struct RawQuery {
    values: Vec<(String, String)>,
    ticket: Option<String>,
    url_token: bool,
}

/// 已校验的查询参数。
#[derive(Debug)]
struct Params {
    after: Option<i64>,
}

/// `GET /api/v1/events`：全部校验通过后才把连接升级为 WebSocket。
pub async fn upgrade(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    request: axum::extract::Request,
) -> Response {
    let (mut parts, _body) = request.into_parts();

    // 1. 查询串：先只识别票据与 URL 长效 Token；语法错误留到认证之后。
    let raw = match raw_query(parts.uri.query()) {
        Ok(raw) => raw,
        Err(error) => return error.into_response(&request_id),
    };
    if raw.url_token {
        return AuthError::event_ticket_invalid(
            "Long-lived tokens are not accepted in the event stream URL.",
        )
        .into_response(&request_id);
    }

    let Some(pool) = state.db().cloned() else {
        return AuthError::internal().into_response(&request_id);
    };
    let runtime = state.events().clone();

    // 2. 凭据：有效 Cookie 单独认证；否则必须有一张有效的一次性票据。
    //    `401 -> 403 -> 422` 的状态优先级要求认证先于 Origin 判定。
    let session_valid = match session::parse_cookie(&parts.headers) {
        Some(token) => match session::find(&pool, &token).await {
            Ok(found) => found.is_some(),
            Err(error) => return error.into_response(&request_id),
        },
        None => false,
    };
    let ticket_value = raw.ticket.filter(|value| !value.is_empty());
    if !session_valid {
        let Some(raw_ticket) = ticket_value.as_deref() else {
            return AuthError::event_ticket_invalid("A one-time event ticket is required.")
                .into_response(&request_id);
        };
        match ticket::verify(&pool, raw_ticket).await {
            Ok(true) => {}
            Ok(false) => {
                return AuthError::event_ticket_invalid(
                    "The event ticket is invalid, expired or already used.",
                )
                .into_response(&request_id);
            }
            Err(_) => return AuthError::internal().into_response(&request_id),
        }
    }

    // 3. Origin：每次升级都校验，票据认证也不例外。
    if let Err(error) = csrf::verify_origin(&parts.headers, state.security().public_url.as_deref())
    {
        return error.into_response(&request_id);
    }

    // 4. 其余查询参数与游标语法：认证与来源通过后才可能是 422。
    let params = match parse_params(raw.values) {
        Ok(params) => params,
        Err(error) => return error.into_response(&request_id),
    };

    // 5. 回放快照：同一读快照内取 floor/high_water 与分页事件。
    let snapshot = match store::snapshot(&pool, runtime.limits(), params.after).await {
        Ok(snapshot) => snapshot,
        Err(error) => return store_error_response(error, &request_id),
    };

    // 6. 消费票据：到这里边界已通过，失败只可能是并发复用或刚过期。
    if !session_valid {
        let Some(raw_ticket) = ticket_value.as_deref() else {
            return AuthError::event_ticket_invalid("A one-time event ticket is required.")
                .into_response(&request_id);
        };
        match ticket::consume(&pool, raw_ticket).await {
            Ok(true) => {}
            Ok(false) => {
                return AuthError::event_ticket_invalid(
                    "The event ticket is invalid, expired or already used.",
                )
                .into_response(&request_id);
            }
            Err(_) => return AuthError::internal().into_response(&request_id),
        }
    }

    let cursor = snapshot.replay.last().map_or_else(
        || params.after.unwrap_or(snapshot.high_water),
        |event| event.sequence,
    );

    tracing::info!(
        request_id = request_id.as_str(),
        target = %crate::redacted_request_target(&parts.uri),
        floor = snapshot.floor,
        high_water = snapshot.high_water,
        replay = snapshot.replay.len(),
        "event stream accepted"
    );

    // 7. 升级：到这里已经通过 Origin、认证与游标校验。
    let upgrade =
        match <WebSocketUpgrade as axum::extract::FromRequestParts<AppState>>::from_request_parts(
            &mut parts, &state,
        )
        .await
        {
            Ok(upgrade) => upgrade,
            Err(rejection) => return rejection.into_response(),
        };
    let replay = snapshot.replay;
    upgrade.on_upgrade(move |socket| stream(socket, pool, runtime, cursor, replay))
}

/// 错误信封的稳定映射。
fn store_error_response(error: store::StoreError, request_id: &RequestId) -> Response {
    match error {
        store::StoreError::ReplayExpired { floor, high_water } => crate::error_response_with(
            StatusCode::CONFLICT,
            ErrorCode::EventReplayWindowExpired,
            "The replay cursor is older than the retained window; reload a snapshot.",
            request_id.as_str(),
            false,
            Some(serde_json::json!({ "floor": floor, "high_water": high_water })),
        ),
        store::StoreError::FutureCursor { high_water } => crate::error_response_with(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationFailed,
            "The replay cursor is ahead of the event stream.",
            request_id.as_str(),
            false,
            Some(serde_json::json!({
                "fields": { AFTER_PARAM: "must not be greater than high_water" },
                "high_water": high_water,
            })),
        ),
        store::StoreError::TooLarge { bytes, limit } => crate::error_response_with(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationFailed,
            "The event payload exceeds the retention budget.",
            request_id.as_str(),
            false,
            Some(serde_json::json!({ "bytes": bytes, "limit": limit })),
        ),
        store::StoreError::InvalidKind | store::StoreError::InvalidPayload => {
            crate::error_response_with(
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorCode::InternalUnexpected,
                "The event stream storage is inconsistent.",
                request_id.as_str(),
                false,
                None,
            )
        }
        store::StoreError::Sqlx(_) => crate::error_response_with(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::InternalUnexpected,
            "The server could not read its event storage.",
            request_id.as_str(),
            false,
            None,
        ),
    }
}

fn raw_query(query: Option<&str>) -> Result<RawQuery, AuthError> {
    let mut values = Vec::new();
    let mut ticket = None;
    let mut url_token = false;
    if let Some(query) = query {
        for pair in query.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (raw_name, raw_value) = pair.split_once('=').unwrap_or((pair, ""));
            let name = percent_decode(raw_name)
                .ok_or_else(|| AuthError::validation("query", "has an invalid escape"))?;
            let value = percent_decode(raw_value)
                .ok_or_else(|| AuthError::validation("query", "has an invalid escape"))?;
            let lowered = name.to_ascii_lowercase();
            if URL_TOKEN_PARAMS.contains(&lowered.as_str()) {
                url_token = true;
            }
            if name == TICKET_PARAM && ticket.is_none() {
                ticket = Some(value.clone());
            }
            values.push((name, value));
        }
    }
    Ok(RawQuery {
        values,
        ticket,
        url_token,
    })
}

/// 校验除票据外只允许 `after`，且 `after` 是非负十进制整数。
fn parse_params(values: Vec<(String, String)>) -> Result<Params, AuthError> {
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    for (name, value) in values {
        if name != TICKET_PARAM && name != AFTER_PARAM {
            return Err(AuthError::validation(
                &name,
                "is not a supported event stream parameter",
            ));
        }
        if seen.insert(name.clone(), value).is_some() {
            return Err(AuthError::validation(&name, "is repeated"));
        }
    }
    let after = match seen.get(AFTER_PARAM) {
        None => None,
        Some(raw) => Some(parse_after(raw)?),
    };
    Ok(Params { after })
}

/// `after` 只接受非负十进制整数；其余（负数、小数、前导 `+`、空串、超界）都是 422。
fn parse_after(raw: &str) -> Result<i64, AuthError> {
    if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(AuthError::validation(
            AFTER_PARAM,
            "must be a non-negative integer event sequence",
        ));
    }
    raw.parse::<i64>().map_err(|_| {
        AuthError::validation(AFTER_PARAM, "must be a non-negative integer event sequence")
    })
}

fn percent_decode(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                let high = hex_digit(*bytes.get(index + 1)?)?;
                let low = hex_digit(*bytes.get(index + 2)?)?;
                out.push((high << 4) | low);
                index += 3;
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            other => {
                out.push(other);
                index += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// 升级后的连接：先发回放，再按 `sequence > cursor` 持续推送，并定期心跳。
async fn stream(
    mut socket: WebSocket,
    pool: SqlitePool,
    runtime: EventRuntime,
    mut cursor: i64,
    replay: Vec<store::EventRecord>,
) {
    for event in replay {
        if socket
            .send(Message::Text(event.envelope_json().into()))
            .await
            .is_err()
        {
            return;
        }
    }
    let poll = runtime.live_poll().max(Duration::from_millis(10));
    let heartbeat = runtime.heartbeat();
    let mut last_ping = tokio::time::Instant::now();

    loop {
        if let Ok(Some(Ok(Message::Close(_))) | None) =
            tokio::time::timeout(poll, socket.recv()).await
        {
            let _ = socket.send(Message::Close(None)).await;
            return;
        }
        match store::live_page(&pool, cursor, LIVE_PAGE).await {
            Ok(page) if page.has_gap(cursor) => {
                // 保留窗口在连接期间推进到游标之后：绝不静默跳过，明确要求重载快照。
                let _ = socket
                    .send(Message::Close(Some(axum::extract::ws::CloseFrame {
                        code: GAP_CLOSE_CODE,
                        reason: "EVENT_REPLAY_WINDOW_EXPIRED".into(),
                    })))
                    .await;
                return;
            }
            Ok(page) => {
                for event in page.events {
                    cursor = event.sequence;
                    if socket
                        .send(Message::Text(event.envelope_json().into()))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            }
            Err(_) => tracing::warn!(cursor, "cannot read the event stream page"),
        }
        if last_ping.elapsed() >= heartbeat {
            if socket
                .send(Message::Ping(axum::body::Bytes::from_static(b"mien")))
                .await
                .is_err()
            {
                return;
            }
            last_ping = tokio::time::Instant::now();
        }
    }
}

/// 日志目标脱敏：`ticket` 与任何 URL 长效 Token 参数替换为 `redacted`。
pub fn log_target(uri: &Uri) -> String {
    crate::redacted_request_target(uri)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(value: &str) -> Uri {
        value.parse().expect("uri")
    }

    #[test]
    fn log_target_redacts_tickets_and_url_tokens() {
        let target = log_target(&uri(
            "/api/v1/events?ticket=mien_t_deadbeef&after=7&access_token=secret",
        ));
        assert_eq!(
            target,
            "/api/v1/events?ticket=redacted&after=7&access_token=redacted"
        );
        assert!(!target.contains("deadbeef"));
        assert!(!target.contains("secret"));
        assert_eq!(
            log_target(&uri("/api/v1/events?after=3")),
            "/api/v1/events?after=3"
        );
        assert_eq!(log_target(&uri("/api/v1/events")), "/api/v1/events");
    }

    #[test]
    fn after_accepts_only_non_negative_integers() {
        assert_eq!(parse_after("0").expect("zero"), 0);
        assert_eq!(parse_after("42").expect("sequence"), 42);
        assert_eq!(parse_after("007").expect("leading zeros"), 7);
        for invalid in ["-1", "1.5", "abc", "", "+1", "1_0", " 1 2"] {
            let error = parse_after(invalid).expect_err(invalid);
            assert_eq!(error.code, ErrorCode::ValidationFailed);
            assert_eq!(error.status, StatusCode::UNPROCESSABLE_ENTITY);
        }
        assert_eq!(
            parse_after("9223372036854775808")
                .expect_err("overflow")
                .code,
            ErrorCode::ValidationFailed
        );
    }

    #[test]
    fn url_tokens_are_detected_and_unknown_parameters_rejected() {
        let raw = raw_query(Some("ticket=abc&after=1")).expect("params");
        assert_eq!(raw.ticket.as_deref(), Some("abc"));
        assert!(!raw.url_token);
        assert_eq!(parse_params(raw.values).expect("params").after, Some(1));

        assert!(raw_query(Some("token=abc")).expect("token param").url_token);
        assert!(
            raw_query(Some("ACCESS_TOKEN=abc"))
                .expect("uppercase token param")
                .url_token
        );
        let raw = raw_query(Some("limit=10")).expect("raw");
        let error = parse_params(raw.values).expect_err("unknown parameter");
        assert_eq!(error.code, ErrorCode::ValidationFailed);
        assert_eq!(
            parse_params(raw_query(Some("after=1&after=2")).expect("raw").values)
                .expect_err("repeated")
                .code,
            ErrorCode::ValidationFailed
        );
        assert!(
            parse_params(raw_query(None).expect("no query").values)
                .expect("no query")
                .after
                .is_none()
        );
    }
}

//! 事件频道：一次性票据、WebSocket 升级与 `event_log` 回放（`REQ-API-011`）。
//!
//! 契约：`docs/specs/control-api/SPEC.md` `REQ-API-011`/`REQ-API-006`、
//! `docs/contracts/openapi.yaml`（`eventsTicket`/`eventsWebSocket`）、
//! `docs/contracts/events.schema.json` 与 `docs/DATABASE-SCHEMA.md` §2 基线 DDL。
//!
//! 认证模型：升级只接受一次性票据（`?ticket=`）或有效会话 Cookie；长效 Token
//! 无论放在查询串还是请求体都不被接受。每次升级（包括票据认证）都校验 `Origin`。
//! 回放游标 `after` 是整数 `event.sequence`，保留窗口 7 天且 `payload_json` 至多
//! 100 MiB；`floor`/`high_water` 与分页读取来自同一个 SQLite 读快照。

pub mod store;
pub mod ticket;
pub mod ws;

use std::time::Duration;

use axum::Extension;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};

use crate::RequestId;
use crate::auth::{AuthError, CurrentSession, Principal};
use crate::state::AppState;
use store::EventLimits;

/// 事件频道运行时参数；测试注入更小的窗口，生产使用契约默认值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRuntime {
    limits: EventLimits,
    ticket_ttl: Duration,
    heartbeat: Duration,
    live_poll: Duration,
}

/// 默认心跳间隔：用于探活并顺带推进实时游标。
pub const DEFAULT_HEARTBEAT: Duration = Duration::from_secs(30);
/// 默认实时轮询间隔；`event_log` 是唯一事实来源，轮询保证不丢衔接事件。
pub const DEFAULT_LIVE_POLL: Duration = Duration::from_millis(250);

impl Default for EventRuntime {
    fn default() -> Self {
        Self {
            limits: EventLimits::default(),
            ticket_ttl: ticket::TICKET_TTL,
            heartbeat: DEFAULT_HEARTBEAT,
            live_poll: DEFAULT_LIVE_POLL,
        }
    }
}

impl EventRuntime {
    /// 保留策略。
    pub fn limits(&self) -> &EventLimits {
        &self.limits
    }

    /// 票据有效期。
    pub fn ticket_ttl(&self) -> Duration {
        self.ticket_ttl
    }

    /// 心跳间隔。
    pub fn heartbeat(&self) -> Duration {
        self.heartbeat
    }

    /// 实时轮询间隔。
    pub fn live_poll(&self) -> Duration {
        self.live_poll
    }

    /// 替换保留策略（验收与测试构造超限时使用）。
    pub fn with_limits(mut self, limits: EventLimits) -> Self {
        self.limits = limits;
        self
    }

    /// 替换票据有效期。
    pub fn with_ticket_ttl(mut self, ttl: Duration) -> Self {
        self.ticket_ttl = ttl;
        self
    }

    /// 替换实时轮询间隔。
    pub fn with_live_poll(mut self, poll: Duration) -> Self {
        self.live_poll = poll;
        self
    }
}

/// `POST /api/v1/events/ticket` 与 `GET /api/v1/events`。
pub fn routes() -> axum::Router<AppState> {
    axum::Router::new()
        .route("/api/v1/events/ticket", post(issue_ticket))
        .route(ws::EVENTS_PATH, get(ws::upgrade))
}

/// 票据响应体（`docs/contracts/openapi.yaml#/components/schemas/EventTicket`）。
#[derive(Debug, serde::Serialize)]
struct TicketBody {
    ticket: String,
    expires_at: String,
}

/// `POST /api/v1/events/ticket`：签发一次性票据，明文只在本次响应中出现。
///
/// scope 由认证门禁按 `x-required-scope: events.read` 检查；这里再断言一次，
/// 因为 handler 也可能被直接挂载（测试）而绕过门禁。
pub async fn issue_ticket(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Extension(principal): Extension<Principal>,
    current: Option<Extension<CurrentSession>>,
) -> Response {
    if let Err(error) = principal.require_scope("events.read") {
        return error.into_response(&request_id);
    }
    let Some(pool) = state.db() else {
        return AuthError::internal().into_response(&request_id);
    };
    // Cookie 会话绑定 session_id；Bearer 绑定 token_id（subject 形如 `api_token:<id>`）。
    let binding = match current.as_ref() {
        Some(current) => ticket::Binding::Session(current.0.id.clone()),
        None => match principal.subject.strip_prefix("api_token:") {
            Some(id) if !id.is_empty() => ticket::Binding::Token(id.to_owned()),
            _ => return AuthError::internal().into_response(&request_id),
        },
    };
    match ticket::issue(pool, state.events().ticket_ttl(), &binding).await {
        Ok(issued) => {
            let mut response = (
                StatusCode::CREATED,
                axum::Json(TicketBody {
                    ticket: issued.ticket,
                    expires_at: issued.expires_at,
                }),
            )
                .into_response();
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, "no-store".parse().expect("static"));
            response
        }
        Err(error) => {
            tracing::warn!(
                request_id = request_id.as_str(),
                error = %error,
                "cannot issue the event ticket"
            );
            AuthError::internal().into_response(&request_id)
        }
    }
}

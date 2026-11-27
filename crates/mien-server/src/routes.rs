//! `/api/v1` 路由装配。
//!
//! 认证门禁由 [`crate::router`] 统一安装；本模块只声明端点与处理函数，不重复
//! 认证逻辑。公开白名单由 [`crate::auth::enforce`] 依据方法与路径判定。

use axum::Router;
use axum::routing::{delete, get, post};

use crate::auth;
use crate::state::AppState;

/// 认证端点：`login` 公开，其余仅 Cookie 会话可用。
pub fn auth_routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/auth/login", post(auth::login))
        .route("/api/v1/auth/logout", post(auth::logout))
        .route("/api/v1/auth/session", get(auth::session_info))
        .route("/api/v1/auth/password", post(auth::change_password))
}

/// Token 端点：全部要求 `tokens.manage`（由认证门禁统一检查）。
pub fn token_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/tokens",
            get(crate::tokens::list).post(crate::tokens::create),
        )
        .route("/api/v1/tokens/{token_id}", delete(crate::tokens::revoke))
}

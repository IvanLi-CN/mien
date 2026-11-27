//! API 版本发现（`REQ-API-001`）。
//!
//! 响应字段与 `docs/contracts/openapi.yaml#/paths/~1api~1v1/get` 一致；
//! `supported_versions` 让客户端在未知版本路径得到 404 之前先能发现可用版本。

use axum::Json;
use serde::Serialize;

/// 当前唯一的 API 版本。
pub const CURRENT_VERSION: &str = "v1";

/// 已弃用版本的保留天数；契约下限为 90。
pub const DEPRECATION_WINDOW_DAYS: u32 = 90;

/// 全部受支持的版本，最旧在前。
pub const SUPPORTED_VERSIONS: &[&str] = &[CURRENT_VERSION];

/// `GET /api/v1` 响应。
#[derive(Debug, Serialize)]
pub struct Discovery {
    pub version: &'static str,
    pub deprecation_window_days: u32,
    pub supported_versions: &'static [&'static str],
}

/// `GET /api/v1`：版本与弃用窗口，公开端点。
pub async fn discover() -> Json<Discovery> {
    Json(Discovery {
        version: CURRENT_VERSION,
        deprecation_window_days: DEPRECATION_WINDOW_DAYS,
        supported_versions: SUPPORTED_VERSIONS,
    })
}

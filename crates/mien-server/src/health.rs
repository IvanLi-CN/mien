//! 存活与就绪检查（`REQ-API-012`）。
//!
//! 存活只表示进程能响应；就绪检查数据库可查询与内核清单可加载，纯 headless
//! 部署不要求显示环境。响应只含状态与稳定错误码，不含路径、URL 或其他配置。

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::state::AppState;

/// `GET /healthz`：不查任何依赖。
pub async fn healthz() -> Json<Health> {
    Json(Health { status: "ok" })
}

/// 存活响应。
#[derive(Debug, Serialize)]
pub struct Health {
    pub status: &'static str,
}

/// `GET /readyz`：依赖可用 200，否则 503。
pub async fn readyz(State(state): State<AppState>) -> Response {
    let mut checks = BTreeMap::new();
    checks.insert("database", database_check(&state).await);
    checks.insert(
        "kernel",
        match AppState::kernel() {
            Ok(_) => Check::ok(),
            Err(code) => Check::failed(code),
        },
    );

    let ready = checks.values().all(|check| check.ok);
    let body = Readiness {
        status: if ready { "ok" } else { "unavailable" },
        checks,
    };
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(body)).into_response()
}

/// 就绪响应，与 `docs/contracts/openapi.yaml#/components/schemas/Readiness` 一致。
#[derive(Debug, Serialize)]
pub struct Readiness {
    pub status: &'static str,
    pub checks: BTreeMap<&'static str, Check>,
}

/// 单项检查结果。
#[derive(Debug, Serialize)]
pub struct Check {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<&'static str>,
}

impl Check {
    const fn ok() -> Self {
        Self {
            ok: true,
            detail: None,
        }
    }

    const fn failed(detail: &'static str) -> Self {
        Self {
            ok: false,
            detail: Some(detail),
        }
    }
}

/// 数据库就绪：进程持有一个池、数据库文件仍在、且能执行查询。
///
/// 文件存在性单独检查：SQLite 对已 unlink 的文件仍能查询（句柄继续有效），
/// 只靠查询会把“数据根失联”误报为就绪。
async fn database_check(state: &AppState) -> Check {
    let Some(pool) = state.db() else {
        return Check::failed("database is not open");
    };
    if !state.db_path().exists() {
        return Check::failed("database file is missing");
    }
    match sqlx::query_scalar::<_, i64>("SELECT 1")
        .fetch_one(pool)
        .await
    {
        Ok(1) => Check::ok(),
        Ok(_) => Check::failed("unexpected database reply"),
        Err(error) => Check::failed(stability_code(&error)),
    }
}

/// Map a query failure to its catalogued code without leaking driver text.
fn stability_code(error: &sqlx::Error) -> &'static str {
    match error {
        sqlx::Error::Database(database) if database.code().as_deref() == Some("5") => {
            mien_core::ErrorCode::StorageBusy.as_str()
        }
        _ => mien_core::ErrorCode::InternalUnexpected.as_str(),
    }
}

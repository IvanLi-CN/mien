//! 档案与运行时传输对象。
//!
//! 形状以 `docs/contracts/openapi.yaml#/components/schemas/{Profile,Runtime}` 为准：
//! Profile 的 `isolation` 只取 `runtimes.last_known_isolation`，Runtime 取本次
//! `runtimes.isolation` 或进程内快照；两者都返回 `direct` 与推导出的
//! `reduced_assurance`（`REQ-API-021`）。

use mien_core::domain::profile::{Profile, ProfileId, ProfileMode};
use mien_core::runtime::{Isolation, RuntimeSnapshot, RuntimeState};
use serde::Serialize;
use sqlx::{Row, SqlitePool};

use super::ApiError;

/// 档案分页结果（`ProfilePage`）。
#[derive(Debug, Clone, Serialize)]
pub struct ProfilePage {
    pub items: Vec<ProfileView>,
    pub next_cursor: Option<String>,
}

/// 档案响应：核心档案字段 + 运行时派生证据。
#[derive(Debug, Clone, Serialize)]
pub struct ProfileView {
    #[serde(flatten)]
    pub profile: Profile,
    pub group_id: Option<String>,
    pub tags: Vec<String>,
    pub isolation: Isolation,
    pub direct: bool,
    pub reduced_assurance: bool,
    pub limitations: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<RuntimeView>,
}

/// 运行时响应。
#[derive(Debug, Clone, Serialize)]
pub struct RuntimeView {
    pub id: String,
    pub profile_id: String,
    pub state: RuntimeState,
    pub mode: ProfileMode,
    pub isolation: Isolation,
    pub direct: bool,
    pub reduced_assurance: bool,
    pub ua_scope: Option<String>,
    pub exit_code: Option<i32>,
    pub proxy_port: Option<u16>,
    pub cdp_url: Option<String>,
    pub started_at: Option<String>,
    pub error_code: Option<String>,
    pub limitations: Vec<String>,
}

impl RuntimeView {
    /// 本次启动的运行时快照；`isolation` 是本次探测值。
    pub fn from_snapshot(snapshot: &RuntimeSnapshot, limitations: Vec<String>) -> Self {
        Self {
            id: snapshot.runtime_id.clone(),
            profile_id: snapshot.profile_id.clone(),
            state: snapshot.state,
            mode: snapshot.mode,
            isolation: snapshot.isolation,
            direct: snapshot.direct,
            reduced_assurance: snapshot.reduced_assurance,
            ua_scope: snapshot.ua_scope.clone(),
            exit_code: snapshot.exit_code,
            proxy_port: snapshot.proxy_port,
            cdp_url: snapshot.cdp_port.map(cdp_url),
            started_at: None,
            error_code: snapshot.error_code.clone(),
            limitations,
        }
    }

    /// 持久化行；进程内没有受监督记录时（例如服务端重启后）作为唯一来源。
    pub fn from_row(row: &RuntimeRow, direct: bool, limitations: Vec<String>) -> Self {
        Self {
            id: row.id.clone(),
            profile_id: row.profile_id.clone(),
            state: row.state,
            mode: row.mode,
            isolation: row.isolation,
            direct,
            reduced_assurance: mien_core::runtime::state::reduced_assurance(
                row.mode,
                direct,
                row.isolation,
            ),
            ua_scope: (row.mode == ProfileMode::PseudoHeadful).then(|| "chrome".to_owned()),
            exit_code: row.exit_code,
            proxy_port: row.proxy_port,
            cdp_url: row.cdp_port.map(cdp_url),
            started_at: row.started_at.clone(),
            error_code: row.last_error.clone(),
            limitations,
        }
    }

    /// 没有任何运行时证据时的 `stopped` 视图；不虚报隔离结论。
    pub fn never_started(profile: &Profile) -> Self {
        let direct = profile.proxy_id.is_none();
        Self {
            id: runtime_id(&profile.id),
            profile_id: profile.id.0.clone(),
            state: RuntimeState::Stopped,
            mode: profile.mode,
            isolation: Isolation::Unknown,
            direct,
            reduced_assurance: mien_core::runtime::state::reduced_assurance(
                profile.mode,
                direct,
                Isolation::Unknown,
            ),
            ua_scope: (profile.mode == ProfileMode::PseudoHeadful).then(|| "chrome".to_owned()),
            exit_code: None,
            proxy_port: None,
            cdp_url: None,
            started_at: None,
            error_code: None,
            limitations: Vec::new(),
        }
    }
}

/// 运行时标识；与监督器使用的 `rt-<profile_id>` 一致。
pub fn runtime_id(profile_id: &ProfileId) -> String {
    format!("rt-{}", profile_id.0)
}

fn cdp_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

/// `runtimes` 行。
#[derive(Debug, Clone)]
pub struct RuntimeRow {
    pub id: String,
    pub profile_id: String,
    pub state: RuntimeState,
    pub mode: ProfileMode,
    pub isolation: Isolation,
    pub last_known_isolation: Isolation,
    pub exit_code: Option<i32>,
    pub proxy_port: Option<u16>,
    pub cdp_port: Option<u16>,
    pub started_at: Option<String>,
    pub last_error: Option<String>,
}

impl RuntimeRow {
    fn from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Self, ApiError> {
        Ok(Self {
            id: row.try_get("id").map_err(|_| integrity())?,
            profile_id: row.try_get("profile_id").map_err(|_| integrity())?,
            state: parse_state(&row.try_get::<String, _>("state").map_err(|_| integrity())?)?,
            mode: parse_mode(&row.try_get::<String, _>("mode").map_err(|_| integrity())?)?,
            isolation: parse_isolation(
                &row.try_get::<String, _>("isolation")
                    .map_err(|_| integrity())?,
            )?,
            last_known_isolation: parse_isolation(
                &row.try_get::<String, _>("last_known_isolation")
                    .map_err(|_| integrity())?,
            )?,
            exit_code: row
                .try_get::<Option<i64>, _>("exit_code")
                .map_err(|_| integrity())?
                .map(|value| value as i32),
            proxy_port: row
                .try_get::<Option<i64>, _>("proxy_port")
                .map_err(|_| integrity())?
                .map(|value| value as u16),
            cdp_port: row
                .try_get::<Option<i64>, _>("cdp_port")
                .map_err(|_| integrity())?
                .map(|value| value as u16),
            started_at: row.try_get("started_at").map_err(|_| integrity())?,
            last_error: row.try_get("last_error").map_err(|_| integrity())?,
        })
    }
}

fn integrity() -> ApiError {
    ApiError::from_core(mien_core::CoreError::StorageIntegrityFailed)
}

/// 读取档案的运行时行。
///
/// # Errors
/// 存储不可读或枚举值越界。
pub async fn runtime_row(
    pool: &SqlitePool,
    profile_id: &str,
) -> Result<Option<RuntimeRow>, ApiError> {
    let row = sqlx::query(
        "SELECT id,profile_id,state,mode,isolation,last_known_isolation,exit_code,proxy_port,cdp_port,started_at,last_error FROM runtimes WHERE profile_id=?1",
    )
    .bind(profile_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| ApiError::storage(&error))?;
    row.as_ref().map(RuntimeRow::from_row).transpose()
}

/// Profile/proof 的唯一隔离来源：最近一次成功进入 `running` 的结论。
///
/// # Errors
/// 存储不可读或枚举值越界。
pub async fn last_known_isolation(
    pool: &SqlitePool,
    profile_id: &str,
) -> Result<Isolation, ApiError> {
    let value: Option<String> =
        sqlx::query_scalar("SELECT last_known_isolation FROM runtimes WHERE profile_id=?1")
            .bind(profile_id)
            .fetch_optional(pool)
            .await
            .map_err(|error| ApiError::storage(&error))?;
    match value {
        Some(value) => parse_isolation(&value),
        None => Ok(Isolation::Unknown),
    }
}

/// 档案显式绑定的分组；分组 CRUD 不在 T24 范围，读取原值。
///
/// # Errors
/// 存储不可读。
pub async fn group_id(pool: &SqlitePool, profile_id: &str) -> Result<Option<String>, ApiError> {
    let value: Option<Option<String>> =
        sqlx::query_scalar("SELECT group_id FROM profiles WHERE id=?1")
            .bind(profile_id)
            .fetch_optional(pool)
            .await
            .map_err(|error| ApiError::storage(&error))?;
    Ok(value.flatten())
}

/// 档案标签名（升序）。
///
/// # Errors
/// 存储不可读。
pub async fn tags(pool: &SqlitePool, profile_id: &str) -> Result<Vec<String>, ApiError> {
    sqlx::query_scalar(
        "SELECT t.name FROM tags t JOIN profile_tags pt ON pt.tag_id=t.id WHERE pt.profile_id=?1 ORDER BY t.name",
    )
    .bind(profile_id)
    .fetch_all(pool)
    .await
    .map_err(|error| ApiError::storage(&error))
}

/// 组装一个档案视图。
///
/// # Errors
/// 存储不可读或运行时行损坏。
pub async fn profile_view(
    pool: &SqlitePool,
    profile: Profile,
    live: Option<RuntimeSnapshot>,
    limitations: Vec<String>,
) -> Result<ProfileView, ApiError> {
    let id = profile.id.0.clone();
    let isolation = last_known_isolation(pool, &id).await?;
    let direct = profile.proxy_id.is_none();
    let runtime = match live {
        Some(snapshot) => Some(RuntimeView::from_snapshot(&snapshot, limitations.clone())),
        None => runtime_row(pool, &id)
            .await?
            .map(|row| RuntimeView::from_row(&row, direct, limitations.clone())),
    };
    Ok(ProfileView {
        reduced_assurance: mien_core::runtime::state::reduced_assurance(
            profile.mode,
            direct,
            isolation,
        ),
        isolation,
        direct,
        group_id: group_id(pool, &id).await?,
        tags: tags(pool, &id).await?,
        limitations,
        runtime,
        profile,
    })
}

fn parse_state(value: &str) -> Result<RuntimeState, ApiError> {
    match value {
        "starting" => Ok(RuntimeState::Starting),
        "running" => Ok(RuntimeState::Running),
        "stopping" => Ok(RuntimeState::Stopping),
        "stopped" => Ok(RuntimeState::Stopped),
        "crashed" => Ok(RuntimeState::Crashed),
        "failed" => Ok(RuntimeState::Failed),
        _ => Err(integrity()),
    }
}

fn parse_mode(value: &str) -> Result<ProfileMode, ApiError> {
    match value {
        "headful" => Ok(ProfileMode::Headful),
        "headless" => Ok(ProfileMode::Headless),
        "pseudo_headful" => Ok(ProfileMode::PseudoHeadful),
        _ => Err(integrity()),
    }
}

fn parse_isolation(value: &str) -> Result<Isolation, ApiError> {
    match value {
        "enforced" => Ok(Isolation::Enforced),
        "unsandboxed" => Ok(Isolation::Unsandboxed),
        "unknown" => Ok(Isolation::Unknown),
        _ => Err(integrity()),
    }
}

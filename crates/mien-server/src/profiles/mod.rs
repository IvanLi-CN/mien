//! 档案与运行时 REST 端点（T24）。
//!
//! 契约：`docs/contracts/openapi.yaml` 的 `profiles*` 操作与
//! `docs/specs/{control-api,profile-management,browser-runtime}/SPEC.md`。
//! 领域逻辑全部来自 `mien_core`：CRUD/生命周期走 [`SqliteProfileRepository`]，
//! 启动走 [`StartProfile`] + [`RuntimeSupervisor`]，代理凭据只经
//! [`MasterKey`] + [`ProxyRepository::load_secret`] 读取。
//!
//! 本模块只承担传输层职责：DTO 形状、游标分页、幂等键、HTTP 状态映射与
//! `REQ-API-021` 的派生字段。`401 -> 403 -> 404 -> 409` 的顺序由认证中间件与
//! 各处理函数里「先授权、再查存在性」的固定次序保证。

pub mod idempotency;
pub mod routes;
pub mod view;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::body::Bytes;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use mien_core::CoreError;
use mien_core::domain::profile::{
    CopyOptions, CreateProfile, Profile, ProfileId, ProfileMode, ProfilePatch, ProfileStatus,
};
use mien_core::error::ErrorCode;
use mien_core::fingerprint::spec::{
    FingerprintSpec, Network, Platform, Spoof, Webrtc, Window, generate_seed,
};
use mien_core::fingerprint::{CapabilityManifest, validate_spec};
use mien_core::ports::proxy::{ProxyId, ProxyInput, ProxyRepository};
use mien_core::ports::repository::ProfileRepository;
use mien_core::proxy::api::GrpcXrayApi;
use mien_core::proxy::attach::GatewayAttach;
use mien_core::proxy::engine::{EngineBinary, EngineController, EngineProcess, EngineState};
use mien_core::proxy::keys::{
    Credentials, MuxSettings, OutboundSpec, ProxyConfig, ProxyProtocol, TlsSettings, Transport,
    build_outbound,
};
use mien_core::runtime::{ProxyAttach, RestartPolicy, RuntimeSnapshot, RuntimeState, StartProfile};
use mien_core::secrets::MasterKey;
use mien_core::storage::proxy::SqliteProxyRepository;
use mien_core::storage::sqlite::SqliteProfileRepository;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Row, SqlitePool};

use crate::RequestId;
use crate::state::{AppState, XRAY_BIN_ENV};

/// 列表页大小上限（`docs/contracts/openapi.yaml#/components/parameters/Limit`）。
pub const MAX_LIMIT: u32 = 200;
/// 默认页大小。
pub const DEFAULT_LIMIT: u32 = 50;
/// 请求体上限；档案与复制输入远小于此值。
pub const MAX_BODY_BYTES: usize = 256 * 1024;

// ---------------------------------------------------------------------------
// 错误信封
// ---------------------------------------------------------------------------

/// 传输层错误：稳定错误码 + HTTP 状态 + 可重试标记。
#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
    pub details: Option<Value>,
}

impl ApiError {
    pub fn new(status: StatusCode, code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            retryable: false,
            details: None,
        }
    }

    /// `422 VALIDATION_FAILED`；`details.fields` 给出字段级原因。
    pub fn validation(field: &str, reason: impl Into<String>) -> Self {
        let mut error = Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationFailed,
            "The request is not valid.",
        );
        error.details = Some(serde_json::json!({ "fields": { field: reason.into() } }));
        error
    }

    pub fn not_found() -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            ErrorCode::ProfileNotFound,
            "No such profile.",
        )
    }

    pub fn profile_deleted() -> Self {
        Self::new(
            StatusCode::CONFLICT,
            ErrorCode::ProfileDeleted,
            "The profile is soft-deleted; restore it first.",
        )
    }

    pub fn idempotency_conflict() -> Self {
        Self::new(
            StatusCode::CONFLICT,
            ErrorCode::IdempotencyConflict,
            "The Idempotency-Key was already used for a different request.",
        )
    }

    pub fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::InternalUnexpected,
            "The server could not complete the request.",
        )
    }

    /// SQLx 读取错误；只暴露稳定错误码。
    pub fn storage(error: &sqlx::Error) -> Self {
        match error {
            sqlx::Error::Database(database)
                if matches!(database.code().as_deref(), Some("5") | Some("6")) =>
            {
                let mut error = Self::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    ErrorCode::StorageBusy,
                    "The database is busy; retry the request.",
                );
                error.retryable = true;
                error
            }
            _ => Self::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorCode::InternalUnexpected,
                "The server could not read its storage.",
            ),
        }
    }

    /// 领域错误到 HTTP 的稳定映射（`docs/ERROR-CODES.md`）。
    pub fn from_core(error: CoreError) -> Self {
        let code = error.code();
        let message = match &error {
            CoreError::Validation { message, .. } if !message.is_empty() => message.clone(),
            _ => core_message(code).to_owned(),
        };
        Self {
            status: status_for(code),
            code,
            message,
            retryable: retryable(code),
            details: None,
        }
    }

    pub fn into_response(self, request_id: &RequestId) -> Response {
        // `PROFILE_PURGE_FAILED` 契约要求顶层 `deletion_error`（PurgeFailure），
        // 不是嵌套在 `details` 里；其它错误码保持通用信封。
        if self.code == ErrorCode::ProfilePurgeFailed {
            if let Some(deletion_error) = self
                .details
                .as_ref()
                .and_then(|details| details.get("deletion_error"))
                .and_then(Value::as_str)
            {
                return response_with_deletion_error(
                    self.status,
                    self.code,
                    &self.message,
                    request_id,
                    true,
                    deletion_error,
                );
            }
        }
        crate::error_response_with(
            self.status,
            self.code,
            &self.message,
            request_id.as_str(),
            self.retryable,
            self.details,
        )
    }
}

/// `PROFILE_PURGE_FAILED` 的响应体：通用 Error 字段 + 顶层 `deletion_error`。
fn response_with_deletion_error(
    status: StatusCode,
    code: ErrorCode,
    message: &str,
    request_id: &RequestId,
    retryable: bool,
    deletion_error: &str,
) -> Response {
    let body = serde_json::json!({
        "code": code.as_str(),
        "message": message,
        "request_id": request_id.as_str(),
        "requestId": request_id.as_str(),
        "retryable": retryable,
        "deletion_error": deletion_error,
    });
    let mut response = (status, Json(body)).into_response();
    if let Ok(value) = axum::http::HeaderValue::from_str(request_id.as_str()) {
        response.headers_mut().insert(
            axum::http::HeaderName::from_static(crate::REQUEST_ID_HEADER),
            value,
        );
    }
    response
}

fn status_for(code: ErrorCode) -> StatusCode {
    match code {
        ErrorCode::ProfileNotFound
        | ErrorCode::ProxyNotFound
        | ErrorCode::GroupNotFound
        | ErrorCode::TagNotFound
        | ErrorCode::RuntimeNotFound
        | ErrorCode::TokenNotFound
        | ErrorCode::KernelVersionNotFound
        | ErrorCode::ProxyEngineVersionNotFound
        | ErrorCode::ViewerNotAvailable => StatusCode::NOT_FOUND,
        ErrorCode::ProfileDeleted
        | ErrorCode::ProfileNotDeleted
        | ErrorCode::ProfileArchived
        | ErrorCode::RuntimeConflict
        | ErrorCode::IdempotencyConflict
        | ErrorCode::ResourceInUse
        | ErrorCode::GroupNameConflict
        | ErrorCode::TagNameConflict
        | ErrorCode::ProfileImportConflict
        | ErrorCode::ProfileRestoreUnavailable
        | ErrorCode::ProxyInboundConflict => StatusCode::CONFLICT,
        ErrorCode::DirectEgressDisabled
        | ErrorCode::ForbiddenScope
        | ErrorCode::AuthCsrfInvalid
        | ErrorCode::AuthOriginInvalid => StatusCode::FORBIDDEN,
        ErrorCode::AuthRequired | ErrorCode::AuthInvalid | ErrorCode::EventTicketInvalid => {
            StatusCode::UNAUTHORIZED
        }
        ErrorCode::AuthRateLimited | ErrorCode::QuotaExceeded => StatusCode::TOO_MANY_REQUESTS,
        ErrorCode::ValidationFailed
        | ErrorCode::FingerprintInvalid
        | ErrorCode::ProxyConfigInvalid
        | ErrorCode::ShareLinkInvalid
        | ErrorCode::RuntimeModeUnsupported
        | ErrorCode::KernelUnsupported
        | ErrorCode::UnsupportedProtocol
        | ErrorCode::TuicNotSupported => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::StorageBusy
        | ErrorCode::ProxyEngineNotReady
        | ErrorCode::ProxyEngineApiUnavailable
        | ErrorCode::ProxyEngineCrashed
        | ErrorCode::InsufficientResources
        | ErrorCode::DisplayUnavailable => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn retryable(code: ErrorCode) -> bool {
    matches!(
        code,
        ErrorCode::StorageBusy
            | ErrorCode::ProfilePurgeFailed
            | ErrorCode::ProfileImportCleanupFailed
            | ErrorCode::RuntimeCdpTimeout
            | ErrorCode::ProxyEngineNotReady
            | ErrorCode::ProxyEngineApiUnavailable
            | ErrorCode::ProxyEngineCrashed
            | ErrorCode::ProxyInboundConflict
            | ErrorCode::InsufficientResources
            | ErrorCode::DisplayUnavailable
            | ErrorCode::QuotaExceeded
    )
}

/// 错误码的稳定人读消息；不含路径、凭据或驱动文本。
fn core_message(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::ProfileDeleted => "The profile is soft-deleted; restore it first.",
        ErrorCode::ProfileNotDeleted => "Only a soft-deleted profile can be purged.",
        ErrorCode::ProfileArchived => "The profile is archived; restore it first.",
        ErrorCode::ProfilePurgeFailed => "The profile directory could not be moved or removed.",
        ErrorCode::ProfileRestoreUnavailable => "The profile directory cannot be restored.",
        ErrorCode::RuntimeConflict => "A runtime is already active for this profile.",
        ErrorCode::ProxyEngineNotReady => "The shared proxy engine is not ready.",
        ErrorCode::ProxyEngineStartFailed => "The shared proxy engine could not start.",
        ErrorCode::KernelNotFound => "The kernel executable is missing.",
        ErrorCode::KernelChecksumMismatch => {
            "The kernel executable digest does not match the install record."
        }
        ErrorCode::StorageBusy => "The database is busy; retry the request.",
        ErrorCode::StorageIntegrityFailed => "Stored data failed its integrity check.",
        ErrorCode::SandboxUnavailable => "The browser sandbox could not be confirmed.",
        ErrorCode::RuntimeSpawnFailed => "The kernel process could not be started.",
        ErrorCode::RuntimeCdpTimeout => "The kernel did not expose CDP in time.",
        ErrorCode::DirectEgressDisabled => "Direct egress is disabled on this server.",
        ErrorCode::ValidationFailed => "The request is not valid.",
        _ => "The server could not complete the request.",
    }
}

// ---------------------------------------------------------------------------
// 输入 DTO
// ---------------------------------------------------------------------------

/// `ProfileInput`（`docs/contracts/openapi.yaml#/components/schemas/ProfileInput`）。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileInput {
    pub name: String,
    #[serde(default)]
    pub group_id: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub notes: Option<String>,
    pub fingerprint: FingerprintInput,
    #[serde(default)]
    pub proxy_id: Option<String>,
    pub mode: ProfileMode,
    #[serde(default)]
    pub automation_api: bool,
    #[serde(default)]
    pub start_url: Option<String>,
    #[serde(default)]
    pub kernel_version: Option<String>,
}

/// `ProfilePatch`；未给出的字段不变，显式 `null` 清空可空字段。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfilePatchInput {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    pub group_id: Option<Option<String>>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub notes: Option<Option<String>>,
    #[serde(default)]
    pub fingerprint: Option<FingerprintInput>,
    #[serde(default, deserialize_with = "double_option")]
    pub proxy_id: Option<Option<String>>,
    #[serde(default)]
    pub mode: Option<ProfileMode>,
    #[serde(default)]
    pub automation_api: Option<bool>,
    #[serde(default, deserialize_with = "double_option")]
    pub start_url: Option<Option<String>>,
}

/// 保留「字段缺省」与「显式 null」的差别；`ProfilePatch` 需要清空可空列。
fn double_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// 指纹输入（`docs/contracts/fingerprint-input.schema.json`）：不含 seed。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FingerprintInput {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub platform: Platform,
    pub platform_version: String,
    pub brand: String,
    pub brand_version: String,
    pub hardware_concurrency: u32,
    pub languages: Vec<String>,
    pub timezone: String,
    pub window: Window,
    #[serde(default)]
    pub spoof: Option<Spoof>,
    pub webrtc: Webrtc,
    pub network: Network,
}

const fn default_schema_version() -> u32 {
    mien_core::fingerprint::spec::SCHEMA_VERSION
}

impl FingerprintInput {
    /// 构造持久化规格；`seed_override` 用于 PATCH 保留已存 seed。
    ///
    /// # Errors
    /// seed 生成失败或规格不在内核能力清单内。
    pub fn into_spec(self, seed_override: Option<u32>) -> Result<FingerprintSpec, CoreError> {
        let spec = FingerprintSpec {
            schema_version: self.schema_version,
            seed: seed_override.unwrap_or(generate_seed()?),
            platform: self.platform,
            platform_version: self.platform_version,
            brand: self.brand,
            brand_version: self.brand_version,
            hardware_concurrency: self.hardware_concurrency,
            languages: self.languages,
            timezone: self.timezone,
            window: self.window,
            spoof: self.spoof,
            webrtc: self.webrtc,
            network: self.network,
        };
        validate_spec(&spec)?;
        Ok(spec)
    }
}

/// `POST /api/v1/profiles/{id}/duplicate` 请求体。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DuplicateInput {
    #[serde(default)]
    pub copy_fingerprint: bool,
    #[serde(default = "default_copy_proxy")]
    pub copy_proxy: bool,
}

const fn default_copy_proxy() -> bool {
    true
}

// ---------------------------------------------------------------------------
// 列表查询与游标
// ---------------------------------------------------------------------------

/// 排序字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortField {
    Name,
    UpdatedAt,
    CreatedAt,
    Status,
}

impl SortField {
    const fn as_sql(self) -> &'static str {
        match self {
            Self::Name => "p.name",
            Self::UpdatedAt => "p.updated_at",
            Self::CreatedAt => "p.created_at",
            Self::Status => "p.status",
        }
    }
}

/// 排序方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    Asc,
    Desc,
}

/// 已校验的列表查询。
#[derive(Debug, Clone)]
pub struct ListQuery {
    pub limit: u32,
    pub q: Option<String>,
    pub group_id: Option<String>,
    pub tag: Option<String>,
    pub state: Option<RuntimeState>,
    pub status: Option<ProfileStatus>,
    pub sort: SortField,
    pub order: SortOrder,
    pub cursor: Option<Cursor>,
}

/// 游标：上一页最后一行的排序键与档案标识。
#[derive(Debug, Clone)]
pub struct Cursor {
    pub key: String,
    pub id: String,
}

const CURSOR_SEPARATOR: u8 = 0x1f;
/// 游标原文上限，防止任意超长输入参与解码。
const CURSOR_MAX_LEN: usize = 512;

/// 解析查询串；未知参数与非法枚举一律 422（`REQ-API-009`）。
///
/// # Errors
/// 未知筛选字段、非法枚举或超出上限的 `limit`。
pub fn parse_list_query(params: &BTreeMap<String, String>) -> Result<ListQuery, ApiError> {
    let mut unknown: Vec<&String> = params
        .keys()
        .filter(|key| {
            !matches!(
                key.as_str(),
                "limit"
                    | "cursor"
                    | "q"
                    | "group_id"
                    | "tag"
                    | "state"
                    | "status"
                    | "sort"
                    | "order"
            )
        })
        .collect();
    unknown.sort();
    if let Some(field) = unknown.first() {
        return Err(ApiError::validation(
            field,
            "is not a supported filter for profile listing",
        ));
    }

    let limit = match params.get("limit") {
        Some(raw) => raw
            .parse::<u32>()
            .map_err(|_| ApiError::validation("limit", "must be an integer between 1 and 200"))?,
        None => DEFAULT_LIMIT,
    };
    if !(1..=MAX_LIMIT).contains(&limit) {
        return Err(ApiError::validation(
            "limit",
            "must be an integer between 1 and 200",
        ));
    }

    let q = match params.get("q") {
        Some(value) if value.chars().count() > 200 => {
            return Err(ApiError::validation("q", "must be at most 200 characters"));
        }
        Some(value) if !value.is_empty() => Some(value.clone()),
        _ => None,
    };
    let group_id = params.get("group_id").cloned().filter(|v| !v.is_empty());
    if let Some(group_id) = &group_id {
        validate_ulid(group_id, "group_id")?;
    }
    let tag = params.get("tag").cloned().filter(|v| !v.is_empty());
    if let Some(tag) = &tag {
        if tag.chars().count() > 64 {
            return Err(ApiError::validation("tag", "must be at most 64 characters"));
        }
    }

    let state = match params.get("state") {
        Some(raw) => Some(parse_runtime_state(raw)?),
        None => None,
    };
    let status = match params.get("status") {
        Some(raw) => Some(parse_profile_status(raw)?),
        None => Some(ProfileStatus::Active),
    };
    let sort = match params.get("sort").map(String::as_str) {
        None => SortField::UpdatedAt,
        Some("name") => SortField::Name,
        Some("updated_at") => SortField::UpdatedAt,
        Some("created_at") => SortField::CreatedAt,
        Some("status") => SortField::Status,
        Some(_) => {
            return Err(ApiError::validation(
                "sort",
                "must be one of name, updated_at, created_at, status",
            ));
        }
    };
    let order = match params.get("order").map(String::as_str) {
        None => SortOrder::Desc,
        Some("asc") => SortOrder::Asc,
        Some("desc") => SortOrder::Desc,
        Some(_) => return Err(ApiError::validation("order", "must be asc or desc")),
    };
    let cursor = match params.get("cursor") {
        Some(raw) => Some(decode_cursor(raw)?),
        None => None,
    };

    Ok(ListQuery {
        limit,
        q,
        group_id,
        tag,
        state,
        status,
        sort,
        order,
        cursor,
    })
}

fn validate_ulid(value: &str, field: &str) -> Result<(), ApiError> {
    if ProfileId::from_string(value).is_valid() {
        Ok(())
    } else {
        Err(ApiError::validation(field, "must be a valid ULID"))
    }
}

fn parse_runtime_state(value: &str) -> Result<RuntimeState, ApiError> {
    match value {
        "starting" => Ok(RuntimeState::Starting),
        "running" => Ok(RuntimeState::Running),
        "stopping" => Ok(RuntimeState::Stopping),
        "stopped" => Ok(RuntimeState::Stopped),
        "crashed" => Ok(RuntimeState::Crashed),
        "failed" => Ok(RuntimeState::Failed),
        _ => Err(ApiError::validation(
            "state",
            "must be one of starting, running, stopping, stopped, crashed, failed",
        )),
    }
}

fn parse_profile_status(value: &str) -> Result<ProfileStatus, ApiError> {
    match value {
        "active" => Ok(ProfileStatus::Active),
        "archived" => Ok(ProfileStatus::Archived),
        "deleted" => Ok(ProfileStatus::Deleted),
        _ => Err(ApiError::validation(
            "status",
            "must be one of active, archived, deleted",
        )),
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() / 2);
    for pair in text.as_bytes().chunks_exact(2) {
        let high = (pair[0] as char).to_digit(16)?;
        let low = (pair[1] as char).to_digit(16)?;
        out.push(((high << 4) | low) as u8);
    }
    Some(out)
}

fn encode_cursor(key: &str, id: &str) -> String {
    let mut bytes = Vec::with_capacity(key.len() + id.len() + 1);
    bytes.extend_from_slice(key.as_bytes());
    bytes.push(CURSOR_SEPARATOR);
    bytes.extend_from_slice(id.as_bytes());
    hex_encode(&bytes)
}

fn decode_cursor(text: &str) -> Result<Cursor, ApiError> {
    if text.is_empty() || text.len() > CURSOR_MAX_LEN {
        return Err(ApiError::validation("cursor", "is not a valid page cursor"));
    }
    let bytes = hex_decode(text)
        .ok_or_else(|| ApiError::validation("cursor", "is not a valid page cursor"))?;
    let text = String::from_utf8(bytes)
        .map_err(|_| ApiError::validation("cursor", "is not a valid page cursor"))?;
    let (key, id) = text
        .split_once(CURSOR_SEPARATOR as char)
        .ok_or_else(|| ApiError::validation("cursor", "is not a valid page cursor"))?;
    validate_ulid(id, "cursor")?;
    Ok(Cursor {
        key: key.to_owned(),
        id: id.to_owned(),
    })
}

// ---------------------------------------------------------------------------
// 档案用例
// ---------------------------------------------------------------------------

/// 创建档案。
///
/// # Errors
/// 字段校验失败、代理/分组/标签不存在或存储繁忙。
pub async fn create(state: &AppState, input: ProfileInput) -> Result<view::ProfileView, ApiError> {
    let repository = repository(state)?;
    let pool = pool_for(state)?;
    let manifest = manifest()?;
    validate_name(&input.name)?;
    validate_notes(input.notes.as_deref())?;
    validate_start_url(input.start_url.as_deref())?;
    validate_tags(&input.tags)?;
    if let Some(group_id) = &input.group_id {
        ensure_group(pool, group_id).await?;
    }
    ensure_tags_exist(pool, &input.tags).await?;
    if let Some(proxy_id) = &input.proxy_id {
        ensure_proxy(pool, proxy_id).await?;
    }
    let kernel_version = match &input.kernel_version {
        Some(version) => {
            if !valid_kernel_version(version) {
                return Err(ApiError::validation(
                    "kernel_version",
                    "must be a four-part dotted version such as 148.0.7778.215",
                ));
            }
            version.clone()
        }
        None => manifest.kernel_version.clone(),
    };
    let fingerprint = input
        .fingerprint
        .into_spec(None)
        .map_err(ApiError::from_core)?;
    let profile = repository
        .create(CreateProfile {
            name: input.name,
            fingerprint,
            start_url: input.start_url,
            proxy_id: input.proxy_id,
            notes: input.notes,
            kernel_version,
            mode: input.mode,
            automation_api: input.automation_api,
        })
        .await
        .map_err(ApiError::from_core)?;
    if let Err(error) =
        write_relations(pool, &profile.id.0, input.group_id.as_deref(), &input.tags).await
    {
        // 关系写入失败时不留半成品档案；目录与行一起回收。
        let _ = repository.soft_delete(&profile.id).await;
        let _ = repository.purge(&profile.id).await;
        return Err(error);
    }
    let profile = repository
        .get(&profile.id)
        .await
        .map_err(ApiError::from_core)?;
    view::profile_view(pool, profile, None, manifest.limitations.clone()).await
}

/// 读取单个档案。
///
/// # Errors
/// 档案不存在或存储错误。
pub async fn get(state: &AppState, id: &str) -> Result<view::ProfileView, ApiError> {
    let profile = load_profile(state, id).await?;
    let live = state.supervisor().state(&view::runtime_id(&profile.id));
    view::profile_view(
        pool_for(state)?,
        profile,
        live,
        manifest()?.limitations.clone(),
    )
    .await
}

/// 分页列表（`REQ-API-009`）。
///
/// 排序键后固定用 `id` 作为决胜键，因此游标翻页无重复、无遗漏。
///
/// # Errors
/// 查询参数非法、游标非法或存储错误。
pub async fn list(state: &AppState, query: ListQuery) -> Result<view::ProfilePage, ApiError> {
    let repository = repository(state)?;
    let pool = pool_for(state)?;
    let rows = list_ids(pool, &query).await?;
    let has_more = rows.len() > query.limit as usize;
    let rows: Vec<(String, String)> = rows.into_iter().take(query.limit as usize).collect();
    let next_cursor = if has_more {
        rows.last().map(|(id, key)| encode_cursor(key, id))
    } else {
        None
    };
    let limitations = manifest()?.limitations.clone();
    let mut items = Vec::with_capacity(rows.len());
    for (id, _) in rows {
        let profile = repository
            .get(&ProfileId::from_string(id))
            .await
            .map_err(ApiError::from_core)?;
        let live = state.supervisor().state(&view::runtime_id(&profile.id));
        items.push(view::profile_view(pool, profile, live, limitations.clone()).await?);
    }
    Ok(view::ProfilePage { items, next_cursor })
}

async fn list_ids(pool: &SqlitePool, query: &ListQuery) -> Result<Vec<(String, String)>, ApiError> {
    let comparison = match query.order {
        SortOrder::Asc => ">",
        SortOrder::Desc => "<",
    };
    let direction = match query.order {
        SortOrder::Asc => "ASC",
        SortOrder::Desc => "DESC",
    };
    let sort = query.sort.as_sql();
    let state_filter = query.state.map(runtime_state_str);
    let status_filter = query.status.map(profile_status_str);
    let cursor_key = query.cursor.as_ref().map(|cursor| cursor.key.clone());
    let cursor_id = query.cursor.as_ref().map(|cursor| cursor.id.clone());

    let sql = format!(
        "SELECT p.id AS id, {sort} AS sort_key \
         FROM profiles p \
         LEFT JOIN runtimes r ON r.profile_id = p.id \
         WHERE (?1 IS NULL OR instr(p.name, ?1) > 0) \
           AND (?2 IS NULL OR p.group_id = ?2) \
           AND (?3 IS NULL OR EXISTS (SELECT 1 FROM profile_tags pt JOIN tags t ON t.id = pt.tag_id \
                                       WHERE pt.profile_id = p.id AND t.name = ?3)) \
           AND (?4 IS NULL OR COALESCE(r.state, 'stopped') = ?4) \
           AND (?5 IS NULL OR p.status = ?5) \
           AND (?6 IS NULL OR ({sort}, p.id) {comparison} (?6, ?7)) \
         ORDER BY {sort} {direction}, p.id {direction} \
         LIMIT ?8"
    );
    let rows = sqlx::query(&sql)
        .bind(query.q.clone())
        .bind(query.group_id.clone())
        .bind(query.tag.clone())
        .bind(state_filter)
        .bind(status_filter)
        .bind(cursor_key)
        .bind(cursor_id)
        .bind(i64::from(query.limit) + 1)
        .fetch_all(pool)
        .await
        .map_err(|error| ApiError::storage(&error))?;
    rows.iter()
        .map(|row| {
            Ok((
                row.try_get::<String, _>("id")
                    .map_err(|error| ApiError::storage(&error))?,
                row.try_get::<String, _>("sort_key")
                    .map_err(|error| ApiError::storage(&error))?,
            ))
        })
        .collect()
}

/// 更新档案。
///
/// # Errors
/// 档案不存在、已软删除、字段非法或存储错误。
pub async fn update(
    state: &AppState,
    id: &str,
    patch: ProfilePatchInput,
) -> Result<view::ProfileView, ApiError> {
    let repository = repository(state)?;
    let pool = pool_for(state)?;
    let current = load_profile(state, id).await?;
    if current.status == ProfileStatus::Deleted {
        return Err(ApiError::profile_deleted());
    }
    if let Some(name) = &patch.name {
        validate_name(name)?;
    }
    if let Some(notes) = patch.notes.as_ref().and_then(Option::as_deref) {
        validate_notes(Some(notes))?;
    }
    if let Some(start_url) = patch.start_url.as_ref().and_then(Option::as_deref) {
        validate_start_url(Some(start_url))?;
    }
    if let Some(tags) = &patch.tags {
        validate_tags(tags)?;
        ensure_tags_exist(pool, tags).await?;
    }
    if let Some(Some(group_id)) = &patch.group_id {
        ensure_group(pool, group_id).await?;
    }
    if let Some(Some(proxy_id)) = &patch.proxy_id {
        ensure_proxy(pool, proxy_id).await?;
    }
    if let Some(fingerprint) = &patch.fingerprint {
        fingerprint
            .clone()
            .into_spec(Some(current.fingerprint.seed))
            .map_err(ApiError::from_core)?;
    }

    // 显式 `null` 先清空可空列；核心 PATCH 的 COALESCE 会保留其余列。
    let mut clear: Vec<&str> = Vec::new();
    if matches!(patch.notes, Some(None)) {
        clear.push("notes=NULL");
    }
    if matches!(patch.start_url, Some(None)) {
        clear.push("start_url=NULL");
    }
    if !clear.is_empty() {
        let statement = format!(
            "UPDATE profiles SET {}, updated_at=?1 WHERE id=?2",
            clear.join(",")
        );
        sqlx::query(&statement)
            .bind(mien_server_timestamp())
            .bind(&current.id.0)
            .execute(pool)
            .await
            .map_err(|error| ApiError::storage(&error))?;
    }

    let fingerprint = match patch.fingerprint {
        Some(input) => Some(
            input
                .into_spec(Some(current.fingerprint.seed))
                .map_err(ApiError::from_core)?,
        ),
        None => None,
    };
    repository
        .update(
            &current.id,
            ProfilePatch {
                name: patch.name,
                notes: patch.notes.flatten(),
                fingerprint,
                start_url: patch.start_url.flatten(),
                kernel_version: None,
                mode: patch.mode,
                automation_api: patch.automation_api,
            },
        )
        .await
        .map_err(ApiError::from_core)?;

    if let Some(proxy_id) = &patch.proxy_id {
        sqlx::query("UPDATE profiles SET proxy_id=?1, updated_at=?2 WHERE id=?3")
            .bind(proxy_id.clone())
            .bind(mien_server_timestamp())
            .bind(&current.id.0)
            .execute(pool)
            .await
            .map_err(|error| ApiError::storage(&error))?;
    }
    if patch.group_id.is_some() || patch.tags.is_some() {
        let group = match &patch.group_id {
            Some(value) => value.clone(),
            None => view::group_id(pool, &current.id.0).await?,
        };
        let tags = match &patch.tags {
            Some(tags) => tags.clone(),
            None => view::tags(pool, &current.id.0).await?,
        };
        write_relations(pool, &current.id.0, group.as_deref(), &tags).await?;
    }

    let profile = repository
        .get(&current.id)
        .await
        .map_err(ApiError::from_core)?;
    let live = state.supervisor().state(&view::runtime_id(&profile.id));
    view::profile_view(pool, profile, live, manifest()?.limitations.clone()).await
}

/// 归档：`active -> archived`。
///
/// # Errors
/// 档案不存在、已删除/已归档、运行时非终态或存储错误。
pub async fn archive(state: &AppState, id: &str) -> Result<(), ApiError> {
    let repository = repository(state)?;
    let current = load_profile(state, id).await?;
    match current.status {
        ProfileStatus::Deleted => return Err(ApiError::profile_deleted()),
        ProfileStatus::Archived => {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                ErrorCode::ProfileArchived,
                "The profile is already archived.",
            ));
        }
        ProfileStatus::Active => {}
    }
    ensure_stopped(state, &current).await?;
    repository
        .archive(&current.id)
        .await
        .map_err(ApiError::from_core)?;
    Ok(())
}

/// 恢复：`archived|deleted -> active`。
///
/// # Errors
/// 档案不存在、回收站目录缺失（409）或存储错误。
pub async fn restore(state: &AppState, id: &str) -> Result<(), ApiError> {
    let repository = repository(state)?;
    let current = load_profile(state, id).await?;
    repository
        .restore(&current.id)
        .await
        .map(|_| ())
        .map_err(ApiError::from_core)
}

/// 删除：默认软删除，`permanent=true` 永久清理。
///
/// # Errors
/// 档案不存在、运行时非终态（409）、状态不符或目录操作失败。
pub async fn delete(state: &AppState, id: &str, permanent: bool) -> Result<(), ApiError> {
    let repository = repository(state)?;
    let pool = pool_for(state)?;
    let current = load_profile(state, id).await?;
    ensure_stopped(state, &current).await?;
    let result = if permanent {
        repository.purge(&current.id).await
    } else {
        repository.soft_delete(&current.id).await.map(|_| ())
    };
    match result {
        Ok(()) => Ok(()),
        Err(error) => Err(purge_error(pool, &current.id.0, error).await),
    }
}

/// `PROFILE_PURGE_FAILED` 携带行上写入的 `deletion_error`（PurgeFailure 契约）。
///
/// 其余错误保持通用信封；行读取失败时退化为 `unknown`，仍可重试。
async fn purge_error(pool: &SqlitePool, profile_id: &str, error: CoreError) -> ApiError {
    let mut mapped = ApiError::from_core(error);
    if mapped.code == ErrorCode::ProfilePurgeFailed {
        let deletion_error = sqlx::query_scalar::<_, Option<String>>(
            "SELECT deletion_error FROM profiles WHERE id=?1",
        )
        .bind(profile_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .flatten()
        .unwrap_or_else(|| "unknown".to_owned());
        mapped.details = Some(serde_json::json!({ "deletion_error": deletion_error }));
    }
    mapped
}

/// 复制档案（`REQ-PROFILE-004`）：新标识、新种子、新数据目录。
///
/// `copy_proxy=true` 时以源代理为模板新建 `proxies` 行，凭据用新 `proxy_id`
/// 作为 AAD 重新加密，不复用源行。
///
/// # Errors
/// 档案不存在/已删除、代理需要可用主密钥与凭据、存储错误。
pub async fn duplicate(
    state: &AppState,
    id: &str,
    input: DuplicateInput,
) -> Result<view::ProfileView, ApiError> {
    let repository = repository(state)?;
    let pool = pool_for(state)?;
    let source = load_profile(state, id).await?;
    if source.status == ProfileStatus::Deleted {
        return Err(ApiError::profile_deleted());
    }
    let source_proxy = source.proxy_id.clone().filter(|_| input.copy_proxy);
    let copy = repository
        .copy(
            &source.id,
            CopyOptions {
                copy_fingerprint: input.copy_fingerprint,
                copy_proxy: false,
            },
        )
        .await
        .map_err(ApiError::from_core)?;
    let copy_id = copy.id.clone();
    if let Some(proxy_id) = source_proxy {
        match clone_proxy(state, &proxy_id).await {
            Ok(new_proxy) => {
                if let Err(error) =
                    sqlx::query("UPDATE profiles SET proxy_id=?1, updated_at=?2 WHERE id=?3")
                        .bind(new_proxy.0)
                        .bind(mien_server_timestamp())
                        .bind(&copy_id.0)
                        .execute(pool)
                        .await
                {
                    return Err(ApiError::storage(&error));
                }
            }
            Err(error) => {
                // 复制失败不留半成品档案：目录与行一起回收。
                let _ = repository.soft_delete(&copy_id).await;
                let _ = repository.purge(&copy_id).await;
                return Err(error);
            }
        }
    }
    let profile = repository
        .get(&copy_id)
        .await
        .map_err(ApiError::from_core)?;
    view::profile_view(pool, profile, None, manifest()?.limitations.clone()).await
}

/// 新建一条代理行，内容取自源代理并重新加密。
async fn clone_proxy(state: &AppState, proxy_id: &str) -> Result<ProxyId, ApiError> {
    let pool = pool_for(state)?;
    let key = master_key(state)?;
    let key_bytes: [u8; 32] = key
        .as_bytes()
        .try_into()
        .map_err(|_| ApiError::internal())?;
    let repository = SqliteProxyRepository::new(pool.clone(), MasterKey::from_bytes(key_bytes));
    let source_id = ProxyId(proxy_id.to_owned());
    let view = repository
        .get(&source_id)
        .await
        .map_err(ApiError::from_core)?;
    let secret = repository
        .load_secret(&source_id)
        .await
        .map_err(|_error| proxy_not_ready())?;
    let created = repository
        .create(ProxyInput {
            name: view.name,
            kind: view.kind,
            server: view.server,
            port: view.port,
            options: secret.options.clone(),
            credentials: secret.credentials.clone(),
        })
        .await
        .map_err(ApiError::from_core)?;
    Ok(created.id)
}

// ---------------------------------------------------------------------------
// 启动 / 停止 / 运行时
// ---------------------------------------------------------------------------

/// 启动档案：`REQ-API-021` 要求直连显式授权、代理缺失 fail-closed。
///
/// # Errors
/// 档案不存在、已删除/已归档、直连未授权、代理引擎未就绪、内核缺失或启动失败。
pub async fn start(state: &AppState, id: &str) -> Result<view::RuntimeView, ApiError> {
    let pool = pool_for(state)?;
    let profile = load_profile(state, id).await?;
    match profile.status {
        ProfileStatus::Deleted => return Err(ApiError::profile_deleted()),
        ProfileStatus::Archived => {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                ErrorCode::ProfileArchived,
                "Archived profiles cannot be started.",
            ));
        }
        ProfileStatus::Active => {}
    }
    let direct = profile.proxy_id.is_none();
    if direct && !state.allow_direct_egress() {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            ErrorCode::DirectEgressDisabled,
            "Direct egress is disabled on this server.",
        ));
    }

    let manifest = manifest()?;
    let (binary, binary_sha256) = resolve_kernel(state, &profile, manifest)?;
    let runtime_id = view::runtime_id(&profile.id);

    // 代理路径：真实主密钥 + 真实凭据；任何缺失都以 PROXY_ENGINE_NOT_READY 失败关闭。
    let mut attach: Option<Arc<GatewayAttach<GrpcXrayApi>>> = None;
    let mut engine: Option<Arc<SharedEngine>> = None;
    if let Some(proxy_id) = &profile.proxy_id {
        let (shared, spec) = proxy_readiness(state, proxy_id).await?;
        attach = Some(shared.attach(spec));
        engine = Some(shared);
    }
    let proxy: Option<&dyn ProxyAttach> = attach.as_deref().map(|value| value as &dyn ProxyAttach);

    let request = StartProfile {
        profile: &profile,
        binary,
        binary_sha256,
        manifest,
        data_root: state.data_root().to_path_buf(),
        sandbox_kind: manifest.sandbox_kind.clone(),
        proxy,
        stop_timeout: Duration::from_secs(5),
        restart: RestartPolicy::none(),
    };
    let result = request.start(state.supervisor(), &runtime_id).await;

    match result {
        Ok(started) => {
            persist_started(pool, &profile, &started.snapshot).await?;
            // 成功启动必然进入 `running`：同一事务把本次 isolation 复制为历史结论。
            match view::runtime_row(pool, &profile.id.0).await? {
                Some(row) => Ok(view::RuntimeView::from_row(
                    &row,
                    direct,
                    manifest.limitations.clone(),
                )),
                None => Ok(view::RuntimeView::from_snapshot(
                    &started.snapshot,
                    manifest.limitations.clone(),
                )),
            }
        }
        Err(error) => {
            let _ = persist_failed(pool, &profile, error.code_str()).await;
            if let Some(engine) = engine {
                // 失败启动不得留下代理绑定。
                let _ = engine.detach(&runtime_id);
            }
            Err(ApiError::from_core(error))
        }
    }
}

/// 停止档案；没有运行时记录时幂等返回 `stopped`。
///
/// # Errors
/// 档案不存在、进程组无法终止或存储错误。
pub async fn stop(state: &AppState, id: &str) -> Result<view::RuntimeView, ApiError> {
    let pool = pool_for(state)?;
    let profile = load_profile(state, id).await?;
    let runtime_id = view::runtime_id(&profile.id);
    let snapshot = if state.supervisor().state(&runtime_id).is_some() {
        Some(
            state
                .supervisor()
                .stop(&runtime_id)
                .await
                .map_err(ApiError::from_core)?,
        )
    } else {
        None
    };
    if profile.proxy_id.is_some() {
        let engine = state.engine_slot().lock().await.clone();
        if let Some(engine) = engine {
            let _ = engine.detach(&runtime_id);
        }
    }
    match snapshot {
        Some(snapshot) => {
            persist_stopped(pool, &profile, &snapshot).await?;
            match view::runtime_row(pool, &profile.id.0).await? {
                Some(row) => Ok(view::RuntimeView::from_row(
                    &row,
                    profile.proxy_id.is_none(),
                    manifest()?.limitations.clone(),
                )),
                None => Ok(view::RuntimeView::from_snapshot(
                    &snapshot,
                    manifest()?.limitations.clone(),
                )),
            }
        }
        None => {
            persist_stopped_no_snapshot(pool, &profile).await?;
            match view::runtime_row(pool, &profile.id.0).await? {
                Some(row) => Ok(view::RuntimeView::from_row(
                    &row,
                    profile.proxy_id.is_none(),
                    manifest()?.limitations.clone(),
                )),
                None => Ok(view::RuntimeView::never_started(&profile)),
            }
        }
    }
}

/// 当前运行时：优先进程内快照，其次持久化行，最后合成 `stopped`。
///
/// # Errors
/// 档案不存在或存储错误。
pub async fn runtime(state: &AppState, id: &str) -> Result<view::RuntimeView, ApiError> {
    let pool = pool_for(state)?;
    let profile = load_profile(state, id).await?;
    let runtime_id = view::runtime_id(&profile.id);
    if let Some(snapshot) = state.supervisor().state(&runtime_id) {
        return Ok(view::RuntimeView::from_snapshot(
            &snapshot,
            manifest()?.limitations.clone(),
        ));
    }
    match view::runtime_row(pool, &profile.id.0).await? {
        Some(row) => Ok(view::RuntimeView::from_row(
            &row,
            profile.proxy_id.is_none(),
            manifest()?.limitations.clone(),
        )),
        None => Ok(view::RuntimeView::never_started(&profile)),
    }
}

// ---------------------------------------------------------------------------
// 共享代理引擎
// ---------------------------------------------------------------------------

/// 宿主共享的单个 Xray 引擎：所有代理档案共用同一进程与控制器。
///
/// `process` 必须与控制器同寿命：`EngineProcess` 在 drop 时终止子进程，因此
/// 这里保留它，直到服务端进程退出。
pub struct SharedEngine {
    controller: Arc<EngineController<GrpcXrayApi>>,
    api: Arc<GrpcXrayApi>,
    _process: tokio::sync::Mutex<Option<EngineProcess>>,
    binary: PathBuf,
    sha256: String,
}

impl SharedEngine {
    /// 为一个代理配置构造出站适配器；不复用其他档案的出站规格。
    pub fn attach(&self, spec: OutboundSpec) -> Arc<GatewayAttach<GrpcXrayApi>> {
        let attach = Arc::new(GatewayAttach::new(self.controller.clone(), spec));
        attach.register_engine(EngineBinary {
            path: self.binary.clone(),
            sha256: self.sha256.clone(),
        });
        attach
    }

    /// 释放一个运行时的入站与出站引用。
    ///
    /// # Errors
    /// 引用计数与绑定不一致时返回 `PROXY_OUTBOUND_REFCOUNT_MISMATCH`。
    pub fn detach(&self, runtime_id: &str) -> Result<(), CoreError> {
        self.controller.release(runtime_id).map(|_| ())
    }

    /// 共享引擎的管理 API 客户端；代理检查用它创建与回收 `chk:*` 临时资源。
    ///
    /// 复用同一进程与同一控制器：调用方不得借此再起第二个引擎。
    pub fn api(&self) -> Arc<GrpcXrayApi> {
        self.api.clone()
    }
}

/// 解析代理档案的共享引擎并返回本次启动的出站规格。
async fn proxy_readiness(
    state: &AppState,
    proxy_id: &str,
) -> Result<(Arc<SharedEngine>, OutboundSpec), ApiError> {
    let pool = pool_for(state)?;
    let key = master_key(state)?;
    let key_bytes: [u8; 32] = key
        .as_bytes()
        .try_into()
        .map_err(|_| ApiError::internal())?;
    let repository = SqliteProxyRepository::new(pool.clone(), MasterKey::from_bytes(key_bytes));
    let id = ProxyId(proxy_id.to_owned());
    let view = repository.get(&id).await.map_err(|error| {
        if error.code() == ErrorCode::ProxyNotFound {
            proxy_not_ready()
        } else {
            ApiError::from_core(error)
        }
    })?;
    let secret = repository
        .load_secret(&id)
        .await
        .map_err(|_error| proxy_not_ready())?;
    let config = ProxyConfig {
        protocol: protocol_for(view.kind),
        server: view.server,
        port: view.port,
        transport: Transport::Tcp,
        tls: TlsSettings::default(),
        mux: MuxSettings::default(),
        options: secret
            .options
            .iter()
            .filter_map(|(name, value)| value.as_str().map(|text| (name.clone(), text.to_owned())))
            .collect(),
        credentials: Credentials {
            values: secret.credentials.clone(),
        },
    };
    let spec = build_outbound(&config, key.as_bytes()).map_err(ApiError::from_core)?;
    let engine = ensure_engine(state).await?;
    Ok((engine, spec))
}

fn protocol_for(kind: mien_core::ports::proxy::ProxyKind) -> ProxyProtocol {
    use mien_core::ports::proxy::ProxyKind;
    match kind {
        ProxyKind::Vless => ProxyProtocol::Vless,
        ProxyKind::Vmess => ProxyProtocol::Vmess,
        ProxyKind::Shadowsocks => ProxyProtocol::Shadowsocks,
        ProxyKind::Trojan => ProxyProtocol::Trojan,
        ProxyKind::Hysteria2 => ProxyProtocol::Hysteria2,
        ProxyKind::Wireguard => ProxyProtocol::Wireguard,
        ProxyKind::Http => ProxyProtocol::Http,
        ProxyKind::Https => ProxyProtocol::Https,
        ProxyKind::Socks5 => ProxyProtocol::Socks5,
    }
}

/// 代理资源缺失统一映射为 `503 PROXY_ENGINE_NOT_READY`；绝不回落到直连或占位配置。
fn proxy_not_ready() -> ApiError {
    ApiError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        code: ErrorCode::ProxyEngineNotReady,
        message: core_message(ErrorCode::ProxyEngineNotReady).to_owned(),
        retryable: true,
        details: None,
    }
}

/// 首次代理启动时拉起共享引擎；此后复用同一进程与控制器。
pub(crate) async fn ensure_engine(state: &AppState) -> Result<Arc<SharedEngine>, ApiError> {
    let mut slot = state.engine_slot().lock().await;
    if let Some(engine) = slot.as_ref() {
        return Ok(engine.clone());
    }
    let binary = xray_binary(state);
    let sha256 =
        mien_core::kernel::registry::binary_sha256(&binary).map_err(|_| proxy_not_ready())?;
    let process = EngineProcess::start(&binary, &sha256)
        .await
        .map_err(|_| proxy_not_ready())?;
    let api = process.api();
    let controller = Arc::new(EngineController::with_fallback(Arc::clone(&api), false));
    controller.set_state(EngineState::Running);
    controller.register_binary(EngineBinary {
        path: binary.clone(),
        sha256: sha256.clone(),
    });
    let shared = Arc::new(SharedEngine {
        controller,
        api,
        _process: tokio::sync::Mutex::new(Some(process)),
        binary,
        sha256,
    });
    *slot = Some(shared.clone());
    Ok(shared)
}

fn xray_binary(state: &AppState) -> PathBuf {
    std::env::var_os(XRAY_BIN_ENV).map_or_else(
        || state.data_root().join("xray").join("xray"),
        PathBuf::from,
    )
}

// ---------------------------------------------------------------------------
// 运行时持久化
// ---------------------------------------------------------------------------

async fn persist_started(
    pool: &SqlitePool,
    profile: &Profile,
    snapshot: &RuntimeSnapshot,
) -> Result<(), ApiError> {
    let isolation = isolation_str(snapshot.isolation);
    let now = mien_server_timestamp();
    sqlx::query(
        "INSERT INTO runtimes \
         (id, profile_id, state, mode, isolation, last_known_isolation, pid, pgid, proxy_port, cdp_port, started_at, exit_code, last_error, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?10) \
         ON CONFLICT(profile_id) DO UPDATE SET \
           state=excluded.state, mode=excluded.mode, isolation=excluded.isolation, \
           last_known_isolation=excluded.last_known_isolation, pid=excluded.pid, pgid=excluded.pgid, \
           proxy_port=excluded.proxy_port, cdp_port=excluded.cdp_port, started_at=excluded.started_at, \
           exit_code=excluded.exit_code, last_error=excluded.last_error, updated_at=excluded.updated_at",
    )
    .bind(&snapshot.runtime_id)
    .bind(&profile.id.0)
    .bind(runtime_state_str(snapshot.state))
    .bind(profile.mode.as_str())
    .bind(isolation)
    .bind(snapshot.pid.map(i64::from))
    .bind(snapshot.pgid.map(i64::from))
    .bind(snapshot.proxy_port.map(i64::from))
    .bind(snapshot.cdp_port.map(i64::from))
    .bind(&now)
    .bind(snapshot.exit_code.map(i64::from))
    .bind(snapshot.error_code.clone())
    .execute(pool)
    .await
    .map_err(|error| ApiError::storage(&error))?;
    Ok(())
}

async fn persist_failed(
    pool: &SqlitePool,
    profile: &Profile,
    error_code: &str,
) -> Result<(), ApiError> {
    // 失败尝试不得覆盖 `last_known_isolation`；新行默认 `unknown`。
    sqlx::query(
        "INSERT INTO runtimes (id, profile_id, state, mode, isolation, last_known_isolation, last_error, updated_at) \
         VALUES (?1, ?2, 'failed', ?3, 'unknown', 'unknown', ?4, ?5) \
         ON CONFLICT(profile_id) DO UPDATE SET \
           state='failed', mode=excluded.mode, isolation='unknown', \
           last_error=excluded.last_error, updated_at=excluded.updated_at",
    )
    .bind(view::runtime_id(&profile.id))
    .bind(&profile.id.0)
    .bind(profile.mode.as_str())
    .bind(error_code)
    .bind(mien_server_timestamp())
    .execute(pool)
    .await
    .map_err(|error| ApiError::storage(&error))?;
    Ok(())
}

async fn persist_stopped(
    pool: &SqlitePool,
    profile: &Profile,
    snapshot: &RuntimeSnapshot,
) -> Result<(), ApiError> {
    sqlx::query(
        "UPDATE runtimes SET state=?1, isolation=?2, pid=NULL, pgid=NULL, exit_code=?3, \
         last_error=?4, updated_at=?5 WHERE profile_id=?6",
    )
    .bind(runtime_state_str(snapshot.state))
    .bind(isolation_str(snapshot.isolation))
    .bind(snapshot.exit_code.map(i64::from))
    .bind(snapshot.error_code.clone())
    .bind(mien_server_timestamp())
    .bind(&profile.id.0)
    .execute(pool)
    .await
    .map_err(|error| ApiError::storage(&error))?;
    Ok(())
}

async fn persist_stopped_no_snapshot(pool: &SqlitePool, profile: &Profile) -> Result<(), ApiError> {
    sqlx::query(
        "UPDATE runtimes SET state='stopped', pid=NULL, pgid=NULL, updated_at=?1 WHERE profile_id=?2",
    )
    .bind(mien_server_timestamp())
    .bind(&profile.id.0)
    .execute(pool)
    .await
    .map_err(|error| ApiError::storage(&error))?;
    Ok(())
}

fn runtime_state_str(state: RuntimeState) -> &'static str {
    match state {
        RuntimeState::Starting => "starting",
        RuntimeState::Running => "running",
        RuntimeState::Stopping => "stopping",
        RuntimeState::Stopped => "stopped",
        RuntimeState::Crashed => "crashed",
        RuntimeState::Failed => "failed",
    }
}

fn profile_status_str(status: ProfileStatus) -> &'static str {
    match status {
        ProfileStatus::Active => "active",
        ProfileStatus::Archived => "archived",
        ProfileStatus::Deleted => "deleted",
    }
}

fn isolation_str(isolation: mien_core::runtime::Isolation) -> &'static str {
    match isolation {
        mien_core::runtime::Isolation::Enforced => "enforced",
        mien_core::runtime::Isolation::Unsandboxed => "unsandboxed",
        mien_core::runtime::Isolation::Unknown => "unknown",
    }
}

/// 服务端统一时间戳；与迁移和认证使用同一 `strftime` 文本格式。
fn mien_server_timestamp() -> String {
    crate::auth::session::now_timestamp()
}

// ---------------------------------------------------------------------------
// 共用辅助
// ---------------------------------------------------------------------------

fn repository(state: &AppState) -> Result<&SqliteProfileRepository, ApiError> {
    state.profiles().ok_or_else(|| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::InternalUnexpected,
            "The server has no writable profile storage.",
        )
    })
}

fn pool_for(state: &AppState) -> Result<&SqlitePool, ApiError> {
    state.db().ok_or_else(ApiError::internal)
}

fn manifest() -> Result<&'static CapabilityManifest, ApiError> {
    AppState::manifest().map_err(|_| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::KernelUnsupported,
            "The kernel capability manifest is unavailable.",
        )
    })
}

fn master_key(state: &AppState) -> Result<MasterKey, ApiError> {
    MasterKey::load(state.data_root()).map_err(|_| proxy_not_ready())
}

/// 读取档案；非法 ULID 与不存在同为 404（契约只暴露 `PROFILE_NOT_FOUND`）。
async fn load_profile(state: &AppState, id: &str) -> Result<Profile, ApiError> {
    let profile_id = ProfileId::from_string(id);
    if !profile_id.is_valid() {
        return Err(ApiError::not_found());
    }
    repository(state)?
        .get(&profile_id)
        .await
        .map_err(|error| match error.code() {
            ErrorCode::ProfileNotFound => ApiError::not_found(),
            _ => ApiError::from_core(error),
        })
}

/// 运行时非终态时拒绝会移动/删除目录的操作（`REQ-PROFILE-006`）。
async fn ensure_stopped(state: &AppState, profile: &Profile) -> Result<(), ApiError> {
    if let Some(snapshot) = state.supervisor().state(&view::runtime_id(&profile.id)) {
        if !snapshot.state.is_terminal() {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                ErrorCode::RuntimeConflict,
                "Stop the runtime before changing the profile lifecycle.",
            ));
        }
    }
    if let Some(row) = view::runtime_row(pool_for(state)?, &profile.id.0).await? {
        if !row.state.is_terminal() {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                ErrorCode::RuntimeConflict,
                "Stop the runtime before changing the profile lifecycle.",
            ));
        }
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), ApiError> {
    let length = name.chars().count();
    if length == 0 || length > 120 {
        return Err(ApiError::validation("name", "must be 1-120 characters"));
    }
    Ok(())
}

fn validate_notes(notes: Option<&str>) -> Result<(), ApiError> {
    if notes.is_some_and(|notes| notes.chars().count() > 4000) {
        return Err(ApiError::validation(
            "notes",
            "must be at most 4000 characters",
        ));
    }
    Ok(())
}

fn validate_start_url(url: Option<&str>) -> Result<(), ApiError> {
    let Some(url) = url else {
        return Ok(());
    };
    if url.starts_with("https://") || url.starts_with("http://") || url.starts_with("about:") {
        Ok(())
    } else {
        Err(ApiError::validation(
            "start_url",
            "must start with https://, http:// or about:",
        ))
    }
}

fn validate_tags(tags: &[String]) -> Result<(), ApiError> {
    if tags.len() > 32 {
        return Err(ApiError::validation("tags", "must contain at most 32 tags"));
    }
    let mut seen: Vec<&String> = Vec::with_capacity(tags.len());
    for tag in tags {
        let length = tag.chars().count();
        if length == 0 || length > 64 {
            return Err(ApiError::validation(
                "tags",
                "each tag must be 1-64 characters",
            ));
        }
        if seen.contains(&tag) {
            return Err(ApiError::validation("tags", "must not contain duplicates"));
        }
        seen.push(tag);
    }
    Ok(())
}

fn valid_kernel_version(version: &str) -> bool {
    let parts: Vec<&str> = version.split('.').collect();
    parts.len() == 4
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

async fn ensure_group(pool: &SqlitePool, group_id: &str) -> Result<(), ApiError> {
    validate_ulid(group_id, "group_id")?;
    let exists: Option<String> = sqlx::query_scalar("SELECT id FROM groups WHERE id=?1")
        .bind(group_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| ApiError::storage(&error))?;
    if exists.is_none() {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            ErrorCode::GroupNotFound,
            "No such group.",
        ));
    }
    Ok(())
}

async fn ensure_tags_exist(pool: &SqlitePool, tags: &[String]) -> Result<(), ApiError> {
    for tag in tags {
        let exists: Option<String> = sqlx::query_scalar("SELECT id FROM tags WHERE name=?1")
            .bind(tag)
            .fetch_optional(pool)
            .await
            .map_err(|error| ApiError::storage(&error))?;
        if exists.is_none() {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                ErrorCode::TagNotFound,
                "No such tag.",
            ));
        }
    }
    Ok(())
}

async fn ensure_proxy(pool: &SqlitePool, proxy_id: &str) -> Result<(), ApiError> {
    if !ProfileId::from_string(proxy_id).is_valid() {
        return Err(ApiError::validation("proxy_id", "must be a valid ULID"));
    }
    let exists: Option<String> = sqlx::query_scalar("SELECT id FROM proxies WHERE id=?1")
        .bind(proxy_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| ApiError::storage(&error))?;
    if exists.is_none() {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            ErrorCode::ProxyNotFound,
            "No such proxy.",
        ));
    }
    Ok(())
}

/// 写入 `group_id` 与标签关联。
async fn write_relations(
    pool: &SqlitePool,
    profile_id: &str,
    group_id: Option<&str>,
    tags: &[String],
) -> Result<(), ApiError> {
    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| ApiError::storage(&error))?;
    sqlx::query("UPDATE profiles SET group_id=?1 WHERE id=?2")
        .bind(group_id)
        .bind(profile_id)
        .execute(&mut *transaction)
        .await
        .map_err(|error| ApiError::storage(&error))?;
    sqlx::query("DELETE FROM profile_tags WHERE profile_id=?1")
        .bind(profile_id)
        .execute(&mut *transaction)
        .await
        .map_err(|error| ApiError::storage(&error))?;
    for tag in tags {
        sqlx::query(
            "INSERT OR IGNORE INTO profile_tags (profile_id, tag_id) SELECT ?1, id FROM tags WHERE name=?2",
        )
        .bind(profile_id)
        .bind(tag)
        .execute(&mut *transaction)
        .await
        .map_err(|error| ApiError::storage(&error))?;
    }
    transaction
        .commit()
        .await
        .map_err(|error| ApiError::storage(&error))?;
    Ok(())
}

/// 解析本次启动的内核可执行文件与摘要。
fn resolve_kernel(
    state: &AppState,
    profile: &Profile,
    manifest: &CapabilityManifest,
) -> Result<(PathBuf, String), ApiError> {
    let path = match state.kernel_bin() {
        Some(path) => path.to_path_buf(),
        None => {
            let mut candidates = Vec::new();
            for version in [&profile.kernel_version, &manifest.kernel_version] {
                for name in ["Chromium.app/Contents/MacOS/Chromium", "Chromium", "chrome"] {
                    candidates.push(
                        state
                            .data_root()
                            .join("kernels")
                            .join(version)
                            .join(platform_triplet())
                            .join(name),
                    );
                }
            }
            candidates
                .into_iter()
                .find(|candidate| candidate.is_file())
                .ok_or_else(|| {
                    ApiError::from_core(CoreError::validation(
                        ErrorCode::KernelNotFound,
                        "kernel executable is missing",
                    ))
                })?
        }
    };
    if !path.is_file() {
        return Err(ApiError::from_core(CoreError::validation(
            ErrorCode::KernelNotFound,
            "kernel executable is missing",
        )));
    }
    let sha256 = mien_core::kernel::registry::binary_sha256(&path).map_err(ApiError::from_core)?;
    Ok((path, sha256))
}

fn platform_triplet() -> &'static str {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "macos-arm64"
    } else if cfg!(target_os = "macos") {
        "macos-x86_64"
    } else {
        "linux-x86_64"
    }
}

/// 解析 JSON 请求体；未知字段、类型错误与超限都返回 422。
///
/// # Errors
/// 请求体不是合法 JSON 或不匹配 DTO。
pub fn parse_body<T: DeserializeOwned>(body: &Bytes) -> Result<T, ApiError> {
    if body.len() > MAX_BODY_BYTES {
        return Err(ApiError::validation("body", "is too large"));
    }
    serde_json::from_slice(body)
        .map_err(|error| ApiError::validation("body", format!("is not a valid request: {error}")))
}

/// `204 No Content` 响应。
pub fn no_content() -> Response {
    StatusCode::NO_CONTENT.into_response()
}

/// `201 Created` + JSON。
pub fn created<T: Serialize>(body: &T) -> Response {
    (StatusCode::CREATED, Json(body)).into_response()
}

/// `200 OK` + JSON。
pub fn ok<T: Serialize>(body: &T) -> Response {
    (StatusCode::OK, Json(body)).into_response()
}

/// `202 Accepted` + JSON。
pub fn accepted<T: Serialize>(body: &T) -> Response {
    (StatusCode::ACCEPTED, Json(body)).into_response()
}

/// 删除请求的 `permanent` 查询参数。
///
/// # Errors
/// 非布尔值或未知查询字段。
pub fn parse_permanent(params: &BTreeMap<String, String>) -> Result<bool, ApiError> {
    if let Some(field) = params.keys().find(|key| key.as_str() != "permanent") {
        return Err(ApiError::validation(field, "is not a supported parameter"));
    }
    match params.get("permanent").map(String::as_str) {
        None | Some("false") => Ok(false),
        Some("true") => Ok(true),
        Some(_) => Err(ApiError::validation("permanent", "must be true or false")),
    }
}

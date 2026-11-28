//! 档案导出与导入（`REQ-API-020`/`REQ-PROFILE-009`/`REQ-PROFILE-010`）。
//!
//! 三个端点：`POST /api/v1/profiles/{profile_id}/export`、
//! `POST /api/v1/profiles/import/validate`、`POST /api/v1/profiles/import`。
//!
//! 边界：
//! - 明文载荷形状由 [`super::payload`] 按 `profile-export.schema.json` 校验；
//!   加密载荷是 [`super::container`] 的 `profile-encrypted-v1` 不透明字符串，
//!   envelope 版本参与 AAD。
//! - 代理只导出 `credential_fields` 名称与非秘密 `options`，`credentials_removed=true`。
//! - `passphrase` 只存在于请求体：DTO 不派生 `Debug`，错误只提字段名，
//!   审计元数据只有计数与格式。
//! - `import/validate` 零写入；`import` 的整批事务、请求目录清单与回滚后清理都在
//!   `mien_core::storage::profile` 内完成。

use std::collections::BTreeMap;

use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use mien_core::CoreError;
use mien_core::domain::profile::Profile;
use mien_core::error::ErrorCode;
use mien_core::ports::proxy::{ProxyId, ProxyRepository};
use mien_core::runtime::Isolation;
use mien_core::storage::profile::{
    ConflictStrategy, ImportBatch, ImportProfile, ImportProxy, ImportTag,
};
use mien_core::storage::proxy::SqliteProxyRepository;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::container::{self, Passphrase};
use super::payload::{
    self, ExportPayload, PayloadGroup, PayloadProfile, PayloadProxy, PayloadRuntime, PayloadTag,
};
use super::routes::{SCOPE_READ, SCOPE_WRITE};
use super::view;
use super::{
    ApiError, created, manifest, master_key, ok, pool_for, validate_name, validate_notes,
    validate_start_url,
};
use crate::RequestId;
use crate::auth::Principal;
use crate::state::AppState;

/// 单次导入的档案上限：防止一次事务拖垮服务。
pub const MAX_IMPORT_PROFILES: usize = 256;
/// 导入请求体上限；`EXPORT-005` 要求独立大小上限。
pub const MAX_IMPORT_BODY_BYTES: usize = 8 * 1024 * 1024;

// ---------------------------------------------------------------------------
// 请求 DTO（不派生 Debug：口令不得随调试输出泄漏）
// ---------------------------------------------------------------------------

/// `ProfileExportRequest`。
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportRequest {
    pub format: Format,
    #[serde(default)]
    pub passphrase: Option<Passphrase>,
}

/// `ProfileImportInput` / `ProfileImportValidateInput`。
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportRequest {
    pub format: Format,
    pub payload: Value,
    #[serde(default)]
    pub passphrase: Option<Passphrase>,
    #[serde(default)]
    pub conflict_strategy: Option<StrategyInput>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Plain,
    Encrypted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategyInput {
    NewId,
    Abort,
}

impl From<StrategyInput> for ConflictStrategy {
    fn from(value: StrategyInput) -> Self {
        match value {
            StrategyInput::NewId => Self::NewId,
            StrategyInput::Abort => Self::Abort,
        }
    }
}

// ---------------------------------------------------------------------------
// 响应 DTO
// ---------------------------------------------------------------------------

/// `ProfileExportEnvelope`；两种格式的 `payload` 类型不同，因此手写序列化。
pub struct ExportEnvelope {
    pub exported_at: String,
    pub format: Format,
    pub payload: Value,
}

impl Serialize for ExportEnvelope {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("ProfileExportEnvelope", 4)?;
        state.serialize_field("schema_version", &payload::PAYLOAD_SCHEMA_VERSION)?;
        state.serialize_field(
            "format",
            match self.format {
                Format::Plain => "plain",
                Format::Encrypted => "encrypted",
            },
        )?;
        state.serialize_field("exported_at", &self.exported_at)?;
        state.serialize_field("payload", &self.payload)?;
        state.end()
    }
}

/// `ProfileImportPreview`。
#[derive(Debug, Serialize)]
pub struct ImportPreview {
    pub items: Vec<PreviewItem>,
}

#[derive(Debug, Serialize)]
pub struct PreviewItem {
    pub index: usize,
    pub name: String,
    pub status: PreviewStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draft: Option<Value>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PreviewStatus {
    Valid,
    Conflict,
    Invalid,
}

/// `ProfileImportResult`。
#[derive(Debug, Serialize)]
pub struct ImportResult {
    pub items: Vec<ImportItem>,
}

#[derive(Debug, Serialize)]
pub struct ImportItem {
    pub index: usize,
    pub status: &'static str,
    pub profile: view::ProfileView,
    pub errors: Vec<String>,
}

// ---------------------------------------------------------------------------
// 导出
// ---------------------------------------------------------------------------

/// 导出单个档案。
///
/// # Errors
/// 档案不存在、口令与格式不匹配、指纹/代理不可读或序列化失败。
pub async fn export(
    state: &AppState,
    profile_id: &str,
    request: ExportRequest,
    request_id: &RequestId,
) -> Result<ExportEnvelope, ApiError> {
    validate_passphrase_pair(request.format, request.passphrase.as_ref())?;
    let profile = super::load_profile(state, profile_id).await?;
    let pool = pool_for(state)?;
    let exported_at = mien_core::storage::now_utc();
    let payload = build_payload(state, pool, &profile, &exported_at).await?;
    let payload_value = serde_json::to_value(&payload).map_err(|_| ApiError::internal())?;
    let envelope = match request.format {
        Format::Plain => ExportEnvelope {
            exported_at: payload.exported_at.clone(),
            format: Format::Plain,
            payload: payload_value,
        },
        Format::Encrypted => {
            let passphrase = request
                .passphrase
                .as_ref()
                .expect("encrypted export requires a passphrase");
            let plaintext = serde_json::to_vec(&payload_value).map_err(|_| ApiError::internal())?;
            let sealed = container::seal(passphrase, &plaintext)?;
            ExportEnvelope {
                exported_at: payload.exported_at.clone(),
                format: Format::Encrypted,
                payload: Value::String(sealed),
            }
        }
    };
    super::audit(
        state,
        match request.format {
            Format::Plain => "profile.export.plain",
            Format::Encrypted => "profile.export.encrypted",
        },
        &format!("profile:{}", profile.id.0),
        request_id.as_str(),
        serde_json::json!({ "format": format_str(request.format) }),
    )
    .await;
    Ok(envelope)
}

/// 构造明文载荷；先自检，确保导出永远符合 `profile-export.schema.json`。
async fn build_payload(
    state: &AppState,
    pool: &sqlx::SqlitePool,
    profile: &Profile,
    exported_at: &str,
) -> Result<ExportPayload, ApiError> {
    let _ = state;
    let relations = mien_core::storage::profile::relations(pool, &profile.id.0)
        .await
        .map_err(ApiError::from_core)?;
    let proxy = match &profile.proxy_id {
        Some(proxy_id) => Some(export_proxy(state, proxy_id).await?),
        None => None,
    };
    let fingerprint = payload::fingerprint_value(&profile.fingerprint).map_err(|reason| {
        ApiError::from_core(CoreError::validation(ErrorCode::FingerprintInvalid, reason))
    })?;
    let payload = ExportPayload {
        schema_version: payload::PAYLOAD_SCHEMA_VERSION,
        exported_at: exported_at.to_owned(),
        groups: relations
            .group
            .as_ref()
            .map(|group| {
                vec![PayloadGroup {
                    name: group.name.clone(),
                    // schema 要求 `sort_order >= 0`；历史负值省略而不是发出坏载荷。
                    sort_order: (group.sort_order >= 0).then_some(group.sort_order),
                }]
            })
            .unwrap_or_default(),
        tags: relations
            .tags
            .iter()
            .map(|tag| PayloadTag {
                name: tag.name.clone(),
                color: tag.color.clone(),
            })
            .collect(),
        profiles: vec![PayloadProfile {
            source_profile_id: mien_core::storage::profile::source_identity(pool, &profile.id.0)
                .await
                .map_err(ApiError::from_core)?
                .unwrap_or_else(|| profile.id.0.clone()),
            name: profile.name.clone(),
            group: relations.group.as_ref().map(|group| group.name.clone()),
            tags: relations.tags.iter().map(|tag| tag.name.clone()).collect(),
            notes: profile.notes.clone(),
            kernel_version: profile.kernel_version.clone(),
            fingerprint,
            proxy,
            runtime: PayloadRuntime {
                start_url: profile.start_url.clone(),
                mode: profile.mode,
                automation_api: profile.automation_api,
            },
        }],
    };
    // 自检：任何超出 schema 的字段都在这里变成内部错误，而不是发出坏载荷。
    payload::parse_payload(&serde_json::to_value(&payload).map_err(|_| ApiError::internal())?)
        .map_err(|errors| {
            tracing::error!(
                fields = ?errors.iter().map(|error| error.field.as_str()).collect::<Vec<_>>(),
                "export payload failed its own schema check"
            );
            ApiError::internal()
        })?;
    Ok(payload)
}

/// 脱敏代理：只取名称型 `credential_fields` 与非秘密 `options`。
async fn export_proxy(state: &AppState, proxy_id: &str) -> Result<PayloadProxy, ApiError> {
    let pool = pool_for(state)?;
    let key = master_key(state)?;
    let repository = SqliteProxyRepository::new(pool.clone(), key);
    let view = repository
        .get(&ProxyId(proxy_id.to_owned()))
        .await
        .map_err(|error| match error.code() {
            ErrorCode::ProxyNotFound => ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorCode::StorageIntegrityFailed,
                "The profile references a proxy that no longer exists.",
            ),
            _ => ApiError::from_core(error),
        })?;
    Ok(PayloadProxy {
        kind: view.kind,
        server: view.server,
        port: view.port,
        options: view.options,
        credential_fields: view.credential_fields,
        credentials_removed: true,
    })
}

// ---------------------------------------------------------------------------
// 导入
// ---------------------------------------------------------------------------

/// `import/validate`：零写入 dry-run。
///
/// # Errors
/// 请求形状、口令/格式不匹配或载荷不符 schema。
pub async fn validate_import(
    state: &AppState,
    request: ImportRequest,
) -> Result<ImportPreview, ApiError> {
    validate_passphrase_pair(request.format, request.passphrase.as_ref())?;
    let payload = decode_payload(
        request.format,
        &request.payload,
        request.passphrase.as_ref(),
    )?;
    let strategy = strategy(request.conflict_strategy);
    let pool = pool_for(state)?;
    let conflicts = mien_core::storage::profile::find_import_conflicts(
        pool,
        &payload
            .profiles
            .iter()
            .map(|profile| profile.source_profile_id.clone())
            .collect::<Vec<_>>(),
    )
    .await
    .map_err(ApiError::from_core)?;
    let items = payload
        .profiles
        .iter()
        .enumerate()
        .map(|(index, profile)| {
            let conflict = conflicts.contains(&profile.source_profile_id);
            let errors = match (conflict, strategy) {
                (true, ConflictStrategy::Abort) => {
                    vec![
                        "PROFILE_IMPORT_CONFLICT: abort refuses an existing source profile"
                            .to_owned(),
                    ]
                }
                (true, ConflictStrategy::NewId) => {
                    vec!["PROFILE_IMPORT_CONFLICT: a new profile ID will be assigned".to_owned()]
                }
                (false, _) => Vec::new(),
            };
            PreviewItem {
                index,
                name: profile.name.clone(),
                status: if conflict {
                    PreviewStatus::Conflict
                } else {
                    PreviewStatus::Valid
                },
                draft: Some(draft(profile)),
                errors,
            }
        })
        .collect();
    Ok(ImportPreview { items })
}

/// `import`：整批提交；冲突 `abort` 在写入前返回 409。
///
/// # Errors
/// 输入无效 422、冲突 409、目录清理失败 500、存储错误按目录映射。
pub async fn commit_import(
    state: &AppState,
    request: ImportRequest,
    request_id: &RequestId,
) -> Result<ImportResult, ApiError> {
    validate_passphrase_pair(request.format, request.passphrase.as_ref())?;
    let payload = decode_payload(
        request.format,
        &request.payload,
        request.passphrase.as_ref(),
    )?;
    if payload.profiles.len() > MAX_IMPORT_PROFILES {
        return Err(ApiError::validation(
            "payload.profiles",
            format!("must contain at most {MAX_IMPORT_PROFILES} profiles"),
        ));
    }
    let strategy = strategy(request.conflict_strategy);
    let pool = pool_for(state)?;
    let conflicts = mien_core::storage::profile::find_import_conflicts(
        pool,
        &payload
            .profiles
            .iter()
            .map(|profile| profile.source_profile_id.clone())
            .collect::<Vec<_>>(),
    )
    .await
    .map_err(ApiError::from_core)?;
    if strategy == ConflictStrategy::Abort && !conflicts.is_empty() {
        return Err(import_conflict());
    }
    let key = master_key(state).ok();
    let inputs = import_inputs(&payload)?;
    let imported = mien_core::storage::profile::insert_batch(
        pool,
        ImportBatch {
            request_id: request_id.as_str(),
            data_root: state.data_root(),
            master_key: key.as_ref(),
            strategy,
            profiles: inputs,
        },
    )
    .await
    .map_err(import_error)?;
    let limitations = manifest()?.limitations.clone();
    let mut items = Vec::with_capacity(imported.len());
    for (index, imported) in imported.into_iter().enumerate() {
        // 刚提交的档案从未启动：隔离结论如实为 `unknown`，不虚报运行时证据。
        let direct = imported.profile.proxy_id.is_none();
        let isolation = Isolation::Unknown;
        let view = view::ProfileView {
            reduced_assurance: mien_core::runtime::state::reduced_assurance(
                imported.profile.mode,
                direct,
                isolation,
            ),
            isolation,
            direct,
            group_id: imported.group_id,
            tags: imported.tags,
            limitations: limitations.clone(),
            runtime: None,
            profile: imported.profile,
        };
        items.push(ImportItem {
            index,
            status: "imported",
            profile: view,
            errors: Vec::new(),
        });
    }
    super::audit(
        state,
        "profile.import",
        "profiles",
        request_id.as_str(),
        serde_json::json!({
            "format": format_str(request.format),
            "profiles": items.len(),
            "conflict_strategy": strategy_str(strategy),
        }),
    )
    .await;
    Ok(ImportResult { items })
}

/// 转录层输入 → 领域批量输入。
fn import_inputs(payload: &ExportPayload) -> Result<Vec<ImportProfile>, ApiError> {
    let groups: BTreeMap<&str, i64> = payload
        .groups
        .iter()
        .map(|group| (group.name.as_str(), group.sort_order.unwrap_or(0)))
        .collect();
    let colors: BTreeMap<&str, Option<&str>> = payload
        .tags
        .iter()
        .map(|tag| (tag.name.as_str(), tag.color.as_deref()))
        .collect();
    let mut inputs = Vec::with_capacity(payload.profiles.len());
    for (index, profile) in payload.profiles.iter().enumerate() {
        let field = format!("profiles[{index}]");
        validate_name(&profile.name)
            .and_then(|()| validate_notes(profile.notes.as_deref()))
            .and_then(|()| validate_start_url(profile.runtime.start_url.as_deref()))
            .map_err(|error| rename_field(error, &field))?;
        let fingerprint = payload::fingerprint_spec(&profile.fingerprint)
            .map_err(|reason| ApiError::validation(&format!("{field}.fingerprint"), reason))?;
        let proxy = profile.proxy.as_ref().map(|proxy| ImportProxy {
            kind: proxy.kind,
            server: proxy.server.clone(),
            port: proxy.port,
            options: proxy.options.clone(),
        });
        inputs.push(ImportProfile {
            source_profile_id: profile.source_profile_id.clone(),
            name: profile.name.clone(),
            group: profile.group.clone(),
            group_sort_order: profile
                .group
                .as_deref()
                .and_then(|name| groups.get(name).copied())
                .unwrap_or(0),
            tags: profile
                .tags
                .iter()
                .map(|name| ImportTag {
                    name: name.clone(),
                    color: colors
                        .get(name.as_str())
                        .copied()
                        .flatten()
                        .map(str::to_owned),
                })
                .collect(),
            notes: profile.notes.clone(),
            kernel_version: profile.kernel_version.clone(),
            fingerprint,
            mode: profile.runtime.mode,
            automation_api: profile.runtime.automation_api,
            start_url: profile.runtime.start_url.clone(),
            proxy,
        });
    }
    Ok(inputs)
}

/// 把 `name`/`notes`/`start_url` 的字段错误改成载荷路径。
fn rename_field(mut error: ApiError, prefix: &str) -> ApiError {
    if let Some(Value::Object(fields)) = error
        .details
        .as_mut()
        .and_then(|details| details.get_mut("fields"))
    {
        let renamed: Map<String, Value> = fields
            .iter()
            .map(|(field, reason)| {
                let key = match field.as_str() {
                    "name" | "notes" => format!("{prefix}.{field}"),
                    "start_url" => format!("{prefix}.runtime.start_url"),
                    other => other.to_owned(),
                };
                (key, reason.clone())
            })
            .collect();
        *fields = renamed;
    }
    error
}

/// 解密/解析载荷并做 schema 校验。
fn decode_payload(
    format: Format,
    payload: &Value,
    passphrase: Option<&Passphrase>,
) -> Result<ExportPayload, ApiError> {
    let value = match format {
        Format::Plain => payload.clone(),
        Format::Encrypted => {
            let text = payload.as_str().ok_or_else(|| {
                ApiError::validation("payload", "must be a profile-encrypted-v1 string")
            })?;
            let passphrase = passphrase.expect("encrypted import requires a passphrase");
            let plaintext = container::open(passphrase, text)?;
            serde_json::from_slice(&plaintext)
                .map_err(|_| ApiError::validation("payload", "is not a profile export document"))?
        }
    };
    if !value.is_object() {
        return Err(ApiError::validation(
            "payload",
            "must be a profile export object",
        ));
    }
    payload::parse_payload(&value).map_err(payload_errors)
}

fn payload_errors(errors: Vec<payload::PayloadError>) -> ApiError {
    let fields: Map<String, Value> = errors
        .into_iter()
        .map(|error| {
            let field = if error.field.is_empty() {
                "payload".to_owned()
            } else {
                format!("payload.{}", error.field)
            };
            (field, Value::String(error.reason))
        })
        .collect();
    let mut error = ApiError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::ValidationFailed,
        "The request is not valid.",
    );
    error.details = Some(serde_json::json!({ "fields": fields }));
    error
}

/// `conflict_strategy` 缺省为 `new_id`（契约默认值）。
fn strategy(input: Option<StrategyInput>) -> ConflictStrategy {
    input.map_or(ConflictStrategy::NewId, ConflictStrategy::from)
}

fn validate_passphrase_pair(
    format: Format,
    passphrase: Option<&Passphrase>,
) -> Result<(), ApiError> {
    match (format, passphrase) {
        (Format::Plain, Some(_)) => Err(ApiError::validation(
            "passphrase",
            "must be omitted when format=plain",
        )),
        (Format::Encrypted, None) => Err(ApiError::validation(
            "passphrase",
            "is required when format=encrypted",
        )),
        (Format::Encrypted, Some(passphrase)) if !passphrase.is_valid_length() => {
            Err(ApiError::validation(
                "passphrase",
                format!(
                    "must be {}-{} characters",
                    container::MIN_PASSPHRASE_LEN,
                    container::MAX_PASSPHRASE_LEN
                ),
            ))
        }
        _ => Ok(()),
    }
}

fn import_conflict() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        ErrorCode::ProfileImportConflict,
        "An imported profile already exists.",
    )
}

/// 导入错误映射：冲突 409、清理失败可重试 500、其它按错误码目录。
fn import_error(error: CoreError) -> ApiError {
    let mut mapped = ApiError::from_core(error);
    if mapped.code == ErrorCode::ProfileImportCleanupFailed {
        mapped.retryable = true;
    }
    mapped
}

fn format_str(format: Format) -> &'static str {
    match format {
        Format::Plain => "plain",
        Format::Encrypted => "encrypted",
    }
}

fn strategy_str(strategy: ConflictStrategy) -> &'static str {
    match strategy {
        ConflictStrategy::NewId => "new_id",
        ConflictStrategy::Abort => "abort",
    }
}

/// 预览草稿：脱敏字段，不含口令或凭据。
fn draft(profile: &PayloadProfile) -> Value {
    serde_json::json!({
        "name": profile.name,
        "group": profile.group,
        "tags": profile.tags,
        "kernel_version": profile.kernel_version,
        "mode": profile.runtime.mode,
        "automation_api": profile.runtime.automation_api,
        "start_url": profile.runtime.start_url,
        "seed_preserved": true,
        "proxy": profile.proxy.as_ref().map(|proxy| serde_json::json!({
            "kind": proxy.kind.as_str(),
            "server": proxy.server,
            "port": proxy.port,
            "credential_fields": proxy.credential_fields,
        })),
    })
}

// ---------------------------------------------------------------------------
// 处理函数
// ---------------------------------------------------------------------------

/// `POST /api/v1/profiles/{profile_id}/export`。
pub async fn export_handler(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Extension(request_id): Extension<RequestId>,
    Path(profile_id): Path<String>,
    body: Bytes,
) -> Response {
    if let Err(error) = require_scope(&principal, SCOPE_READ) {
        return error.into_response(&request_id);
    }
    let request: ExportRequest = match parse_export_body(&body) {
        Ok(request) => request,
        Err(error) => return error.into_response(&request_id),
    };
    match export(&state, &profile_id, request, &request_id).await {
        Ok(envelope) => ok(&envelope),
        Err(error) => error.into_response(&request_id),
    }
}

/// `POST /api/v1/profiles/import/validate`。
pub async fn validate_handler(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Extension(request_id): Extension<RequestId>,
    body: Bytes,
) -> Response {
    if let Err(error) = require_scope(&principal, SCOPE_WRITE) {
        return error.into_response(&request_id);
    }
    let request: ImportRequest = match parse_import_body(&body) {
        Ok(request) => request,
        Err(error) => return error.into_response(&request_id),
    };
    match validate_import(&state, request).await {
        Ok(preview) => ok(&preview),
        Err(error) => error.into_response(&request_id),
    }
}

/// `POST /api/v1/profiles/import`。
pub async fn import_handler(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Extension(request_id): Extension<RequestId>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(error) = require_scope(&principal, SCOPE_WRITE) {
        return error.into_response(&request_id);
    }
    let request: ImportRequest = match parse_import_body(&body) {
        Ok(request) => request,
        Err(error) => return error.into_response(&request_id),
    };
    let key = match super::idempotency::Key::parse(idempotency_header(&headers)) {
        Ok(key) => key,
        Err(error) => return error.into_response(&request_id),
    };
    // 幂等指纹不含口令原文，也不保留含口令的请求体摘要。
    let fingerprint = super::idempotency::RequestFingerprint::new(
        "POST",
        "/api/v1/profiles/import",
        &redacted_body(&body),
    );
    let inner_request_id = request_id.clone();
    super::routes::execute_idempotent(
        &state,
        &principal,
        key,
        fingerprint,
        &request_id,
        move |state| async move {
            commit_import(&state, request, &inner_request_id)
                .await
                .map(|result| created(&result))
        },
    )
    .await
}

/// 导入请求体的独立上限。
///
/// 解析失败一律只报固定结论：serde 的 `invalid type` 文本会回显原始标量，而
/// 这里恰好可能出现 `passphrase` 字段，因此不能把驱动文本写进错误消息。
fn parse_import_body(body: &Bytes) -> Result<ImportRequest, ApiError> {
    if body.len() > MAX_IMPORT_BODY_BYTES {
        return Err(ApiError::validation(
            "body",
            format!("must be at most {MAX_IMPORT_BODY_BYTES} bytes"),
        ));
    }
    serde_json::from_slice(body).map_err(|_| invalid_request())
}

/// 导出请求体的严格解析；错误同样不回显任何请求值。
fn parse_export_body(body: &Bytes) -> Result<ExportRequest, ApiError> {
    if body.len() > MAX_IMPORT_BODY_BYTES {
        return Err(ApiError::validation(
            "body",
            format!("must be at most {MAX_IMPORT_BODY_BYTES} bytes"),
        ));
    }
    serde_json::from_slice(body).map_err(|_| invalid_request())
}

fn invalid_request() -> ApiError {
    ApiError::validation("body", "is not a valid request")
}

fn idempotency_header(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(super::idempotency::IDEMPOTENCY_HEADER)
        .and_then(|value| value.to_str().ok())
}

/// 幂等指纹用的请求体：删除 `passphrase` 键，避免对它做任何持久化摘要。
fn redacted_body(body: &[u8]) -> Vec<u8> {
    let Ok(mut value) = serde_json::from_slice::<Value>(body) else {
        return Vec::new();
    };
    if let Some(object) = value.as_object_mut() {
        object.remove("passphrase");
    }
    serde_json::to_vec(&value).unwrap_or_default()
}

fn require_scope(principal: &Principal, scope: &str) -> Result<(), ApiError> {
    principal.require_scope(scope).map_err(|error| ApiError {
        status: error.status,
        code: error.code,
        message: error.message.to_owned(),
        retryable: error.retryable,
        details: error.details,
    })
}

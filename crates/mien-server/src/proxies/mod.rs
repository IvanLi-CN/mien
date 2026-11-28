//! 代理配置 REST 端点（T26）：CRUD、单条分享链接导入与代理检查。
//!
//! 契约：`docs/contracts/openapi.yaml` 的 `proxies*` 操作与 `Proxy`/`ProxyInput`/
//! `ProxyPatch`/`ProxyDraft`/`ProxyCheck`，以及 `docs/specs/control-api/SPEC.md`
//! `REQ-API-017`。领域逻辑全部复用 `mien_core`：CRUD 走
//! [`SqliteProxyRepository`]，导入走 [`parse_share_link`]，检查走共享引擎（见 [`check`]）。
//! 本模块只承担传输层职责：DTO 形状、脱敏出参、HTTP 状态映射与审计/事件写入。
//!
//! 脱敏边界：出参只用 [`ProxyView`]（凭据只有字段名）或 `ProxyDraft` 的既有
//! `Serialize`（同样只含字段名）；本模块从不读取、记录或回显凭据明文与完整分享链接。
//! `401 -> 403 -> 404` 的顺序由认证中间件与「先授权、再查存在性」的固定次序保证。

pub mod check;
pub mod routes;
pub mod view;

use mien_core::CoreError;
use mien_core::error::ErrorCode;
use mien_core::ports::proxy::{
    ProxyId, ProxyInput, ProxyKind, ProxyPage, ProxyPatch, ProxyRepository, ProxyView,
};
use mien_core::proxy::share_link::{ProxyDraft, parse_share_link};
use mien_core::secrets::MasterKey;
use mien_core::storage::proxy::SqliteProxyRepository;
use sqlx::SqlitePool;

use crate::profiles::ApiError;
use crate::state::AppState;

/// 代理读操作 scope（`proxiesList`/`proxiesCheck`）。
pub const SCOPE_READ: &str = "proxies.read";
/// 代理写操作 scope（`proxiesCreate`/`proxiesImportLink`/`proxiesUpdate`/`proxiesDelete`）。
pub const SCOPE_WRITE: &str = "proxies.write";

/// 列表页大小上限（`docs/contracts/openapi.yaml#/components/parameters/Limit`）。
pub const MAX_LIMIT: u32 = 200;
/// 默认页大小，与契约一致。
pub const DEFAULT_LIMIT: u32 = 50;
/// 仓储单次读取上限；`ProxyRepository::list` 把 `limit` 收敛到 1..=100。
const REPOSITORY_PAGE: u32 = 100;
/// 为过滤 `kind` 而连续读取的最大页数；仍有余量时返回游标由客户端续读。
const MAX_FILTER_PAGES: usize = 20;

/// 列表查询：游标、页大小与 `kind` 过滤。
#[derive(Debug, Clone, Default)]
pub struct ListQuery {
    pub limit: Option<u32>,
    pub cursor: Option<String>,
    pub kind: Option<ProxyKind>,
}

fn pool_for(state: &AppState) -> Result<&SqlitePool, ApiError> {
    state.db().ok_or_else(ApiError::internal)
}

/// 代理仓储；主密钥不可用时按 `PROXY_ENGINE_NOT_READY` 失败关闭，绝不生成新密钥。
///
/// # Errors
/// 数据根缺少主密钥或数据根不可读。
fn repository(state: &AppState) -> Result<SqliteProxyRepository, ApiError> {
    let pool = pool_for(state)?;
    let key = MasterKey::load(state.data_root()).map_err(|_| proxy_not_ready())?;
    Ok(SqliteProxyRepository::new(pool.clone(), key))
}

/// 主密钥不可用时统一映射为 `503 PROXY_ENGINE_NOT_READY`。
pub(crate) fn proxy_not_ready() -> ApiError {
    ApiError::new(
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::ProxyEngineNotReady,
        "The shared proxy engine is not ready.",
    )
}

/// 代理不存在（或标识不合法）统一 404 `PROXY_NOT_FOUND`。
pub(crate) fn not_found() -> ApiError {
    ApiError::new(
        axum::http::StatusCode::NOT_FOUND,
        ErrorCode::ProxyNotFound,
        "No such proxy.",
    )
}

/// 仓储错误映射：不存在统一 404，其余沿用 `ApiError::from_core` 的码表。
pub(crate) fn map_repository_error(error: CoreError) -> ApiError {
    match error.code() {
        ErrorCode::ProxyNotFound => not_found(),
        _ => ApiError::from_core(error),
    }
}

/// 分页读取脱敏代理列表。
///
/// 仓储没有 `kind` 过滤，因此按页读取后过滤；过滤后不足一页时继续读取，连续读取
/// 上限见 [`MAX_FILTER_PAGES`]，命中上限时保留 `next_cursor` 由客户端续读。
///
/// # Errors
/// 存储或解密失败。
pub async fn list(state: &AppState, query: ListQuery) -> Result<ProxyPage, ApiError> {
    let repository = repository(state)?;
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let mut cursor = query.cursor.filter(|value| !value.is_empty());
    let mut items: Vec<ProxyView> = Vec::new();
    for _ in 0..MAX_FILTER_PAGES {
        let page = repository
            .list(Some(REPOSITORY_PAGE), cursor.clone())
            .await
            .map_err(map_repository_error)?;
        for view in page.items {
            if query.kind.is_none_or(|kind| view.kind == kind) {
                items.push(view);
            }
            if items.len() as u32 == limit {
                return Ok(ProxyPage {
                    items,
                    next_cursor: None,
                });
            }
        }
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => {
                return Ok(ProxyPage {
                    items,
                    next_cursor: None,
                });
            }
        }
    }
    Ok(ProxyPage {
        items,
        next_cursor: cursor,
    })
}

/// 读取单条脱敏视图。
///
/// # Errors
/// 不存在返回 `PROXY_NOT_FOUND`；存储或解密失败。
pub async fn get(state: &AppState, id: &str) -> Result<ProxyView, ApiError> {
    repository(state)?
        .get(&ProxyId(id.to_owned()))
        .await
        .map_err(map_repository_error)
}

/// 新增代理；输入已由路由层按 `additionalProperties: false` 解析。
///
/// # Errors
/// 字段非法返回 `PROXY_CONFIG_INVALID`；存储失败。
pub async fn create(state: &AppState, input: ProxyInput) -> Result<ProxyView, ApiError> {
    repository(state)?
        .create(input)
        .await
        .map_err(map_repository_error)
}

/// 部分更新；未给出的字段保持原值。
///
/// # Errors
/// 不存在返回 `PROXY_NOT_FOUND`；字段非法返回 `PROXY_CONFIG_INVALID`。
pub async fn update(state: &AppState, id: &str, patch: ProxyPatch) -> Result<ProxyView, ApiError> {
    repository(state)?
        .update(&ProxyId(id.to_owned()), patch)
        .await
        .map_err(map_repository_error)
}

/// 删除代理；被档案引用时返回 `409 RESOURCE_IN_USE`。
///
/// # Errors
/// 不存在返回 `PROXY_NOT_FOUND`；仍被引用或存储失败。
pub async fn delete(state: &AppState, id: &str) -> Result<(), ApiError> {
    repository(state)?
        .delete(&ProxyId(id.to_owned()))
        .await
        .map_err(map_repository_error)
}

/// 解析单条分享链接为草稿；不保存、不启动、不联网。
///
/// 返回值的 `Serialize` 只含 `kind/server/port/options/credential_fields/warnings`，
/// 错误消息只含字段名（均由 `mien_core` 保证）。
///
/// # Errors
/// 链接为空、超长、scheme 或参数不受支持时返回 `422 PROXY_CONFIG_INVALID`。
pub fn import_link(link: &str) -> Result<ProxyDraft, ApiError> {
    parse_share_link(link).map_err(ApiError::from_core)
}

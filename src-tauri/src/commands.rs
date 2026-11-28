//! Tauri 命令注册表。每个命令是薄适配器，直接进入 `mien-core` 用例。
//!
//! 命名固定为 `<domain>_<action>`，参数与返回值用 `rename_all = "camelCase"`（ADR-0008）。
//! 失败统一返回 `{ code, message }`，`code` 是 `CoreError::code_str()`。

use mien_core::CoreError;
use mien_core::domain::profile::ProfileQuery;
use serde::Serialize;
use tauri::State;

use crate::state::{
    AppState, CreateProfileInput, ProfileListView, ProfileView, RuntimeStartView,
    RuntimeStatusView, UpdateProfileInput, parse_status,
};

/// 命令失败载荷：稳定错误码 + 人类可读诊断。
#[derive(Debug, Clone, Serialize)]
pub struct CommandError {
    pub code: String,
    pub message: String,
}

impl From<CoreError> for CommandError {
    fn from(error: CoreError) -> Self {
        Self {
            code: error.code_str().to_owned(),
            message: error.to_string(),
        }
    }
}

type CommandResult<T> = Result<T, CommandError>;

/// 应用版本信息，供壳层首帧展示。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub name: &'static str,
    pub version: &'static str,
    pub schema_version: u32,
}

/// 返回应用名、版本与已应用的迁移版本。
#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> AppInfo {
    AppInfo {
        name: env!("CARGO_PKG_NAME"),
        version: env!("CARGO_PKG_VERSION"),
        schema_version: state.schema_version(),
    }
}

/// 档案分页列表；`status` 过滤生命周期状态，省略时列出全部。
#[tauri::command]
pub async fn profiles_list(
    state: State<'_, AppState>,
    limit: u32,
    cursor: Option<String>,
    status: Option<String>,
) -> CommandResult<ProfileListView> {
    let query = ProfileQuery {
        cursor,
        limit: Some(limit),
        search: None,
        status: status.as_deref().map(parse_status).transpose()?,
    };
    Ok(state.list_profiles(query).await?)
}

/// 单个档案。
#[tauri::command]
pub async fn profiles_get(state: State<'_, AppState>, id: String) -> CommandResult<ProfileView> {
    Ok(state.get_profile(&id).await?)
}

/// 创建档案；`fingerprint` 缺省时使用 macOS 新规格。
#[tauri::command]
pub async fn profiles_create(
    state: State<'_, AppState>,
    input: CreateProfileInput,
) -> CommandResult<ProfileView> {
    Ok(state.create_profile(input).await?)
}

/// 更新档案；`patch` 只修改给出的字段。
#[tauri::command]
pub async fn profiles_update(
    state: State<'_, AppState>,
    id: String,
    patch: UpdateProfileInput,
) -> CommandResult<ProfileView> {
    Ok(state.update_profile(&id, patch).await?)
}

/// 启动档案运行时，返回 `disclosures`。
#[tauri::command]
pub async fn runtime_start(
    state: State<'_, AppState>,
    profile_id: String,
) -> CommandResult<RuntimeStartView> {
    Ok(state.start_runtime(&profile_id).await?)
}

/// 停止档案运行时；未启动时幂等返回 `stopped`。
#[tauri::command]
pub async fn runtime_stop(
    state: State<'_, AppState>,
    profile_id: String,
) -> CommandResult<RuntimeStatusView> {
    Ok(state.stop_runtime(&profile_id).await?)
}

/// 读取档案运行时状态。
#[tauri::command]
pub async fn runtime_status(
    state: State<'_, AppState>,
    profile_id: String,
) -> CommandResult<RuntimeStatusView> {
    Ok(state.runtime_status(&profile_id).await?)
}

#[cfg(test)]
mod tests {
    use super::CommandError;
    use mien_core::CoreError;
    use mien_core::error::ErrorCode;

    #[test]
    fn command_error_carries_the_stable_code() {
        let error = CommandError::from(CoreError::validation(
            ErrorCode::KernelNotFound,
            "kernel executable is missing",
        ));
        assert_eq!(error.code, "KERNEL_NOT_FOUND");
        assert_eq!(error.message, "kernel executable is missing");
    }
}

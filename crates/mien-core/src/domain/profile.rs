//! 档案聚合。数据模型见 `docs/ARCHITECTURE.md` §6 与 `docs/DATABASE-SCHEMA.md`。
//!
//! 占位归属：T9-A（创建/复制/导入/生命周期），启动入口由 T9-D 接入。

use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::fingerprint::spec::FingerprintSpec;

/// 档案生命周期状态；`deleted` 不得启动。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileStatus {
    Active,
    Archived,
    Deleted,
}

/// 档案标识（ULID 文本）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProfileId(pub String);

/// 档案详情。种子与数据目录由核心生成，不外露于 UI 传输之外。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub id: ProfileId,
    pub name: String,
    pub status: ProfileStatus,
    pub fingerprint: FingerprintSpec,
    pub start_url: Option<String>,
}

/// 分页查询。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProfileQuery {
    pub cursor: Option<String>,
    pub limit: Option<u32>,
    pub search: Option<String>,
}

/// 分页结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfilePage {
    pub items: Vec<Profile>,
    pub next_cursor: Option<String>,
}

/// 创建档案输入。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateProfile {
    pub name: String,
    pub fingerprint: FingerprintSpec,
    pub start_url: Option<String>,
}

/// 创建档案。
///
/// # Errors
/// 名称冲突、规格校验失败或存储繁忙。
pub fn create(_input: CreateProfile) -> Result<Profile, CoreError> {
    todo!("T9-A：档案创建")
}

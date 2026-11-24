//! 档案仓储端口；SQLite 适配器在 `mien-store-sqlite`。
//!
//! 占位归属：T9-A。

use crate::domain::profile::{CreateProfile, Profile, ProfileId};
use crate::error::CoreError;

/// 档案仓储。
pub trait ProfileRepository: Send + Sync {
    /// 持久化新档案。
    ///
    /// # Errors
    /// 名称冲突或存储繁忙。
    fn insert(
        &self,
        input: CreateProfile,
    ) -> impl Future<Output = Result<Profile, CoreError>> + Send;

    /// 读取档案。
    ///
    /// # Errors
    /// 不存在时返回 [`CoreError::NotFound`]。
    fn get(&self, id: &ProfileId) -> impl Future<Output = Result<Profile, CoreError>> + Send;
}

//! 档案仓储端口；SQLite 适配器在 `mien-core::storage::sqlite`。
//!
//! 占位归属：T9-A。

use crate::domain::profile::{
    CopyOptions, CreateProfile, Profile, ProfileId, ProfilePage, ProfilePatch, ProfileQuery,
};
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

    fn list(
        &self,
        query: ProfileQuery,
    ) -> impl Future<Output = Result<ProfilePage, CoreError>> + Send;
    fn update(
        &self,
        id: &ProfileId,
        patch: ProfilePatch,
    ) -> impl Future<Output = Result<Profile, CoreError>> + Send;
    fn archive(&self, id: &ProfileId) -> impl Future<Output = Result<Profile, CoreError>> + Send;
    fn restore(&self, id: &ProfileId) -> impl Future<Output = Result<Profile, CoreError>> + Send;
    fn soft_delete(
        &self,
        id: &ProfileId,
    ) -> impl Future<Output = Result<Profile, CoreError>> + Send;
    fn purge(&self, id: &ProfileId) -> impl Future<Output = Result<(), CoreError>> + Send;
    fn copy(
        &self,
        id: &ProfileId,
        options: CopyOptions,
    ) -> impl Future<Output = Result<Profile, CoreError>> + Send;
}

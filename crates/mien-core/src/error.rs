//! 领域错误。稳定错误码目录见 `docs/ERROR-CODES.md`。
//!
//! 占位：具体变体与分类由 T8（错误码冻结）和 T9-A（存储）落地。

/// 领域层错误。宿主把它映射为 `ApiError`（Tapes IPC/HTTP 各自的适配层）。
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// 存储写冲突或繁忙，调用方短退避重试。
    #[error("storage busy")]
    StorageBusy,
    /// A database migration failed and needs operator attention.
    #[error("storage migration failed")]
    StorageMigrationFailed,
    /// A database integrity check failed.
    #[error("storage integrity failed")]
    StorageIntegrityFailed,
    /// A database or filesystem operation failed without exposing raw details.
    #[error("storage error")]
    Storage,
    /// 资源不存在。
    #[error("not found")]
    NotFound,
    /// 输入未通过校验；码值稳定，文本可直接面向用户。
    #[error("validation failed: {0}")]
    Validation(String),
    /// A path would escape the configured data root.
    #[error("path is outside data root")]
    PathUnsafe,
    /// The requested profile lifecycle transition is not valid.
    #[error("invalid profile state transition")]
    InvalidTransition,
    /// A soft-deleted profile cannot be used for a normal write/start operation.
    #[error("profile is deleted")]
    ProfileDeleted,
    /// Permanent deletion is only valid for a soft-deleted profile.
    #[error("profile is not deleted")]
    ProfileNotDeleted,
    /// The profile data directory could not be moved or removed.
    #[error("profile purge failed")]
    ProfilePurgeFailed,
    /// A deleted profile has no recoverable trash directory.
    #[error("profile restore unavailable")]
    ProfileRestoreUnavailable,
    /// A non-terminal runtime blocks a destructive profile operation.
    #[error("runtime conflict")]
    RuntimeConflict,
    /// Group name uniqueness was violated.
    #[error("group name conflict")]
    GroupNameConflict,
    /// Tag name uniqueness was violated.
    #[error("tag name conflict")]
    TagNameConflict,
    /// 尚未实现。
    #[error("not implemented")]
    NotImplemented,
}

impl CoreError {
    /// Stable error-code string for host adapters.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::StorageBusy => "STORAGE_BUSY",
            Self::StorageMigrationFailed => "STORAGE_MIGRATION_FAILED",
            Self::StorageIntegrityFailed => "STORAGE_INTEGRITY_FAILED",
            Self::Storage => "INTERNAL_UNEXPECTED",
            Self::NotFound => "PROFILE_NOT_FOUND",
            Self::Validation(_) => "VALIDATION_FAILED",
            Self::PathUnsafe => "VALIDATION_FAILED",
            Self::InvalidTransition => "VALIDATION_FAILED",
            Self::ProfileDeleted => "PROFILE_DELETED",
            Self::ProfileNotDeleted => "PROFILE_NOT_DELETED",
            Self::ProfilePurgeFailed => "PROFILE_PURGE_FAILED",
            Self::ProfileRestoreUnavailable => "PROFILE_RESTORE_UNAVAILABLE",
            Self::RuntimeConflict => "RUNTIME_CONFLICT",
            Self::GroupNameConflict => "GROUP_NAME_CONFLICT",
            Self::TagNameConflict => "TAG_NAME_CONFLICT",
            Self::NotImplemented => "INTERNAL_UNEXPECTED",
        }
    }
}

//! Domain errors with stable machine-readable error codes.

/// Stable machine-readable error codes from `docs/ERROR-CODES.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    AuthRequired,
    AuthInvalid,
    AuthRateLimited,
    AuthCsrfInvalid,
    AuthOriginInvalid,
    ForbiddenScope,
    AuthPasswordChangeRequired,
    EventTicketInvalid,
    EventReplayWindowExpired,
    ProfileNotFound,
    ProfileDeleted,
    ProfileNotDeleted,
    ProfilePurgeFailed,
    ProxyNotFound,
    RuntimeNotFound,
    TokenNotFound,
    ProfileArchived,
    RuntimeConflict,
    IdempotencyConflict,
    ResourceInUse,
    GroupNameConflict,
    TagNameConflict,
    ProfileImportConflict,
    ProfileImportCleanupFailed,
    ProfileRestoreUnavailable,
    ViewerNotAvailable,
    GroupNotFound,
    TagNotFound,
    ValidationFailed,
    FingerprintInvalid,
    ProxyConfigInvalid,
    ShareLinkInvalid,
    RuntimeModeUnsupported,
    KernelUnsupported,
    UnsupportedProtocol,
    TuicNotSupported,
    QuotaExceeded,
    InsufficientResources,
    KernelNotFound,
    KernelChecksumMismatch,
    KernelVersionNotFound,
    ProxyEngineNotFound,
    ProxyEngineChecksumMismatch,
    ProxyEngineVersionNotFound,
    ProxyEngineNotReady,
    ProxyEngineApiUnavailable,
    ProxyEngineStartFailed,
    ProxyEngineCrashed,
    ProxyInboundConflict,
    ProxyOutboundRefcountMismatch,
    ProxyConnectionRefused,
    ProxyAuthFailed,
    ProxyTlsFailed,
    ProxyTimeout,
    ProxyConnectionFailed,
    ProxyDnsFailed,
    RuntimeSpawnFailed,
    RuntimeCdpTimeout,
    SandboxUnavailable,
    DirectEgressDisabled,
    DisplayUnavailable,
    StorageBusy,
    StorageMigrationFailed,
    StorageIntegrityFailed,
    InternalUnexpected,
}

impl ErrorCode {
    /// Stable machine-readable string; hosts branch only on this value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AuthRequired => "AUTH_REQUIRED",
            Self::AuthInvalid => "AUTH_INVALID",
            Self::AuthRateLimited => "AUTH_RATE_LIMITED",
            Self::AuthCsrfInvalid => "AUTH_CSRF_INVALID",
            Self::AuthOriginInvalid => "AUTH_ORIGIN_INVALID",
            Self::ForbiddenScope => "FORBIDDEN_SCOPE",
            Self::AuthPasswordChangeRequired => "AUTH_PASSWORD_CHANGE_REQUIRED",
            Self::EventTicketInvalid => "EVENT_TICKET_INVALID",
            Self::EventReplayWindowExpired => "EVENT_REPLAY_WINDOW_EXPIRED",
            Self::ProfileNotFound => "PROFILE_NOT_FOUND",
            Self::ProfileDeleted => "PROFILE_DELETED",
            Self::ProfileNotDeleted => "PROFILE_NOT_DELETED",
            Self::ProfilePurgeFailed => "PROFILE_PURGE_FAILED",
            Self::ProxyNotFound => "PROXY_NOT_FOUND",
            Self::RuntimeNotFound => "RUNTIME_NOT_FOUND",
            Self::TokenNotFound => "TOKEN_NOT_FOUND",
            Self::ProfileArchived => "PROFILE_ARCHIVED",
            Self::RuntimeConflict => "RUNTIME_CONFLICT",
            Self::IdempotencyConflict => "IDEMPOTENCY_CONFLICT",
            Self::ResourceInUse => "RESOURCE_IN_USE",
            Self::GroupNameConflict => "GROUP_NAME_CONFLICT",
            Self::TagNameConflict => "TAG_NAME_CONFLICT",
            Self::ProfileImportConflict => "PROFILE_IMPORT_CONFLICT",
            Self::ProfileImportCleanupFailed => "PROFILE_IMPORT_CLEANUP_FAILED",
            Self::ProfileRestoreUnavailable => "PROFILE_RESTORE_UNAVAILABLE",
            Self::ViewerNotAvailable => "VIEWER_NOT_AVAILABLE",
            Self::GroupNotFound => "GROUP_NOT_FOUND",
            Self::TagNotFound => "TAG_NOT_FOUND",
            Self::ValidationFailed => "VALIDATION_FAILED",
            Self::FingerprintInvalid => "FINGERPRINT_INVALID",
            Self::ProxyConfigInvalid => "PROXY_CONFIG_INVALID",
            Self::ShareLinkInvalid => "SHARE_LINK_INVALID",
            Self::RuntimeModeUnsupported => "RUNTIME_MODE_UNSUPPORTED",
            Self::KernelUnsupported => "KERNEL_UNSUPPORTED",
            Self::UnsupportedProtocol => "UNSUPPORTED_PROTOCOL",
            Self::TuicNotSupported => "TUIC_NOT_SUPPORTED",
            Self::QuotaExceeded => "QUOTA_EXCEEDED",
            Self::InsufficientResources => "INSUFFICIENT_RESOURCES",
            Self::KernelNotFound => "KERNEL_NOT_FOUND",
            Self::KernelChecksumMismatch => "KERNEL_CHECKSUM_MISMATCH",
            Self::KernelVersionNotFound => "KERNEL_VERSION_NOT_FOUND",
            Self::ProxyEngineNotFound => "PROXY_ENGINE_NOT_FOUND",
            Self::ProxyEngineChecksumMismatch => "PROXY_ENGINE_CHECKSUM_MISMATCH",
            Self::ProxyEngineVersionNotFound => "PROXY_ENGINE_VERSION_NOT_FOUND",
            Self::ProxyEngineNotReady => "PROXY_ENGINE_NOT_READY",
            Self::ProxyEngineApiUnavailable => "PROXY_ENGINE_API_UNAVAILABLE",
            Self::ProxyEngineStartFailed => "PROXY_ENGINE_START_FAILED",
            Self::ProxyEngineCrashed => "PROXY_ENGINE_CRASHED",
            Self::ProxyInboundConflict => "PROXY_INBOUND_CONFLICT",
            Self::ProxyOutboundRefcountMismatch => "PROXY_OUTBOUND_REFCOUNT_MISMATCH",
            Self::ProxyConnectionRefused => "PROXY_CONNECTION_REFUSED",
            Self::ProxyAuthFailed => "PROXY_AUTH_FAILED",
            Self::ProxyTlsFailed => "PROXY_TLS_FAILED",
            Self::ProxyTimeout => "PROXY_TIMEOUT",
            Self::ProxyConnectionFailed => "PROXY_CONNECTION_FAILED",
            Self::ProxyDnsFailed => "PROXY_DNS_FAILED",
            Self::RuntimeSpawnFailed => "RUNTIME_SPAWN_FAILED",
            Self::RuntimeCdpTimeout => "RUNTIME_CDP_TIMEOUT",
            Self::SandboxUnavailable => "SANDBOX_UNAVAILABLE",
            Self::DirectEgressDisabled => "DIRECT_EGRESS_DISABLED",
            Self::DisplayUnavailable => "DISPLAY_UNAVAILABLE",
            Self::StorageBusy => "STORAGE_BUSY",
            Self::StorageMigrationFailed => "STORAGE_MIGRATION_FAILED",
            Self::StorageIntegrityFailed => "STORAGE_INTEGRITY_FAILED",
            Self::InternalUnexpected => "INTERNAL_UNEXPECTED",
        }
    }
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Domain error. Host adapters branch on [`CoreError::code`]; the `Display`
/// text is human-readable diagnostics and never the contract.
#[derive(Debug)]
pub enum CoreError {
    StorageBusy,
    StorageMigrationFailed,
    StorageIntegrityFailed,
    Storage,
    NotFound,
    /// Validation or admission failure with its stable code.
    Validation {
        code: ErrorCode,
        message: String,
    },
    PathUnsafe,
    InvalidTransition,
    ProfileDeleted,
    ProfileNotDeleted,
    ProfilePurgeFailed,
    ProfileRestoreUnavailable,
    RuntimeConflict,
    GroupNameConflict,
    TagNameConflict,
    NotImplemented,
}

impl CoreError {
    pub fn validation(code: ErrorCode, message: impl Into<String>) -> Self {
        Self::Validation {
            code,
            message: message.into(),
        }
    }

    /// Stable machine-readable code from `docs/ERROR-CODES.md`.
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::StorageBusy => ErrorCode::StorageBusy,
            Self::StorageMigrationFailed => ErrorCode::StorageMigrationFailed,
            Self::StorageIntegrityFailed => ErrorCode::StorageIntegrityFailed,
            Self::Storage => ErrorCode::InternalUnexpected,
            Self::NotFound => ErrorCode::ProfileNotFound,
            Self::Validation { code, .. } => *code,
            Self::PathUnsafe | Self::InvalidTransition => ErrorCode::ValidationFailed,
            Self::ProfileDeleted => ErrorCode::ProfileDeleted,
            Self::ProfileNotDeleted => ErrorCode::ProfileNotDeleted,
            Self::ProfilePurgeFailed => ErrorCode::ProfilePurgeFailed,
            Self::ProfileRestoreUnavailable => ErrorCode::ProfileRestoreUnavailable,
            Self::RuntimeConflict => ErrorCode::RuntimeConflict,
            Self::GroupNameConflict => ErrorCode::GroupNameConflict,
            Self::TagNameConflict => ErrorCode::TagNameConflict,
            Self::NotImplemented => ErrorCode::InternalUnexpected,
        }
    }

    /// Stable code string; convenience for serialization and comparisons.
    pub fn code_str(&self) -> &'static str {
        self.code().as_str()
    }
}

impl std::fmt::Display for CoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StorageBusy => f.write_str("storage is busy"),
            Self::StorageMigrationFailed => f.write_str("storage migration failed"),
            Self::StorageIntegrityFailed => f.write_str("storage integrity check failed"),
            Self::Storage => f.write_str("storage error"),
            Self::NotFound => f.write_str("resource not found"),
            Self::Validation { code, message } if message.is_empty() => f.write_str(code.as_str()),
            Self::Validation { message, .. } => f.write_str(message),
            Self::PathUnsafe => f.write_str("path is outside data root"),
            Self::InvalidTransition => f.write_str("invalid state transition"),
            Self::ProfileDeleted => f.write_str("profile is deleted"),
            Self::ProfileNotDeleted => f.write_str("profile is not deleted"),
            Self::ProfilePurgeFailed => f.write_str("profile purge failed"),
            Self::ProfileRestoreUnavailable => f.write_str("profile restore is unavailable"),
            Self::RuntimeConflict => f.write_str("runtime conflicts with an active instance"),
            Self::GroupNameConflict => f.write_str("group name already exists"),
            Self::TagNameConflict => f.write_str("tag name already exists"),
            Self::NotImplemented => f.write_str("not implemented"),
        }
    }
}

impl std::error::Error for CoreError {}

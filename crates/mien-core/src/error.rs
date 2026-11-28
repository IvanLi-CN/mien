//! Domain errors with stable machine-readable error codes.

/// Stable machine-readable error codes from `docs/ERROR-CODES.md`.
///
/// The catalog is the contract; every variant appears here exactly once and a
/// test asserts bidirectional equality with the documented code set.
macro_rules! error_codes {
    ($($variant:ident => $code:literal,)*) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum ErrorCode {
            $($variant,)*
        }

        impl ErrorCode {
            /// Every catalogued code, in documentation order.
            pub const ALL: &'static [Self] = &[$(Self::$variant,)*];

            /// Stable machine-readable string; hosts branch only on this value.
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $code,)*
                }
            }

            /// Parse a stable code string; unknown values are rejected.
            pub fn parse(value: &str) -> Option<Self> {
                match value {
                    $($code => Some(Self::$variant),)*
                    _ => None,
                }
            }
        }
    };
}

error_codes! {
    AuthRequired => "AUTH_REQUIRED",
    AuthInvalid => "AUTH_INVALID",
    AuthRateLimited => "AUTH_RATE_LIMITED",
    AuthCsrfInvalid => "AUTH_CSRF_INVALID",
    AuthOriginInvalid => "AUTH_ORIGIN_INVALID",
    ForbiddenScope => "FORBIDDEN_SCOPE",
    AuthPasswordChangeRequired => "AUTH_PASSWORD_CHANGE_REQUIRED",
    EventTicketInvalid => "EVENT_TICKET_INVALID",
    EventReplayWindowExpired => "EVENT_REPLAY_WINDOW_EXPIRED",
    ProfileNotFound => "PROFILE_NOT_FOUND",
    ProfileDeleted => "PROFILE_DELETED",
    ProfileNotDeleted => "PROFILE_NOT_DELETED",
    ProfilePurgeFailed => "PROFILE_PURGE_FAILED",
    ProxyNotFound => "PROXY_NOT_FOUND",
    RuntimeNotFound => "RUNTIME_NOT_FOUND",
    TokenNotFound => "TOKEN_NOT_FOUND",
    ProfileArchived => "PROFILE_ARCHIVED",
    RuntimeConflict => "RUNTIME_CONFLICT",
    IdempotencyConflict => "IDEMPOTENCY_CONFLICT",
    ResourceInUse => "RESOURCE_IN_USE",
    GroupNameConflict => "GROUP_NAME_CONFLICT",
    TagNameConflict => "TAG_NAME_CONFLICT",
    ProfileImportConflict => "PROFILE_IMPORT_CONFLICT",
    ProfileImportCleanupFailed => "PROFILE_IMPORT_CLEANUP_FAILED",
    ProfileRestoreUnavailable => "PROFILE_RESTORE_UNAVAILABLE",
    ViewerNotAvailable => "VIEWER_NOT_AVAILABLE",
    GroupNotFound => "GROUP_NOT_FOUND",
    TagNotFound => "TAG_NOT_FOUND",
    ValidationFailed => "VALIDATION_FAILED",
    FingerprintInvalid => "FINGERPRINT_INVALID",
    ProxyConfigInvalid => "PROXY_CONFIG_INVALID",
    ShareLinkInvalid => "SHARE_LINK_INVALID",
    RuntimeModeUnsupported => "RUNTIME_MODE_UNSUPPORTED",
    KernelUnsupported => "KERNEL_UNSUPPORTED",
    UnsupportedProtocol => "UNSUPPORTED_PROTOCOL",
    TuicNotSupported => "TUIC_NOT_SUPPORTED",
    QuotaExceeded => "QUOTA_EXCEEDED",
    InsufficientResources => "INSUFFICIENT_RESOURCES",
    KernelNotFound => "KERNEL_NOT_FOUND",
    KernelChecksumMismatch => "KERNEL_CHECKSUM_MISMATCH",
    KernelVersionNotFound => "KERNEL_VERSION_NOT_FOUND",
    ProxyEngineNotFound => "PROXY_ENGINE_NOT_FOUND",
    ProxyEngineChecksumMismatch => "PROXY_ENGINE_CHECKSUM_MISMATCH",
    ProxyEngineVersionNotFound => "PROXY_ENGINE_VERSION_NOT_FOUND",
    ProxyEngineNotReady => "PROXY_ENGINE_NOT_READY",
    ProxyEngineApiUnavailable => "PROXY_ENGINE_API_UNAVAILABLE",
    ProxyEngineStartFailed => "PROXY_ENGINE_START_FAILED",
    ProxyEngineCrashed => "PROXY_ENGINE_CRASHED",
    ProxyInboundConflict => "PROXY_INBOUND_CONFLICT",
    ProxyOutboundRefcountMismatch => "PROXY_OUTBOUND_REFCOUNT_MISMATCH",
    ProxyConnectionRefused => "PROXY_CONNECTION_REFUSED",
    ProxyAuthFailed => "PROXY_AUTH_FAILED",
    ProxyTlsFailed => "PROXY_TLS_FAILED",
    ProxyTimeout => "PROXY_TIMEOUT",
    ProxyConnectionFailed => "PROXY_CONNECTION_FAILED",
    ProxyDnsFailed => "PROXY_DNS_FAILED",
    RuntimeSpawnFailed => "RUNTIME_SPAWN_FAILED",
    RuntimeCdpTimeout => "RUNTIME_CDP_TIMEOUT",
    SandboxUnavailable => "SANDBOX_UNAVAILABLE",
    DirectEgressDisabled => "DIRECT_EGRESS_DISABLED",
    DisplayUnavailable => "DISPLAY_UNAVAILABLE",
    StorageBusy => "STORAGE_BUSY",
    StorageMigrationFailed => "STORAGE_MIGRATION_FAILED",
    StorageIntegrityFailed => "STORAGE_INTEGRITY_FAILED",
    InternalUnexpected => "INTERNAL_UNEXPECTED",
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
    ProfileImportConflict,
    ProfileImportCleanupFailed,
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
            Self::ProfileImportConflict => ErrorCode::ProfileImportConflict,
            Self::ProfileImportCleanupFailed => ErrorCode::ProfileImportCleanupFailed,
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
            Self::ProfileImportConflict => f.write_str("profile import conflict"),
            Self::ProfileImportCleanupFailed => f.write_str("profile import cleanup failed"),
            Self::RuntimeConflict => f.write_str("runtime conflicts with an active instance"),
            Self::GroupNameConflict => f.write_str("group name already exists"),
            Self::TagNameConflict => f.write_str("tag name already exists"),
            Self::NotImplemented => f.write_str("not implemented"),
        }
    }
}

impl std::error::Error for CoreError {}

#[cfg(test)]
mod tests {
    use super::*;

    /// `docs/ERROR-CODES.md` is the contract: the enum and the document must
    /// list exactly the same codes, with no additions and no omissions.
    #[test]
    fn enum_matches_the_documented_catalog_bidirectionally() {
        let document = include_str!("../../../docs/ERROR-CODES.md");
        let mut documented: Vec<&str> = document
            .lines()
            .filter_map(|line| {
                let rest = line.strip_prefix("| `")?;
                let (code, _) = rest.split_once('`')?;
                Some(code)
            })
            .collect();
        documented.sort_unstable();
        documented.dedup();

        let mut implemented: Vec<&str> = ErrorCode::ALL.iter().map(|code| code.as_str()).collect();
        implemented.sort_unstable();
        implemented.dedup();

        assert_eq!(implemented, documented);
        assert_eq!(ErrorCode::ALL.len(), implemented.len());
    }

    #[test]
    fn every_code_round_trips_and_display_is_the_code() {
        for code in ErrorCode::ALL {
            assert_eq!(ErrorCode::parse(code.as_str()), Some(*code));
            assert_eq!(code.to_string(), code.as_str());
        }
        assert_eq!(ErrorCode::parse("NOT_A_CATALOGUED_CODE"), None);
    }

    #[test]
    fn validation_keeps_a_stable_code_and_a_human_message() {
        let error = CoreError::validation(ErrorCode::RuntimeCdpTimeout, "no CDP after 10s");
        assert_eq!(error.code(), ErrorCode::RuntimeCdpTimeout);
        assert_eq!(error.code_str(), "RUNTIME_CDP_TIMEOUT");
        assert_eq!(error.to_string(), "no CDP after 10s");
    }
}

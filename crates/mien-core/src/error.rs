//! 领域错误。稳定错误码目录见 `docs/ERROR-CODES.md`。
//!
//! 占位：具体变体与分类由 T8（错误码冻结）和 T9-A（存储）落地。

/// 领域层错误。宿主把它映射为 `ApiError`（Tapes IPC/HTTP 各自的适配层）。
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// 存储写冲突或繁忙，调用方短退避重试。
    #[error("storage busy")]
    StorageBusy,
    /// 资源不存在。
    #[error("not found")]
    NotFound,
    /// 输入未通过校验；码值稳定，文本可直接面向用户。
    #[error("validation failed: {0}")]
    Validation(String),
    /// 尚未实现。
    #[error("not implemented")]
    NotImplemented,
}

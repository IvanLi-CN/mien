//! 已安装内核与能力清单。manifest schema：`docs/contracts/kernel-manifest.schema.json`。
//!
//! 占位归属：T4（清单结论）、T9-B（能力门禁）。

use serde::{Deserialize, Serialize};

use crate::error::CoreError;

/// 已安装内核记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KernelInstall {
    pub version: String,
    pub platform: String,
    pub binary_sha256: String,
}

/// 运行时使用的可执行文件摘要（十六进制小写），写入安装记录并参与启动前校验。
///
/// # Errors
/// 文件不可读。
pub fn binary_sha256(path: &std::path::Path) -> Result<String, CoreError> {
    let bytes = std::fs::read(path).map_err(|_| {
        CoreError::validation(
            crate::error::ErrorCode::KernelNotFound,
            "kernel executable is missing",
        )
    })?;
    Ok(crate::proxy::keys::sha256(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// 运行时校验可执行文件摘要；不符返回 `KERNEL_CHECKSUM_MISMATCH`。
///
/// # Errors
/// 文件缺失或摘要不符。
pub fn verify_binary(_install: &KernelInstall) -> Result<(), CoreError> {
    todo!("T9-B：binary_sha256 校验")
}

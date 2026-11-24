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

/// 运行时校验可执行文件摘要；不符返回 `KERNEL_CHECKSUM_MISMATCH`。
///
/// # Errors
/// 文件缺失或摘要不符。
pub fn verify_binary(_install: &KernelInstall) -> Result<(), CoreError> {
    todo!("T9-B：binary_sha256 校验")
}

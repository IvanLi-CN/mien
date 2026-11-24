//! 指纹规格。schema 源文件：`docs/contracts/fingerprint-spec.schema.json`。
//!
//! 占位归属：T9-B。

use serde::{Deserialize, Serialize};

use crate::error::CoreError;

/// 指纹规格（字段全集由 T9-B 按 schema 展开）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FingerprintSpec {
    /// 种子；导入保留导出种子以复现指纹。
    pub seed: Option<String>,
}

/// 按 schema 与能力清单校验规格。
///
/// # Errors
/// 规格非法或与当前内核能力冲突。
pub fn validate_spec(_spec: &FingerprintSpec) -> Result<(), CoreError> {
    todo!("T9-B：规格校验")
}

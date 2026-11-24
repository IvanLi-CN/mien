//! 共享引擎网关。协议矩阵见 `docs/XRAY-MAPPING.md`。
//!
//! 占位归属：T9-C。

use crate::error::CoreError;

/// 共享引擎状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineState {
    Stopped,
    Running,
    Rebuilding,
    Failed,
}

/// 获取协议出站引用；相同 outbound key 复用出站与连接池。
///
/// # Errors
/// 引擎不可用或配置非法。
pub fn acquire_outbound(_key: &str) -> Result<(), CoreError> {
    todo!("T9-C：出站引用计数")
}

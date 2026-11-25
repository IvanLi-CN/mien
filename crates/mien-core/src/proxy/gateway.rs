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

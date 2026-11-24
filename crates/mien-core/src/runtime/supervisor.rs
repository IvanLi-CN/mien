//! 运行时状态机。状态转移见 `docs/ARCHITECTURE.md` §6.1。
//!
//! 占位归属：T9-D。

use serde::{Deserialize, Serialize};

use crate::domain::profile::ProfileId;
use crate::error::CoreError;

/// 运行时状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeState {
    Starting,
    Running,
    Stopping,
    Stopped,
    Failed,
}

/// 按档案启动运行时。
///
/// # Errors
/// 档案已删除、已在运行或启动失败。
pub fn start(_id: &ProfileId) -> Result<RuntimeState, CoreError> {
    todo!("T9-D：启动状态机")
}

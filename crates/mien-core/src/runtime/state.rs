//! 受监督运行时的生命周期状态与隔离结论。

use serde::{Deserialize, Serialize};

/// 一个运行时实例的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeState {
    Stopped,
    Starting,
    Running,
    Stopping,
    Crashed,
    Failed,
}

impl RuntimeState {
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Stopped | Self::Crashed | Self::Failed)
    }

    pub const fn event_name(self) -> &'static str {
        match self {
            Self::Stopped => "runtime.stopped",
            Self::Starting => "runtime.starting",
            Self::Running => "runtime.running",
            Self::Stopping => "runtime.stopping",
            Self::Crashed => "runtime.crashed",
            Self::Failed => "runtime.failed",
        }
    }
}

/// 运行时本次启动的隔离结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Isolation {
    Enforced,
    Unsandboxed,
    Unknown,
}

/// 仅允许 SPEC.md 中声明的转移。
pub const fn valid_transition(from: RuntimeState, to: RuntimeState) -> bool {
    matches!(
        (from, to),
        (RuntimeState::Stopped, RuntimeState::Starting)
            | (RuntimeState::Starting, RuntimeState::Running)
            | (RuntimeState::Starting, RuntimeState::Stopping)
            | (RuntimeState::Starting, RuntimeState::Failed)
            | (RuntimeState::Running, RuntimeState::Stopping)
            | (RuntimeState::Stopping, RuntimeState::Stopped)
            | (RuntimeState::Stopping, RuntimeState::Crashed)
            | (RuntimeState::Stopping, RuntimeState::Failed)
            | (RuntimeState::Crashed, RuntimeState::Starting)
            | (RuntimeState::Crashed, RuntimeState::Stopped)
            | (RuntimeState::Failed, RuntimeState::Starting)
            | (RuntimeState::Failed, RuntimeState::Stopped)
    )
}

/// Headless 与伪有头都明确拒绝，避免静默降低保证。
pub const fn reduced_assurance(
    mode: crate::domain::profile::ProfileMode,
    direct: bool,
    isolation: Isolation,
) -> bool {
    direct
        || !matches!(isolation, Isolation::Enforced)
        || !matches!(mode, crate::domain::profile::ProfileMode::Headful)
}

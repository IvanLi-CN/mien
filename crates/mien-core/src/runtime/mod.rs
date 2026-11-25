//! 运行档案监督：状态机、进程组、日志与对账。

pub mod logs;
pub mod probe;
pub mod starter;
pub mod state;
pub mod supervisor;

pub use starter::{ProxyAttach, ProxyAttachment, StartProfile, StartedProfile};
pub use state::{Isolation, RuntimeState};
pub use supervisor::{
    RuntimeEvent, RuntimeEventPayload, RuntimeOptions, RuntimeSnapshot, RuntimeSupervisor,
};

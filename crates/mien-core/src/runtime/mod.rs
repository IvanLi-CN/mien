//! 运行档案监督：状态机、进程组、日志与对账。

pub mod admission;
pub mod logs;
pub mod probe;
pub mod starter;
pub mod state;
pub mod supervisor;

pub use admission::{
    AdmissionPermit, Backoff, DEFAULT_MAX_CONCURRENT_RUNTIMES, DEFAULT_RESTART_DELAYS,
    RuntimeAdmission,
};
pub use starter::{ProxyAttach, ProxyAttachment, StartProfile, StartedProfile};
pub use state::{Isolation, RuntimeState};
pub use supervisor::{
    RestartPolicy, RuntimeEvent, RuntimeEventPayload, RuntimeOptions, RuntimeSnapshot,
    RuntimeSupervisor,
};

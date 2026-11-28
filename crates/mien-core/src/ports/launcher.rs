//! 进程启动器端口：参数数组、独立进程组、超时终止，不经 shell。
//!
//! 占位归属：T9-D。

use crate::error::CoreError;

/// 子进程退出结果。
#[derive(Debug, Clone, Copy)]
pub struct ExitStatus {
    pub code: Option<i32>,
}

/// 启动器。
pub trait ProcessLauncher: Send + Sync {
    /// 以 argv 启动进程，返回退出状态。
    ///
    /// # Errors
    /// 可执行文件缺失、权限或启动失败。
    fn spawn(&self, argv: &[String]) -> impl Future<Output = Result<ExitStatus, CoreError>> + Send;
}

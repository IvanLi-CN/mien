//! 控制面优雅关闭：停止全部受监督内核，并把结果如实写回 `runtimes`。
//!
//! 关闭顺序固定（`REQ-RUNTIME-012`），且每一段都在前一段返回之后开始：
//!
//! 1. 落下监督器闸门（[`RuntimeSupervisor::begin_shutdown`]）——拒绝新启动；
//! 2. `axum` 停止接受新连接并排空在途请求；
//! 3. 逐个停止活动运行时：每行走 [`RuntimeSupervisor::stop`]，即
//!    `stopping` → 完整停止序列（`REQ-RUNTIME-011`）→ `stopped`；
//! 4. 把每个观测到的快照按 HTTP `stop` 的同一语义写回（状态、清 `pid`/`pgid`、
//!    保留 `last_known_isolation`）；
//! 5. 关闭数据库池，进程退出。
//!
//! 顺序不能倒：先杀运行时再等 `axum` 排空，会让排空中的请求看到半死状态。
//!
//! 预算与失败语义：
//! - 全局预算 [`shutdown_budget`]（`MIEN_SHUTDOWN_TIMEOUT_SECS`，默认 10s，1..=60）；
//!   宿主编排的停止宽限期必须大于它（容器 run 命令显式设置，见 `README.server.md`）。
//! - 停止并发进行：一行卡住不占用其他行的预算份额。
//! - 单行失败、或预算耗尽都不阻断其余行；最终一次 `tracing::warn!` 给出未停
//!   干净的行数、剩余行数与 `profile_id`。
//! - 未停干净的行**不写 `stopped`**：写库用的是监督器的真实快照。进程组无法
//!   确认退出时是 `stopping`，保留 `pid`/`pgid` 与端口租约，由下次启动的
//!   `reconcile` 继续处理；`last_error` 记录本次失败。
//! - 已处于终态的行不在 `active_runtimes` 里，天然不会被改写。
//! - 第二次信号通过 [`stop_runtimes_unless`] 的 `abort` 立即放弃停止遍历：
//!   进程仍会先写回已观测到的状态，再退出。

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use mien_core::runtime::{RuntimeState, RuntimeSupervisor};
use sqlx::SqlitePool;
use tokio::task::JoinSet;

use crate::profiles;

/// 关闭预算环境变量。
pub const SHUTDOWN_TIMEOUT_ENV: &str = "MIEN_SHUTDOWN_TIMEOUT_SECS";
/// 默认关闭预算。
pub const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
/// 预算下限。
pub const MIN_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);
/// 预算上限。
pub const MAX_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(60);
/// 写回状态与关闭数据库池的固定余量。
///
/// 它加在停止预算之上，因此宿主停止宽限期必须大于
/// `MIEN_SHUTDOWN_TIMEOUT_SECS + 2s`；容器运行命令显式设置（见 `README.server.md`）。
pub const PERSIST_GRACE: Duration = Duration::from_secs(2);

/// `MIEN_SHUTDOWN_TIMEOUT_SECS`：非数字或越界时回落默认值，与代理检查预算同一
/// 处理策略（配置错误不阻止服务启动，但预算永远是有界的）。
#[must_use]
pub fn shutdown_budget() -> Duration {
    shutdown_budget_from(|key| std::env::var(key).ok())
}

/// 从任意环境样查找读取预算；纯函数供测试直接断言。
#[must_use]
pub fn shutdown_budget_from(lookup: impl Fn(&str) -> Option<String>) -> Duration {
    lookup(SHUTDOWN_TIMEOUT_ENV)
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
        .filter(|budget| (MIN_SHUTDOWN_TIMEOUT..=MAX_SHUTDOWN_TIMEOUT).contains(budget))
        .unwrap_or(DEFAULT_SHUTDOWN_TIMEOUT)
}

/// 一次关闭遍历的结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShutdownReport {
    /// 本次要停止的活动行数。
    pub attempted: usize,
    /// 确认停止（快照 `stopped`、无 `pid`/`pgid`）并按 `stopped` 写回的行。
    pub stopped: Vec<String>,
    /// 未停干净的行：`(profile_id, 观测到的状态)`。它们保持真实状态，绝不写
    /// `stopped`。
    pub unfinished: Vec<(String, &'static str)>,
    /// 第二次信号是否提前结束了遍历。
    pub aborted: bool,
}

impl ShutdownReport {
    /// 有行没停干净（含被中期信号打断）。
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.unfinished.is_empty()
    }
}

/// 停止全部活动运行时并写回真实状态。
pub async fn stop_runtimes(
    supervisor: &Arc<RuntimeSupervisor>,
    pool: Option<&SqlitePool>,
    budget: Duration,
) -> ShutdownReport {
    stop_runtimes_unless(supervisor, pool, budget, std::future::pending::<()>()).await
}

/// 同 [`stop_runtimes`]，但 `abort` 完成时立即放弃停止遍历。
///
/// 生产路径传入第二次信号（第二个 `SIGTERM`/`SIGINT`），测试传入一次性触发器。
/// 放弃不等于丢数据：仍会按监督器此刻的真实快照写回每一行。
pub async fn stop_runtimes_unless<F>(
    supervisor: &Arc<RuntimeSupervisor>,
    pool: Option<&SqlitePool>,
    budget: Duration,
    abort: F,
) -> ShutdownReport
where
    F: Future<Output = ()>,
{
    let mut report = ShutdownReport::default();
    let active = supervisor.active_runtimes();
    report.attempted = active.len();
    if active.is_empty() {
        tracing::info!("shutdown: no active runtimes to stop");
        return report;
    }
    tracing::info!(
        count = active.len(),
        budget_secs = budget.as_secs(),
        "shutdown: stopping active runtimes"
    );

    let deadline = tokio::time::Instant::now() + budget;
    let mut width = std::pin::pin!(abort);
    let mut tasks = JoinSet::new();
    for snapshot in &active {
        let supervisor = supervisor.clone();
        let runtime_id = snapshot.runtime_id.clone();
        tasks.spawn(async move {
            // 结果留在记录里，由写回阶段重新读取：这样一次被预算取消的
            // 停止也能留下它真实到达的状态，而不是调用方的猜测。
            let _ = supervisor.stop(&runtime_id).await;
            runtime_id
        });
    }

    // 完成一行就立刻写回一行：容器可能在预算结束后立刻被 SIGKILL。
    let mut settled: Vec<String> = Vec::with_capacity(active.len());
    let mut write_deadline: Option<tokio::time::Instant> = None;
    loop {
        let next = tokio::select! {
            biased;
            () = &mut width => {
                report.aborted = true;
                tracing::warn!(
                    remaining = active.len() - settled.len(),
                    "shutdown: a second signal abandoned the stop sweep"
                );
                None
            }
            () = tokio::time::sleep_until(deadline) => {
                tracing::warn!(
                    remaining = active.len() - settled.len(),
                    profiles = %remaining_profiles(&active, &settled),
                    "shutdown: budget exhausted with runtimes still stopping"
                );
                None
            }
            joined = tasks.join_next() => joined,
        };
        let Some(joined) = next else { break };
        // 任务体只做停止；`join_next` 的错误（panic）同样按未停干净处理。
        let runtime_id = match joined {
            Ok(runtime_id) => runtime_id,
            Err(error) => {
                tracing::warn!(%error, "shutdown: a runtime stop task failed");
                continue;
            }
        };
        let fallback_profile_id = active
            .iter()
            .find(|snapshot| snapshot.runtime_id == runtime_id)
            .map_or("unknown", |snapshot| snapshot.profile_id.as_str());
        settle_row(
            supervisor,
            pool,
            &runtime_id,
            fallback_profile_id,
            &mut write_deadline,
            &mut report,
        )
        .await;
        settled.push(runtime_id);
    }

    // 预算内没有返回的行：取消它们，再按监督器此刻的真实快照写回。
    tasks.abort_all();
    drop(tasks);
    for snapshot in &active {
        if settled.contains(&snapshot.runtime_id) {
            continue;
        }
        if settle_row(
            supervisor,
            pool,
            &snapshot.runtime_id,
            &snapshot.profile_id,
            &mut write_deadline,
            &mut report,
        )
        .await
        {
            settled.push(snapshot.runtime_id.clone());
        }
    }

    if report.is_complete() {
        tracing::info!(
            stopped = report.stopped.len(),
            "shutdown: all active runtimes stopped"
        );
    } else {
        tracing::warn!(
            stopped = report.stopped.len(),
            unfinished = report.unfinished.len(),
            profiles = %report
                .unfinished
                .iter()
                .map(|(profile_id, _)| profile_id.as_str())
                .collect::<Vec<_>>()
                .join(","),
            "shutdown: some runtimes did not stop cleanly; their rows keep the observed state"
        );
    }
    report
}

/// 写回一行，并把它归入 `stopped` 或 `unfinished`。
///
/// 返回该行是否已被本函数处理（无论写库成功与否），供调用方避免重复计数。
async fn settle_row(
    supervisor: &Arc<RuntimeSupervisor>,
    pool: Option<&SqlitePool>,
    runtime_id: &str,
    fallback_profile_id: &str,
    write_deadline: &mut Option<tokio::time::Instant>,
    report: &mut ShutdownReport,
) -> bool {
    let Some(snapshot) = supervisor.state(runtime_id) else {
        // 记录消失：不能据此断言任何终态，如实记为未完成而不是猜一个 `stopped`。
        tracing::warn!(runtime_id, "shutdown: runtime record disappeared");
        report
            .unfinished
            .push((fallback_profile_id.to_owned(), "record disappeared"));
        return true;
    };
    let profile_id = snapshot.profile_id.clone();
    let settled = snapshot.state == RuntimeState::Stopped;

    if let Some(pool) = pool {
        let deadline =
            *write_deadline.get_or_insert_with(|| tokio::time::Instant::now() + PERSIST_GRACE);
        let write = profiles::persist_stopped(pool, &profile_id, &snapshot);
        match tokio::time::timeout_at(deadline, write).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::warn!(
                    %profile_id,
                    code = error.code.as_str(),
                    "shutdown: cannot write the stopped runtime row"
                );
                report
                    .unfinished
                    .push((profile_id, state_str(snapshot.state)));
                return true;
            }
            Err(_) => {
                tracing::warn!(
                    %profile_id,
                    "shutdown: writing the stopped runtime row exceeded the persistence grace"
                );
                report
                    .unfinished
                    .push((profile_id, state_str(snapshot.state)));
                return true;
            }
        }
    }

    if settled {
        tracing::info!(
            profile_id = %profile_id,
            runtime_id = %runtime_id,
            "shutdown: runtime stopped"
        );
        report.stopped.push(profile_id);
    } else {
        tracing::warn!(
            profile_id = %profile_id,
            runtime_id = %runtime_id,
            state = state_str(snapshot.state),
            "shutdown: runtime did not reach `stopped`; row keeps the observed state"
        );
        report
            .unfinished
            .push((profile_id, state_str(snapshot.state)));
    }
    true
}

fn remaining_profiles(
    active: &[mien_core::runtime::RuntimeSnapshot],
    settled: &[String],
) -> String {
    active
        .iter()
        .filter(|snapshot| !settled.contains(&snapshot.runtime_id))
        .map(|snapshot| snapshot.profile_id.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

fn state_str(state: RuntimeState) -> &'static str {
    match state {
        RuntimeState::Starting => "starting",
        RuntimeState::Running => "running",
        RuntimeState::Stopping => "stopping",
        RuntimeState::Stopped => "stopped",
        RuntimeState::Crashed => "crashed",
        RuntimeState::Failed => "failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_defaults_and_clamps_to_one_through_sixty_seconds() {
        let parse = |value: Option<&'static str>| {
            shutdown_budget_from(move |key| {
                assert_eq!(key, SHUTDOWN_TIMEOUT_ENV);
                value.map(str::to_owned)
            })
        };
        assert_eq!(parse(None), DEFAULT_SHUTDOWN_TIMEOUT);
        assert_eq!(parse(Some("7")), Duration::from_secs(7));
        assert_eq!(parse(Some(" 3 ")), Duration::from_secs(3));
        assert_eq!(parse(Some("1")), MIN_SHUTDOWN_TIMEOUT);
        assert_eq!(parse(Some("60")), MAX_SHUTDOWN_TIMEOUT);
        // 越界与非数字回落默认值，而不是让关闭变成无界等待。
        assert_eq!(parse(Some("0")), DEFAULT_SHUTDOWN_TIMEOUT);
        assert_eq!(parse(Some("61")), DEFAULT_SHUTDOWN_TIMEOUT);
        assert_eq!(parse(Some("soon")), DEFAULT_SHUTDOWN_TIMEOUT);
        assert_eq!(parse(Some("")), DEFAULT_SHUTDOWN_TIMEOUT);
    }
}

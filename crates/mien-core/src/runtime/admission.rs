//! 并发准入与崩溃退避。
//!
//! 两个独立的限制都放在这里，因为它们共享同一条「先准入、再失败关闭」的时序：
//!
//! - [`RuntimeAdmission`] 是 `max_concurrent_runtimes` 信号量。槽位从启动被接纳开始，
//!   一直持有到运行时进入终态，因此超限在创建任何进程之前就返回 `RUNTIME_CONFLICT`，
//!   停止或终态清理会立即释放槽位。
//! - [`Backoff`] 是内核与共享引擎共用的有界重启阶梯（`1s, 2s, 4s, 8s`）。
//!   [`Backoff::next_delay`] 返回 `None` 表示窗口耗尽，调用方必须先完成 teardown，
//!   再写终态失败，不得再次拉起进程。

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::error::{CoreError, ErrorCode};

/// 同时被接纳的受监督运行时默认上限。
pub const DEFAULT_MAX_CONCURRENT_RUNTIMES: usize = 10;

/// 内核或共享引擎连续崩溃后的重启间隔。
pub const DEFAULT_RESTART_DELAYS: [Duration; 4] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
];

/// 并发运行时准入闸门。
pub struct RuntimeAdmission {
    limit: usize,
    semaphore: Arc<Semaphore>,
    holders: Mutex<BTreeMap<String, ()>>,
}

impl RuntimeAdmission {
    /// 最多接纳 `limit` 个运行时；`0` 视为 `1`，避免永久拒绝所有启动。
    #[must_use]
    pub fn new(limit: usize) -> Arc<Self> {
        let limit = limit.max(1);
        Arc::new(Self {
            limit,
            semaphore: Arc::new(Semaphore::new(limit)),
            holders: Mutex::new(BTreeMap::new()),
        })
    }

    #[must_use]
    pub fn default_limit() -> Arc<Self> {
        Self::new(DEFAULT_MAX_CONCURRENT_RUNTIMES)
    }

    #[must_use]
    pub fn limit(&self) -> usize {
        self.limit
    }

    /// 当前持有槽位的运行时数。
    #[must_use]
    pub fn active(&self) -> usize {
        self.holders.lock().expect("admission lock").len()
    }

    #[must_use]
    pub fn holds(&self, runtime_id: &str) -> bool {
        self.holders
            .lock()
            .expect("admission lock")
            .contains_key(runtime_id)
    }

    /// 接纳一次启动；槽位由返回的许可持有，丢弃即释放。
    ///
    /// # Errors
    /// 槽位已满时返回 `RUNTIME_CONFLICT`。
    pub fn try_admit(self: &Arc<Self>, runtime_id: &str) -> Result<AdmissionPermit, CoreError> {
        let permit = self.semaphore.clone().try_acquire_owned().map_err(|_| {
            CoreError::validation(
                ErrorCode::RuntimeConflict,
                format!(
                    "concurrent runtime limit of {} reached; stop a runtime before starting another",
                    self.limit
                ),
            )
        })?;
        self.holders
            .lock()
            .expect("admission lock")
            .insert(runtime_id.to_owned(), ());
        Ok(AdmissionPermit {
            gate: self.clone(),
            runtime_id: runtime_id.to_owned(),
            permit: Some(permit),
        })
    }

    fn release(&self, runtime_id: &str) {
        self.holders
            .lock()
            .expect("admission lock")
            .remove(runtime_id);
    }
}

impl std::fmt::Debug for RuntimeAdmission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeAdmission")
            .field("limit", &self.limit)
            .field("active", &self.active())
            .finish()
    }
}

/// 一次启动占用的准入槽位；drop 即释放。
pub struct AdmissionPermit {
    gate: Arc<RuntimeAdmission>,
    runtime_id: String,
    permit: Option<OwnedSemaphorePermit>,
}

impl AdmissionPermit {
    #[must_use]
    pub fn runtime_id(&self) -> &str {
        &self.runtime_id
    }
}

impl Drop for AdmissionPermit {
    fn drop(&mut self) {
        let _ = self.permit.take();
        self.gate.release(&self.runtime_id);
    }
}

impl std::fmt::Debug for AdmissionPermit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdmissionPermit")
            .field("runtime_id", &self.runtime_id)
            .finish()
    }
}

/// 有界重启阶梯：每次消耗一个间隔，用尽后再不重启。
#[derive(Debug, Clone)]
pub struct Backoff {
    delays: Vec<Duration>,
    attempts: usize,
}

impl Default for Backoff {
    fn default() -> Self {
        Self::with_delays(&DEFAULT_RESTART_DELAYS)
    }
}

impl Backoff {
    #[must_use]
    pub fn with_delays(delays: &[Duration]) -> Self {
        Self {
            delays: delays.to_vec(),
            attempts: 0,
        }
    }

    /// 已消耗的重启次数。
    #[must_use]
    pub fn attempts(&self) -> usize {
        self.attempts
    }

    /// 允许的重启次数上限。
    #[must_use]
    pub fn max_attempts(&self) -> usize {
        self.delays.len()
    }

    #[must_use]
    pub fn exhausted(&self) -> bool {
        self.attempts >= self.delays.len()
    }

    /// 稳定运行后重置阶梯。
    pub fn reset(&mut self) {
        self.attempts = 0;
    }

    /// 消耗一次重启机会，返回重启前的等待时间；`None` 表示窗口已耗尽。
    pub fn next_delay(&mut self) -> Option<Duration> {
        let delay = *self.delays.get(self.attempts)?;
        self.attempts += 1;
        Some(delay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_ladder_is_one_two_four_eight_seconds() {
        let mut backoff = Backoff::default();
        assert_eq!(backoff.max_attempts(), 4);
        let ladder: Vec<_> = std::iter::from_fn(|| backoff.next_delay()).collect();
        assert_eq!(
            ladder,
            [1, 2, 4, 8].map(Duration::from_secs).to_vec(),
            "kernel and engine crashes share the documented ladder"
        );
        assert!(backoff.exhausted());
        assert_eq!(backoff.next_delay(), None, "no fifth restart");
    }

    #[test]
    fn reset_restores_the_full_window() {
        let mut backoff = Backoff::default();
        assert_eq!(backoff.next_delay(), Some(Duration::from_secs(1)));
        backoff.reset();
        assert_eq!(backoff.attempts(), 0);
        assert_eq!(backoff.next_delay(), Some(Duration::from_secs(1)));
    }

    #[test]
    fn admission_rejects_beyond_the_limit_and_frees_on_drop() {
        let gate = RuntimeAdmission::new(2);
        let first = gate.try_admit("a").expect("first slot");
        let second = gate.try_admit("b").expect("second slot");
        assert_eq!(gate.active(), 2);
        assert!(gate.holds("a"));
        let error = gate.try_admit("c").expect_err("third slot refused");
        assert_eq!(error.code(), ErrorCode::RuntimeConflict);
        assert_eq!(error.code_str(), "RUNTIME_CONFLICT");
        drop(first);
        assert_eq!(gate.active(), 1);
        assert!(!gate.holds("a"));
        let third = gate.try_admit("c").expect("slot released by stop");
        assert_eq!(third.runtime_id(), "c");
        drop((second, third));
        assert_eq!(gate.active(), 0);
    }

    #[test]
    fn a_zero_limit_still_admits_one_runtime() {
        let gate = RuntimeAdmission::new(0);
        assert_eq!(gate.limit(), 1);
        let _permit = gate.try_admit("only").expect("one slot");
    }
}

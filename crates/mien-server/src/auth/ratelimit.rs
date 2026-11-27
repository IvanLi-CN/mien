//! 登录失败计数与递增锁定（`REQ-API-007`）。
//!
//! 计数按「来源地址 + 账户」聚合，在滑动时间窗口内累计失败；窗口内失败达到阈值后
//! 锁定，锁定时间随重复超限递增并封顶。所有时间都基于 [`std::time::Instant`]，
//! 不依赖系统时钟回拨。状态保存在进程内：服务重启会清空计数，这符合「临时锁定」
//! 的语义，也不把攻击者的失败记录写进数据库。

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 锁定策略；默认值即为生产参数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// 失败计数窗口。
    pub window: Duration,
    /// 窗口内允许的连续失败次数；达到后立即锁定。
    pub max_failures: u32,
    /// 首次锁定时长。
    pub base_lockout: Duration,
    /// 锁定时长上限。
    pub max_lockout: Duration,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            window: Duration::from_secs(15 * 60),
            max_failures: 5,
            base_lockout: Duration::from_secs(30),
            max_lockout: Duration::from_secs(15 * 60),
        }
    }
}

/// 限流键：来源地址 + 账户。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Key {
    pub address: IpAddr,
    pub account: String,
}

impl Key {
    pub fn new(address: IpAddr, account: impl Into<String>) -> Self {
        Self {
            address,
            account: account.into(),
        }
    }
}

#[derive(Debug, Clone)]
struct Entry {
    failures: u32,
    window_started: Instant,
    locked_until: Option<Instant>,
    lockouts: u32,
}

impl Entry {
    fn fresh(now: Instant) -> Self {
        Self {
            failures: 0,
            window_started: now,
            locked_until: None,
            lockouts: 0,
        }
    }

    fn reset_window(&mut self, now: Instant) {
        self.failures = 0;
        self.window_started = now;
    }
}

/// 进程内失败计数器。
#[derive(Debug)]
pub struct Limiter {
    policy: Policy,
    entries: Mutex<HashMap<Key, Entry>>,
}

/// 一次登录尝试的判定结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// 允许继续校验口令。
    Allow,
    /// 已锁定；参数是 `Retry-After` 秒数（至少 1）。
    Locked { retry_after: u64 },
}

impl Limiter {
    pub fn new(policy: Policy) -> Self {
        Self {
            policy,
            entries: Mutex::new(HashMap::new()),
        }
    }

    pub fn policy(&self) -> Policy {
        self.policy
    }

    /// 检查当前是否已锁定；不会修改计数。
    pub fn check(&self, key: &Key) -> Verdict {
        self.check_at(key, Instant::now())
    }

    /// 记录一次失败；达到阈值时开始/延长锁定并返回锁定判定。
    pub fn record_failure(&self, key: &Key) -> Verdict {
        self.record_failure_at(key, Instant::now())
    }

    /// 登录成功后清空该键的失败计数与锁定层级。
    pub fn record_success(&self, key: &Key) {
        let mut entries = self.entries.lock().expect("limiter mutex poisoned");
        entries.remove(key);
    }

    fn check_at(&self, key: &Key, now: Instant) -> Verdict {
        let mut entries = self.entries.lock().expect("limiter mutex poisoned");
        let Some(entry) = entries.get_mut(key) else {
            return Verdict::Allow;
        };
        match entry.locked_until {
            Some(until) => retry_after(until, now).map_or(Verdict::Allow, |retry_after| {
                Verdict::Locked { retry_after }
            }),
            None => Verdict::Allow,
        }
    }

    fn record_failure_at(&self, key: &Key, now: Instant) -> Verdict {
        let mut entries = self.entries.lock().expect("limiter mutex poisoned");
        prune(&mut entries, now, self.policy);
        let entry = entries
            .entry(key.clone())
            .or_insert_with(|| Entry::fresh(now));

        if let Some(until) = entry.locked_until {
            if let Some(retry_after) = retry_after(until, now) {
                return Verdict::Locked { retry_after };
            }
            entry.locked_until = None;
        }
        if now.duration_since(entry.window_started) >= self.policy.window {
            entry.reset_window(now);
        }
        entry.failures += 1;
        if entry.failures < self.policy.max_failures {
            return Verdict::Allow;
        }

        // 达到阈值：按已有锁定次数指数退避，封顶 `max_lockout`。
        let factor = 1u32 << entry.lockouts.min(16);
        let duration = self
            .policy
            .base_lockout
            .saturating_mul(factor)
            .min(self.policy.max_lockout);
        entry.lockouts += 1;
        entry.failures = 0;
        entry.window_started = now;
        entry.locked_until = Some(now + duration);
        Verdict::Locked {
            retry_after: duration.as_secs().max(1),
        }
    }
}

/// 剩余锁定秒数（向上取整）；已过期返回 `None`。
fn retry_after(until: Instant, now: Instant) -> Option<u64> {
    let remaining = until.checked_duration_since(now)?;
    let seconds = remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0);
    Some(seconds.max(1))
}

/// 清掉窗口已过期且未锁定的键，避免无限增长。
fn prune(entries: &mut HashMap<Key, Entry>, now: Instant, policy: Policy) {
    if entries.len() < 1024 {
        return;
    }
    entries.retain(|_, entry| {
        if let Some(until) = entry.locked_until {
            return until > now;
        }
        now.duration_since(entry.window_started) < policy.window
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn key() -> Key {
        Key::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10)), "admin")
    }

    fn fast_policy() -> Policy {
        Policy {
            window: Duration::from_secs(60),
            max_failures: 3,
            base_lockout: Duration::from_secs(30),
            max_lockout: Duration::from_secs(60),
        }
    }

    #[test]
    fn failures_below_the_threshold_do_not_lock() {
        let limiter = Limiter::new(fast_policy());
        assert_eq!(limiter.record_failure(&key()), Verdict::Allow);
        assert_eq!(limiter.record_failure(&key()), Verdict::Allow);
        assert_eq!(limiter.check(&key()), Verdict::Allow);
    }

    #[test]
    fn reaching_the_threshold_locks_with_retry_after() {
        let limiter = Limiter::new(fast_policy());
        assert_eq!(limiter.record_failure(&key()), Verdict::Allow);
        assert_eq!(limiter.record_failure(&key()), Verdict::Allow);
        assert_eq!(
            limiter.record_failure(&key()),
            Verdict::Locked { retry_after: 30 }
        );
        assert_eq!(limiter.check(&key()), Verdict::Locked { retry_after: 30 });
        assert!(limiter.record_failure(&key()).is_locked());
    }

    #[test]
    fn lockout_escalates_and_is_capped() {
        let limiter = Limiter::new(fast_policy());
        let mut now = Instant::now();
        let mut retries = Vec::new();
        for _ in 0..4 {
            let mut verdict = Verdict::Allow;
            for _ in 0..3 {
                verdict = limiter.record_failure_at(&key(), now);
            }
            retries.push(match verdict {
                Verdict::Locked { retry_after } => retry_after,
                Verdict::Allow => 0,
            });
            // 越过当前锁定，进入下一轮计数。
            now += Duration::from_secs(match retries.last() {
                Some(seconds) => seconds + 1,
                None => 1,
            });
        }
        assert_eq!(retries, vec![30, 60, 60, 60], "30s 起翻倍，封顶 60s");
    }

    #[test]
    fn an_expired_lockout_allows_new_attempts() {
        let limiter = Limiter::new(fast_policy());
        let start = Instant::now();
        for _ in 0..3 {
            limiter.record_failure_at(&key(), start);
        }
        let after = start + Duration::from_secs(31);
        assert_eq!(limiter.check_at(&key(), after), Verdict::Allow);
        assert_eq!(
            limiter.record_failure_at(&key(), after),
            Verdict::Allow,
            "a stale lock must clear before counting the new failure"
        );
    }

    #[test]
    fn success_resets_the_counter() {
        let limiter = Limiter::new(fast_policy());
        limiter.record_failure(&key());
        limiter.record_failure(&key());
        limiter.record_success(&key());
        assert_eq!(limiter.check(&key()), Verdict::Allow);
        assert_eq!(limiter.record_failure(&key()), Verdict::Allow);
    }

    #[test]
    fn addresses_are_isolated() {
        let limiter = Limiter::new(fast_policy());
        for _ in 0..3 {
            limiter.record_failure(&key());
        }
        let other = Key::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 11)), "admin");
        assert_eq!(limiter.check(&other), Verdict::Allow);
    }

    impl Verdict {
        fn is_locked(self) -> bool {
            matches!(self, Self::Locked { .. })
        }
    }
}

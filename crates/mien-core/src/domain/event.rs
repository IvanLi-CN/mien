//! 领域事件。先写 `event_log` 再广播（ADR-0008）；payload 契约见 `docs/contracts/events.schema.json`。
//!
//! 占位归属：T8（事件枚举与 payload），T9-D 落地 `EventSink` 适配。

use serde::{Deserialize, Serialize};

/// 事件信封；`sequence` 是单调回放游标，`event_id` 只用于去重。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainEvent {
    pub event_id: String,
    pub sequence: i64,
    pub kind: String,
    pub payload: serde_json::Value,
}

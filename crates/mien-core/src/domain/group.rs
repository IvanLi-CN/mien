//! 分组。名称唯一，冲突返回 `GROUP_NAME_CONFLICT`。
//!
//! 占位归属：T9-A。

use serde::{Deserialize, Serialize};

/// 分组。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    pub id: String,
    pub name: String,
    pub sort_order: i64,
}

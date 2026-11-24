//! 宿主装配测试：状态对象可构造，迁移查询走到 `mien-core`。

#[path = "../src/state.rs"]
mod state;

use state::{AppState, migration_version};

#[test]
fn app_state_starts_without_data_root() {
    assert!(AppState::new().data_root().is_none());
}

#[test]
fn migration_version_reaches_core_placeholder() {
    let error = migration_version("sqlite::memory:").expect_err("T9-A 前未实现");
    assert!(matches!(error, mien_core::CoreError::NotImplemented));
}

//! 宿主装配测试：状态对象可构造，迁移查询走到 `mien-core`。

#[path = "../src/state.rs"]
mod state;

use state::AppState;
use tempfile::tempdir;

#[test]
fn app_state_starts_without_data_root() {
    assert!(AppState::new().data_root().is_none());
}

#[test]
fn migrations_run_to_current_version() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("mien.sqlite");
    let pool = mien_core::storage::open_and_migrate(&database).expect("first migration run");
    assert_eq!(
        mien_core::storage::current_version(&pool).expect("current version"),
        3
    );
    drop(pool);
    let pool = mien_core::storage::open_and_migrate(&database).expect("idempotent migration run");
    assert_eq!(
        mien_core::storage::current_version(&pool).expect("current version"),
        3
    );
}

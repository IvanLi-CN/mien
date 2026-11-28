//! 控制面启动对账：遗留的运行时行不得在重启后继续声称 `running`。
//!
//! 覆盖 `REQ-RUNTIME-012` 的数据库侧：`RuntimeSupervisor::reconcile` 只处理
//! `runtime.json` 与进程；`mien-server` 过去从不把结果写回 `runtimes`，于是
//! `docker kill` 后行永久停留在 `running`。这里钉住启动路径的落库行为，以及
//! 没有元数据的残留行也必须落定。
//!
//! 真实内核不在本测试范围内；它只断言数据库与元数据两层的一致性。

use mien_server::state::AppState;
use sqlx::SqlitePool;
use tempfile::TempDir;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

/// 一个已迁移的数据根，含一个 `running` 的档案与运行时行。
fn seeded_root() -> (TempDir, SqlitePool) {
    let dir = TempDir::new().expect("temp dir");
    let pool = mien_core::storage::open_and_migrate(&dir.path().join("mien.db")).expect("migrate");
    let rt = runtime();
    rt.block_on(async {
        sqlx::query(
            "INSERT INTO profiles (id,name,fingerprint_json,fingerprint_schema,kernel_version,mode,status,automation_api,created_at,updated_at) \
             VALUES ('prof-1','seeded','{}',1,'148','headful','active',0,'2026-01-01T00:00:00.000Z','2026-01-01T00:00:00.000Z')",
        )
        .execute(&pool)
        .await
        .expect("profile row");
        sqlx::query(
            "INSERT INTO runtimes (id,profile_id,state,mode,isolation,last_known_isolation,pid,pgid,started_at,updated_at) \
             VALUES ('rt-prof-1','prof-1','running','headful','enforced','enforced',4242,4242,'2026-01-01T00:00:00.000Z','2026-01-01T00:00:00.000Z')",
        )
        .execute(&pool)
        .await
        .expect("runtime row");
    });
    (dir, pool)
}

fn row(pool: &SqlitePool) -> (String, Option<i64>, Option<i64>, String) {
    runtime()
        .block_on(async {
            sqlx::query_as::<_, (String, Option<i64>, Option<i64>, String)>(
                "SELECT state,pid,pgid,last_known_isolation FROM runtimes WHERE profile_id='prof-1'",
            )
            .fetch_one(pool)
            .await
        })
        .expect("runtime row")
}

/// A `running` row whose `runtime.json` is gone must not survive a boot: the
/// kernel cannot be claimed as running without metadata, so the row settles to a
/// terminal state and its pid/pgid are cleared.
#[test]
fn boot_settles_a_running_row_without_metadata() {
    let (dir, pool) = seeded_root();
    assert_eq!(row(&pool).0, "running");

    let state = AppState::new(dir.path(), Some(pool.clone()));
    mien_server::profiles::reconcile_runtimes_on_boot(&state);

    let (state_after, pid, pgid, isolation) = row(&pool);
    assert_eq!(
        state_after, "crashed",
        "a restart must not leave a phantom `running` row"
    );
    assert_eq!(pid, None, "a settled row must not keep a pid");
    assert_eq!(pgid, None, "a settled row must not keep a pgid");
    assert_eq!(
        isolation, "enforced",
        "last_known_isolation is the last successful running verdict and must survive"
    );
}

/// A terminal row is untouched: reconciliation only repairs claims that a
/// restart invalidated, and must not rewrite history for `stopped`/`crashed`.
#[test]
fn boot_leaves_terminal_rows_alone() {
    let (dir, pool) = seeded_root();
    runtime().block_on(async {
        sqlx::query(
            "UPDATE runtimes SET state='stopped', pid=NULL, pgid=NULL WHERE profile_id='prof-1'",
        )
        .execute(&pool)
        .await
        .expect("settle");
    });

    let state = AppState::new(dir.path(), Some(pool.clone()));
    mien_server::profiles::reconcile_runtimes_on_boot(&state);

    let (state_after, pid, pgid, isolation) = row(&pool);
    assert_eq!(state_after, "stopped");
    assert_eq!(pid, None);
    assert_eq!(pgid, None);
    assert_eq!(isolation, "enforced");
}

/// No database is not an error: a host that could not open storage still boots.
#[test]
fn boot_without_database_is_a_no_op() {
    let dir = TempDir::new().expect("temp dir");
    let state = AppState::new(dir.path(), None);
    mien_server::profiles::reconcile_runtimes_on_boot(&state);
}

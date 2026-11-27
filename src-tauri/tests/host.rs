//! 宿主装配测试：真实数据根、迁移、档案命令与运行时失败路径。
//!
//! 命令层经由 `AppState` 直接验证；`MIEN_KERNEL_BIN` 未设置时内核解析必须返回
//! `KERNEL_NOT_FOUND`，不允许静默通过。

#[path = "../src/state.rs"]
#[allow(dead_code)]
mod state;

use mien_core::domain::profile::ProfileStatus;
use mien_core::runtime::RuntimeState;
use state::{AppState, CreateProfileInput, UpdateProfileInput};
use tempfile::tempdir;

fn state(directory: &tempfile::TempDir) -> AppState {
    AppState::open_at_with_kernel(directory.path().join("data"), None).expect("host state")
}

/// 测试直接读写同一个库，避免为主机二进制暴露仅测试用的仓储访问器。
async fn pool(data_root: &std::path::Path) -> sqlx::SqlitePool {
    sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&format!("sqlite://{}", data_root.join("mien.db").display()))
        .await
        .expect("test pool")
}

#[test]
fn opens_the_database_and_reports_the_schema_version() {
    let directory = tempdir().expect("temporary directory");
    let state = state(&directory);
    assert_eq!(state.schema_version(), 7);
    assert!(state.data_root().join("mien.db").is_file());
}

#[test]
fn creates_lists_and_updates_profiles() {
    let directory = tempdir().expect("temporary directory");
    let state = state(&directory);
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async {
        let created = state
            .create_profile(CreateProfileInput {
                name: "desktop".into(),
                fingerprint: None,
                mode: None,
                start_url: Some("https://example.com".into()),
                notes: None,
            })
            .await
            .expect("create");
        assert_eq!(created.profile.status, ProfileStatus::Active);
        assert!(created.direct, "a profile without a proxy is direct");
        assert!(!created.limitations.is_empty(), "manifest limitations");
        assert!(created.runtime.is_none());

        let listed = state
            .list_profiles(mien_core::domain::profile::ProfileQuery {
                limit: Some(10),
                status: Some(ProfileStatus::Active),
                ..Default::default()
            })
            .await
            .expect("list");
        assert_eq!(listed.items.len(), 1);
        assert_eq!(listed.items[0].profile.name, "desktop");

        let updated = state
            .update_profile(
                &created.profile.id.0,
                UpdateProfileInput {
                    name: Some("renamed".into()),
                    ..Default::default()
                },
            )
            .await
            .expect("update");
        assert_eq!(updated.profile.name, "renamed");
        assert_eq!(
            updated.profile.fingerprint.seed, created.profile.fingerprint.seed,
            "update must not rotate the seed"
        );
    });
}

#[test]
fn status_filter_hides_archived_profiles() {
    let directory = tempdir().expect("temporary directory");
    let state = state(&directory);
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async {
        state
            .create_profile(CreateProfileInput {
                name: "active-only".into(),
                fingerprint: None,
                mode: None,
                start_url: None,
                notes: None,
            })
            .await
            .expect("create");
        let archived = state
            .create_profile(CreateProfileInput {
                name: "archived".into(),
                fingerprint: None,
                mode: None,
                start_url: None,
                notes: None,
            })
            .await
            .expect("create");
        sqlx::query(
            "UPDATE profiles SET status='archived',archived_at='2026-01-01T00:00:00Z' WHERE id=?1",
        )
        .bind(&archived.profile.id.0)
        .execute(&pool(state.data_root()).await)
        .await
        .expect("archive");

        let active = state
            .list_profiles(mien_core::domain::profile::ProfileQuery {
                limit: Some(10),
                status: Some(ProfileStatus::Active),
                ..Default::default()
            })
            .await
            .expect("list active");
        assert_eq!(active.items.len(), 1);
        assert_eq!(active.items[0].profile.name, "active-only");
    });
}

#[test]
fn runtime_status_and_stop_are_safe_before_any_start() {
    let directory = tempdir().expect("temporary directory");
    let state = state(&directory);
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async {
        let created = state
            .create_profile(CreateProfileInput {
                name: "idle".into(),
                fingerprint: None,
                mode: None,
                start_url: None,
                notes: None,
            })
            .await
            .expect("create");
        let status = state
            .runtime_status(&created.profile.id.0)
            .await
            .expect("status");
        assert_eq!(status.state, RuntimeState::Stopped);
        assert_eq!(
            status.isolation,
            mien_core::runtime::Isolation::Unknown,
            "never claim enforced isolation without evidence"
        );
        let stopped = state
            .stop_runtime(&created.profile.id.0)
            .await
            .expect("stop");
        assert_eq!(stopped.state, RuntimeState::Stopped);
    });
}

/// `MIEN_KERNEL_BIN` 未设置时，启动必须在解析内核阶段失败且不改写隔离结论。
#[test]
fn runtime_start_without_a_kernel_reports_kernel_not_found() {
    let directory = tempdir().expect("temporary directory");
    if std::env::var_os("MIEN_KERNEL_BIN").is_some() {
        eprintln!("skipped: MIEN_KERNEL_BIN is set in this environment");
        return;
    }
    let state = state(&directory);
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async {
        let created = state
            .create_profile(CreateProfileInput {
                name: "no-kernel".into(),
                fingerprint: None,
                mode: None,
                start_url: None,
                notes: None,
            })
            .await
            .expect("create");
        let error = state
            .start_runtime(&created.profile.id.0)
            .await
            .expect_err("missing kernel must fail closed");
        assert_eq!(error.code_str(), "KERNEL_NOT_FOUND");
        let status = state
            .runtime_status(&created.profile.id.0)
            .await
            .expect("status");
        assert_eq!(status.state, RuntimeState::Stopped);
    });
}

/// 显式配置不存在的内核路径同样必须 fail-closed，而不是回退到 direct 模式成功。
#[test]
fn runtime_start_with_a_missing_kernel_bin_fails_closed() {
    let directory = tempdir().expect("temporary directory");
    let missing = directory.path().join("no-such-kernel");
    let state =
        AppState::open_at_with_kernel(directory.path().join("data"), Some(missing)).expect("state");
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async {
        let created = state
            .create_profile(CreateProfileInput {
                name: "missing-kernel".into(),
                fingerprint: None,
                mode: None,
                start_url: None,
                notes: None,
            })
            .await
            .expect("create");
        let error = state
            .start_runtime(&created.profile.id.0)
            .await
            .expect_err("missing kernel must fail closed");
        assert_eq!(error.code_str(), "KERNEL_NOT_FOUND");
    });
}

/// 有代理绑定但引擎尚未落地时必须是 `PROXY_ENGINE_NOT_READY`，不得走 direct。
#[test]
fn proxy_bound_profiles_are_refused_until_the_engine_exists() {
    let directory = tempdir().expect("temporary directory");
    let state = state(&directory);
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async {
        let profile = state
            .create_profile(CreateProfileInput {
                name: "proxy-bound".into(),
                fingerprint: None,
                mode: None,
                start_url: None,
                notes: None,
            })
            .await
            .expect("create");
        // 直连档案没有代理绑定；这里直接写一条带绑定的行以验证 fail-closed 分支，
        // 不依赖尚未落地的代理命令。
        let pool = pool(state.data_root()).await;
        sqlx::query("INSERT INTO proxies (id,name,kind,server,port,config_ciphertext,credential_fields_json,created_at,updated_at) VALUES ('proxy-1','p','vless','127.0.0.1',443,x'00','{}','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')")
            .execute(&pool)
            .await
            .expect("seed proxy");
        sqlx::query("UPDATE profiles SET proxy_id='proxy-1' WHERE id=?1")
            .bind(&profile.profile.id.0)
            .execute(&pool)
            .await
            .expect("bind proxy");

        let error = state
            .start_runtime(&profile.profile.id.0)
            .await
            .expect_err("proxy binding without an engine must fail closed");
        assert_eq!(error.code_str(), "PROXY_ENGINE_NOT_READY");
    });
}

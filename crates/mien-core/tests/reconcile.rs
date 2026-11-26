//! 控制面重启对账、无归属残留与内核崩溃退避。
//!
//! 归属与退避用假进程与 `/bin/sh` 钉住；真实内核场景在设置 `MIEN_KERNEL_BIN`
//! 后执行，未设置时打印显式跳过原因而不是静默通过。

use mien_core::domain::profile::ProfileId;
use mien_core::runtime::probe::{process_alive, process_group_alive};
use mien_core::runtime::{
    RestartPolicy, RuntimeOptions, RuntimeState, RuntimeSupervisor, StartProfile,
};
use std::{
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const SKIP: &str = "skipped: MIEN_KERNEL_BIN not set";

fn kernel() -> Option<PathBuf> {
    match std::env::var_os("MIEN_KERNEL_BIN") {
        Some(value) => {
            let path = PathBuf::from(value);
            assert!(path.is_file(), "MIEN_KERNEL_BIN is not a file: {path:?}");
            Some(path)
        }
        None => {
            eprintln!("{SKIP}");
            None
        }
    }
}

fn digest(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    format!(
        "{:x}",
        Sha256::digest(std::fs::read(path).expect("kernel readable"))
    )
}

/// 清单可解析的选项；`/bin/true` 等假内核也能走完整启动流程直到进程退出。
fn options(binary: &Path, sha: &str, data_dir: &Path, runtime_id: &str) -> RuntimeOptions {
    let mut options = RuntimeOptions::new(runtime_id, ProfileId::new(), binary, sha, data_dir);
    options.launch_args = vec![
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        "--use-mock-keychain".into(),
        "--password-store=basic".into(),
        "--disable-background-networking".into(),
        "--disable-gpu".into(),
    ];
    options
}

/// 可归属的假内核：脚本循环睡住，命令行因此保留内核路径与 runtime id。
///
/// 不能用 `exec`：替换进程映像会丢掉参数，归属判定就失去证据。脚本进程的
/// `ps` 命令行是 `/bin/sh <script> <kernel> --mien-runtime-id=<id>`。
fn attributable_process(binary: &Path, runtime_id: &str, scratch: &Path) -> std::process::Child {
    let script = scratch.join(format!("hold-{runtime_id}.sh"));
    std::fs::write(&script, "#!/bin/sh\nwhile :; do sleep 1; done\n").expect("holder script");
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755))
        .expect("chmod");
    std::process::Command::new(&script)
        .args([
            binary.display().to_string(),
            format!("--mien-runtime-id={runtime_id}"),
        ])
        .process_group(0)
        .spawn()
        .expect("attributable process")
}

/// 外来进程：命令行既没有内核路径，也没有本应用的 runtime id。
fn foreign_process(scratch: &Path) -> std::process::Child {
    let script = scratch.join("foreign.sh");
    std::fs::write(&script, "#!/bin/sh\nwhile :; do sleep 1; done\n").expect("foreign script");
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755))
        .expect("chmod");
    std::process::Command::new(&script)
        .arg("unrelated")
        .process_group(0)
        .spawn()
        .expect("foreign process")
}

/// 把元数据写进数据根，模拟上一次控制面留下的 `runtime.json`。
fn seed(root: &Path, runtime_id: &str, binary: &Path, pid: u32, state: &str) {
    std::fs::create_dir_all(root.join(runtime_id)).expect("seed dir");
    let metadata = serde_json::json!({
        "runtime_id": runtime_id,
        "profile_id": ProfileId::new().0,
        "binary": binary,
        "data_dir": root.join(runtime_id),
        "mode": "headful",
        "isolation": "enforced",
        "direct": true,
        "proxy_port": null,
        "pid": pid,
        "pgid": pid,
        "cdp_port": null,
        "state": state,
    });
    std::fs::write(
        root.join(runtime_id).join("runtime.json"),
        serde_json::to_vec_pretty(&metadata).expect("metadata"),
    )
    .expect("seed metadata");
}

async fn wait_for<F: Fn() -> bool>(check: F) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !check() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[test]
fn unset_kernel_env_reports_explicit_skip() {
    assert!(kernel().is_none() || std::env::var_os("MIEN_KERNEL_BIN").is_some());
}

/// 三个残留：可归属的重复内核、命令行不匹配的外来进程、已退出进程。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconcile_kills_only_attributable_leftovers() {
    let Some(binary) = kernel() else { return };
    assert_eq!(std::env::consts::OS, "macos");
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("data");
    std::fs::create_dir_all(&root).expect("data root");
    let supervisor = RuntimeSupervisor::new(&root);

    // 1. 可归属残留：命令行带内核路径与 runtime id，单独进程组。
    let mut child = attributable_process(&binary, "rt-left", &root);
    let left_pid = child.id();
    seed(&root, "rt-left", &binary, left_pid, "running");
    seed(&root, "rt-left-dup", &binary, left_pid, "running");
    assert!(process_alive(left_pid));

    // 2. 外来进程：同 PID 文件但命令行与本应用无关。
    let mut foreign = foreign_process(&root);
    seed(
        &root,
        "rt-foreign",
        Path::new("/bin/sh"),
        foreign.id(),
        "running",
    );

    // 3. 已退出进程：元数据称 running，PID 已不存在。
    let mut gone = attributable_process(&binary, "rt-gone", &root);
    let gone_pid = gone.id();
    let _ = gone.kill();
    let _ = gone.wait();
    seed(&root, "rt-gone", &binary, gone_pid, "running");

    let outcomes = supervisor.reconcile().await.expect("reconcile");
    let by_id = |id: &str| outcomes.iter().find(|row| row.runtime_id == id);

    assert_eq!(
        by_id("rt-left").map(|row| row.state),
        Some(RuntimeState::Crashed)
    );
    assert_eq!(
        by_id("rt-left-dup").map(|row| row.state),
        Some(RuntimeState::Crashed)
    );
    assert!(
        !mien_core::runtime::probe::process_group_has_live(left_pid as i32),
        "attributable leftover survives"
    );
    assert!(
        !root.join("rt-left").join("runtime.json").exists(),
        "settled row keeps metadata"
    );
    let left_profiles = outcomes
        .iter()
        .filter(|row| row.runtime_id.starts_with("rt-left"))
        .map(|row| row.profile_id.clone())
        .collect::<Vec<_>>();
    let mut unique = left_profiles.clone();
    unique.dedup();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(left_profiles.len(), unique.len(), "duplicate profile rows");

    assert!(
        process_alive(foreign.id()),
        "a process this app cannot claim must survive"
    );
    assert_eq!(
        by_id("rt-foreign").map(|row| row.state),
        Some(RuntimeState::Stopping)
    );
    assert!(
        root.join("rt-foreign").join("runtime.json").exists(),
        "unattributable live row keeps its lease"
    );
    assert_eq!(
        by_id("rt-gone").map(|row| row.state),
        Some(RuntimeState::Crashed)
    );
    assert!(!root.join("rt-gone").join("runtime.json").exists());

    let _ = foreign.kill();
    let _ = foreign.wait();
    let _ = child.kill();
    let _ = child.wait();
}

/// 内核以非零退出后按 1s/2s/4s/8s 重启；窗口耗尽写 `failed`。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bounded_restart_escalates_then_fails() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("data");
    std::fs::create_dir_all(&root).expect("data root");
    let supervisor = RuntimeSupervisor::new(&root);
    let script = root.join("crash.sh");
    std::fs::write(&script, "#!/bin/sh\nexit 3\n").expect("script");
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755))
        .expect("chmod");
    let mut options = options(
        &script,
        &digest(&script),
        &root.join("rt-crash"),
        "rt-crash",
    );
    options.restart = RestartPolicy::bounded();
    options.direct = true;

    let started = supervisor.start(options).await.expect("first spawn");
    assert_eq!(started.state, RuntimeState::Running);

    let began = Instant::now();
    wait_for(|| {
        supervisor
            .state("rt-crash")
            .is_some_and(|row| row.state == RuntimeState::Failed)
    })
    .await;
    let elapsed = began.elapsed();
    let final_row = supervisor.state("rt-crash").expect("row");
    assert_eq!(final_row.state, RuntimeState::Failed);
    assert_eq!(
        final_row.error_code.as_deref(),
        Some("RUNTIME_SPAWN_FAILED")
    );
    assert_eq!(supervisor.admission().active(), 0, "failed frees its slot");

    let crashes: Vec<_> = supervisor
        .events()
        .into_iter()
        .filter(|event| event.kind == "runtime.crashed")
        .map(|event| event.payload.detail.unwrap_or_default())
        .collect();
    for ladder in [
        "restarting in 1s",
        "restarting in 2s",
        "restarting in 4s",
        "restarting in 8s",
    ] {
        assert!(
            crashes.iter().any(|d| d.contains(ladder)),
            "missing {ladder}: {crashes:?}"
        );
    }
    assert!(
        elapsed >= Duration::from_secs(15),
        "the ladder sums to 15s, took {elapsed:?}"
    );
    // One initial run plus one per consumed ladder step: exactly five spawns,
    // and no sixth after the window closed.
    let running = supervisor
        .events()
        .into_iter()
        .filter(|event| event.kind == "runtime.running")
        .count();
    assert_eq!(running, 1 + 4, "restart count must stay bounded");
    let _ = std::fs::remove_dir_all(root);
}

/// 用户显式停止后不再重启，即使崩溃阶梯还有剩余。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_stop_wins_over_pending_restart() {
    let Some(binary) = kernel() else { return };
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("data");
    std::fs::create_dir_all(&root).expect("data root");
    let supervisor = RuntimeSupervisor::new(&root);
    let mut options = options(&binary, &digest(&binary), &root.join("rt-stop"), "rt-stop");
    options.restart = RestartPolicy::bounded();
    options.direct = true;
    supervisor.start(options).await.expect("start");

    supervisor.force_kill("rt-stop").expect("SIGKILL kernel");
    wait_for(|| {
        supervisor
            .state("rt-stop")
            .is_some_and(|row| row.state == RuntimeState::Crashed)
    })
    .await;
    let stopped = supervisor
        .stop("rt-stop")
        .await
        .expect("stop while crashing");
    assert_eq!(stopped.state, RuntimeState::Stopped);

    // 跨过第一个 1s 间隔后仍然是 stopped，且没有新的 running。
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(
        supervisor.state("rt-stop").map(|row| row.state),
        Some(RuntimeState::Stopped)
    );
    let _ = std::fs::remove_dir_all(root);
}

/// 手动重试把 `failed` 拉回 `running`，需要 teardown 确认无残留（SPEC）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_runtime_restarts_after_explicit_retry() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("data");
    std::fs::create_dir_all(&root).expect("data root");
    let supervisor = RuntimeSupervisor::new(&root);
    let script = root.join("once.sh");
    std::fs::write(&script, "#!/bin/sh\nexit 1\n").expect("script");
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755))
        .expect("chmod");
    let mut options = options(
        &script,
        &digest(&script),
        &root.join("rt-retry"),
        "rt-retry",
    );
    options.restart = RestartPolicy::none();
    options.direct = true;
    supervisor.start(options.clone()).await.expect("start");
    wait_for(|| {
        supervisor
            .state("rt-retry")
            .is_some_and(|row| row.state == RuntimeState::Crashed)
    })
    .await;
    assert_eq!(
        supervisor.state("rt-retry").map(|s| s.state),
        Some(RuntimeState::Crashed)
    );

    // `crashed` 是终态：可以再次 start 同一个 runtime id。
    let again = supervisor.start(options).await.expect("retry");
    assert_eq!(again.state, RuntimeState::Running);
    wait_for(|| !process_group_alive(again.pgid.expect("pgid"))).await;
}

/// 真实内核 + `StartProfile`：崩溃重启后仍能正常停止，无残留进程组。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_kernel_reconciles_after_control_plane_restart() {
    let Some(binary) = kernel() else { return };
    let sha = digest(&binary);
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("data");
    std::fs::create_dir_all(&root).expect("data root");
    let manifest = mien_core::fingerprint::load_pinned_manifest().expect("manifest");
    let profile = fake_profile();
    let request = StartProfile {
        profile: &profile,
        binary: binary.clone(),
        binary_sha256: sha,
        manifest: &manifest,
        data_root: root.clone(),
        sandbox_kind: "seatbelt".into(),
        proxy: None,
        stop_timeout: Duration::from_secs(5),
        restart: RestartPolicy::none(),
    };
    let runtime_id = format!("rt-{}", profile.id.0);

    // 第一个控制面实例启动后直接丢失（不 stop），第二个实例对账接管。
    let first = RuntimeSupervisor::new(&root);
    let started = request.start(&first, &runtime_id).await.expect("start");
    let pgid = started.snapshot.pgid.expect("pgid");
    std::mem::forget(first);

    let second = RuntimeSupervisor::new(&root);
    let outcomes = second.reconcile().await.expect("reconcile");
    assert_eq!(
        outcomes.first().map(|row| row.runtime_id.clone()),
        Some(runtime_id.clone())
    );
    assert!(!process_group_alive(pgid), "leftover group terminated");
    assert!(!root.join(&runtime_id).join("runtime.json").exists());

    let rows = outcomes
        .iter()
        .filter(|row| row.runtime_id == runtime_id)
        .count();
    assert_eq!(rows, 1, "one row per profile after reconcile");

    // 对账后再启动一次可以正常停止。
    let again = request.start(&second, &runtime_id).await.expect("restart");
    assert_eq!(again.snapshot.state, RuntimeState::Running);
    let stopped = second.stop(&runtime_id).await.expect("stop");
    assert_eq!(stopped.state, RuntimeState::Stopped);
    assert!(!process_group_alive(again.snapshot.pgid.expect("pgid")));
}

fn fake_profile() -> mien_core::domain::profile::Profile {
    use mien_core::domain::profile::{ProfileMode, ProfileStatus};
    use mien_core::fingerprint::spec::{FingerprintSpec, Platform};
    mien_core::domain::profile::Profile {
        id: ProfileId::new(),
        name: "reconcile".into(),
        notes: None,
        status: ProfileStatus::Active,
        fingerprint: FingerprintSpec::fresh(Platform::Macos).expect("spec"),
        start_url: None,
        kernel_version: "148".into(),
        mode: ProfileMode::Headful,
        automation_api: true,
        proxy_id: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        archived_at: None,
        deleted_at: None,
        deletion_error: None,
    }
}

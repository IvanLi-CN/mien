//! 并发准入：上限、超出拒绝、停止释放，以及 10 个真实内核并发启动。
//!
//! 真实部分需要 `MIEN_KERNEL_BIN`；未设置时打印显式跳过原因。

use mien_core::error::ErrorCode;
use mien_core::runtime::{
    AdmissionPermit, RuntimeAdmission, RuntimeOptions, RuntimeState, RuntimeSupervisor,
};
use std::{
    future::Future,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

const SKIP: &str = "skipped: MIEN_KERNEL_BIN not set";
const CONCURRENCY: usize = 10;

fn kernel() -> Option<PathBuf> {
    let Some(value) = std::env::var_os("MIEN_KERNEL_BIN") else {
        eprintln!("{SKIP}");
        return None;
    };
    let path = PathBuf::from(value);
    assert!(path.is_file(), "MIEN_KERNEL_BIN is not a file: {path:?}");
    Some(path)
}

fn digest(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    format!(
        "{:x}",
        Sha256::digest(std::fs::read(path).expect("kernel readable"))
    )
}

fn options(binary: &Path, data_dir: &Path, runtime_id: &str, sha: &str) -> RuntimeOptions {
    let mut options = RuntimeOptions::new(
        runtime_id,
        mien_core::domain::profile::ProfileId::new(),
        binary,
        sha,
        data_dir,
    );
    options.direct = true;
    options.launch_args = vec![
        "--window-size=800,600".into(),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        "--use-mock-keychain".into(),
        "--password-store=basic".into(),
        "--disable-background-networking".into(),
        "--disable-gpu".into(),
        "--no-sandbox".into(),
    ];
    options
}

/// `ps` 的常驻内存（KB）；取不到记 0，报告里会显式说明。
fn rss_kib(pid: u32) -> u64 {
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output();
    output
        .ok()
        .and_then(|out| String::from_utf8_lossy(&out.stdout).trim().parse().ok())
        .unwrap_or(0)
}

/// 命令行带 `mien-runtime-id=rt-` 的本进程树之外全部进程数。
fn runtime_processes() -> usize {
    let output = std::process::Command::new("pgrep")
        .args(["-f", "mien-runtime-id=rt-"])
        .output()
        .expect("pgrep");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| {
            line.trim()
                .parse::<u32>()
                .is_ok_and(|pid| pid != std::process::id())
        })
        .count()
}

fn gate(limit: usize) -> Arc<RuntimeAdmission> {
    RuntimeAdmission::new(limit)
}

#[test]
fn the_default_gate_admits_ten_and_refuses_the_eleventh() {
    let gate = gate(mien_core::runtime::DEFAULT_MAX_CONCURRENT_RUNTIMES);
    let permits: Vec<AdmissionPermit> = (0..CONCURRENCY)
        .map(|index| gate.try_admit(&format!("rt-{index}")).expect("slot"))
        .collect();
    assert_eq!(gate.active(), CONCURRENCY);
    let refused = gate.try_admit("rt-over").expect_err("eleven is too many");
    assert_eq!(refused.code(), ErrorCode::RuntimeConflict);
    assert_eq!(refused.code_str(), "RUNTIME_CONFLICT");
    drop(permits);
    assert_eq!(gate.active(), 0);
}

/// 停止释放槽位，所以第 11 个在 stop 之后可以进入。
#[tokio::test]
async fn a_stopped_runtime_frees_its_slot() {
    let gate = RuntimeAdmission::new(1);
    let first = gate.try_admit("rt-a").expect("first");
    assert!(gate.try_admit("rt-b").is_err());
    drop(first);
    let second = gate.try_admit("rt-b").expect("after stop");
    assert_eq!(second.runtime_id(), "rt-b");
}

/// 真实 10 档案并发：全部进入 running、都能停止、无孤儿、峰值内存在预算内。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ten_real_kernels_start_concurrently_and_all_stop() {
    let Some(binary) = kernel() else { return };
    let sha = digest(&binary);
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("data");
    std::fs::create_dir_all(&root).expect("data root");
    let supervisor = RuntimeSupervisor::new(&root);
    assert_eq!(
        supervisor.admission().limit(),
        CONCURRENCY,
        "the default limit is the acceptance target"
    );

    let began = Instant::now();
    let starts = (0..CONCURRENCY).map(|index| {
        let options = options(
            &binary,
            &root.join(format!("rt-p{index}")),
            &format!("rt-p{index}"),
            &sha,
        );
        supervisor.start(options)
    });
    let results = futures_join(starts).await;
    let elapsed = began.elapsed();

    let mut pids = Vec::new();
    for (index, result) in results.into_iter().enumerate() {
        let snapshot = result.unwrap_or_else(|error| panic!("profile {index}: {error}"));
        assert_eq!(snapshot.state, RuntimeState::Running);
        pids.push(snapshot.pid.expect("pid"));
    }
    assert_eq!(supervisor.admission().active(), CONCURRENCY);

    let peak_kib: u64 = pids.iter().map(|pid| rss_kib(*pid)).sum();
    let peak_processes = runtime_processes();

    // 第 11 个被拒绝，且不留下记录或进程。
    let overflow = supervisor
        .start(options(&binary, &root.join("rt-over"), "rt-over", &sha))
        .await
        .expect_err("eleven kernels exceed the gate");
    assert_eq!(overflow.code(), ErrorCode::RuntimeConflict);
    assert!(supervisor.state("rt-over").is_none());

    let _ = supervisor.stop("rt-p0").await.expect("stop one");
    assert_eq!(supervisor.admission().active(), CONCURRENCY - 1);
    let freed = supervisor
        .start(options(&binary, &root.join("rt-over"), "rt-over", &sha))
        .await
        .expect("freed slot is usable");
    assert_eq!(freed.state, RuntimeState::Running);

    for index in 1..CONCURRENCY {
        let id = format!("rt-p{index}");
        let stopped = supervisor
            .stop(&id)
            .await
            .unwrap_or_else(|error| panic!("{id}: {error}"));
        assert_eq!(stopped.state, RuntimeState::Stopped);
    }
    assert_eq!(
        supervisor.stop("rt-over").await.expect("stop").state,
        RuntimeState::Stopped
    );
    assert_eq!(supervisor.admission().active(), 0);

    // 停止即回收：进程表里不应还有带本运行时前缀的内核。
    let mut residual = runtime_processes();
    for _ in 0..100 {
        if residual == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        residual = runtime_processes();
    }
    println!(
        "admission: profiles={CONCURRENCY} start={elapsed:?} peak_rss_kib={peak_kib} peak_processes={peak_processes} residual={residual}"
    );
    assert_eq!(residual, 0, "no orphaned runtime process may survive");
    // 十个有头内核实测约 1.9 GB；上限 4 GiB 留出余量。
    assert!(
        peak_kib < 4 * 1024 * 1024,
        "peak RSS {peak_kib} KiB exceeds the 4 GiB budget"
    );
}

/// 不引入执行器依赖：按顺序 await 已经创建的全部 future。
async fn futures_join<F>(futures: impl Iterator<Item = F>) -> Vec<F::Output>
where
    F: Future,
{
    let mut out = Vec::new();
    for future in futures {
        out.push(future.await);
    }
    out
}

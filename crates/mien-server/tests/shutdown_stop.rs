//! 控制面优雅关闭：活动运行时经 `stopping` 停到 `stopped`，不再落成 `crashed`。
//!
//! 覆盖 `REQ-RUNTIME-012` 的关闭侧：`docker restart` 过去只让 `axum::serve`
//! 返回，内核继续跑、行停在 `running`，重启后的对账只能写 `crashed`。这里钉住
//! 关闭路径实际做了什么：
//!
//! - 每行状态 `stopped`（不是 `crashed`），`pid`/`pgid` 为 NULL，
//!   `last_known_isolation` 保留；
//! - 内核进程组确实不存在；
//! - 忽略 SIGTERM 的内核耗尽预算后**不**被写成 `stopped`，其余行照常停止，
//!   日志有 `warn`；
//! - 终态行不被改写。
//!
//! 假内核是 `/bin/sh` 循环脚本，`manifest_sandbox_kind = "none"` 声明了它无法
//! 沙箱（与 `reconcile.rs` 相同）：否则 Linux 上 `require_linux_isolation` 会
//! 拒绝启动。全程不读 `MIEN_KERNEL_BIN`，不会拉起真实内核。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mien_core::domain::profile::ProfileId;
use mien_core::runtime::probe::process_group_alive;
use mien_core::runtime::{RuntimeOptions, RuntimeState, RuntimeSupervisor};
use mien_server::shutdown::{ShutdownReport, stop_runtimes_unless};
use sqlx::SqlitePool;
use tempfile::TempDir;

/// 停止预算：足够三条 `/bin/sh` 在内核停止序列里退出，够不上真实的 5s 超时。
const BUDGET: Duration = Duration::from_secs(10);

/// 与真实服务一致的**多线程**运行时。
///
/// `mien_core` 的同步 gRPC 适配层会在已有运行时内另起线程并 `join`；单线程
/// 运行时被 `join` 阻塞后无法驱动连接任务（见 `proxies_api.rs`）。
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime")
}

/// 一个已迁移的数据根；关闭路径要读写 `runtimes`，所以每个用例都要真库。
///
/// 库放在 `<temp>/data/mien.db`，与 `AppState` 的 `<data_root>/mien.db` 布局
/// 一致：关闭路径会关掉池，重启后的读取必须能按同一路径重新打开。
fn data_root() -> (TempDir, SqlitePool) {
    let dir = TempDir::new().expect("temp dir");
    let root = dir.path().join("data");
    std::fs::create_dir_all(&root).expect("data root");
    let pool = mien_core::storage::open_and_migrate(&root.join("mien.db")).expect("migrate");
    (dir, pool)
}

fn sha256_hex(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    format!(
        "{:x}",
        Sha256::digest(std::fs::read(path).expect("kernel readable"))
    )
}

/// 假内核：循环睡住，收到 SIGTERM 正常退出。
///
/// `trap '' TERM` 版本只能靠进程组 SIGKILL 收场（预算耗尽的用例）。`trap`
/// 不是原子的：进程在 `exec` 之后、`trap` 执行之前收到 SIGTERM 仍会退出，而
/// 关闭路径在启动后几毫秒就可能发出第一个信号。所以脚本在装好 `trap` 之后写
/// 一个标记文件，测试必须等到它才发起关闭——否则用例会以“恰好抢在 trap 之前”
/// 的方式偶发通过。
fn fake_kernel(dir: &Path, name: &str, ignore_term: bool) -> PathBuf {
    let path = dir.join(name);
    let marker = dir.join(format!("{name}.ready"));
    let body = if ignore_term {
        format!(
            "#!/bin/sh\ntrap '' TERM\ntrap '' INT\n: >'{marker}'\nwhile :; do sleep 1; done\n",
            marker = marker.display()
        )
    } else {
        "#!/bin/sh\nwhile :; do sleep 1; done\n".to_owned()
    };
    std::fs::write(&path, body).expect("kernel script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    path
}

/// 等到假内核确认自己装好了信号处置。
fn wait_for_marker(kernel: &Path) {
    let marker = PathBuf::from(format!("{}.ready", kernel.display()));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !marker.exists() {
        assert!(
            Instant::now() < deadline,
            "the stub kernel never signalled readiness: {marker:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// 插入一条档案与一条 `running` 运行时行；关闭路径要能按 `profile_id` 找到它。
fn seed_row(
    rt: &tokio::runtime::Runtime,
    pool: &SqlitePool,
    profile_id: &ProfileId,
    isolation: &str,
) {
    rt.block_on(async {
        sqlx::query(
            "INSERT INTO profiles (id,name,fingerprint_json,fingerprint_schema,kernel_version,mode,status,automation_api,created_at,updated_at) \
             VALUES (?1,'shutdown','{}',1,'148','headful','active',0,'2026-01-01T00:00:00.000Z','2026-01-01T00:00:00.000Z')",
        )
        .bind(&profile_id.0)
        .execute(pool)
        .await
        .expect("profile row");
        sqlx::query(
            "INSERT INTO runtimes (id,profile_id,state,mode,isolation,last_known_isolation,pid,pgid,started_at,updated_at) \
             VALUES (?1,?2,'running','headful',?3,?3,1,1,'2026-01-01T00:00:00.000Z','2026-01-01T00:00:00.000Z')",
        )
        .bind(format!("rt-{}", profile_id.0))
        .bind(&profile_id.0)
        .bind(isolation)
        .execute(pool)
        .await
        .expect("runtime row");
    });
}

/// 启动一个受监督的假内核并把它写入 `runtimes`（相当于 HTTP `start` 的落库）。
fn start_runtime(
    rt: &tokio::runtime::Runtime,
    supervisor: &RuntimeSupervisor,
    pool: &SqlitePool,
    root: &Path,
    binary: &Path,
    profile_id: &ProfileId,
) -> u32 {
    let runtime_id = format!("rt-{}", profile_id.0);
    let data_dir = root.join(format!("profiles/{}", profile_id.0));
    std::fs::create_dir_all(&data_dir).expect("profile data dir");

    let mut options = RuntimeOptions::new(
        runtime_id.clone(),
        profile_id.clone(),
        binary,
        sha256_hex(binary),
        data_dir,
    );
    // 假内核声明自己无法沙箱；hips Linux 命名空间闸门因此放行，真实内核仍需
    // 证明自己的机制（见 7771958）。
    options.manifest_sandbox_kind = "none".into();
    options.launch_args = vec!["--no-first-run".into()];

    rt.block_on(supervisor.start(options))
        .expect("fake kernel starts");

    seed_row(rt, pool, profile_id, "unsandboxed");

    let snapshot = supervisor.state(&runtime_id).expect("runtime record");
    let pid = snapshot.pid.expect("kernel pid");
    // 行里的 pid/pgid 是启动事务写进去的真实值。
    rt.block_on(async {
        sqlx::query("UPDATE runtimes SET pid=?1, pgid=?2 WHERE profile_id=?3")
            .bind(i64::from(pid))
            .bind(i64::from(pid as i32))
            .bind(&profile_id.0)
            .execute(pool)
            .await
            .expect("runtime pid");
    });
    pid
}

/// `(state, pid, pgid, last_known_isolation, last_error)`。
type Row = (String, Option<i64>, Option<i64>, String, Option<String>);

/// 关闭路径会关掉服务自己的池，所以重启后的读取要另开一个句柄。
fn reopen(dir: &Path) -> SqlitePool {
    mien_core::storage::open_and_migrate(&dir.join("mien.db")).expect("reopen migrated database")
}

fn row(rt: &tokio::runtime::Runtime, pool: &SqlitePool, profile_id: &ProfileId) -> Row {
    rt.block_on(async {
            sqlx::query_as::<_, Row>(
                "SELECT state,pid,pgid,last_known_isolation,last_error FROM runtimes WHERE profile_id=?1",
            )
            .bind(&profile_id.0)
            .fetch_one(pool)
            .await
        })
        .expect("runtime row")
}

/// 收集 tracing 输出的内存 sink；关闭路径的 `warn` 必须可观测。
#[derive(Clone, Default)]
struct LogSink(Arc<Mutex<Vec<u8>>>);

impl LogSink {
    fn contents(&self) -> String {
        String::from_utf8(self.0.lock().expect("log sink").clone()).expect("utf-8 logs")
    }
}

struct LogWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("log sink").extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogSink {
    type Writer = LogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        LogWriter(Arc::clone(&self.0))
    }
}

/// 安装一次全局 sink；同一进程内的其他用例共享它，所以断言只针对本用例唯一
/// 的标记（`profile_id`）。
fn log_sink() -> &'static LogSink {
    static INIT: std::sync::Once = std::sync::Once::new();
    static SINK: std::sync::LazyLock<LogSink> = std::sync::LazyLock::new(LogSink::default);
    INIT.call_once(|| {
        let _ = tracing_subscriber::fmt()
            .with_writer(SINK.clone())
            .with_ansi(false)
            .with_target(false)
            .try_init();
    });
    &SINK
}

/// 等到内核真的消失。
///
/// 必须把监督器所在的运行时一起传进来：`tokio::process::Child` 的回收任务
/// 活在那个运行时里，运行时被 drop 之后没有人 `waitpid`，僵尸会被
/// `kill(pid, 0)` 一直判为存活。
fn wait_until_exit(rt: &tokio::runtime::Runtime, pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while mien_core::runtime::probe::process_alive(pid) && Instant::now() < deadline {
        rt.block_on(async { tokio::time::sleep(Duration::from_millis(20)).await });
    }
    // 监督器在停止序列里会回收子进程；这里只等进程表不再显示它。
    assert!(
        !mien_core::runtime::probe::process_alive(pid),
        "kernel {pid} must be gone after shutdown"
    );
}

/// 主用例：三条活动运行时全部经关闭路径停到 `stopped`，行与内核都收拾干净。
#[test]
fn shutdown_stops_every_active_runtime_and_writes_stopped() {
    let rt = runtime();
    let (dir, pool) = data_root();
    let root = dir.path().join("data");
    let supervisor = Arc::new(RuntimeSupervisor::new(&root));
    let kernel = fake_kernel(&root, "hold.sh", false);

    let profiles: Vec<ProfileId> = (0..3).map(|_| ProfileId::new()).collect();
    let pids: Vec<u32> = profiles
        .iter()
        .map(|profile_id| start_runtime(&rt, &supervisor, &pool, &root, &kernel, profile_id))
        .collect();
    for pid in &pids {
        assert!(
            process_group_alive(*pid as i32),
            "kernel {pid} must be running before shutdown"
        );
    }
    assert_eq!(supervisor.active_runtimes().len(), 3);

    supervisor.begin_shutdown();
    let report = rt.block_on(stop_runtimes_unless(
        &supervisor,
        Some(&pool),
        BUDGET,
        std::future::pending::<()>(),
    ));

    assert_eq!(report.attempted, 3, "every active runtime is attempted");
    assert!(
        report.is_complete(),
        "no runtime may be left unfinished: {report:?}"
    );
    assert!(!report.aborted);

    for (profile_id, pid) in profiles.iter().zip(&pids) {
        let (state, pid_column, pgid_column, isolation, _) = row(&rt, &pool, profile_id);
        assert_eq!(
            state, "stopped",
            "a graceful shutdown writes `stopped`, never `crashed`"
        );
        assert_eq!(pid_column, None, "a stopped row must not keep a pid");
        assert_eq!(pgid_column, None, "a stopped row must not keep a pgid");
        assert_eq!(
            isolation, "unsandboxed",
            "last_known_isolation is the last successful running verdict and must survive"
        );
        wait_until_exit(&rt, *pid);
        assert!(
            !process_group_alive(*pid as i32),
            "the kernel process group must be gone after a graceful shutdown"
        );
        let snapshot = supervisor
            .state(&format!("rt-{}", profile_id.0))
            .expect("runtime record");
        assert_eq!(snapshot.state, RuntimeState::Stopped);
    }
}

/// 一个内核忽略 SIGTERM：预算被它耗尽，但它**不得**被写成 `stopped`；同一轮
/// 里另外一个内核照常停在 `stopped`，日志里有可定位的 `warn`。
#[test]
fn shutdown_timeout_is_reported_truthfully_and_does_not_block_other_rows() {
    let rt = runtime();
    let sink = log_sink();
    let (dir, pool) = data_root();
    let root = dir.path().join("data");
    let supervisor = Arc::new(RuntimeSupervisor::new(&root));
    let stubborn = fake_kernel(&root, "ignore-term.sh", true);
    let cooperative = fake_kernel(&root, "hold.sh", false);

    let stuck_profile = ProfileId::new();
    let fine_profile = ProfileId::new();
    let stuck_pid = start_runtime(&rt, &supervisor, &pool, &root, &stubborn, &stuck_profile);
    let fine_pid = start_runtime(&rt, &supervisor, &pool, &root, &cooperative, &fine_profile);
    wait_for_marker(&stubborn);

    supervisor.begin_shutdown();
    // 单行停止序列是 5s 宽限 + 2s 强杀；预算只给 1s，让忽略 SIGTERM 的内核
    // 停不完。协作内核通常在同一预算内完成。
    let started = Instant::now();
    let report = rt.block_on(stop_runtimes_unless(
        &supervisor,
        Some(&pool),
        Duration::from_secs(1),
        std::future::pending::<()>(),
    ));
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(15),
        "shutdown must stay bounded by the budget, took {elapsed:?}"
    );

    assert!(
        report
            .unfinished
            .iter()
            .any(|(id, _)| id == &stuck_profile.0),
        "the SIGTERM-ignoring runtime must be reported unfinished: {report:?}"
    );
    let (stuck_state, _, _, stuck_isolation, _) = row(&rt, &pool, &stuck_profile);
    assert_ne!(
        stuck_state, "stopped",
        "a runtime that did not stop must never be written as `stopped`"
    );
    assert_eq!(
        stuck_isolation, "unsandboxed",
        "last_known_isolation survives a failed stop"
    );

    let (fine_state, fine_pid_column, fine_pgid_column, fine_isolation, _) =
        row(&rt, &pool, &fine_profile);
    assert_eq!(
        fine_state, "stopped",
        "one stuck runtime must not block another row"
    );
    assert_eq!(fine_pid_column, None);
    assert_eq!(fine_pgid_column, None);
    assert_eq!(fine_isolation, "unsandboxed");

    let logs = sink.contents();
    let stuck_warnings: Vec<&str> = logs
        .lines()
        .filter(|line| line.contains("WARN") && line.contains(&stuck_profile.0))
        .collect();
    assert!(
        !stuck_warnings.is_empty(),
        "the shutdown must log a warn naming the unfinished runtime {}",
        stuck_profile.0
    );

    // 清理：强杀残留进程组，避免污染同一台机器的后续用例。
    supervisor
        .force_kill(&format!("rt-{}", stuck_profile.0))
        .ok();
    wait_until_exit(&rt, stuck_pid);
    wait_until_exit(&rt, fine_pid);
}

/// 终态行不在活动集合里：关闭路径不重写 `stopped`/`crashed`/`failed`。
#[test]
fn shutdown_leaves_terminal_rows_alone() {
    let rt = runtime();
    let (dir, pool) = data_root();
    let root = dir.path().join("data");
    let supervisor = Arc::new(RuntimeSupervisor::new(&root));

    let profile_id = ProfileId::new();
    seed_row(&rt, &pool, &profile_id, "enforced");
    rt.block_on(async {
        sqlx::query(
            "UPDATE runtimes SET state='crashed', pid=NULL, pgid=NULL, last_error='RUNTIME_SPAWN_FAILED', \
             updated_at='2026-01-02T00:00:00.000Z' WHERE profile_id=?1",
        )
        .bind(&profile_id.0)
        .execute(&pool)
        .await
        .expect("terminal row");
    });
    let before = row(&rt, &pool, &profile_id);

    supervisor.begin_shutdown();
    let report = rt.block_on(stop_runtimes_unless(
        &supervisor,
        Some(&pool),
        BUDGET,
        std::future::pending::<()>(),
    ));
    assert_eq!(report.attempted, 0, "a terminal row is not active");
    assert!(report.is_complete());
    assert_eq!(
        row(&rt, &pool, &profile_id),
        before,
        "a terminal row must not be rewritten by shutdown"
    );
}

/// 关闭闸门落下后，启动请求被拒绝，也不会留下记录或进程。
#[test]
fn shutdown_gate_refuses_starts() {
    let rt = runtime();
    let (dir, _pool) = data_root();
    let root = dir.path().join("data");
    std::fs::create_dir_all(&root).expect("data root");
    let supervisor = RuntimeSupervisor::new(&root);
    let kernel = fake_kernel(&root, "hold.sh", false);
    let profile_id = ProfileId::new();
    let data_dir = root.join(format!("profiles/{}", profile_id.0));
    std::fs::create_dir_all(&data_dir).expect("profile data dir");

    // 先证明同一份选项在闸门落下前可以启动，排除“失败因为参数不对”。
    let mut options = RuntimeOptions::new(
        "rt-gate".to_owned(),
        profile_id.clone(),
        &kernel,
        sha256_hex(&kernel),
        data_dir.clone(),
    );
    options.manifest_sandbox_kind = "none".into();
    let started = rt
        .block_on(supervisor.start(options.clone()))
        .expect("gate is open before shutdown");
    let pid = started.pid.expect("kernel pid");

    supervisor.begin_shutdown();
    let mut second = options;
    second.runtime_id = "rt-gate-2".to_owned();
    let error = rt
        .block_on(supervisor.start(second))
        .expect_err("starts are refused after the gate closes");
    assert_eq!(error.code_str(), "RUNTIME_CONFLICT");
    assert!(
        supervisor.state("rt-gate-2").is_none(),
        "a refused start must not create a runtime record"
    );

    // 收尾：把闸门已经拦住的那条记录停掉并清掉进程。
    rt.block_on(supervisor.stop("rt-gate")).ok();
    wait_until_exit(&rt, pid);
}

/// 报告结构本身：空集合、`is_complete` 与 `aborted` 各自独立可读。
#[test]
fn shutdown_report_distinguishes_clean_and_incomplete_runs() {
    let clean = ShutdownReport {
        attempted: 1,
        stopped: vec!["p".into()],
        ..ShutdownReport::default()
    };
    assert!(clean.is_complete());
    let dirty = ShutdownReport {
        attempted: 1,
        unfinished: vec![("p".into(), "stopping")],
        aborted: true,
        ..ShutdownReport::default()
    };
    assert!(!dirty.is_complete());
}

/// 进程组存活的判定在用例间可复用；这里只钉住它对新进程组返回 true。
#[test]
fn probe_sees_a_fresh_process_group() {
    let dir = TempDir::new().expect("temp dir");
    let kernel = fake_kernel(dir.path(), "probe.sh", false);
    let mut command = std::process::Command::new(&kernel);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("probe child");
    let pid = child.id();
    assert!(process_group_alive(pid as i32));
    let _ = child.kill();
    let _ = child.wait();
}

/// 生产关闭路径的端到端用例：`serve_with_shutdown` 返回时，行已是 `stopped`。
///
/// 这条用例走的是与 `main` 相同的顺序（闸门 → 排空 → 停止 → 落库 → 关池），
/// 而不只是停止函数本身：`docker restart` 修的正是这条路径。
#[test]
fn serve_shutdown_path_writes_stopped_and_stops_the_kernel() {
    let rt = runtime();
    let (dir, pool) = data_root();
    let root = dir.path().join("data");
    let kernel = fake_kernel(&root, "hold.sh", false);
    let profile_id = ProfileId::new();

    let state = mien_server::state::AppState::new(&root, Some(pool.clone()));
    let supervisor = state.supervisor().clone();
    start_runtime(&rt, &supervisor, &pool, &root, &kernel, &profile_id);
    let pid = supervisor
        .state(&format!("rt-{}", profile_id.0))
        .and_then(|snapshot| snapshot.pid)
        .expect("kernel pid");

    let listener = rt
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .expect("listener");
    let address = listener.local_addr().expect("local addr");

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let serving = rt.block_on(async move {
        let serve = mien_server::serve_with_shutdown(
            listener,
            state,
            Duration::from_secs(10),
            async move {
                let _ = shutdown_rx.await;
            },
            std::future::pending::<()>(),
        );
        // 服务必须先真的接上连接，才值得触发关闭。
        let health = tokio::spawn(async move {
            for _ in 0..200 {
                if tokio::net::TcpStream::connect(address).await.is_ok() {
                    return true;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            false
        });
        assert!(health.await.expect("health probe"), "server never listened");
        shutdown_tx.send(()).expect("shutdown trigger");
        serve.await
    });
    assert!(serving.is_ok(), "serve must return cleanly: {serving:?}");
    drop(pool);

    // 关闭已释放服务自己的池；重启后的读取另开句柄，读到的就是落盘结果。
    let reloaded = reopen(&root);
    let (row_state, pid_column, pgid_column, isolation, _) = row(&rt, &reloaded, &profile_id);
    assert_eq!(
        row_state, "stopped",
        "the production shutdown path writes `stopped`, not `crashed`"
    );
    assert_eq!(pid_column, None);
    assert_eq!(pgid_column, None);
    assert_eq!(isolation, "unsandboxed");
    wait_until_exit(&rt, pid);
    assert!(!process_group_alive(pid as i32));
    let (row_state, ..) = row(&rt, &reloaded, &profile_id);
    assert_eq!(
        row_state, "stopped",
        "the persisted rows survive a fresh handle: nothing is left for boot reconciliation"
    );
}

/// 第二次信号放弃停止遍历：进程在预算内返回，行保留真实状态而不是假 `stopped`。
#[test]
fn second_signal_abandons_the_sweep_without_faking_stopped() {
    let rt = runtime();
    let (dir, pool) = data_root();
    let root = dir.path().join("data");
    let kernel = fake_kernel(&root, "ignore-term.sh", true);
    let profile_id = ProfileId::new();

    let state = mien_server::state::AppState::new(&root, Some(pool.clone()));
    let supervisor = state.supervisor().clone();
    let pid = start_runtime(&rt, &supervisor, &pool, &root, &kernel, &profile_id);
    wait_for_marker(&kernel);

    let listener = rt
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .expect("listener");

    let started = Instant::now();
    let serving = rt.block_on(async {
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let (interrupt_tx, interrupt_rx) = tokio::sync::oneshot::channel::<()>();
        let serve = tokio::spawn(mien_server::serve_with_shutdown(
            listener,
            state,
            // 预算远大于第二次信号的等待时间：返回速度只能由中断解释。
            Duration::from_secs(60),
            async move {
                let _ = shutdown_rx.await;
            },
            async move {
                let _ = interrupt_rx.await;
            },
        ));
        // 第一次信号立刻到达；第二次在停止序列开始后 200ms 到达。
        shutdown_tx.send(()).expect("shutdown trigger");
        tokio::time::sleep(Duration::from_millis(200)).await;
        interrupt_tx.send(()).expect("interrupt trigger");
        serve.await.expect("serve task")
    });
    let elapsed = started.elapsed();
    assert!(
        serving.is_ok() && elapsed < Duration::from_secs(20),
        "a second signal must end the sweep early, took {elapsed:?}"
    );
    drop(pool);

    let reloaded = reopen(&root);
    let (row_state, _, _, isolation, _) = row(&rt, &reloaded, &profile_id);
    assert_ne!(
        row_state, "stopped",
        "an abandoned sweep must never claim the runtime stopped"
    );
    assert_eq!(
        isolation, "unsandboxed",
        "last_known_isolation survives an abandoned sweep"
    );

    // 收尾：这条内核忽略 SIGTERM，只能强杀，避免影响同机后续用例。
    supervisor.force_kill(&format!("rt-{}", profile_id.0)).ok();
    wait_until_exit(&rt, pid);
}

/// 真实信号用例：向服务进程发 SIGTERM，它必须走完关闭序列。
///
/// 这条路径曾经静默失败：信号任务用 `send(*counts.borrow() + 1)` 更新计数，读
/// 守卫跨过写锁造成自死锁；`tokio::signal` 已接管 SIGTERM，进程于是既不退出也
/// 不记录，最终只能由容器宽限期 SIGKILL。只测 `serve_with_shutdown` 的一次性
/// 触发器发现不了它，所以这里发真实信号。
///
/// 信号是进程级的，和同一测试二进制里的其他用例共享会互相干扰，所以服务跑在
/// 独立子进程里：子进程自己开库、启动假内核、再进入 `serve_with_signals`；父进程
/// 等到就绪标记后 `kill -TERM`，再断言内核消失且行已落 `stopped`。
#[test]
fn real_sigterm_drives_the_whole_shutdown_path() {
    if std::env::var_os("MIEN_T33_SIGNAL_CHILD").is_some() {
        signal_child();
        return;
    }

    let rt = runtime();
    let (dir, pool) = data_root();
    let root = dir.path().join("data");
    let kernel = fake_kernel(&root, "hold.sh", false);
    let profile_id = ProfileId::new();
    // 子进程自己开库；父进程先放开句柄，避免两个连接同时写同一条测试行。
    drop(pool);

    let exe = std::env::current_exe().expect("test binary");
    let mut child = std::process::Command::new(exe)
        .args([
            "--exact",
            "real_sigterm_drives_the_whole_shutdown_path",
            "--nocapture",
        ])
        .env("MIEN_T33_SIGNAL_CHILD", "1")
        .env("MIEN_T33_ROOT", &root)
        .env("MIEN_T33_KERNEL", &kernel)
        .env("MIEN_T33_PROFILE", &profile_id.0)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("signal child");

    let (address, child_pid) = wait_for_child_ready(&mut child);
    assert!(
        address.starts_with("127.0.0.1:"),
        "child address: {address}"
    );

    // 发送真实 SIGTERM。用 `kill(1)` 而不是新增 `libc` 依赖：这里只需一个信号。
    let signal = std::process::Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .expect("kill(1)");
    assert!(signal.success(), "kill -TERM {} failed", child.id());

    let output = child.wait_with_output().expect("child output");
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        output.status.success(),
        "the signal child must exit cleanly, status={:?}\nstderr:\n{stderr}",
        output.status
    );
    assert!(
        !process_group_alive(child_pid as i32),
        "the kernel process group must be gone after SIGTERM\nstderr:\n{stderr}"
    );

    let reloaded = reopen(&root);
    let (state, pid_column, pgid_column, isolation, _) = row(&rt, &reloaded, &profile_id);
    assert_eq!(
        state, "stopped",
        "a real SIGTERM must persist `stopped`, not leave the row to boot reconciliation"
    );
    assert_eq!(pid_column, None);
    assert_eq!(pgid_column, None);
    assert_eq!(isolation, "unsandboxed");
}

/// 子进程：开库、启动假内核、打印就绪标记，然后等待真实信号。
///
/// 不装 tracing subscriber（那是 `main` 的职责）：父进程断言的是落库结果与进程
/// 状态，不是日志文本。
fn signal_child() {
    use std::io::Write;

    let rt = runtime();
    let root = PathBuf::from(std::env::var("MIEN_T33_ROOT").expect("root"));
    let kernel = PathBuf::from(std::env::var("MIEN_T33_KERNEL").expect("kernel"));
    let profile_id = ProfileId(std::env::var("MIEN_T33_PROFILE").expect("profile"));
    let pool = mien_core::storage::open_and_migrate(&root.join("mien.db")).expect("migrate");
    let state = mien_server::state::AppState::new(&root, Some(pool.clone()));
    let supervisor = state.supervisor().clone();
    let pid = start_runtime(&rt, &supervisor, &pool, &root, &kernel, &profile_id);

    let served = rt.block_on(async {
        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => listener,
            Err(error) => {
                eprintln!("child bind error: {error}");
                std::process::exit(1);
            }
        };
        let address = listener.local_addr().expect("local addr");
        eprintln!("child ready address={address} kernel_pid={pid}");
        std::io::stderr().flush().ok();
        mien_server::serve_with_signals(listener, state, Duration::from_secs(10)).await
    });
    match served {
        Ok(()) => std::process::exit(0),
        Err(error) => {
            eprintln!("child serve error: {error}");
            std::process::exit(1);
        }
    }
}

/// 读子进程 stderr 直到出现就绪标记，返回监听地址与内核 pid。
fn wait_for_child_ready(child: &mut std::process::Child) -> (String, u32) {
    use std::io::{BufRead, BufReader};
    let stderr = child.stderr.take().expect("child stderr");
    let mut reader = BufReader::new(stderr);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => panic!("child exited before it was ready"),
            Ok(_) => {
                let Some(rest) = line.strip_prefix("child ready ") else {
                    eprintln!("child: {}", line.trim_end());
                    continue;
                };
                let fields: std::collections::HashMap<&str, &str> = rest
                    .split_whitespace()
                    .filter_map(|field| field.split_once('='))
                    .collect();
                let address = fields.get("address").expect("address").to_string();
                let pid: u32 = fields
                    .get("kernel_pid")
                    .expect("kernel_pid")
                    .parse()
                    .expect("kernel pid");
                // 其余输出转发到测试 stderr，失败时可见。
                std::thread::spawn(move || {
                    let mut rest = String::new();
                    let _ = std::io::Read::read_to_string(&mut reader, &mut rest);
                    if !rest.trim().is_empty() {
                        eprintln!("{rest}");
                    }
                });
                return (address, pid);
            }
            Err(error) => panic!("child stderr: {error}"),
        }
    }
}

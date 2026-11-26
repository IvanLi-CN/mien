//! 运行模式矩阵：`headful` / `headless` / `pseudo_headful`。
//!
//! 真实启动需要解包内核；`MIEN_KERNEL_BIN` 未设置时打印显式跳过原因并返回，
//! 不允许静默通过。`pseudo_headful` 在负数清单下必须拒绝启动，这条路径不需要内核。
//!
//! 断言的三件事：
//! - argv 差异只来自模式：`--headless=new` 出现在 headless 与 pseudo_headful；
//! - 响应字段：`mode`、`ua_scope`（仅 pseudo_headful 为 `chrome`）、`reduced_assurance`；
//! - 能力缺失时 `RUNTIME_MODE_UNSUPPORTED`，且没有创建运行时记录或进程。

use std::path::{Path, PathBuf};
use std::time::Duration;

use mien_core::ErrorCode;
use mien_core::domain::profile::{Profile, ProfileId, ProfileMode, ProfileStatus};
use mien_core::fingerprint::spec::{FingerprintSpec, Platform};
use mien_core::fingerprint::{CapabilityManifest, load_pinned_manifest};
use mien_core::runtime::state::reduced_assurance;
use mien_core::runtime::{Isolation, RuntimeState, RuntimeSupervisor, StartProfile};
use sha2::{Digest, Sha256};

/// 未配置内核时必须打印的跳过原因。
const SKIP_MESSAGE: &str = "skipped: MIEN_KERNEL_BIN not set";
const HEADLESS_ARG: &str = "--headless=new";
const PSEUDO_LIMITATION: &str =
    "pseudo_headful changes UA only; other Headless characteristics remain";

fn kernel_from(value: Option<std::ffi::OsString>) -> Result<PathBuf, &'static str> {
    match value {
        Some(path) => {
            let path = PathBuf::from(path);
            assert!(path.is_file(), "MIEN_KERNEL_BIN is not a file: {path:?}");
            Ok(path)
        }
        None => Err(SKIP_MESSAGE),
    }
}

fn kernel() -> Option<PathBuf> {
    match kernel_from(std::env::var_os("MIEN_KERNEL_BIN")) {
        Ok(path) => Some(path),
        Err(skip) => {
            println!("{skip}");
            None
        }
    }
}

fn sha256(path: &Path) -> String {
    let bytes = std::fs::read(path).expect("kernel readable");
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn profile(mode: ProfileMode) -> Profile {
    Profile {
        id: ProfileId::new(),
        name: format!("mode-{}", mode.as_str()),
        status: ProfileStatus::Active,
        fingerprint: FingerprintSpec::fresh(Platform::Macos).expect("fresh spec"),
        start_url: None,
        notes: None,
        kernel_version: "148".into(),
        mode,
        automation_api: true,
        proxy_id: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        archived_at: None,
        deleted_at: None,
        deletion_error: None,
    }
}

/// 内核真实命令行；argv 断言以此为准，而不是只看计划。
fn process_command(pid: u32) -> String {
    let output = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()
        .expect("ps");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    supervisor: RuntimeSupervisor,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("data");
        std::fs::create_dir_all(&root).expect("data root");
        Self {
            supervisor: RuntimeSupervisor::new(&root),
            root,
            _dir: dir,
        }
    }

    async fn start(
        &self,
        profile: &Profile,
        binary: &Path,
        digest: &str,
    ) -> mien_core::runtime::StartedProfile {
        let manifest = load_pinned_manifest().expect("pinned manifest");
        let request = StartProfile {
            profile,
            binary: binary.to_path_buf(),
            binary_sha256: digest.to_owned(),
            manifest: &manifest,
            data_root: self.root.clone(),
            sandbox_kind: "seatbelt".into(),
            proxy: None,
            stop_timeout: Duration::from_secs(5),
        };
        request
            .start(&self.supervisor, &format!("rt-{}", profile.id.0))
            .await
            .unwrap_or_else(|error| panic!("{} start failed: {error}", profile.mode.as_str()))
    }
}

/// 断言失败也不能留下孤儿内核：直接 SIGKILL 进程组，同步、无嵌套 runtime。
struct KillOnDrop<'a> {
    supervisor: &'a RuntimeSupervisor,
    runtime_id: String,
}

impl Drop for KillOnDrop<'_> {
    fn drop(&mut self) {
        self.supervisor.force_kill(&self.runtime_id).ok();
    }
}

#[test]
fn unset_kernel_env_reports_explicit_skip() {
    assert_eq!(
        kernel_from(None).unwrap_err(),
        SKIP_MESSAGE,
        "a missing kernel must produce the explicit skip text, not a silent pass"
    );
}

/// 三种模式各启动一次：argv 差异、响应字段、事件 `ua_scope` 范围。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn three_modes_launch_with_explicit_guarantees() {
    let Some(binary) = kernel() else { return };
    assert_eq!(std::env::consts::OS, "macos", "the slice is macOS-only");
    let digest = sha256(&binary);
    let fixture = Fixture::new();

    for mode in [
        ProfileMode::Headful,
        ProfileMode::Headless,
        ProfileMode::PseudoHeadful,
    ] {
        let profile = profile(mode);
        let runtime_id = format!("rt-{}", profile.id.0);
        let started = fixture.start(&profile, &binary, &digest).await;
        let _guard = KillOnDrop {
            supervisor: &fixture.supervisor,
            runtime_id: runtime_id.clone(),
        };

        // 响应字段：模式不被改写，UA 范围只在 pseudo_headful 存在。
        assert_eq!(started.mode, mode);
        assert_eq!(started.snapshot.mode, mode);
        assert_eq!(started.launch.mode, mode);
        let expected_scope = (mode == ProfileMode::PseudoHeadful).then_some("chrome");
        assert_eq!(
            started.ua_scope,
            expected_scope,
            "{} ua_scope",
            mode.as_str()
        );
        assert_eq!(started.snapshot.ua_scope.as_deref(), expected_scope);

        // 计划 argv 与真实命令行都必须一致地包含/不含 headless 参数。
        let plan_has_headless = started.launch.argv.iter().any(|arg| arg == HEADLESS_ARG);
        assert_eq!(
            plan_has_headless,
            mode != ProfileMode::Headful,
            "{} plan argv: {:?}",
            mode.as_str(),
            started.launch.argv
        );
        let pid = started.snapshot.pid.expect("running pid");
        let command = process_command(pid);
        assert!(
            command.contains(&binary.to_string_lossy().to_string()),
            "unexpected kernel: {command}"
        );
        assert_eq!(
            command.split_whitespace().any(|arg| arg == HEADLESS_ARG),
            mode != ProfileMode::Headful,
            "{} process argv: {command}",
            mode.as_str()
        );

        // 宿主交互必须被压住：任何模式都不得弹钥匙串或默认浏览器对话框。
        for flag in mien_core::runtime::supervisor::LAUNCHER_FLAGS {
            assert!(
                command.split_whitespace().any(|arg| arg == *flag),
                "{} process argv is missing {flag}: {command}",
                mode.as_str()
            );
        }

        // 降级保证与限制说明：headful 只有 direct 一条，其余模式各有模式声明。
        assert!(
            started.reduced_assurance,
            "{} is reduced assurance",
            mode.as_str()
        );
        assert_eq!(
            started.snapshot.reduced_assurance,
            mode != ProfileMode::Headful
                || started.snapshot.direct
                || started.snapshot.isolation != Isolation::Enforced
        );
        match mode {
            ProfileMode::Headful => {
                assert!(
                    !started
                        .disclosures
                        .iter()
                        .any(|line| line.contains("Headless")),
                    "headful must not claim Headless limits: {:?}",
                    started.disclosures
                );
            }
            ProfileMode::Headless => {
                assert!(
                    started
                        .disclosures
                        .iter()
                        .any(|line| line.contains("no user-agent claim")),
                    "headless must disclaim UA: {:?}",
                    started.disclosures
                );
                assert!(
                    !started
                        .disclosures
                        .iter()
                        .any(|line| line == PSEUDO_LIMITATION),
                    "headless must not borrow the pseudo_headful claim"
                );
            }
            ProfileMode::PseudoHeadful => {
                assert!(
                    started
                        .disclosures
                        .iter()
                        .any(|line| line == PSEUDO_LIMITATION),
                    "pseudo_headful must keep the other-Headless-characteristics note: {:?}",
                    started.disclosures
                );
            }
        }

        // 事件 payload 的 ua_scope 只在 pseudo_headful 非空。
        let events: Vec<_> = fixture
            .supervisor
            .events()
            .into_iter()
            .filter(|event| event.payload.runtime_id == runtime_id)
            .collect();
        assert!(!events.is_empty(), "{} emitted no events", mode.as_str());
        let running = events
            .iter()
            .find(|event| event.kind == "runtime.running")
            .unwrap_or_else(|| panic!("{} emitted no runtime.running", mode.as_str()));
        assert_eq!(running.payload.mode, mode);
        assert_eq!(running.payload.ua_scope.as_deref(), expected_scope);
        assert!(running.payload.reduced_assurance);
        for event in &events {
            assert_eq!(
                event.payload.ua_scope.as_deref(),
                expected_scope,
                "{} event {} ua_scope",
                mode.as_str(),
                event.kind
            );
        }

        println!(
            "mode={} pid={pid} ua_scope={:?} reduced_assurance={} disclosures={:?} argv_tail={:?}",
            mode.as_str(),
            started.ua_scope,
            started.reduced_assurance,
            started.disclosures,
            &started.launch.argv[started.launch.argv.len().saturating_sub(3)..]
        );

        let stopped = fixture.supervisor.stop(&runtime_id).await.expect("stop");
        assert_eq!(stopped.state, RuntimeState::Stopped);
    }
}

/// `pseudo_headful` 在 `headless_chrome_ua_patch=false` 的临时清单下必须拒绝启动。
#[tokio::test]
async fn pseudo_headful_is_refused_without_the_ua_patch_capability() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut manifest = load_pinned_manifest().expect("pinned manifest").clone();
    assert!(
        manifest.headless_chrome_ua_patch,
        "the pinned manifest must declare the capability, else this test is vacuous"
    );
    manifest.headless_chrome_ua_patch = false;
    let path = dir.path().join("manifest-without-ua-patch.json");
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&manifest).expect("manifest json"),
    )
    .expect("write manifest");
    let negative = CapabilityManifest::from_path(&path).expect("negative manifest");
    assert!(!negative.headless_chrome_ua_patch);

    let root = dir.path().join("data");
    std::fs::create_dir_all(&root).expect("data root");
    let supervisor = RuntimeSupervisor::new(&root);
    let profile = profile(ProfileMode::PseudoHeadful);
    let runtime_id = format!("rt-{}", profile.id.0);
    // 故意使用不存在的可执行文件：拒绝必须发生在 sha256 校验与 spawn 之前。
    let request = StartProfile {
        profile: &profile,
        binary: dir.path().join("no-such-kernel"),
        binary_sha256: "0".repeat(64),
        manifest: &negative,
        data_root: root.clone(),
        sandbox_kind: "seatbelt".into(),
        proxy: None,
        stop_timeout: Duration::from_secs(1),
    };
    let error = request
        .start(&supervisor, &runtime_id)
        .await
        .expect_err("pseudo_headful without the UA patch must not start");
    println!("refused: code={} detail={error}", error.code_str());
    assert_eq!(error.code(), ErrorCode::RuntimeModeUnsupported);
    assert_eq!(error.code_str(), "RUNTIME_MODE_UNSUPPORTED");
    assert!(
        supervisor.state(&runtime_id).is_none(),
        "a refused mode must not create a runtime record"
    );
    assert!(
        supervisor.events().is_empty(),
        "a refused mode emits no events"
    );
}

/// `reduced_assurance` 真值表：18 个 (mode, direct, isolation) 组合全量枚举。
#[test]
fn reduced_assurance_truth_table() {
    let cases = [
        (ProfileMode::Headful, false, Isolation::Enforced, false),
        (ProfileMode::Headful, false, Isolation::Unsandboxed, true),
        (ProfileMode::Headful, false, Isolation::Unknown, true),
        (ProfileMode::Headful, true, Isolation::Enforced, true),
        (ProfileMode::Headful, true, Isolation::Unsandboxed, true),
        (ProfileMode::Headful, true, Isolation::Unknown, true),
        (ProfileMode::Headless, false, Isolation::Enforced, true),
        (ProfileMode::Headless, false, Isolation::Unsandboxed, true),
        (ProfileMode::Headless, false, Isolation::Unknown, true),
        (ProfileMode::Headless, true, Isolation::Enforced, true),
        (ProfileMode::Headless, true, Isolation::Unsandboxed, true),
        (ProfileMode::Headless, true, Isolation::Unknown, true),
        (ProfileMode::PseudoHeadful, false, Isolation::Enforced, true),
        (
            ProfileMode::PseudoHeadful,
            false,
            Isolation::Unsandboxed,
            true,
        ),
        (ProfileMode::PseudoHeadful, false, Isolation::Unknown, true),
        (ProfileMode::PseudoHeadful, true, Isolation::Enforced, true),
        (
            ProfileMode::PseudoHeadful,
            true,
            Isolation::Unsandboxed,
            true,
        ),
        (ProfileMode::PseudoHeadful, true, Isolation::Unknown, true),
    ];
    for (mode, direct, isolation, expected) in cases {
        assert_eq!(
            reduced_assurance(mode, direct, isolation),
            expected,
            "mode={} direct={direct} isolation={isolation:?}",
            mode.as_str()
        );
        // 与 SPEC 的反向等价：false 只能在三者都未降级时出现。
        assert_eq!(
            !expected,
            mode == ProfileMode::Headful && !direct && isolation == Isolation::Enforced
        );
    }
}

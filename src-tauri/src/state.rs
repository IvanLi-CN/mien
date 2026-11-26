//! 宿主状态：数据根、SQLite 仓储、运行时监督器与内核解析。
//!
//! 这里只依赖 `mien-core` 与 serde，不依赖 Tauri；`commands.rs` 是唯一适配层，
//! 因此本文件可以按 `#[path]` 直接编入宿主测试。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mien_core::CoreError;
use mien_core::domain::profile::{
    CreateProfile, Profile, ProfileId, ProfileMode, ProfilePage, ProfilePatch, ProfileQuery,
    ProfileStatus,
};
use mien_core::error::ErrorCode;
use mien_core::fingerprint::spec::{FingerprintSpec, Network, Platform, Spoof, Webrtc, Window};
use mien_core::fingerprint::{CapabilityManifest, load_pinned_manifest};
use mien_core::ports::proxy::{ProxyId, ProxyKind, ProxyRepository};
use mien_core::ports::repository::ProfileRepository;
use mien_core::proxy::api::GrpcXrayApi;
use mien_core::proxy::attach::GatewayAttach;
use mien_core::proxy::engine::{EngineBinary, EngineController, EngineProcess};
use mien_core::proxy::keys::{
    Credentials, MuxSettings, OutboundSpec, ProxyConfig, ProxyProtocol, TlsSettings, Transport,
    build_outbound,
};
use mien_core::runtime::{
    Isolation, RestartPolicy, RuntimeSnapshot, RuntimeState, RuntimeSupervisor, StartProfile,
};
use mien_core::secrets::MasterKey;
use mien_core::storage::{
    open_and_migrate, proxy::SqliteProxyRepository, sqlite::SqliteProfileRepository,
};
use serde::{Deserialize, Serialize};

/// 内核可执行文件相对安装目录的候选位置（macOS 解包布局优先）。
const KERNEL_NAMES: [&str; 3] = ["Chromium.app/Contents/MacOS/Chromium", "Chromium", "chrome"];

/// 代理种类 → 引擎协议。清单声明为 Tier 之外的种类在这里被拒绝，不回退直连。
fn protocol_for(kind: ProxyKind) -> Result<ProxyProtocol, CoreError> {
    match kind {
        ProxyKind::Vless => Ok(ProxyProtocol::Vless),
        ProxyKind::Vmess => Ok(ProxyProtocol::Vmess),
        ProxyKind::Shadowsocks => Ok(ProxyProtocol::Shadowsocks),
        ProxyKind::Trojan => Ok(ProxyProtocol::Trojan),
        ProxyKind::Hysteria2 => Ok(ProxyProtocol::Hysteria2),
        ProxyKind::Wireguard => Ok(ProxyProtocol::Wireguard),
        ProxyKind::Http => Ok(ProxyProtocol::Http),
        ProxyKind::Https => Ok(ProxyProtocol::Https),
        ProxyKind::Socks5 => Ok(ProxyProtocol::Socks5),
    }
}

/// 引擎可执行文件：`MIEN_XRAY_BIN`（验收）优先，否则按安装目录解析；摘要缺失即空串。
fn xray_bin(data_root: &Path) -> (PathBuf, String) {
    let path = std::env::var_os("MIEN_XRAY_BIN")
        .map_or_else(|| data_root.join("xray").join("xray"), PathBuf::from);
    let sha256 = mien_core::kernel::registry::binary_sha256(&path).unwrap_or_default();
    (path, sha256)
}

/// `MIEN_RESTART=0` 关闭崩溃自动重启；默认开启有界阶梯。
fn restart_policy() -> RestartPolicy {
    match std::env::var("MIEN_RESTART").as_deref() {
        Ok("0") => RestartPolicy::none(),
        _ => RestartPolicy::bounded(),
    }
}

fn kernel_missing() -> CoreError {
    CoreError::validation(ErrorCode::KernelNotFound, "kernel executable is missing")
}

fn runtime_id(profile_id: &str) -> String {
    format!("rt-{profile_id}")
}

/// 内核平台片段，与安装目录 `kernels/<version>/<platform>/` 一致。
const fn platform_triplet() -> &'static str {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "macos-arm64"
    } else if cfg!(target_os = "macos") {
        "macos-x86_64"
    } else {
        "linux-x86_64"
    }
}

/// 通过校验的内核可执行文件：路径与其当前摘要。
#[derive(Debug, Clone)]
pub struct KernelBinary {
    pub path: PathBuf,
    pub sha256: String,
}

/// 指纹输入（`fingerprint-input.schema`）：不含 seed，seed 由宿主持有。
#[derive(Debug, Clone, Deserialize)]
pub struct FingerprintInput {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub seed: Option<u32>,
    pub platform: Platform,
    pub platform_version: String,
    pub brand: String,
    pub brand_version: String,
    pub hardware_concurrency: u32,
    pub languages: Vec<String>,
    pub timezone: String,
    pub window: Window,
    #[serde(default)]
    pub spoof: Option<Spoof>,
    #[serde(default)]
    pub webrtc: Option<Webrtc>,
    #[serde(default)]
    pub network: Option<Network>,
}

impl FingerprintInput {
    /// `seed_override` 优先（更新时保留已存 seed）；都不存在时生成新 seed。
    fn into_spec(self, seed_override: Option<u32>) -> Result<FingerprintSpec, CoreError> {
        let seed = match seed_override.or(self.seed) {
            Some(seed) if seed != 0 => seed,
            _ => mien_core::fingerprint::spec::generate_seed()?,
        };
        Ok(FingerprintSpec {
            schema_version: self.schema_version,
            seed,
            platform: self.platform,
            platform_version: self.platform_version,
            brand: self.brand,
            brand_version: self.brand_version,
            hardware_concurrency: self.hardware_concurrency,
            languages: self.languages,
            timezone: self.timezone,
            window: self.window,
            spoof: self.spoof,
            webrtc: self.webrtc.unwrap_or(Webrtc {
                disable_non_proxied_udp: true,
            }),
            network: self.network.unwrap_or(Network {
                doh_mode: "disabled".to_owned(),
                ipv6_mode: "block".to_owned(),
            }),
        })
    }
}

const fn default_schema_version() -> u32 {
    mien_core::fingerprint::spec::SCHEMA_VERSION
}

/// 创建档案输入；`fingerprint` 缺省时使用 `FingerprintSpec::fresh(Platform::Macos)`。
#[derive(Debug, Clone, Deserialize)]
pub struct CreateProfileInput {
    pub name: String,
    #[serde(default)]
    pub fingerprint: Option<FingerprintInput>,
    #[serde(default)]
    pub mode: Option<ProfileMode>,
    #[serde(default)]
    pub start_url: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

/// 更新档案输入；只遍历给出的字段，seed 不可改。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UpdateProfileInput {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub start_url: Option<String>,
    #[serde(default)]
    pub fingerprint: Option<FingerprintInput>,
    #[serde(default)]
    pub mode: Option<ProfileMode>,
    #[serde(default)]
    pub automation_api: Option<bool>,
}

/// 分页结果：`{ items, nextCursor? }`。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileListView {
    pub items: Vec<ProfileView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// 运行时视图，字段与 `Runtime` 契约一致。
#[derive(Debug, Clone, Serialize)]
pub struct RuntimeView {
    pub id: String,
    pub profile_id: String,
    pub state: RuntimeState,
    pub mode: ProfileMode,
    pub direct: bool,
    pub reduced_assurance: bool,
    pub isolation: Isolation,
    pub pid: Option<u32>,
    pub cdp_port: Option<u16>,
    pub proxy_port: Option<u16>,
    pub error_code: Option<String>,
}

impl From<&RuntimeSnapshot> for RuntimeView {
    fn from(snapshot: &RuntimeSnapshot) -> Self {
        Self {
            id: snapshot.runtime_id.clone(),
            profile_id: snapshot.profile_id.clone(),
            state: snapshot.state,
            mode: snapshot.mode,
            direct: snapshot.direct,
            reduced_assurance: snapshot.reduced_assurance,
            isolation: snapshot.isolation,
            pid: snapshot.pid,
            cdp_port: snapshot.cdp_port,
            proxy_port: snapshot.proxy_port,
            error_code: snapshot.error_code.clone(),
        }
    }
}

/// 档案视图：核心档案 + 运行时派生的证据字段（`Profile` 契约）。
#[derive(Debug, Clone, Serialize)]
pub struct ProfileView {
    #[serde(flatten)]
    pub profile: Profile,
    pub isolation: Isolation,
    pub direct: bool,
    pub reduced_assurance: bool,
    pub limitations: Vec<String>,
    pub runtime: Option<RuntimeView>,
}

/// `runtime_start` 结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStartView {
    pub runtime_id: String,
    pub state: RuntimeState,
    pub mode: ProfileMode,
    pub isolation: Isolation,
    pub reduced_assurance: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ua_scope: Option<&'static str>,
    pub disclosures: Vec<String>,
}

/// `runtime_status` / `runtime_stop` 结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatusView {
    pub state: RuntimeState,
    pub isolation: Isolation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cdp_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

impl RuntimeStatusView {
    fn stopped() -> Self {
        Self {
            state: RuntimeState::Stopped,
            isolation: Isolation::Unknown,
            pid: None,
            cdp_port: None,
            proxy_port: None,
            error_code: None,
        }
    }

    fn from_snapshot(snapshot: &RuntimeSnapshot) -> Self {
        Self {
            state: snapshot.state,
            isolation: snapshot.isolation,
            pid: snapshot.pid,
            cdp_port: snapshot.cdp_port,
            proxy_port: snapshot.proxy_port,
            error_code: snapshot.error_code.clone(),
        }
    }
}

/// 进程内共享状态。
pub struct AppState {
    data_root: PathBuf,
    repository: SqliteProfileRepository,
    supervisor: RuntimeSupervisor,
    manifest: CapabilityManifest,
    schema_version: u32,
    kernel_bin: Option<PathBuf>,
    xray_bin: PathBuf,
    xray_sha256: String,
    /// 共享引擎控制器与失败监听；首次代理启动时创建，宿主全程复用。
    engine: std::sync::OnceLock<Arc<GatewayAttach<GrpcXrayApi>>>,
}

impl AppState {
    /// 按环境装配：`MIEN_DATA_ROOT` 或 `~/Library/Application Support/Mien`。
    ///
    /// # Errors
    /// 数据根不可写、迁移失败或能力清单无效。
    pub fn open() -> Result<Self, CoreError> {
        let root = default_data_root()?;
        let kernel = std::env::var_os("MIEN_KERNEL_BIN").map(PathBuf::from);
        Self::open_at_with_kernel(root, kernel)
    }

    /// 在指定数据根装配；`kernel_bin` 是显式验收用法，其余情况按安装目录解析。
    ///
    /// # Errors
    /// 目录不可写、数据库迁移失败或能力清单无效。
    pub fn open_at_with_kernel(
        data_root: impl Into<PathBuf>,
        kernel_bin: Option<PathBuf>,
    ) -> Result<Self, CoreError> {
        let data_root = data_root.into();
        std::fs::create_dir_all(&data_root).map_err(|_| CoreError::Storage)?;
        let pool = open_and_migrate(&data_root.join("mien.db"))?;
        let schema_version = mien_core::storage::current_version(&pool)?;
        let repository = SqliteProfileRepository::new(pool, &data_root)?;
        let supervisor = RuntimeSupervisor::new(&data_root);
        let manifest = load_pinned_manifest()?;
        let (xray_bin, xray_sha256) = xray_bin(&data_root);
        Ok(Self {
            data_root,
            repository,
            supervisor,
            xray_bin,
            xray_sha256,
            engine: std::sync::OnceLock::new(),
            manifest,
            schema_version,
            kernel_bin,
        })
    }

    pub fn data_root(&self) -> &Path {
        &self.data_root
    }

    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// 解析本次启动使用的内核；缺失时返回 `KERNEL_NOT_FOUND`。
    ///
    /// # Errors
    /// 可执行文件不存在或不可读。
    pub fn kernel_binary(&self, profile: &Profile) -> Result<KernelBinary, CoreError> {
        let path = self
            .kernel_candidates(profile)
            .into_iter()
            .find(|candidate| candidate.is_file())
            .ok_or_else(kernel_missing)?;
        let sha256 = mien_core::kernel::registry::binary_sha256(&path)?;
        Ok(KernelBinary { path, sha256 })
    }

    fn kernel_candidates(&self, profile: &Profile) -> Vec<PathBuf> {
        if let Some(path) = &self.kernel_bin {
            return vec![path.clone()];
        }
        let mut versions: Vec<String> = Vec::new();
        for version in [&profile.kernel_version, &self.manifest.kernel_version] {
            for candidate in [version.clone(), major_version(version)] {
                if !candidate.is_empty() && !versions.contains(&candidate) {
                    versions.push(candidate);
                }
            }
        }
        let mut candidates = Vec::new();
        for version in versions {
            for name in KERNEL_NAMES {
                candidates.push(
                    self.data_root
                        .join("kernels")
                        .join(&version)
                        .join(platform_triplet())
                        .join(name),
                );
            }
        }
        candidates
    }

    fn profile_id(id: &str) -> ProfileId {
        ProfileId::from_string(id)
    }

    /// 档案列表；`status` 为空时列出全部生命周期状态。
    ///
    /// # Errors
    /// 存储不可读。
    pub async fn list_profiles(&self, query: ProfileQuery) -> Result<ProfileListView, CoreError> {
        let page: ProfilePage = self.repository.list(query).await?;
        Ok(ProfileListView {
            items: page.items.into_iter().map(|p| self.view(p)).collect(),
            next_cursor: page.next_cursor,
        })
    }

    /// 单个档案。
    ///
    /// # Errors
    /// 不存在时返回 `PROFILE_NOT_FOUND`。
    pub async fn get_profile(&self, id: &str) -> Result<ProfileView, CoreError> {
        let profile = self.repository.get(&Self::profile_id(id)).await?;
        Ok(self.view(profile))
    }

    /// 创建档案；缺省指纹取 macOS 新规格。
    ///
    /// # Errors
    /// 名称为空/过长、指纹输入无效或存储繁忙。
    pub async fn create_profile(
        &self,
        input: CreateProfileInput,
    ) -> Result<ProfileView, CoreError> {
        let fingerprint = match input.fingerprint {
            Some(fingerprint) => fingerprint.into_spec(None)?,
            None => FingerprintSpec::fresh(Platform::Macos)?,
        };
        let create = CreateProfile {
            name: input.name,
            fingerprint,
            start_url: input.start_url,
            proxy_id: None,
            notes: input.notes,
            kernel_version: self.manifest.kernel_version.clone(),
            mode: input.mode.unwrap_or_default(),
            automation_api: false,
        };
        let profile = self.repository.create(create).await?;
        Ok(self.view(profile))
    }

    /// 更新档案；给出的指纹沿用已存 seed。
    ///
    /// # Errors
    /// 不存在、已删除或字段校验失败。
    pub async fn update_profile(
        &self,
        id: &str,
        input: UpdateProfileInput,
    ) -> Result<ProfileView, CoreError> {
        let profile_id = Self::profile_id(id);
        let current = self.repository.get(&profile_id).await?;
        let fingerprint = match input.fingerprint {
            Some(fingerprint) => Some(fingerprint.into_spec(Some(current.fingerprint.seed))?),
            None => None,
        };
        let patch = ProfilePatch {
            name: input.name,
            notes: input.notes,
            fingerprint,
            start_url: input.start_url,
            kernel_version: None,
            mode: input.mode,
            automation_api: input.automation_api,
        };
        let profile = self.repository.update(&profile_id, patch).await?;
        Ok(self.view(profile))
    }

    /// 启动档案：direct 模式直接进入 `StartProfile`；代理绑定在引擎落地前 fail-closed。
    ///
    /// # Errors
    /// `PROXY_ENGINE_NOT_READY`（有绑定无引擎）、`KERNEL_NOT_FOUND`（内核缺失）、
    /// 指纹/内核准入失败或进程无法启动。
    pub async fn start_runtime(&self, id: &str) -> Result<RuntimeStartView, CoreError> {
        let profile = self.repository.get(&Self::profile_id(id)).await?;
        // The proxy gate runs before kernel resolution: a binding without usable
        // credentials must fail closed as `PROXY_ENGINE_NOT_READY`, not as a
        // missing kernel, so the UI names the actual blocker.
        let spec = match &profile.proxy_id {
            Some(proxy_id) => Some(self.proxy_spec(proxy_id).await?),
            None => None,
        };
        let kernel = self.kernel_binary(&profile)?;
        let runtime_id = runtime_id(&profile.id.0);
        let started = match &profile.proxy_id {
            Some(_) => {
                let spec = spec.ok_or_else(|| {
                    CoreError::validation(
                        ErrorCode::ProxyEngineNotReady,
                        "the profile requires a proxy engine binding",
                    )
                })?;
                // One engine per host: the first proxy start owns the slot.
                if self.engine.get().is_none() {
                    let mut engine =
                        EngineProcess::start(&self.xray_bin, &self.xray_sha256).await?;
                    let attach = GatewayAttach::new(
                        Arc::new(EngineController::with_fallback(engine.api(), false)),
                        spec,
                    );
                    attach.register_engine(EngineBinary {
                        path: self.xray_bin.clone(),
                        sha256: self.xray_sha256.clone(),
                    });
                    let request = self.start_request(&profile, &kernel, Some(&attach));
                    match request.start(&self.supervisor, &runtime_id).await {
                        Ok(started) => {
                            let _ = self.engine.set(Arc::new(attach));
                            started
                        }
                        Err(error) => {
                            let _ = engine.stop(std::time::Duration::from_secs(1)).await;
                            return Err(error);
                        }
                    }
                } else {
                    let Some(attach) = self.engine.get() else {
                        return Err(CoreError::validation(
                            ErrorCode::ProxyEngineNotReady,
                            "shared proxy engine is not ready",
                        ));
                    };
                    let request = self.start_request(&profile, &kernel, Some(attach.as_ref()));
                    request.start(&self.supervisor, &runtime_id).await?
                }
            }
            None => {
                let request = self.start_request(&profile, &kernel, None);
                request.start(&self.supervisor, &runtime_id).await?
            }
        };
        Ok(RuntimeStartView {
            runtime_id: started.snapshot.runtime_id,
            state: started.snapshot.state,
            mode: started.mode,
            isolation: started.snapshot.isolation,
            reduced_assurance: started.snapshot.reduced_assurance,
            ua_scope: started.ua_scope,
            disclosures: started.disclosures,
        })
    }

    /// 停止档案运行时；未启动时是幂等的 `stopped`。
    ///
    /// # Errors
    /// 档案不存在或进程组无法终止。
    pub async fn stop_runtime(&self, id: &str) -> Result<RuntimeStatusView, CoreError> {
        let profile = self.repository.get(&Self::profile_id(id)).await?;
        let runtime_id = runtime_id(&profile.id.0);
        if self.supervisor.state(&runtime_id).is_none() {
            return Ok(RuntimeStatusView::stopped());
        }
        let snapshot = self.supervisor.stop(&runtime_id).await?;
        Ok(RuntimeStatusView::from_snapshot(&snapshot))
    }

    /// 运行时状态；未启动过的档案返回 `stopped` / `unknown`，从不虚报隔离结论。
    ///
    /// # Errors
    /// 档案不存在。
    pub async fn runtime_status(&self, id: &str) -> Result<RuntimeStatusView, CoreError> {
        let profile = self.repository.get(&Self::profile_id(id)).await?;
        let runtime_id = runtime_id(&profile.id.0);
        Ok(match self.supervisor.state(&runtime_id) {
            Some(snapshot) => RuntimeStatusView::from_snapshot(&snapshot),
            None => RuntimeStatusView::stopped(),
        })
    }

    /// 控制面启动对账：终止可归属的残留内核，报告未能归属的端口租约。
    ///
    /// 必须在接受任何启动请求之前调用；数据库状态写入留给代理网关阶段，
    /// 这里只返回分类结果供宿主记录。
    ///
    /// # Errors
    /// 数据根不可读。
    pub async fn reconcile(&self) -> Result<Vec<RuntimeSnapshot>, CoreError> {
        self.supervisor.reconcile().await
    }

    /// 启动请求装配；`proxy` 为代理档案的网关适配器。
    fn start_request<'a>(
        &'a self,
        profile: &'a Profile,
        kernel: &'a KernelBinary,
        proxy: Option<&'a dyn mien_core::runtime::ProxyAttach>,
    ) -> StartProfile<'a> {
        StartProfile {
            profile,
            binary: kernel.path.clone(),
            binary_sha256: kernel.sha256.clone(),
            manifest: &self.manifest,
            data_root: self.data_root.clone(),
            sandbox_kind: self.manifest.sandbox_kind.clone(),
            proxy,
            stop_timeout: std::time::Duration::from_secs(5),
            restart: restart_policy(),
        }
    }

    /// 代理配置 → 出站规格。只接受真实记录与真实凭据，任何一步缺失都失败关闭。
    ///
    /// # Errors
    /// 主密钥缺失、代理不存在或凭据缺失时返回相应错误码；绝不构造占位节点。
    async fn proxy_spec(&self, proxy_id: &str) -> Result<OutboundSpec, CoreError> {
        let key = MasterKey::load(&self.data_root).map_err(|_| {
            CoreError::validation(
                ErrorCode::ProxyEngineNotReady,
                "the master key is not available for proxy credentials",
            )
        })?;
        let key_bytes: [u8; 32] = key.as_bytes().try_into().map_err(|_| CoreError::Storage)?;
        let repository = SqliteProxyRepository::new(
            self.repository.pool().clone(),
            MasterKey::from_bytes(key_bytes),
        );
        let id = ProxyId(proxy_id.to_owned());
        let view = repository.get(&id).await?;
        let secret = repository.load_secret(&id).await?;
        let config = ProxyConfig {
            protocol: protocol_for(view.kind)?,
            server: view.server,
            port: view.port,
            transport: Transport::Tcp,
            tls: TlsSettings::default(),
            mux: MuxSettings::default(),
            options: secret
                .options
                .iter()
                .filter_map(|(name, value)| {
                    value.as_str().map(|text| (name.clone(), text.to_owned()))
                })
                .collect(),
            credentials: Credentials {
                values: secret.credentials.clone(),
            },
        };
        build_outbound(&config, key.as_bytes())
    }

    fn view(&self, profile: Profile) -> ProfileView {
        let direct = profile.proxy_id.is_none();
        let snapshot = self.supervisor.state(&runtime_id(&profile.id.0));
        let isolation = snapshot
            .as_ref()
            .map_or(Isolation::Unknown, |snapshot| snapshot.isolation);
        ProfileView {
            reduced_assurance: mien_core::runtime::state::reduced_assurance(
                profile.mode,
                direct,
                isolation,
            ),
            profile,
            isolation,
            direct,
            limitations: self.manifest.limitations.clone(),
            runtime: snapshot.as_ref().map(RuntimeView::from),
        }
    }
}

/// `148.0.7778.215` → `148`；没有点号时原样返回。
fn major_version(version: &str) -> String {
    version
        .split('.')
        .next()
        .unwrap_or(version)
        .trim()
        .to_owned()
}

fn default_data_root() -> Result<PathBuf, CoreError> {
    if let Some(root) = std::env::var_os("MIEN_DATA_ROOT") {
        return Ok(PathBuf::from(root));
    }
    let home = std::env::var_os("HOME").ok_or(CoreError::Storage)?;
    Ok(PathBuf::from(home)
        .join("Library")
        .join("Application Support")
        .join("Mien"))
}

/// `status` 查询参数解析；未知取值是显式校验失败，不静默忽略。
///
/// # Errors
/// 取值不在 `active` / `archived` / `deleted` 内。
pub fn parse_status(value: &str) -> Result<ProfileStatus, CoreError> {
    match value {
        "active" => Ok(ProfileStatus::Active),
        "archived" => Ok(ProfileStatus::Archived),
        "deleted" => Ok(ProfileStatus::Deleted),
        other => Err(CoreError::validation(
            ErrorCode::ValidationFailed,
            format!("unknown profile status `{other}`"),
        )),
    }
}

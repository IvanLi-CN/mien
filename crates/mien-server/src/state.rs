//! 启动配置与共享状态。
//!
//! 配置只来自环境变量；非回环监听必须同时提供 `MIEN_PUBLIC_URL`（`SECURITY.md` T-26），
//! 该校验是纯函数，启动路径与单元测试共用同一实现。

use std::fmt;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use mien_core::CoreError;
use mien_core::runtime::RuntimeSupervisor;
use mien_core::storage::sqlite::SqliteProfileRepository;
use sqlx::SqlitePool;

/// 监听地址默认值：回环开发模式。
pub const DEFAULT_BIND: &str = "127.0.0.1:8787";
/// 监听地址环境变量。
pub const BIND_ENV: &str = "MIEN_BIND";
/// 对外 https 基址环境变量；非回环监听必填。
pub const PUBLIC_URL_ENV: &str = "MIEN_PUBLIC_URL";
/// 数据根环境变量；默认 `<cwd>/data`。
pub const DATA_ROOT_ENV: &str = "MIEN_DATA_ROOT";
/// 内核可执行文件覆盖；未设置时按 `<data-root>/kernels/<version>/<platform>/` 解析。
pub const KERNEL_BIN_ENV: &str = "MIEN_KERNEL_BIN";
/// 共享代理引擎可执行文件覆盖；未设置时按 `<data-root>/xray/xray` 解析。
pub const XRAY_BIN_ENV: &str = "MIEN_XRAY_BIN";
/// 服务端直连出口开关；未设置时直连档案启动返回 `DIRECT_EGRESS_DISABLED`。
pub const ALLOW_DIRECT_EGRESS_ENV: &str = "MIEN_ALLOW_DIRECT_EGRESS";
/// 代理检查回显端点（`http://host[:port]/path`）；未设置时检查端点返回 503。
pub const PROXY_CHECK_URL_ENV: &str = "MIEN_PROXY_CHECK_URL";
/// 代理检查总预算（秒）；默认 10 秒，上限 60 秒。
pub const PROXY_CHECK_TIMEOUT_ENV: &str = "MIEN_PROXY_CHECK_TIMEOUT_SECS";

/// 数据库文件名，位于数据根下。
const DATABASE_FILE: &str = "mien.db";

/// 代理检查运行参数：回显端点与总预算。
///
/// 回显端点是可选的（`REQ-PROXY-013` 的「可选回显端点」）：未配置时检查端点
/// 返回 503，而不是伪造一次节点失败。默认预算 10 秒，允许 1..=60 秒。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyCheckRuntime {
    pub url: Option<String>,
    pub timeout: Duration,
}

impl Default for ProxyCheckRuntime {
    fn default() -> Self {
        Self {
            url: std::env::var(PROXY_CHECK_URL_ENV)
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty()),
            timeout: proxy_check_timeout_from_env(),
        }
    }
}

impl ProxyCheckRuntime {
    /// 显式指定回显端点与预算（测试与受限宿主使用）。
    pub fn new(url: Option<String>, timeout: Duration) -> Self {
        Self {
            url,
            timeout: timeout.clamp(Duration::from_secs(1), Duration::from_secs(60)),
        }
    }
}

/// `MIEN_PROXY_CHECK_TIMEOUT_SECS`：默认 10 秒，非数字或越界回落默认值。
fn proxy_check_timeout_from_env() -> Duration {
    let seconds = std::env::var(PROXY_CHECK_TIMEOUT_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|seconds| (1..=60).contains(seconds))
        .unwrap_or(10);
    Duration::from_secs(seconds)
}

/// 内核能力清单：进程内只加载一次，失败缓存为稳定错误码。
static KERNEL: LazyLock<Result<String, String>> = LazyLock::new(|| {
    mien_core::fingerprint::load_pinned_manifest()
        .map(|manifest| manifest.kernel_version)
        .map_err(|error| error.code().as_str().to_owned())
});

/// 内核能力清单：进程内只解析一次，失败缓存为稳定错误码。
static MANIFEST: LazyLock<Result<mien_core::fingerprint::CapabilityManifest, String>> =
    LazyLock::new(|| {
        mien_core::fingerprint::load_pinned_manifest()
            .map_err(|error| error.code().as_str().to_owned())
    });

/// 服务端启动配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub bind: SocketAddr,
    pub public_url: Option<String>,
    pub data_root: PathBuf,
}

/// 配置解析或校验失败。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    InvalidBind(String),
    InvalidDataRoot(String),
    MissingPublicUrl { bind: SocketAddr },
    InvalidPublicUrl(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBind(value) => write!(
                f,
                "{BIND_ENV}={value} is not a socket address such as {DEFAULT_BIND}"
            ),
            Self::InvalidDataRoot(detail) => write!(f, "cannot resolve {DATA_ROOT_ENV}: {detail}"),
            Self::MissingPublicUrl { bind } => write!(
                f,
                "{BIND_ENV}={bind} is not a loopback address, so {PUBLIC_URL_ENV} is required"
            ),
            Self::InvalidPublicUrl(value) => write!(
                f,
                "{PUBLIC_URL_ENV}={value} must be an absolute http or https URL"
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// Read the configuration from the process environment.
    ///
    /// # Errors
    /// Invalid bind, unresolvable data root, missing public URL for a
    /// non-loopback bind, or a malformed public URL.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Read the configuration from an arbitrary environment-like lookup.
    ///
    /// # Errors
    /// See [`Config::from_env`].
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let value = lookup(BIND_ENV).unwrap_or_else(|| DEFAULT_BIND.to_owned());
        let bind = value
            .trim()
            .parse::<SocketAddr>()
            .map_err(|_| ConfigError::InvalidBind(value))?;

        let public_url = lookup(PUBLIC_URL_ENV)
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        validate_bind(bind, public_url.as_deref())?;

        let data_root = match lookup(DATA_ROOT_ENV) {
            Some(value) if !value.trim().is_empty() => PathBuf::from(value.trim()),
            _ => std::env::current_dir()
                .map_err(|error| ConfigError::InvalidDataRoot(error.to_string()))?
                .join("data"),
        };

        Ok(Self {
            bind,
            public_url,
            data_root,
        })
    }
}

/// Reject a non-loopback bind without an absolute `MIEN_PUBLIC_URL`.
///
/// # Errors
/// [`ConfigError::MissingPublicUrl`] for a non-loopback bind without a public
/// URL, [`ConfigError::InvalidPublicUrl`] when the value is not an absolute
/// `http`/`https` URL.
pub fn validate_bind(bind: SocketAddr, public_url: Option<&str>) -> Result<(), ConfigError> {
    let Some(public_url) = public_url else {
        return if bind.ip().is_loopback() {
            Ok(())
        } else {
            Err(ConfigError::MissingPublicUrl { bind })
        };
    };

    let invalid = ConfigError::InvalidPublicUrl(public_url.to_owned());
    if public_url.trim() != public_url || public_url.chars().any(char::is_whitespace) {
        return Err(invalid);
    }
    let authority = match public_url
        .strip_prefix("https://")
        .or_else(|| public_url.strip_prefix("http://"))
    {
        Some(authority) => authority,
        None => return Err(invalid),
    };
    if authority.is_empty() || authority.starts_with('/') {
        return Err(invalid);
    }
    Ok(())
}

/// 对外来源与 Cookie 安全策略。
///
/// `docs/SECURITY.md` §5.4：非回环服务端必须配置 `MIEN_PUBLIC_URL`，服务端模式
/// 一律使用 `Secure` Cookie；仅回环开发模式可省略。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SecuritySettings {
    /// 对外 https 基址；用于 Origin 校验与 `Secure` Cookie。
    pub public_url: Option<String>,
    /// 会话 Cookie 是否带 `Secure`。
    pub secure_cookies: bool,
}

impl SecuritySettings {
    /// 回环开发模式：无对外来源，Cookie 不带 `Secure`。
    pub fn loopback() -> Self {
        Self::default()
    }

    /// 从启动配置派生：设置 `MIEN_PUBLIC_URL` 或非回环监听即视为服务端模式。
    pub fn from_config(config: &Config) -> Self {
        Self {
            public_url: config.public_url.clone(),
            secure_cookies: config.public_url.is_some() || !config.bind.ip().is_loopback(),
        }
    }

    /// 从进程环境派生；环境不可解析时按回环开发模式处理（启动路径已先校验）。
    pub fn from_env() -> Self {
        Config::from_env()
            .map(|config| Self::from_config(&config))
            .unwrap_or_default()
    }
}

/// 认证运行时状态：登录限流、会话有效期、Bearer 校验器与启动引导口令。
///
/// 引导口令在启动时只做文件系统层面的解析（`MIEN_ADMIN_PASSWORD`、
/// `MIEN_ADMIN_PASSWORD_FILE` 或生成 `<root>/secrets/admin-bootstrap`），哈希写入
/// `admin_credentials` 发生在首次登录时：这样池只在服务自己的运行时里被使用，
/// 避免在启动阶段额外创建运行时。
pub struct AuthState {
    limiter: crate::auth::ratelimit::Limiter,
    session_ttl: Duration,
    bearer: Option<crate::auth::SharedBearerAuthenticator>,
    bootstrap: Option<crate::auth::password::Bootstrap>,
}

impl Default for AuthState {
    fn default() -> Self {
        Self {
            limiter: crate::auth::ratelimit::Limiter::new(crate::auth::ratelimit::Policy::default()),
            session_ttl: crate::auth::session::DEFAULT_TTL,
            bearer: None,
            bootstrap: None,
        }
    }
}

impl AuthState {
    pub fn limiter(&self) -> &crate::auth::ratelimit::Limiter {
        &self.limiter
    }

    pub fn session_ttl(&self) -> Duration {
        self.session_ttl
    }

    pub fn bearer(&self) -> Option<crate::auth::SharedBearerAuthenticator> {
        self.bearer.clone()
    }

    /// 启动时解析出的引导口令；首次登录时写入哈希。
    pub fn bootstrap(&self) -> Option<&crate::auth::password::Bootstrap> {
        self.bootstrap.as_ref()
    }

    /// 替换登录限流参数（测试与容量规划使用）。
    pub fn with_limiter_policy(mut self, policy: crate::auth::ratelimit::Policy) -> Self {
        self.limiter = crate::auth::ratelimit::Limiter::new(policy);
        self
    }

    /// 替换会话有效期。
    pub fn with_session_ttl(mut self, ttl: Duration) -> Self {
        self.session_ttl = ttl;
        self
    }

    /// 挂载 Bearer 校验器；未挂载时 Bearer 请求一律 `401 AUTH_INVALID`。
    pub fn with_bearer(mut self, verifier: crate::auth::SharedBearerAuthenticator) -> Self {
        self.bearer = Some(verifier);
        self
    }

    /// 指定启动引导口令。
    pub fn with_bootstrap(mut self, bootstrap: crate::auth::password::Bootstrap) -> Self {
        self.bootstrap = Some(bootstrap);
        self
    }
}

/// Shared handlers state. Cloning shares one database pool.
#[derive(Clone)]
pub struct AppState {
    inner: Arc<Inner>,
}

#[derive(Clone)]
struct Inner {
    data_root: PathBuf,
    db_path: PathBuf,
    db: Option<SqlitePool>,
    security: SecuritySettings,
    auth: Arc<AuthState>,
    tokens: Option<Arc<crate::tokens::store::TokenStore>>,
    /// 档案仓储；数据根不可写时为 `None`，档案端点返回内部错误。
    profiles: Option<SqliteProfileRepository>,
    /// 受监督运行时；与服务端进程同寿命。
    supervisor: Arc<RuntimeSupervisor>,
    /// `MIEN_KERNEL_BIN` 显式覆盖；否则按档案绑定版本解析安装目录。
    kernel_bin: Option<PathBuf>,
    /// 服务端是否允许直连档案（`MIEN_ALLOW_DIRECT_EGRESS=1`）。
    allow_direct_egress: bool,
    /// 共享代理引擎槽；首次代理启动时填充，进程存活期间复用。
    engine: Arc<tokio::sync::Mutex<Option<Arc<crate::profiles::SharedEngine>>>>,
    /// 代理检查参数：回显端点与总预算。
    proxy_check: Arc<ProxyCheckRuntime>,
    /// 事件频道参数：保留窗口、票据 TTL、心跳与实时轮询间隔。
    events: Arc<crate::events::EventRuntime>,
}

impl AppState {
    /// Build a state from an already opened database (or none, for a host whose
    /// storage did not open). `db_path` is `<data_root>/mien.db`.
    pub fn new(data_root: impl Into<PathBuf>, db: Option<SqlitePool>) -> Self {
        let data_root = data_root.into();
        let db_path = data_root.join(DATABASE_FILE);
        let profiles = db
            .as_ref()
            .and_then(|pool| SqliteProfileRepository::new(pool.clone(), &data_root).ok());
        let supervisor = Arc::new(RuntimeSupervisor::new(&data_root));
        Self::build(
            data_root,
            db_path,
            db,
            SecuritySettings::loopback(),
            Arc::new(AuthState::default()),
            None,
            profiles,
            supervisor,
            std::env::var_os(KERNEL_BIN_ENV).map(PathBuf::from),
            allow_direct_egress_from_env(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        data_root: PathBuf,
        db_path: PathBuf,
        db: Option<SqlitePool>,
        security: SecuritySettings,
        auth: Arc<AuthState>,
        tokens: Option<Arc<crate::tokens::store::TokenStore>>,
        profiles: Option<SqliteProfileRepository>,
        supervisor: Arc<RuntimeSupervisor>,
        kernel_bin: Option<PathBuf>,
        allow_direct_egress: bool,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                data_root,
                db_path,
                db,
                security,
                auth,
                tokens,
                profiles,
                supervisor,
                kernel_bin,
                allow_direct_egress,
                engine: Arc::new(tokio::sync::Mutex::new(None)),
                proxy_check: Arc::new(ProxyCheckRuntime::default()),
                events: Arc::new(crate::events::EventRuntime::default()),
            }),
        }
    }

    /// 复制共享状态并就地替换一个字段；克隆只复制 `Arc` 与轻量值。
    fn with_mut(self, update: impl FnOnce(&mut Inner)) -> Self {
        let mut inner = (*self.inner).clone();
        update(&mut inner);
        Self {
            inner: Arc::new(inner),
        }
    }

    /// 替换安全策略（Origin 校验与 `Secure` Cookie）。
    pub fn with_security(self, security: SecuritySettings) -> Self {
        self.with_mut(|inner| inner.security = security)
    }

    /// 替换认证运行时状态（限流参数、会话有效期、Bearer 校验器、引导口令）。
    pub fn with_auth(self, auth: AuthState) -> Self {
        self.with_mut(|inner| inner.auth = Arc::new(auth))
    }

    /// 替换事件频道参数（保留窗口、票据 TTL、心跳与轮询）。
    pub fn with_events(self, events: crate::events::EventRuntime) -> Self {
        self.with_mut(|inner| inner.events = Arc::new(events))
    }

    /// 显式内核可执行文件（验收与受限宿主使用）。
    pub fn with_kernel_bin(self, kernel_bin: Option<PathBuf>) -> Self {
        self.with_mut(|inner| inner.kernel_bin = kernel_bin)
    }

    /// 服务端直连出口策略；生产由 [`ALLOW_DIRECT_EGRESS_ENV`] 决定。
    pub fn with_allow_direct_egress(self, allow: bool) -> Self {
        self.with_mut(|inner| inner.allow_direct_egress = allow)
    }

    /// Create the data root, open its migrated SQLite database and resolve the
    /// administrator bootstrap secret (filesystem only; the hash is written on the
    /// first successful login).
    ///
    /// # Errors
    /// Directory creation fails, migrations/integrity checks fail, or the
    /// administrator bootstrap secret is unusable.
    pub fn open(data_root: &Path) -> Result<Self, CoreError> {
        Self::open_with(data_root, SecuritySettings::from_env())
    }

    /// 同 [`AppState::open`]，但显式给出安全策略。
    ///
    /// # Errors
    /// 同 [`AppState::open`]。
    pub fn open_with(data_root: &Path, security: SecuritySettings) -> Result<Self, CoreError> {
        std::fs::create_dir_all(data_root).map_err(|_| CoreError::Storage)?;
        // 打开迁移会创建数据库；先记住它此前是否存在，用来判断这是不是一个全新
        // 数据根（只有全新数据根才允许生成引导口令，已有凭据时不得覆盖）。
        let database_path = data_root.join(DATABASE_FILE);
        let fresh_data_root = !database_path.exists();
        let pool = mien_core::storage::open_and_migrate(&database_path)?;
        // Token 存储要自己的池句柄，先克隆再交出所有权。
        let token_pool = pool.clone();

        let bootstrap =
            crate::auth::password::resolve_bootstrap(data_root, fresh_data_root, |key| {
                std::env::var(key).ok()
            })
            .map_err(|_| CoreError::Storage)?;
        let auth = match bootstrap {
            Some(bootstrap) => {
                if bootstrap.source() == crate::auth::password::BootstrapSource::Generated {
                    tracing::warn!(
                        path = %crate::auth::password::generated_bootstrap_path(data_root).display(),
                        "provisioned a generated administrator password; read it from this file"
                    );
                }
                AuthState::default().with_bootstrap(bootstrap)
            }
            // 已有凭据的旧数据根：不生成、不覆盖。
            None => AuthState::default(),
        };

        let repository = SqliteProfileRepository::new(pool.clone(), data_root)
            .map_err(|_| CoreError::Storage)?;
        let supervisor = Arc::new(RuntimeSupervisor::new(data_root));
        Ok(Self::build(
            data_root.to_path_buf(),
            data_root.join(DATABASE_FILE),
            Some(pool),
            security,
            Arc::new(auth),
            token_store(token_pool, data_root),
            Some(repository),
            supervisor,
            std::env::var_os(KERNEL_BIN_ENV).map(PathBuf::from),
            allow_direct_egress_from_env(),
        ))
    }

    pub fn data_root(&self) -> &Path {
        &self.inner.data_root
    }

    pub fn db_path(&self) -> &Path {
        &self.inner.db_path
    }

    pub fn db(&self) -> Option<&SqlitePool> {
        self.inner.db.as_ref()
    }

    pub fn security(&self) -> &SecuritySettings {
        &self.inner.security
    }

    pub fn auth(&self) -> &AuthState {
        &self.inner.auth
    }

    /// 已挂载的 Token 存储；主密钥不可用时为 `None`。
    pub fn tokens(&self) -> Option<&Arc<crate::tokens::store::TokenStore>> {
        self.inner.tokens.as_ref()
    }

    /// 替换 Token 存储。
    pub fn with_tokens(self, tokens: Option<Arc<crate::tokens::store::TokenStore>>) -> Self {
        self.with_mut(|inner| inner.tokens = tokens)
    }

    /// 档案仓储；数据根不可写时为 `None`。
    pub fn profiles(&self) -> Option<&SqliteProfileRepository> {
        self.inner.profiles.as_ref()
    }

    /// 受监督运行时。
    pub fn supervisor(&self) -> &Arc<RuntimeSupervisor> {
        &self.inner.supervisor
    }

    /// 显式内核覆盖。
    pub fn kernel_bin(&self) -> Option<&Path> {
        self.inner.kernel_bin.as_deref()
    }

    /// 服务端直连出口是否被显式允许。
    pub fn allow_direct_egress(&self) -> bool {
        self.inner.allow_direct_egress
    }

    /// 事件频道参数。
    pub fn events(&self) -> &crate::events::EventRuntime {
        &self.inner.events
    }

    /// 代理检查参数：回显端点与总预算。
    pub fn proxy_check(&self) -> &ProxyCheckRuntime {
        &self.inner.proxy_check
    }

    /// 替换代理检查参数（测试与受限宿主使用）。
    pub fn with_proxy_check(self, proxy_check: ProxyCheckRuntime) -> Self {
        self.with_mut(|inner| inner.proxy_check = Arc::new(proxy_check))
    }

    /// 共享代理引擎槽。
    pub fn engine_slot(
        &self,
    ) -> &Arc<tokio::sync::Mutex<Option<Arc<crate::profiles::SharedEngine>>>> {
        &self.inner.engine
    }

    /// Bearer 校验器：显式挂载的优先，其次是已装载的 Token 存储。
    ///
    /// 两者都没有时返回 `None`，认证门禁据此把所有 Bearer 请求判为
    /// `401 AUTH_INVALID`，不存在“未校验却放行”的路径。
    pub fn bearer_verifier(&self) -> Option<crate::auth::SharedBearerAuthenticator> {
        self.inner.auth.bearer().or_else(|| {
            self.inner
                .tokens
                .as_ref()
                .map(|store| crate::tokens::store::shared(Arc::clone(store)))
        })
    }

    /// Kernel capability manifest version, or the stable code that explains why
    /// the manifest cannot be loaded.
    pub fn kernel() -> Result<&'static str, &'static str> {
        KERNEL.as_ref().map(String::as_str).map_err(String::as_str)
    }

    /// 内核能力清单，或无法加载时的稳定错误码。
    pub fn manifest() -> Result<&'static mien_core::fingerprint::CapabilityManifest, &'static str> {
        MANIFEST.as_ref().map_err(|code| code.as_str())
    }
}

/// 服务端直连出口策略：只有显式 `1` 才允许。
fn allow_direct_egress_from_env() -> bool {
    std::env::var(ALLOW_DIRECT_EGRESS_ENV).is_ok_and(|value| value.trim() == "1")
}
/// 主密钥缺失时返回 `None`：Token 端点 500、Bearer 一律 401，绝不生成新密钥。
fn token_store(
    pool: SqlitePool,
    data_root: &Path,
) -> Option<Arc<crate::tokens::store::TokenStore>> {
    match crate::tokens::store::TokenStore::load(pool, data_root) {
        Ok(store) => Some(Arc::new(store)),
        Err(_) => {
            tracing::warn!(
                "the master key is unavailable, so API tokens are disabled until it is provisioned"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_bind_does_not_need_a_public_url() {
        assert_eq!(
            validate_bind("127.0.0.1:8787".parse().expect("addr"), None),
            Ok(())
        );
        assert_eq!(
            validate_bind("[::1]:8787".parse().expect("addr"), None),
            Ok(())
        );
    }

    #[test]
    fn non_loopback_bind_requires_a_public_url() {
        let bind = "0.0.0.0:8787".parse().expect("addr");
        assert_eq!(
            validate_bind(bind, None),
            Err(ConfigError::MissingPublicUrl { bind })
        );
        assert_eq!(
            validate_bind(bind, Some("https://mien.example.com")),
            Ok(())
        );
        assert_eq!(
            validate_bind(bind, Some("mien.example.com")),
            Err(ConfigError::InvalidPublicUrl("mien.example.com".to_owned()))
        );
        assert_eq!(
            validate_bind(bind, Some("https://")),
            Err(ConfigError::InvalidPublicUrl("https://".to_owned()))
        );
    }

    #[test]
    fn environment_defaults_to_loopback_and_cwd_data_root() {
        let config = Config::from_lookup(|key| match key {
            DATA_ROOT_ENV => Some("/srv/mien".to_owned()),
            _ => None,
        })
        .expect("config");
        assert_eq!(config.bind, "127.0.0.1:8787".parse().expect("addr"));
        assert_eq!(config.data_root, PathBuf::from("/srv/mien"));
        assert!(config.public_url.is_none());
    }

    #[test]
    fn non_loopback_environment_without_public_url_is_rejected() {
        let error = Config::from_lookup(|key| match key {
            BIND_ENV => Some("0.0.0.0:8787".to_owned()),
            _ => None,
        })
        .expect_err("non-loopback bind without MIEN_PUBLIC_URL must fail");
        assert!(matches!(error, ConfigError::MissingPublicUrl { .. }));
    }
}

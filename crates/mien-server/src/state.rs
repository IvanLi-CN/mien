//! 启动配置与共享状态。
//!
//! 配置只来自环境变量；非回环监听必须同时提供 `MIEN_PUBLIC_URL`（`SECURITY.md` T-26），
//! 该校验是纯函数，启动路径与单元测试共用同一实现。

use std::fmt;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use mien_core::CoreError;
use sqlx::SqlitePool;

/// 监听地址默认值：回环开发模式。
pub const DEFAULT_BIND: &str = "127.0.0.1:8787";
/// 监听地址环境变量。
pub const BIND_ENV: &str = "MIEN_BIND";
/// 对外 https 基址环境变量；非回环监听必填。
pub const PUBLIC_URL_ENV: &str = "MIEN_PUBLIC_URL";
/// 数据根环境变量；默认 `<cwd>/data`。
pub const DATA_ROOT_ENV: &str = "MIEN_DATA_ROOT";

/// 数据库文件名，位于数据根下。
const DATABASE_FILE: &str = "mien.db";

/// 内核能力清单：进程内只加载一次，失败缓存为稳定错误码。
static KERNEL: LazyLock<Result<String, String>> = LazyLock::new(|| {
    mien_core::fingerprint::load_pinned_manifest()
        .map(|manifest| manifest.kernel_version)
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

/// Shared handlers state. Cloning shares one database pool.
#[derive(Clone)]
pub struct AppState {
    inner: Arc<Inner>,
}

struct Inner {
    data_root: PathBuf,
    db_path: PathBuf,
    db: Option<SqlitePool>,
}

impl AppState {
    /// Build a state from an already opened database (or none, for a host whose
    /// storage did not open). `db_path` is `<data_root>/mien.db`.
    pub fn new(data_root: impl Into<PathBuf>, db: Option<SqlitePool>) -> Self {
        let data_root = data_root.into();
        let db_path = data_root.join(DATABASE_FILE);
        Self {
            inner: Arc::new(Inner {
                data_root,
                db_path,
                db,
            }),
        }
    }

    /// Create the data root and open its migrated SQLite database.
    ///
    /// # Errors
    /// Directory creation fails, or migrations/integrity checks fail.
    pub fn open(data_root: &Path) -> Result<Self, CoreError> {
        std::fs::create_dir_all(data_root).map_err(|_| CoreError::Storage)?;
        let pool = mien_core::storage::open_and_migrate(&data_root.join(DATABASE_FILE))?;
        Ok(Self::new(data_root, Some(pool)))
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

    /// Kernel capability manifest version, or the stable code that explains why
    /// the manifest cannot be loaded.
    pub fn kernel() -> Result<&'static str, &'static str> {
        KERNEL.as_ref().map(String::as_str).map_err(String::as_str)
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

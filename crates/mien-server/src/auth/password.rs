//! 管理员口令：引导来源、Argon2id 哈希与改密。
//!
//! 契约：`docs/SECURITY.md` §5.1 与 `docs/DATABASE-SCHEMA.md` §3「管理员凭据」。
//! 单行凭据只存 Argon2id 哈希（PHC 字符串）与版本化参数；引导明文不落库、不写日志。
//! 支持三种引导来源：内联 `MIEN_ADMIN_PASSWORD`、0600 文件 `MIEN_ADMIN_PASSWORD_FILE`、
//! 两者皆缺时生成高熵口令写入 `<root>/secrets/admin-bootstrap`（0600）。只有应用自己
//! 生成的文件才在改密后删除；环境变量与外部 Secret 必须由部署者撤销。

use std::path::{Path, PathBuf};

use argon2::password_hash::{PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use rand::RngCore;
use rand::rngs::OsRng;
use sqlx::SqlitePool;

use crate::auth::AuthError;
use crate::auth::session::now_timestamp;

/// 内联引导口令环境变量；仅限一次性引导。
pub const ADMIN_PASSWORD_ENV: &str = "MIEN_ADMIN_PASSWORD";
/// 引导口令文件环境变量；文件必须是 0600 的普通文件。
pub const ADMIN_PASSWORD_FILE_ENV: &str = "MIEN_ADMIN_PASSWORD_FILE";
/// 应用生成的引导口令相对数据根的路径。
pub const BOOTSTRAP_FILE: &str = "secrets/admin-bootstrap";

/// 新口令最小长度，与 `docs/contracts/openapi.yaml#/components/schemas/PasswordChangeInput` 一致。
pub const MIN_NEW_PASSWORD_LEN: usize = 12;
/// 生成引导口令的随机字节数；编码后为 64 个十六进制字符。
const GENERATED_PASSWORD_BYTES: usize = 32;

/// Argon2id 参数版本；写入 `admin_credentials.params_json` 以便日后升级。
pub const PARAMS_VERSION: u32 = 1;
/// 内存开销（KiB）。OWASP 推荐 19 MiB。
const PARAMS_MEMORY_KIB: u32 = 19 * 1024;
/// 迭代次数。
const PARAMS_ITERATIONS: u32 = 2;
/// 并行度。
const PARAMS_PARALLELISM: u32 = 1;

/// 引导口令来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootstrapSource {
    /// 内联环境变量；改密后必须由部署者撤销。
    Environment,
    /// 外部 0600 文件；应用不删除。
    File,
    /// 应用生成并写入 `<root>/secrets/admin-bootstrap`；改密后由应用删除。
    Generated,
}

impl BootstrapSource {
    /// `params_json` 中的稳定标识。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Environment => "environment",
            Self::File => "file",
            Self::Generated => "generated",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "environment" => Some(Self::Environment),
            "file" => Some(Self::File),
            "generated" => Some(Self::Generated),
            _ => None,
        }
    }
}

/// 解析出的引导口令。
#[derive(Clone)]
pub struct Bootstrap {
    password: String,
    source: BootstrapSource,
}

impl Bootstrap {
    /// 测试与外部注入：给定明文口令与来源。
    pub fn new(password: impl Into<String>, source: BootstrapSource) -> Self {
        Self {
            password: password.into(),
            source,
        }
    }

    /// 内联来源的便捷构造。
    pub fn inline(password: impl Into<String>) -> Self {
        Self::new(password, BootstrapSource::Environment)
    }

    pub fn source(&self) -> BootstrapSource {
        self.source
    }

    /// 明文口令；仅供哈希与校验使用，不得打印或序列化。
    pub fn expose(&self) -> &str {
        &self.password
    }
}

impl std::fmt::Debug for Bootstrap {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Bootstrap")
            .field("password", &"<redacted>")
            .field("source", &self.source)
            .finish()
    }
}

/// 存储的凭据快照。
#[derive(Debug, Clone)]
pub struct Credential {
    pub password_hash: String,
    pub params_json: String,
    pub must_change: bool,
    pub bootstrap_pending: bool,
    pub source: Option<BootstrapSource>,
}

/// 按契约顺序解析引导口令：内联环境变量、0600 文件、生成。
///
/// 生成路径会创建 `<root>/secrets` 并写入 0600 文件；已存在的生成文件会被复用，
/// 避免每次启动都重置管理员口令。
///
/// # Errors
/// 文件不可读、权限过宽、内容为空，或生成文件写入失败。
pub fn resolve_bootstrap(
    root: &Path,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<Bootstrap, AuthError> {
    if let Some(value) = lookup(ADMIN_PASSWORD_ENV) {
        if !value.is_empty() {
            return Ok(Bootstrap::new(value, BootstrapSource::Environment));
        }
    }
    if let Some(path) = lookup(ADMIN_PASSWORD_FILE_ENV) {
        let path = PathBuf::from(path);
        let secret = read_private_secret(&path)?;
        return Ok(Bootstrap::new(secret, BootstrapSource::File));
    }

    let path = root.join(BOOTSTRAP_FILE);
    if path.is_file() {
        let secret = read_private_secret(&path)?;
        return Ok(Bootstrap::new(secret, BootstrapSource::Generated));
    }
    let secret = generate_password();
    write_private_secret(&path, &secret)?;
    Ok(Bootstrap::new(secret, BootstrapSource::Generated))
}

/// 64 个十六进制字符的高熵口令（32 字节 CSPRNG 输出）。
pub fn generate_password() -> String {
    let mut bytes = [0u8; GENERATED_PASSWORD_BYTES];
    OsRng.fill_bytes(&mut bytes);
    hex(&bytes)
}

/// 读取 0600 普通文件的秘密内容；拒绝符号链接与宽松权限，去除尾部换行。
///
/// # Errors
/// 文件缺失、不是普通文件、权限过宽或内容为空。
pub fn read_private_secret(path: &Path) -> Result<String, AuthError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|_| AuthError::bootstrap_secret_unavailable())?;
        if !metadata.file_type().is_file() || metadata.permissions().mode() & 0o077 != 0 {
            return Err(AuthError::bootstrap_secret_unavailable());
        }
    }
    #[cfg(not(unix))]
    {
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|_| AuthError::bootstrap_secret_unavailable())?;
        if !metadata.file_type().is_file() {
            return Err(AuthError::bootstrap_secret_unavailable());
        }
    }
    let raw = std::fs::read(path).map_err(|_| AuthError::bootstrap_secret_unavailable())?;
    let text = String::from_utf8(raw).map_err(|_| AuthError::bootstrap_secret_unavailable())?;
    let secret = text.trim_end_matches(['\n', '\r']);
    if secret.is_empty() {
        return Err(AuthError::bootstrap_secret_unavailable());
    }
    Ok(secret.to_owned())
}

/// 写入 0600 秘密文件；父目录 0700。
fn write_private_secret(path: &Path, secret: &str) -> Result<(), AuthError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|_| AuthError::bootstrap_secret_unavailable())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    let mut file = options
        .open(path)
        .map_err(|_| AuthError::bootstrap_secret_unavailable())?;
    writeln!(file, "{secret}").map_err(|_| AuthError::bootstrap_secret_unavailable())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// 应用自己生成的引导文件路径。
pub fn generated_bootstrap_path(root: &Path) -> PathBuf {
    root.join(BOOTSTRAP_FILE)
}

/// 改密后删除应用生成的引导文件；外部来源保持不动。
///
/// # Errors
/// 生成文件存在但无法删除时返回存储错误，调用方仍应完成改密。
pub fn remove_generated_bootstrap(
    root: &Path,
    source: Option<BootstrapSource>,
) -> std::io::Result<()> {
    if source != Some(BootstrapSource::Generated) {
        return Ok(());
    }
    let path = generated_bootstrap_path(root);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// 计算 Argon2id 哈希；返回 PHC 字符串与版本化参数 JSON。
///
/// # Errors
/// 参数非法或哈希失败。
pub fn hash_password(password: &str) -> Result<(String, String), AuthError> {
    let params = params();
    let salt = SaltString::generate(&mut OsRng);
    let hasher = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let hash = hasher
        .hash_password(password.as_bytes(), &salt)
        .map_err(|_| AuthError::internal())?;
    Ok((hash.to_string(), params_json()))
}

/// 校验口令；哈希不可解析时视为不匹配，不泄露内部原因。
pub fn verify_password(password: &str, phc: &str, params_json: &str) -> bool {
    let Ok(parsed) = argon2::PasswordHash::new(phc) else {
        return false;
    };
    let params = params_from_json(params_json).unwrap_or_else(params);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

fn params() -> Params {
    Params::new(
        PARAMS_MEMORY_KIB,
        PARAMS_ITERATIONS,
        PARAMS_PARALLELISM,
        None,
    )
    .expect("static Argon2 parameters are valid")
}

/// 版本化参数 JSON：`{"v":1,"alg":"argon2id","m":19456,"t":2,"p":1,"bootstrap_source":"..."}`。
fn params_json() -> String {
    serde_json::json!({
        "v": PARAMS_VERSION,
        "alg": "argon2id",
        "m": PARAMS_MEMORY_KIB,
        "t": PARAMS_ITERATIONS,
        "p": PARAMS_PARALLELISM,
    })
    .to_string()
}

fn params_with_source(source: BootstrapSource) -> String {
    serde_json::json!({
        "v": PARAMS_VERSION,
        "alg": "argon2id",
        "m": PARAMS_MEMORY_KIB,
        "t": PARAMS_ITERATIONS,
        "p": PARAMS_PARALLELISM,
        "bootstrap_source": source.as_str(),
    })
    .to_string()
}

fn params_from_json(value: &str) -> Option<Params> {
    let parsed: serde_json::Value = serde_json::from_str(value).ok()?;
    let memory = u32::try_from(parsed.get("m")?.as_u64()?).ok()?;
    let iterations = u32::try_from(parsed.get("t")?.as_u64()?).ok()?;
    let parallelism = u32::try_from(parsed.get("p")?.as_u64()?).ok()?;
    Params::new(memory, iterations, parallelism, None).ok()
}

/// 读取单行管理员凭据。
///
/// # Errors
/// 数据库查询失败。
pub async fn load(pool: &SqlitePool) -> Result<Option<Credential>, AuthError> {
    let row = sqlx::query_as::<_, (String, String, i64, i64)>(
        "SELECT password_hash, params_json, must_change, bootstrap_pending \
         FROM admin_credentials WHERE id = 1",
    )
    .fetch_optional(pool)
    .await
    .map_err(|_| AuthError::internal())?;
    Ok(
        row.map(|(password_hash, params_json, must_change, pending)| {
            let source = serde_json::from_str::<serde_json::Value>(&params_json)
                .ok()
                .and_then(|value| {
                    value
                        .get("bootstrap_source")
                        .and_then(|source| source.as_str())
                        .and_then(BootstrapSource::parse)
                });
            Credential {
                password_hash,
                params_json,
                must_change: must_change != 0,
                bootstrap_pending: pending != 0,
                source,
            }
        }),
    )
}

/// 管理员是否仍处于强制改密状态；没有凭据行时为 `false`（尚未引导，由登录返回 401）。
///
/// # Errors
/// 数据库查询失败。
pub async fn must_change_password(pool: &SqlitePool) -> Result<bool, AuthError> {
    let value = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT must_change FROM admin_credentials WHERE id = 1",
    )
    .fetch_optional(pool)
    .await
    .map_err(|_| AuthError::internal())?
    .flatten();
    Ok(value.unwrap_or(0) != 0)
}

/// 首次引导：没有凭据行时写入哈希与引导标记；已有行保持不动。
///
/// # Errors
/// 哈希计算或数据库写入失败。
pub async fn ensure_admin(pool: &SqlitePool, bootstrap: &Bootstrap) -> Result<(), AuthError> {
    if load(pool).await?.is_some() {
        return Ok(());
    }
    let (password_hash, _) = hash_password(bootstrap.expose())?;
    let params_json = params_with_source(bootstrap.source());
    let now = now_timestamp();
    sqlx::query(
        "INSERT INTO admin_credentials \
         (id, password_hash, algo, params_json, must_change, bootstrap_pending, updated_at) \
         VALUES (1, ?1, 'argon2id', ?2, 1, 1, ?3)",
    )
    .bind(password_hash)
    .bind(params_json)
    .bind(now)
    .execute(pool)
    .await
    .map_err(|_| AuthError::internal())?;
    Ok(())
}

/// 改密：写入新哈希并清除 `must_change` 与 `bootstrap_pending`。
///
/// # Errors
/// 哈希计算或数据库写入失败。
pub async fn set_password(pool: &SqlitePool, password: &str) -> Result<(), AuthError> {
    let (password_hash, params_json) = hash_password(password)?;
    sqlx::query(
        "UPDATE admin_credentials \
         SET password_hash = ?1, algo = 'argon2id', params_json = ?2, \
             must_change = 0, bootstrap_pending = 0, updated_at = ?3 \
         WHERE id = 1",
    )
    .bind(password_hash)
    .bind(params_json)
    .bind(now_timestamp())
    .execute(pool)
    .await
    .map_err(|_| AuthError::internal())?;
    Ok(())
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(b"0123456789abcdef"[(byte >> 4) as usize]));
        out.push(char::from(b"0123456789abcdef"[(byte & 0x0f) as usize]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_prefers_the_inline_environment_variable() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let bootstrap = resolve_bootstrap(dir.path(), |key| match key {
            ADMIN_PASSWORD_ENV => Some("inline-secret".to_owned()),
            ADMIN_PASSWORD_FILE_ENV => Some("/nonexistent".to_owned()),
            _ => None,
        })
        .expect("inline bootstrap");
        assert_eq!(bootstrap.source(), BootstrapSource::Environment);
        assert_eq!(bootstrap.expose(), "inline-secret");
    }

    #[test]
    fn bootstrap_generates_a_private_file_when_no_source_is_configured() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let bootstrap = resolve_bootstrap(dir.path(), |_| None).expect("generated bootstrap");
        assert_eq!(bootstrap.source(), BootstrapSource::Generated);
        assert_eq!(bootstrap.expose().len(), GENERATED_PASSWORD_BYTES * 2);
        let path = generated_bootstrap_path(dir.path());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path)
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "bootstrap file must be 0600");
        }
        let again = resolve_bootstrap(dir.path(), |_| None).expect("reused bootstrap");
        assert_eq!(again.expose(), bootstrap.expose());
    }

    #[test]
    fn a_world_readable_secret_file_is_rejected() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join("secret");
        std::fs::write(&path, "hunter2\n").expect("write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
            let error = resolve_bootstrap(dir.path(), |key| match key {
                ADMIN_PASSWORD_FILE_ENV => Some(path.display().to_string()),
                _ => None,
            })
            .expect_err("0644 secret must be rejected");
            assert_eq!(error.code, mien_core::ErrorCode::InternalUnexpected);
        }
    }

    #[test]
    fn hashing_round_trips_and_rejects_other_passwords() {
        let (phc, params) = hash_password("correct horse battery").expect("hash");
        assert!(phc.starts_with("$argon2id$"));
        assert!(verify_password("correct horse battery", &phc, &params));
        assert!(!verify_password("wrong horse battery", &phc, &params));
        assert!(!verify_password(
            "correct horse battery",
            "not-a-phc",
            &params
        ));
    }
}

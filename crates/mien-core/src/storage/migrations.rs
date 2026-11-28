//! SQLite 迁移执行。迁移文件在仓库根 `migrations/`，按整数版本递增。

use serde::Serialize;

use super::map_sqlx_error;
use crate::error::CoreError;

/// Run migration files in order, each in a transaction, and verify SQLite integrity.
pub async fn apply_all(
    pool: &sqlx::SqlitePool,
    files: &[(u32, &str, &str)],
) -> Result<SchemaVersion, CoreError> {
    for (version, name, sql) in files {
        let exists = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT version FROM schema_migrations WHERE version = ?1",
        )
        .bind(*version as i64)
        .fetch_optional(pool)
        .await
        .map_err(map_sqlx_error)
        .unwrap_or_default()
        .flatten()
        .is_some();
        if exists {
            continue;
        }
        let mut transaction = pool.begin().await.map_err(map_sqlx_error)?;
        sqlx::raw_sql(sql)
            .execute(&mut *transaction)
            .await
            .map_err(map_sqlx_error)?;
        sqlx::query("INSERT INTO schema_migrations(version, name, applied_at) VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ','now'))")
            .bind(*version as i64)
            .bind(*name)
            .execute(&mut *transaction)
            .await
            .map_err(map_sqlx_error)?;
        transaction.commit().await.map_err(map_sqlx_error)?;
    }
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(pool)
        .await
        .map_err(map_sqlx_error)?;
    if integrity != "ok" {
        return Err(CoreError::StorageIntegrityFailed);
    }
    current_version_async_pool(pool).await
}

/// Open a single-writer SQLite pool with the required safety pragmas.
///
/// The database file is created `0600` (`docs/SECURITY.md` §5.3: `/data/db` is
/// readable only by the control-plane UID). `create_if_missing` alone yields the
/// process umask, which is `0644` under the default `022` and would leave the
/// database world-readable inside the container.
pub async fn connect(database_url: &str) -> Result<sqlx::SqlitePool, CoreError> {
    let options = database_url
        .parse::<sqlx::sqlite::SqliteConnectOptions>()
        .map_err(|_| CoreError::Storage)?
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(std::time::Duration::from_secs(5));
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(map_sqlx_error)?;
    // `sqlite://` carries no permission bits; tighten the file the driver just
    // created. WAL/SHM siblings inherit the directory mode and are covered by
    // the `0700` data directory.
    if let Some(path) = database_url.strip_prefix("sqlite://") {
        restrict_database_permissions(std::path::Path::new(path));
    }
    Ok(pool)
}

/// Best-effort `0600` on the database file.
///
/// A failure is not an error: the file may live on a filesystem without POSIX
/// modes, and the container's `0700` data directory still bounds access.
fn restrict_database_permissions(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// Open a database and apply its ordered embedded migrations.
pub fn open_and_migrate(path: &std::path::Path) -> Result<sqlx::SqlitePool, CoreError> {
    let database_url = format!("sqlite://{}", path.display());
    let runtime = tokio::runtime::Runtime::new().map_err(|_| CoreError::Storage)?;
    let pool = runtime.block_on(connect(&database_url))?;
    const FILES: &[(u32, &str, &str)] = &[
        (
            1,
            "init",
            include_str!("../../../../migrations/0001_init.sql"),
        ),
        (
            2,
            "profile_base",
            include_str!("../../../../migrations/0002_profile_base.sql"),
        ),
        (
            3,
            "profile_relations",
            include_str!("../../../../migrations/0003_profile_relations.sql"),
        ),
        (
            4,
            "proxy_outbounds",
            include_str!("../../../../migrations/0004_proxy_outbounds.sql"),
        ),
        (
            5,
            "admin_credentials",
            include_str!("../../../../migrations/0005_admin_credentials.sql"),
        ),
        (
            6,
            "api_tokens",
            include_str!("../../../../migrations/0006_api_tokens.sql"),
        ),
        (
            7,
            "idempotency_columns",
            include_str!("../../../../migrations/0007_idempotency_columns.sql"),
        ),
        (
            8,
            "event_log",
            include_str!("../../../../migrations/0008_event_log.sql"),
        ),
        (
            9,
            "kernel_installs",
            include_str!("../../../../migrations/0009_kernel_installs.sql"),
        ),
    ];
    runtime.block_on(apply_all(&pool, FILES))?;
    Ok(pool)
}

/// Read current applied migration version from an already open database.
pub fn current_version(pool: &sqlx::SqlitePool) -> Result<u32, CoreError> {
    let runtime = tokio::runtime::Runtime::new().map_err(|_| CoreError::Storage)?;
    runtime
        .block_on(current_version_async(pool))
        .map(|version| version.0)
}

/// Read-only compatibility query for existing callers that supply a database URL.
pub fn current_version_for_url(database_url: &str) -> Result<SchemaVersion, CoreError> {
    let runtime = tokio::runtime::Runtime::new().map_err(|_| CoreError::Storage)?;
    let pool = runtime.block_on(connect(database_url))?;
    runtime.block_on(current_version_async(&pool))
}

/// Read the highest committed migration version.
pub async fn current_version_async(pool: &sqlx::SqlitePool) -> Result<SchemaVersion, CoreError> {
    let value = sqlx::query_scalar::<_, Option<i64>>("SELECT max(version) FROM schema_migrations")
        .fetch_one(pool)
        .await
        .map_err(map_sqlx_error)?
        .unwrap_or(0);
    Ok(SchemaVersion(
        value.try_into().map_err(|_| CoreError::Storage)?,
    ))
}

pub(crate) async fn current_version_async_pool(
    pool: &sqlx::SqlitePool,
) -> Result<SchemaVersion, CoreError> {
    let value = sqlx::query_scalar::<_, Option<i64>>("SELECT max(version) FROM schema_migrations")
        .fetch_one(pool)
        .await
        .map_err(map_sqlx_error)?
        .unwrap_or(0);
    Ok(SchemaVersion(
        value.try_into().map_err(|_| CoreError::Storage)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_version_rejects_an_empty_database() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let pool = runtime
            .block_on(connect("sqlite::memory:"))
            .expect("empty database");
        assert!(matches!(current_version(&pool), Err(CoreError::Storage)));
    }
}

/// 迁移版本号；接 T9-A 后由 `schema_migrations` 读出。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SchemaVersion(pub u32);

#[cfg(test)]
mod permission_tests {
    use super::*;

    /// `SECURITY.md` §5.3: the database is readable only by the control-plane
    /// UID. A database created under the default umask must not stay `0644`.
    #[test]
    fn a_created_database_is_owner_only() {
        let dir = std::env::temp_dir().join(format!(
            "mien-db-mode-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |value| value.as_nanos())
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("mien.db");
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let pool = runtime
            .block_on(connect(&format!("sqlite://{}", path.display())))
            .expect("connect");
        drop(pool);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "database must be owner-only, got {mode:o}");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}

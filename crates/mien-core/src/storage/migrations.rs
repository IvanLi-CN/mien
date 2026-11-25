//! SQLite 迁移执行。迁移文件在仓库根 `migrations/`，按整数版本递增。

use serde::Serialize;

use crate::error::CoreError;

/// 已提交迁移的最大版本号；空库为 0。
///
/// # Errors
/// 数据库不可读或迁移记录损坏。
pub fn current_version(database_url: &str) -> Result<SchemaVersion, CoreError> {
    let runtime = tokio::runtime::Runtime::new().map_err(|_| CoreError::Storage)?;
    runtime.block_on(current_version_async(database_url))
}

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
pub async fn connect(database_url: &str) -> Result<sqlx::SqlitePool, CoreError> {
    let options = database_url
        .parse::<sqlx::sqlite::SqliteConnectOptions>()
        .map_err(|_| CoreError::Storage)?
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(std::time::Duration::from_secs(5));
    sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(map_sqlx_error)
}

/// Read the highest committed migration version.
pub async fn current_version_async(database_url: &str) -> Result<SchemaVersion, CoreError> {
    let pool = connect(database_url).await?;
    let value = sqlx::query_scalar::<_, Option<i64>>("SELECT max(version) FROM schema_migrations")
        .fetch_one(&pool)
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

fn map_sqlx_error(error: sqlx::Error) -> CoreError {
    match &error {
        sqlx::Error::Database(database) if database.code().as_deref() == Some("5") => {
            CoreError::StorageBusy
        }
        _ => CoreError::Storage,
    }
}

/// 迁移版本号；接 T9-A 后由 `schema_migrations` 读出。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SchemaVersion(pub u32);

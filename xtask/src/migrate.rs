//! 迁移执行。临时库默认建在 `target/tmp/`，成功后由 [`db::TempDatabase`] 清理。

use std::path::Path;

use anyhow::Context;

use crate::db::{self, TempDatabase};

/// 省略 `database` 时新建临时库；给出路径时在其上执行并保留。
pub fn run(root: &Path, database: Option<&Path>) -> anyhow::Result<()> {
    let temp = match database {
        Some(_) => None,
        None => Some(TempDatabase::new(false)?),
    };
    let url = match (&temp, database) {
        (Some(t), _) => t.url(),
        (None, Some(path)) => format!("sqlite://{}", path.display()),
        (None, None) => unreachable!("两种来源必居其一"),
    };
    let version = tokio::runtime::Runtime::new()
        .context("建立 tokio 运行时")?
        .block_on(db::run_all(&url))?;
    println!("migrations ok: {version} 个版本已提交");
    let _ = root;
    Ok(())
}

/// 读取已提交的最大版本；空表或表不存在返回 0。
pub async fn current_version(url: &str) -> anyhow::Result<i64> {
    let pool = db::connect(url)?;
    let version: Option<i64> = sqlx::query_scalar("SELECT max(version) FROM schema_migrations")
        .fetch_one(&pool)
        .await?;
    Ok(version.unwrap_or(0))
}

/// 迁移文件中的最高版本；无文件为 0。
pub fn expected_version() -> anyhow::Result<i64> {
    let files = crate::db::TempDatabase::migration_files()?;
    files
        .iter()
        .filter_map(|f| f.file_stem()?.to_str())
        .filter_map(|s| s.split('_').next()?.parse::<i64>().ok())
        .max()
        .context("无可用迁移版本")
}

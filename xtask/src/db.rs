//! 临时 SQLite 库上的迁移执行器，供 `xtask migrate` 使用。
//!
//! 逐个 `migrations/NNNN_name.sql` 在事务中执行并写入 `schema_migrations`；
//! `0001_init.sql` 自己建 `schema_migrations`，故首次执行时先确保表存在。

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Pool, Sqlite};

use crate::{migrate, repo_root};

pub struct TempDatabase {
    path: PathBuf,
    keep: bool,
}

impl TempDatabase {
    /// 仓库 `target/tmp` 下的唯一路径；随进程退出删除，除非 `keep`。
    pub fn new(keep: bool) -> anyhow::Result<Self> {
        let dir = repo_root()?.join("target/tmp");
        std::fs::create_dir_all(&dir).with_context(|| format!("创建 {} 失败", dir.display()))?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        Ok(Self {
            path: dir.join(format!("migrate-{}-{stamp}.sqlite", std::process::id())),
            keep,
        })
    }

    pub fn url(&self) -> String {
        format!("sqlite://{}", self.path.display())
    }

    /// 迁移文件按文件名（即整数版本）升序。
    pub fn migration_files() -> anyhow::Result<Vec<PathBuf>> {
        let dir = repo_root()?.join("migrations");
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .with_context(|| format!("读取 {} 失败", dir.display()))?
            .map(|entry| entry.map(|e| e.path()))
            .collect::<Result<_, _>>()?;
        files.retain(|p| p.extension().is_some_and(|e| e == "sql"));
        files.sort();
        if files.is_empty() {
            bail!("{} 中没有迁移文件", dir.display());
        }
        Ok(files)
    }
}

impl Drop for TempDatabase {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_file(&self.path);
            for suffix in ["-wal", "-shm"] {
                let mut side = self.path.as_os_str().to_owned();
                side.push(suffix);
                let _ = std::fs::remove_file(PathBuf::from(side));
            }
        }
    }
}

pub fn connect(url: &str) -> anyhow::Result<Pool<Sqlite>> {
    let options: SqliteConnectOptions = url
        .parse::<SqliteConnectOptions>()?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(std::time::Duration::from_secs(5));
    Ok(SqlitePoolOptions::new()
        .max_connections(1)
        .connect_lazy_with(options))
}

/// 执行全部迁移，返回最终版本。
pub async fn run_all(url: &str) -> anyhow::Result<i64> {
    let pool = connect(url)?;
    for file in TempDatabase::migration_files()? {
        let (version, name) = parse_name(&file)?;
        let sql = std::fs::read_to_string(&file)?;
        let mut tx = pool.begin().await?;
        sqlx::raw_sql(&sql)
            .execute(&mut *tx)
            .await
            .with_context(|| format!("执行 {} 失败", file.display()))?;
        sqlx::query("INSERT INTO schema_migrations (version, name, applied_at) VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))")
            .bind(version)
            .bind(&name)
            .execute(&mut *tx)
            .await
            .with_context(|| format!("登记迁移 {version} 失败"))?;
        tx.commit().await?;
    }
    drop(pool);
    migrate::current_version(url).await
}

/// `0001_init.sql` → `(1, "init")`。
fn parse_name(file: &Path) -> anyhow::Result<(i64, String)> {
    let stem = file
        .file_stem()
        .and_then(|s| s.to_str())
        .with_context(|| format!("迁移文件名非法：{}", file.display()))?;
    let (version, name) = stem
        .split_once('_')
        .with_context(|| format!("迁移文件名需为 NNNN_name.sql：{stem}"))?;
    Ok((
        version
            .parse()
            .with_context(|| format!("版本非整数：{version}"))?,
        name.to_owned(),
    ))
}

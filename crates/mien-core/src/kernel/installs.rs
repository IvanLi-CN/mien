//! 内核安装记录：把"安装那一刻的可执行文件摘要"持久化。
//!
//! 没有这条记录时，启动校验拿刚算出的摘要跟它自己比，永远相等——校验存在但
//! 什么也没证明。有记录后，比对的一边是安装时的字节，另一边是现在的字节。
use sqlx::{Row, SqlitePool};

use crate::error::{CoreError, ErrorCode};
use crate::storage::now_utc;

/// 一条安装记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelInstall {
    pub version: String,
    pub platform: String,
    pub executable_sha256: String,
    pub executable_path: String,
    pub source_asset_sha256: Option<String>,
}

fn storage() -> CoreError {
    CoreError::Storage
}

/// 记录一次安装。同一 `(version, platform)` 重复安装覆盖旧摘要。
///
/// # Errors
///
/// 写库失败。
pub async fn record(pool: &SqlitePool, install: &KernelInstall) -> Result<(), CoreError> {
    sqlx::query(
        "INSERT INTO kernel_installs \
         (version,platform,executable_sha256,executable_path,source_asset_sha256,created_at) \
         VALUES (?1,?2,?3,?4,?5,?6) \
         ON CONFLICT(version,platform) DO UPDATE SET \
           executable_sha256=excluded.executable_sha256, \
           executable_path=excluded.executable_path, \
           source_asset_sha256=excluded.source_asset_sha256, \
           created_at=excluded.created_at",
    )
    .bind(&install.version)
    .bind(&install.platform)
    .bind(&install.executable_sha256)
    .bind(&install.executable_path)
    .bind(&install.source_asset_sha256)
    .bind(now_utc())
    .execute(pool)
    .await
    .map_err(|_| storage())?;
    Ok(())
}

/// 读取一次安装记录。
///
/// # Errors
///
/// 读库失败。
pub async fn lookup(
    pool: &SqlitePool,
    version: &str,
    platform: &str,
) -> Result<Option<KernelInstall>, CoreError> {
    let row = sqlx::query(
        "SELECT version,platform,executable_sha256,executable_path,source_asset_sha256 \
         FROM kernel_installs WHERE version=?1 AND platform=?2",
    )
    .bind(version)
    .bind(platform)
    .fetch_optional(pool)
    .await
    .map_err(|_| storage())?;
    Ok(row.map(|row| KernelInstall {
        version: row.get("version"),
        platform: row.get("platform"),
        executable_sha256: row.get("executable_sha256"),
        executable_path: row.get("executable_path"),
        source_asset_sha256: row.get("source_asset_sha256"),
    }))
}

/// 校验磁盘上的可执行文件与安装记录一致。
///
/// 没有记录时拒绝：未登记的内核无法证明来自某个发布物，允许它等于允许任意
/// 二进制冒充内核。
///
/// # Errors
///
/// 未登记返回 `KernelNotInstalled`；摘要不符返回 `KernelChecksumMismatch`。
pub async fn verify(
    pool: &SqlitePool,
    version: &str,
    platform: &str,
    actual_sha256: &str,
) -> Result<(), CoreError> {
    let install = lookup(pool, version, platform).await?.ok_or_else(|| {
        CoreError::validation(
            ErrorCode::KernelChecksumMismatch,
            "kernel has no install record; reinstall it before launching",
        )
    })?;
    if install.executable_sha256 != actual_sha256.to_ascii_lowercase() {
        return Err(CoreError::validation(
            ErrorCode::KernelChecksumMismatch,
            "kernel executable digest does not match the install record",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::open_and_migrate;

    /// `open_and_migrate` 自带 runtime，所以这些用例用同步 `#[test]` 并在各自的
    /// runtime 里跑：在 `#[tokio::test]` 里调用它会撞上"runtime 内建 runtime"。
    fn temp_db() -> (tempfile::TempDir, SqlitePool, tokio::runtime::Runtime) {
        let directory = tempfile::tempdir().expect("tempdir");
        let pool = open_and_migrate(&directory.path().join("mien.db")).expect("migrate");
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        (directory, pool, runtime)
    }

    #[test]
    fn an_unregistered_kernel_is_refused() {
        let (_guard, pool, runtime) = temp_db();
        let error = runtime
            .block_on(verify(&pool, "148.0.7778.215", "macos-arm64", "abc"))
            .expect_err("an unregistered kernel must not pass");
        assert_eq!(error.code(), ErrorCode::KernelChecksumMismatch);
        assert!(
            format!("{error}").contains("no install record"),
            "the message must say why: {error}"
        );
    }

    #[test]
    fn a_recorded_digest_must_still_match_the_file() {
        let (_guard, pool, runtime) = temp_db();
        runtime
            .block_on(record(
                &pool,
                &KernelInstall {
                    version: "148.0.7778.215".into(),
                    platform: "macos-arm64".into(),
                    executable_sha256: "aa".repeat(32),
                    executable_path: "/opt/mien/kernel".into(),
                    source_asset_sha256: None,
                },
            ))
            .expect("record");

        runtime
            .block_on(verify(
                &pool,
                "148.0.7778.215",
                "macos-arm64",
                &"aa".repeat(32),
            ))
            .expect("the recorded digest passes");

        let error = runtime
            .block_on(verify(
                &pool,
                "148.0.7778.215",
                "macos-arm64",
                &"bb".repeat(32),
            ))
            .expect_err("a swapped kernel must be refused");
        assert_eq!(error.code(), ErrorCode::KernelChecksumMismatch);
    }

    #[test]
    fn reinstalling_replaces_the_digest() {
        let (_guard, pool, runtime) = temp_db();
        let install = |sha: String| KernelInstall {
            version: "148.0.7778.215".into(),
            platform: "macos-arm64".into(),
            executable_sha256: sha,
            executable_path: "/opt/mien/kernel".into(),
            source_asset_sha256: None,
        };
        runtime
            .block_on(record(&pool, &install("cc".repeat(32))))
            .expect("first");
        runtime
            .block_on(record(&pool, &install("dd".repeat(32))))
            .expect("second");
        runtime
            .block_on(verify(
                &pool,
                "148.0.7778.215",
                "macos-arm64",
                &"dd".repeat(32),
            ))
            .expect("the latest install wins");
        assert!(
            runtime
                .block_on(verify(
                    &pool,
                    "148.0.7778.215",
                    "macos-arm64",
                    &"cc".repeat(32),
                ))
                .is_err()
        );
    }

    #[test]
    fn platforms_are_recorded_separately() {
        let (_guard, pool, runtime) = temp_db();
        runtime
            .block_on(record(
                &pool,
                &KernelInstall {
                    version: "148.0.7778.215".into(),
                    platform: "macos-arm64".into(),
                    executable_sha256: "ee".repeat(32),
                    executable_path: "/opt/mien/kernel".into(),
                    source_asset_sha256: None,
                },
            ))
            .expect("record");
        let error = runtime
            .block_on(verify(
                &pool,
                "148.0.7778.215",
                "linux-x86_64",
                &"ee".repeat(32),
            ))
            .expect_err("another platform is a different install");
        assert_eq!(error.code(), ErrorCode::KernelChecksumMismatch);
    }
}

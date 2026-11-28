//! 档案导出关系读取与整批导入事务（`REQ-PROFILE-009`/`REQ-PROFILE-010`）。
//!
//! 导出只需要分组名、标签名与颜色：分组/标签是全局共享资源，导入按名称匹配。
//!
//! 导入在**单个事务**内写代理行、档案行、分组、标签与 `profile_tags`。目录创建前先把
//! 本请求的新目录清单写入 `<data-root>/runtimes/imports/<request-id>.json`（0600，
//! 只含 ULID 与相对路径，不含配置、口令或凭据）；失败先回滚全部行，再只删除清单内、
//! 且不对应任何已提交档案行的目录。清理失败返回
//! [`CoreError::ProfileImportCleanupFailed`] 并保留清单，绝不重建已回滚的档案行。

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Row, Sqlite, Transaction};

/// 让只读连接池的调用方（宿主层）不必自己依赖 sqlx 的版本。
pub use sqlx::SqlitePool;

use super::map_sqlx_error;
use crate::domain::profile::{Profile, ProfileId, ProfileMode, ProfileStatus};
use crate::error::{CoreError, ErrorCode};
use crate::fingerprint::spec::FingerprintSpec;
use crate::ports::proxy::{ProxyId, ProxyKind};
use crate::secrets::MasterKey;
use crate::storage::sqlite::{create_profile_dirs, validate_relative_path};

/// 导入清单目录，位于数据根下。
const IMPORT_MANIFEST_DIR: &str = "runtimes/imports";

/// 导出时携带的分组：名称与排序。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedGroup {
    pub name: String,
    pub sort_order: i64,
}

/// 导出时携带的标签：名称与可选颜色。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedTag {
    pub name: String,
    pub color: Option<String>,
}

/// 一个档案的分组与标签关系。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProfileRelations {
    pub group: Option<ExportedGroup>,
    pub tags: Vec<ExportedTag>,
}

/// 读取档案的分组与标签；不存在的关系返回空集合。
///
/// # Errors
/// 存储不可读。
pub async fn relations(pool: &SqlitePool, profile_id: &str) -> Result<ProfileRelations, CoreError> {
    let group = sqlx::query(
        "SELECT g.name, g.sort_order FROM profiles p JOIN groups g ON g.id = p.group_id WHERE p.id = ?1",
    )
    .bind(profile_id)
    .fetch_optional(pool)
    .await
    .map_err(map_sqlx_error)?
    .map(|row| ExportedGroup {
        name: row.get("name"),
        sort_order: row.get("sort_order"),
    });
    let tags = sqlx::query(
        "SELECT t.name, t.color FROM tags t JOIN profile_tags pt ON pt.tag_id = t.id \
         WHERE pt.profile_id = ?1 ORDER BY t.name",
    )
    .bind(profile_id)
    .fetch_all(pool)
    .await
    .map_err(map_sqlx_error)?
    .into_iter()
    .map(|row| ExportedTag {
        name: row.get("name"),
        color: row.get("color"),
    })
    .collect();
    Ok(ProfileRelations { group, tags })
}

/// 档案的稳定导出身份：导入来源优先，其次自身 `id`。
///
/// 导入档案再导出时必须沿用最初的 `source_profile_id`，否则「同一来源」会在
/// 多跳导入后退化成不同标识。
///
/// # Errors
/// 存储不可读。
pub async fn source_identity(
    pool: &SqlitePool,
    profile_id: &str,
) -> Result<Option<String>, CoreError> {
    let row = sqlx::query("SELECT id, import_source_id FROM profiles WHERE id = ?1")
        .bind(profile_id)
        .fetch_optional(pool)
        .await
        .map_err(map_sqlx_error)?;
    Ok(row.map(|row| {
        row.get::<Option<String>, _>("import_source_id")
            .unwrap_or_else(|| row.get("id"))
    }))
}

/// 导出项 `source_profile_id` 在目标库中冲突的来源集合。
///
/// 冲突判据与 `REQ-PROFILE-009` 一致：命中现有 `profiles.import_source_id`
/// 或现有 `profiles.id`。
///
/// # Errors
/// 存储不可读。
pub async fn find_import_conflicts(
    pool: &SqlitePool,
    source_ids: &[String],
) -> Result<BTreeSet<String>, CoreError> {
    let mut conflicts = BTreeSet::new();
    for source_id in source_ids {
        let exists: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM profiles WHERE id = ?1 OR import_source_id = ?1 LIMIT 1",
        )
        .bind(source_id)
        .fetch_optional(pool)
        .await
        .map_err(map_sqlx_error)?;
        if exists.is_some() {
            conflicts.insert(source_id.clone());
        }
    }
    Ok(conflicts)
}

/// 冲突策略（`ProfileImportInput.conflict_strategy`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictStrategy {
    /// 为冲突项分配新 ULID 并保留来源标识。
    NewId,
    /// 任何冲突都在写入前拒绝。
    Abort,
}

/// 导入时重建的代理非秘密配置。
#[derive(Debug, Clone)]
pub struct ImportProxy {
    pub kind: ProxyKind,
    pub server: String,
    pub port: u16,
    pub options: BTreeMap<String, Value>,
}

/// 导入时按名称匹配/创建的标签。
#[derive(Debug, Clone)]
pub struct ImportTag {
    pub name: String,
    pub color: Option<String>,
}

/// 一个待导入档案的领域输入；字段已在传输层按导出 schema 校验。
#[derive(Debug, Clone)]
pub struct ImportProfile {
    pub source_profile_id: String,
    pub name: String,
    pub group: Option<String>,
    pub group_sort_order: i64,
    pub tags: Vec<ImportTag>,
    pub notes: Option<String>,
    pub kernel_version: String,
    pub fingerprint: FingerprintSpec,
    pub mode: ProfileMode,
    pub automation_api: bool,
    pub start_url: Option<String>,
    pub proxy: Option<ImportProxy>,
}

/// 一个已提交的导入结果：档案行、匹配到的分组与标签名。
///
/// 返回关系列避免提交后再查询一次：提交后的读取失败不得把「已导入」变成
/// 「部分结果」的 500。
#[derive(Debug, Clone)]
pub struct ImportedProfile {
    pub profile: Profile,
    pub group_id: Option<String>,
    pub tags: Vec<String>,
}

/// 一批导入请求。
pub struct ImportBatch<'a> {
    pub request_id: &'a str,
    pub data_root: &'a Path,
    pub master_key: Option<&'a MasterKey>,
    pub strategy: ConflictStrategy,
    pub profiles: Vec<ImportProfile>,
}

/// 请求目录清单；只记录本请求新建的目录。
#[derive(Debug, Serialize, Deserialize)]
struct ImportManifest {
    request_id: String,
    created_at: String,
    directories: Vec<String>,
}

/// 在单个事务内导入全部档案。
///
/// 成功返回按输入顺序排列的档案。任何失败都先回滚全部行，再清理本请求的新目录；
/// 清理失败返回 [`CoreError::ProfileImportCleanupFailed`]，此时行已回滚、清单仍保留。
///
/// # Errors
/// 冲突（`Abort`）、存储失败或目录清理失败。
pub async fn insert_batch(
    pool: &SqlitePool,
    batch: ImportBatch<'_>,
) -> Result<Vec<ImportedProfile>, CoreError> {
    if batch.profiles.is_empty() {
        return Err(CoreError::validation(
            ErrorCode::ValidationFailed,
            "an import must contain at least one profile",
        ));
    }
    // 代理行需要主密钥加密 options；缺密钥时在任何写入或建目录之前失败。
    if batch.master_key.is_none() && batch.profiles.iter().any(|profile| profile.proxy.is_some()) {
        return Err(CoreError::Storage);
    }
    let mut transaction = pool.begin().await.map_err(map_sqlx_error)?;

    // 冲突判定必须在任何写入之前（`abort` 不产生档案、目录、分组或标签）。
    for profile in &batch.profiles {
        if batch.strategy == ConflictStrategy::Abort
            && profile_conflicts(&mut transaction, &profile.source_profile_id).await?
        {
            let _ = transaction.rollback().await;
            return Err(CoreError::ProfileImportConflict);
        }
    }

    let ids: Vec<ProfileId> = batch.profiles.iter().map(|_| ProfileId::new()).collect();
    let mut manifest = ImportManifest {
        request_id: batch.request_id.to_owned(),
        created_at: crate::storage::now_utc(),
        directories: Vec::new(),
    };
    if let Err(error) = write_manifest_and_dirs(batch.data_root, &mut manifest, &ids) {
        let _ = transaction.rollback().await;
        return Err(cleanup_or(error, pool, batch.data_root, &manifest).await);
    }

    match write_rows(&mut transaction, &batch, &ids).await {
        Ok(inserted) => match transaction.commit().await {
            Ok(()) => {
                // 已提交的目录不再需要清理清单；删除失败也不影响正确性：清理会先
                // 按档案行判定，已提交档案的目录永不被删除。
                let _ = std::fs::remove_file(manifest_path(batch.data_root, &manifest.request_id));
                Ok(inserted)
            }
            Err(error) => {
                let error = map_sqlx_error(error);
                Err(cleanup_or(error, pool, batch.data_root, &manifest).await)
            }
        },
        Err(error) => {
            let _ = transaction.rollback().await;
            Err(cleanup_or(error, pool, batch.data_root, &manifest).await)
        }
    }
}

/// 回滚后清理本请求目录；清理失败时以 `PROFILE_IMPORT_CLEANUP_FAILED` 取代原错误。
async fn cleanup_or(
    error: CoreError,
    pool: &SqlitePool,
    data_root: &Path,
    manifest: &ImportManifest,
) -> CoreError {
    if cleanup_import(pool, data_root, manifest).await {
        error
    } else {
        CoreError::ProfileImportCleanupFailed
    }
}

async fn profile_conflicts(
    transaction: &mut Transaction<'_, Sqlite>,
    source_profile_id: &str,
) -> Result<bool, CoreError> {
    let exists: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM profiles WHERE id = ?1 OR import_source_id = ?1 LIMIT 1")
            .bind(source_profile_id)
            .fetch_optional(&mut **transaction)
            .await
            .map_err(map_sqlx_error)?;
    Ok(exists.is_some())
}

async fn write_rows(
    transaction: &mut Transaction<'_, Sqlite>,
    batch: &ImportBatch<'_>,
    ids: &[ProfileId],
) -> Result<Vec<ImportedProfile>, CoreError> {
    let mut inserted = Vec::with_capacity(batch.profiles.len());
    for (input, id) in batch.profiles.iter().zip(ids) {
        let group_id = match &input.group {
            Some(name) => Some(ensure_group(transaction, name, input.group_sort_order).await?),
            None => None,
        };
        let proxy_id = match &input.proxy {
            Some(proxy) => {
                let key = batch.master_key.expect("checked before the transaction");
                let proxy_id = ProxyId::new();
                let redacted = super::proxy::RedactedProxy {
                    name: proxy_name(&input.name),
                    kind: proxy.kind,
                    server: proxy.server.clone(),
                    port: proxy.port,
                    options: proxy.options.clone(),
                };
                super::proxy::insert_redacted(transaction, key, &proxy_id, &redacted).await?;
                Some(proxy_id.0)
            }
            None => None,
        };

        let fingerprint_json =
            serde_json::to_string(&input.fingerprint).map_err(|_| CoreError::Storage)?;
        let now = crate::storage::now_utc();
        sqlx::query(
            "INSERT INTO profiles \
             (id,name,group_id,notes,fingerprint_json,fingerprint_schema,kernel_version,proxy_id,\
              import_source_id,mode,status,start_url,automation_api,created_at,updated_at) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,'active',?11,?12,?13,?13)",
        )
        .bind(&id.0)
        .bind(&input.name)
        .bind(&group_id)
        .bind(&input.notes)
        .bind(fingerprint_json)
        .bind(i64::from(input.fingerprint.schema_version))
        .bind(&input.kernel_version)
        .bind(&proxy_id)
        .bind(&input.source_profile_id)
        .bind(input.mode.as_str())
        .bind(&input.start_url)
        .bind(i64::from(input.automation_api))
        .bind(&now)
        .execute(&mut **transaction)
        .await
        .map_err(map_sqlx_error)?;

        for tag in &input.tags {
            let tag_id = ensure_tag(transaction, &tag.name, tag.color.as_deref()).await?;
            sqlx::query("INSERT OR IGNORE INTO profile_tags (profile_id, tag_id) VALUES (?1, ?2)")
                .bind(&id.0)
                .bind(tag_id)
                .execute(&mut **transaction)
                .await
                .map_err(map_sqlx_error)?;
        }

        inserted.push(ImportedProfile {
            group_id: group_id.clone(),
            tags: input.tags.iter().map(|tag| tag.name.clone()).collect(),
            profile: Profile {
                id: id.clone(),
                name: input.name.clone(),
                status: ProfileStatus::Active,
                fingerprint: input.fingerprint.clone(),
                start_url: input.start_url.clone(),
                notes: input.notes.clone(),
                kernel_version: input.kernel_version.clone(),
                mode: input.mode,
                automation_api: input.automation_api,
                proxy_id,
                created_at: now.clone(),
                updated_at: now,
                archived_at: None,
                deleted_at: None,
                deletion_error: None,
            },
        });
    }
    Ok(inserted)
}

/// 分组按名称匹配；缺失时用导出声明的 `sort_order` 新建。
async fn ensure_group(
    transaction: &mut Transaction<'_, Sqlite>,
    name: &str,
    sort_order: i64,
) -> Result<String, CoreError> {
    if let Some(id) = sqlx::query_scalar::<_, String>("SELECT id FROM groups WHERE name = ?1")
        .bind(name)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sqlx_error)?
    {
        return Ok(id);
    }
    let id = ProfileId::new().0;
    let now = crate::storage::now_utc();
    sqlx::query(
        "INSERT INTO groups (id,name,sort_order,created_at,updated_at) VALUES (?1,?2,?3,?4,?4)",
    )
    .bind(&id)
    .bind(name)
    .bind(sort_order)
    .bind(&now)
    .execute(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    Ok(id)
}

/// 标签按名称匹配；缺失时用导出声明的颜色新建。
async fn ensure_tag(
    transaction: &mut Transaction<'_, Sqlite>,
    name: &str,
    color: Option<&str>,
) -> Result<String, CoreError> {
    if let Some(id) = sqlx::query_scalar::<_, String>("SELECT id FROM tags WHERE name = ?1")
        .bind(name)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sqlx_error)?
    {
        return Ok(id);
    }
    let id = ProfileId::new().0;
    let now = crate::storage::now_utc();
    sqlx::query("INSERT INTO tags (id,name,color,created_at) VALUES (?1,?2,?3,?4)")
        .bind(&id)
        .bind(name)
        .bind(color)
        .bind(&now)
        .execute(&mut **transaction)
        .await
        .map_err(map_sqlx_error)?;
    Ok(id)
}

fn proxy_name(profile_name: &str) -> String {
    let name = format!("{profile_name} (imported proxy)");
    if name.chars().count() > 120 {
        name.chars().take(120).collect()
    } else {
        name
    }
}

/// 先落一条空清单（0600），再逐个创建目录；每成功创建一个就原子重写清单。
///
/// 清单必须先于目录存在（`REQ-PROFILE-009`），而清单只登记**确实由本请求创建**的
/// 目录：进程在创建与登记之间崩溃只会漏记（目录泄漏，安全方向），绝不会把别人的
/// 目录写进清单后被清理。清单目录本身为 0700。
fn write_manifest_and_dirs(
    data_root: &Path,
    manifest: &mut ImportManifest,
    ids: &[ProfileId],
) -> Result<(), CoreError> {
    create_private_dir(&data_root.join(IMPORT_MANIFEST_DIR))?;
    write_manifest(data_root, manifest)?;
    for id in ids {
        // 先登记再创建（`REQ-PROFILE-009` 的清单先于目录），ID 是刚生成的 ULID，
        // 不可能属于现有档案。创建失败时撤回该条登记，清理因此不会碰到我们没建成
        // 的路径（例如被外部预先占用的同名目录）。
        let relative = format!("profiles/{}", id.0);
        manifest.directories.push(relative.clone());
        write_manifest(data_root, manifest)?;
        let directory = validate_relative_path(data_root, Path::new(&relative))?;
        if let Err(error) = create_profile_dirs(&directory) {
            manifest.directories.pop();
            write_manifest(data_root, manifest)?;
            return Err(error);
        }
    }
    Ok(())
}

/// 原子重写清单：同目录临时文件 + `rename`，权限 0600。
fn write_manifest(data_root: &Path, manifest: &ImportManifest) -> Result<(), CoreError> {
    let path = manifest_path(data_root, &manifest.request_id);
    let temporary = path.with_extension("json.tmp");
    let body = serde_json::to_vec(manifest).map_err(|_| CoreError::Storage)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary).map_err(|_| CoreError::Storage)?;
    file.write_all(&body).map_err(|_| CoreError::Storage)?;
    file.sync_all().map_err(|_| CoreError::Storage)?;
    drop(file);
    std::fs::rename(&temporary, &path).map_err(|_| CoreError::Storage)
}

/// 启动对账：清理崩溃遗留的导入清单与它们登记的、仍无档案行的目录。
///
/// 只看一分钟以前的清单：更新的清单可能属于正在进行的导入。每条目录先查
/// `profiles.id`，已提交档案的目录永不删除；删除前规范化路径并拒绝符号链接。
/// 返回被清理的清单数。
///
/// # Errors
/// 存储不可读。
pub async fn reconcile_imports(pool: &SqlitePool, data_root: &Path) -> Result<usize, CoreError> {
    let directory = data_root.join(IMPORT_MANIFEST_DIR);
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return Ok(0);
    };
    let mut reconciled = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        match path.extension().and_then(|extension| extension.to_str()) {
            // 原子重写留下的临时文件：内容不完整且没有目录所有权。
            Some("tmp") => {
                let _ = std::fs::remove_file(&path);
                continue;
            }
            Some("json") => {}
            _ => continue,
        }
        let fresh = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age < std::time::Duration::from_secs(60));
        if fresh {
            continue;
        }
        let Ok(body) = std::fs::read(&path) else {
            continue;
        };
        let Ok(manifest) = serde_json::from_slice::<ImportManifest>(&body) else {
            let _ = std::fs::remove_file(&path);
            continue;
        };
        if cleanup_import(pool, data_root, &manifest).await {
            reconciled += 1;
        }
    }
    Ok(reconciled)
}

fn manifest_path(data_root: &Path, request_id: &str) -> PathBuf {
    data_root
        .join(IMPORT_MANIFEST_DIR)
        .join(format!("{request_id}.json"))
}

fn create_private_dir(path: &Path) -> Result<(), CoreError> {
    if path.is_dir() {
        return Ok(());
    }
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path).map_err(|_| CoreError::Storage)
}

/// 只删除清单内、且不对应已提交档案行的目录；全部成功才移除清单。
async fn cleanup_import(pool: &SqlitePool, data_root: &Path, manifest: &ImportManifest) -> bool {
    let mut failed = false;
    for relative in &manifest.directories {
        let profile_id = relative.strip_prefix("profiles/").unwrap_or_default();
        let committed = sqlx::query_scalar::<_, i64>("SELECT 1 FROM profiles WHERE id = ?1")
            .bind(profile_id)
            .fetch_optional(pool)
            .await;
        match committed {
            Ok(Some(_)) => continue,
            Ok(None) => {}
            Err(_) => {
                failed = true;
                continue;
            }
        }
        let Ok(path) = validate_relative_path(data_root, Path::new(relative)) else {
            failed = true;
            continue;
        };
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => failed = true,
            Ok(metadata) if metadata.is_dir() => {
                if std::fs::remove_dir_all(&path).is_err() {
                    failed = true;
                }
            }
            Ok(_) => failed = true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => failed = true,
        }
    }
    if !failed {
        let _ = std::fs::remove_file(manifest_path(data_root, &manifest.request_id));
        true
    } else {
        false
    }
}

/// 写入档案的分组与标签。`group_id` 为 `None` 时清空分组；标签整体替换。
///
/// 宿主层（Tauri IPC）与 HTTP 服务层都要落这两类关系，SQL 留在 core 里，
/// 两边不会各写一份、各有各的语义。
///
/// # Errors
///
/// 事务失败时返回 `CoreError::Storage`，调用方负责回滚档案本身。
pub async fn write_relations(
    pool: &SqlitePool,
    profile_id: &str,
    group_id: Option<&str>,
    tags: &[String],
) -> Result<(), CoreError> {
    let mut transaction = pool.begin().await.map_err(map_sqlx_error)?;
    sqlx::query("UPDATE profiles SET group_id=?1 WHERE id=?2")
        .bind(group_id)
        .bind(profile_id)
        .execute(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;
    sqlx::query("DELETE FROM profile_tags WHERE profile_id=?1")
        .bind(profile_id)
        .execute(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;
    for tag in tags {
        sqlx::query(
            "INSERT OR IGNORE INTO profile_tags (profile_id, tag_id) \
             SELECT ?1, id FROM tags WHERE name=?2",
        )
        .bind(profile_id)
        .bind(tag)
        .execute(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;
    }
    transaction.commit().await.map_err(map_sqlx_error)?;
    Ok(())
}

/// 读取档案当前的标签名，用于只改分组时保留标签。
///
/// # Errors
///
/// 查询失败时返回 `CoreError::Storage`。
pub async fn tag_names(pool: &SqlitePool, profile_id: &str) -> Result<Vec<String>, CoreError> {
    let tags: Vec<String> = sqlx::query_scalar(
        "SELECT t.name FROM tags t JOIN profile_tags pt ON pt.tag_id = t.id \
         WHERE pt.profile_id=?1 ORDER BY t.name",
    )
    .bind(profile_id)
    .fetch_all(pool)
    .await
    .map_err(map_sqlx_error)?;
    Ok(tags)
}

/// 读取档案当前的分组 ID；`None` 表示未分组。
///
/// # Errors
///
/// 查询失败时返回 `CoreError::Storage`。
pub async fn group_id(pool: &SqlitePool, profile_id: &str) -> Result<Option<String>, CoreError> {
    let group: Option<Option<String>> =
        sqlx::query_scalar("SELECT group_id FROM profiles WHERE id=?1")
            .bind(profile_id)
            .fetch_optional(pool)
            .await
            .map_err(map_sqlx_error)?;
    Ok(group.flatten())
}

/// 批量读取档案的分组 ID 与标签名，给列表视图用。
///
/// 单独查询会退化成每个档案两次往返；列表页一次要显示几十行，所以这里用
/// 两条带 `IN (...)` 的查询取回全部关系，再在内存里归并。
///
/// # Errors
///
/// 存储不可读时返回 `CoreError::Storage`。
pub async fn relations_for(
    pool: &SqlitePool,
    profile_ids: &[String],
) -> Result<BTreeMap<String, (Option<String>, Vec<String>)>, CoreError> {
    let mut out: BTreeMap<String, (Option<String>, Vec<String>)> = BTreeMap::new();
    if profile_ids.is_empty() {
        return Ok(out);
    }
    let placeholders = vec!["?"; profile_ids.len()].join(",");
    let group_sql = format!("SELECT id, group_id FROM profiles WHERE id IN ({placeholders})");
    let mut group_query = sqlx::query(&group_sql);
    for id in profile_ids {
        group_query = group_query.bind(id);
    }
    for row in group_query.fetch_all(pool).await.map_err(map_sqlx_error)? {
        let id: String = row.get("id");
        let group: Option<String> = row.get("group_id");
        out.entry(id).or_default().0 = group;
    }
    let tag_sql = format!(
        "SELECT pt.profile_id AS profile_id, t.name AS name FROM tags t \
         JOIN profile_tags pt ON pt.tag_id = t.id \
         WHERE pt.profile_id IN ({placeholders}) ORDER BY t.name"
    );
    let mut tag_query = sqlx::query(&tag_sql);
    for id in profile_ids {
        tag_query = tag_query.bind(id);
    }
    for row in tag_query.fetch_all(pool).await.map_err(map_sqlx_error)? {
        let id: String = row.get("profile_id");
        let name: String = row.get("name");
        out.entry(id).or_default().1.push(name);
    }
    Ok(out)
}

/// 更新档案的代理绑定；`None` 表示改回直连。
///
/// # Errors
///
/// 存储不可写时返回 `CoreError::Storage`。
pub async fn set_proxy_id(
    pool: &SqlitePool,
    profile_id: &str,
    proxy_id: Option<&str>,
) -> Result<(), CoreError> {
    sqlx::query("UPDATE profiles SET proxy_id=?1 WHERE id=?2")
        .bind(proxy_id)
        .bind(profile_id)
        .execute(pool)
        .await
        .map_err(map_sqlx_error)?;
    Ok(())
}

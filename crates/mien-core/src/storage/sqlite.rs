use std::path::{Path, PathBuf};

use sqlx::{Row, SqlitePool};

use super::map_sqlx_error;
use crate::domain::profile::{
    CopyOptions, CreateProfile, DeletionError, Profile, ProfileId, ProfileMode, ProfilePage,
    ProfilePatch, ProfileQuery, ProfileStatus,
};
use crate::error::{CoreError, ErrorCode};
use crate::fingerprint::spec::{FingerprintSpec, generate_seed};
use crate::ports::repository::ProfileRepository;

/// SQLite-backed profile repository and owner of profile data directories.
#[derive(Clone)]
pub struct SqliteProfileRepository {
    pool: SqlitePool,
    data_root: PathBuf,
}

impl SqliteProfileRepository {
    pub fn new(pool: SqlitePool, data_root: impl Into<PathBuf>) -> Result<Self, CoreError> {
        let data_root = data_root.into();
        std::fs::create_dir_all(&data_root).map_err(|_| CoreError::Storage)?;
        let canonical = data_root.canonicalize().map_err(|_| CoreError::Storage)?;
        std::fs::create_dir_all(canonical.join("profiles")).map_err(|_| CoreError::Storage)?;
        std::fs::create_dir_all(canonical.join("trash")).map_err(|_| CoreError::Storage)?;
        Ok(Self {
            pool,
            data_root: canonical,
        })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn create(&self, input: CreateProfile) -> Result<Profile, CoreError> {
        let profile = crate::domain::profile::create(input)?;
        let dir = self.profile_dir(&profile.id)?;
        create_profile_dirs(&dir)?;
        let result = self.insert_row(&profile).await;
        if result.is_err() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        result.map(|_| profile)
    }

    async fn insert_row(&self, profile: &Profile) -> Result<(), CoreError> {
        let fingerprint =
            serde_json::to_string(&profile.fingerprint).map_err(|_| CoreError::Storage)?;
        sqlx::query("INSERT INTO profiles (id,name,notes,fingerprint_json,fingerprint_schema,kernel_version,mode,status,start_url,automation_api,proxy_id,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,'active',?8,?9,?10,?11,?11)")
            .bind(&profile.id.0).bind(&profile.name).bind(&profile.notes).bind(fingerprint).bind(1i64)
            .bind(&profile.kernel_version).bind(profile.mode.as_str()).bind(&profile.start_url)
            .bind(profile.automation_api as i64).bind(&profile.proxy_id).bind(&profile.created_at)
            .execute(&self.pool)
            .await
            .map_err(|error| map_profile_insert_error(error, profile.proxy_id.is_some()))?;
        Ok(())
    }

    async fn fetch(&self, id: &ProfileId) -> Result<Profile, CoreError> {
        let row = sqlx::query("SELECT id,name,status,fingerprint_json,start_url,notes,kernel_version,mode,automation_api,proxy_id,created_at,updated_at,archived_at,deleted_at,deletion_error FROM profiles WHERE id=?1")
            .bind(&id.0).fetch_optional(&self.pool).await.map_err(map_sqlx_error)?.ok_or(CoreError::NotFound)?;
        profile_from_row(&row)
    }

    fn profile_dir(&self, id: &ProfileId) -> Result<PathBuf, CoreError> {
        if !id.is_valid() {
            return Err(invalid_profile_id());
        }
        safe_child(&self.data_root, Path::new("profiles").join(&id.0))
    }

    fn trash_dir(&self, id: &ProfileId) -> Result<PathBuf, CoreError> {
        if !id.is_valid() {
            return Err(invalid_profile_id());
        }
        safe_child(&self.data_root, Path::new("trash").join(&id.0))
    }

    async fn update_fields(
        &self,
        id: &ProfileId,
        patch: ProfilePatch,
    ) -> Result<Profile, CoreError> {
        let current = self.fetch(id).await?;
        if current.status == ProfileStatus::Deleted {
            return Err(CoreError::ProfileDeleted);
        }
        if let Some(name) = &patch.name {
            if name.trim().is_empty() || name.chars().count() > 120 {
                return Err(CoreError::validation(
                    ErrorCode::ValidationFailed,
                    "name must be 1-120 characters",
                ));
            }
        }
        let fingerprint = patch
            .fingerprint
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|_| CoreError::Storage)?;
        let now = crate::storage::now_utc();
        sqlx::query("UPDATE profiles SET name=COALESCE(?1,name),notes=COALESCE(?2,notes),fingerprint_json=COALESCE(?3,fingerprint_json),start_url=COALESCE(?4,start_url),kernel_version=COALESCE(?5,kernel_version),mode=COALESCE(?6,mode),automation_api=COALESCE(?7,automation_api),updated_at=?8 WHERE id=?9")
            .bind(patch.name).bind(patch.notes).bind(fingerprint).bind(patch.start_url).bind(patch.kernel_version)
            .bind(patch.mode.map(ProfileMode::as_str)).bind(patch.automation_api.map(i64::from)).bind(now).bind(&id.0)
            .execute(&self.pool).await.map_err(map_sqlx_error)?;
        self.fetch(id).await
    }

    async fn transition(
        &self,
        id: &ProfileId,
        target: ProfileStatus,
    ) -> Result<Profile, CoreError> {
        let current = self.fetch(id).await?;
        let allowed = matches!(
            (current.status, target),
            (ProfileStatus::Active, ProfileStatus::Archived)
                | (ProfileStatus::Archived, ProfileStatus::Active)
        );
        if !allowed {
            return Err(CoreError::InvalidTransition);
        }
        let now = crate::storage::now_utc();
        let archived_at = (target == ProfileStatus::Archived).then_some(now.clone());
        sqlx::query("UPDATE profiles SET status=?1,archived_at=?2,updated_at=?3 WHERE id=?4")
            .bind(status_str(target))
            .bind(archived_at)
            .bind(now)
            .bind(&id.0)
            .execute(&self.pool)
            .await
            .map_err(map_sqlx_error)?;
        self.fetch(id).await
    }

    async fn delete_to_trash(&self, id: &ProfileId) -> Result<Profile, CoreError> {
        let current = self.fetch(id).await?;
        if current.status == ProfileStatus::Deleted {
            return Ok(current);
        }
        if current.status != ProfileStatus::Active && current.status != ProfileStatus::Archived {
            return Err(CoreError::InvalidTransition);
        }
        let source = self.profile_dir(id)?;
        let trash = self.trash_dir(id)?;
        if !is_real_directory(&source) {
            return Err(CoreError::ProfilePurgeFailed);
        }
        if trash.exists() {
            return Err(CoreError::ProfilePurgeFailed);
        }
        std::fs::rename(&source, &trash).map_err(|error| {
            let _ = error;
            CoreError::ProfilePurgeFailed
        })?;
        let now = crate::storage::now_utc();
        let result = sqlx::query("UPDATE profiles SET status='deleted',deleted_at=?1,deletion_error=NULL,updated_at=?1 WHERE id=?2")
            .bind(now).bind(&id.0).execute(&self.pool).await.map_err(map_sqlx_error);
        if result.is_err() {
            let _ = std::fs::rename(&trash, &source);
        }
        result?;
        self.fetch(id).await
    }

    async fn purge_deleted(&self, id: &ProfileId) -> Result<(), CoreError> {
        let current = self.fetch(id).await?;
        if current.status != ProfileStatus::Deleted {
            return Err(CoreError::ProfileNotDeleted);
        }
        let trash = self.trash_dir(id)?;
        if is_real_directory(&trash) {
            if let Err(error) = std::fs::remove_dir_all(&trash) {
                let _ = error;
                let _ = sqlx::query(
                    "UPDATE profiles SET deletion_error='io_error',updated_at=?1 WHERE id=?2",
                )
                .bind(crate::storage::now_utc())
                .bind(&id.0)
                .execute(&self.pool)
                .await;
                return Err(CoreError::ProfilePurgeFailed);
            }
        } else if trash.exists() {
            return Err(CoreError::ProfilePurgeFailed);
        }
        sqlx::query("DELETE FROM profiles WHERE id=?1")
            .bind(&id.0)
            .execute(&self.pool)
            .await
            .map_err(map_sqlx_error)?;
        Ok(())
    }

    async fn restore_deleted(&self, id: &ProfileId) -> Result<Profile, CoreError> {
        let current = self.fetch(id).await?;
        if current.status == ProfileStatus::Active {
            return Ok(current);
        }
        if current.status == ProfileStatus::Archived {
            return self.transition(id, ProfileStatus::Active).await;
        }
        let trash = self.trash_dir(id)?;
        let destination = self.profile_dir(id)?;
        if !is_real_directory(&trash) || destination.exists() {
            return Err(CoreError::ProfileRestoreUnavailable);
        }
        std::fs::rename(&trash, &destination).map_err(|_| CoreError::ProfileRestoreUnavailable)?;
        let now = crate::storage::now_utc();
        let result = sqlx::query("UPDATE profiles SET status='active',archived_at=NULL,deleted_at=NULL,deletion_error=NULL,updated_at=?1 WHERE id=?2").bind(now).bind(&id.0).execute(&self.pool).await.map_err(map_sqlx_error);
        if result.is_err() {
            let _ = std::fs::rename(&destination, &trash);
        }
        result?;
        self.fetch(id).await
    }
}

impl ProfileRepository for SqliteProfileRepository {
    async fn insert(&self, input: CreateProfile) -> Result<Profile, CoreError> {
        self.create(input).await
    }
    async fn get(&self, id: &ProfileId) -> Result<Profile, CoreError> {
        self.fetch(id).await
    }
    async fn list(&self, query: ProfileQuery) -> Result<ProfilePage, CoreError> {
        let limit = query.limit.unwrap_or(50).clamp(1, 100) as i64;
        let pattern = query.search.map(|v| format!("%{v}%"));
        let status = query.status.map(status_str);
        let rows = sqlx::query("SELECT id,name,status,fingerprint_json,start_url,notes,kernel_version,mode,automation_api,proxy_id,created_at,updated_at,archived_at,deleted_at,deletion_error FROM profiles WHERE (?1 IS NULL OR name LIKE ?1) AND (?2 IS NULL OR id > ?2) AND (?3 IS NULL OR status = ?3) ORDER BY id LIMIT ?4")
            .bind(pattern)
            .bind(query.cursor)
            .bind(status)
            .bind(limit + 1)
            .fetch_all(&self.pool)
            .await
            .map_err(map_sqlx_error)?;
        let mut items = rows
            .into_iter()
            .map(|row| profile_from_row(&row))
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor =
            (items.len() > limit as usize).then(|| items.pop().expect("row exists").id.0);
        Ok(ProfilePage { items, next_cursor })
    }
    async fn update(&self, id: &ProfileId, patch: ProfilePatch) -> Result<Profile, CoreError> {
        self.update_fields(id, patch).await
    }
    async fn archive(&self, id: &ProfileId) -> Result<Profile, CoreError> {
        self.transition(id, ProfileStatus::Archived).await
    }
    async fn restore(&self, id: &ProfileId) -> Result<Profile, CoreError> {
        self.restore_deleted(id).await
    }
    async fn soft_delete(&self, id: &ProfileId) -> Result<Profile, CoreError> {
        self.delete_to_trash(id).await
    }
    async fn purge(&self, id: &ProfileId) -> Result<(), CoreError> {
        self.purge_deleted(id).await
    }
    async fn copy(&self, id: &ProfileId, options: CopyOptions) -> Result<Profile, CoreError> {
        let source = self.fetch(id).await?;
        if source.status == ProfileStatus::Deleted {
            return Err(CoreError::ProfileDeleted);
        }
        let mut fingerprint = if options.copy_fingerprint {
            source.fingerprint.clone()
        } else {
            FingerprintSpec::fresh(source.fingerprint.platform)?
        };
        while fingerprint.seed == source.fingerprint.seed {
            fingerprint.seed = generate_seed()?;
        }
        self.create(CreateProfile {
            name: source.name,
            fingerprint,
            start_url: source.start_url,
            proxy_id: if options.copy_proxy {
                source.proxy_id.clone()
            } else {
                None
            },
            notes: source.notes,
            kernel_version: source.kernel_version,
            mode: source.mode,
            automation_api: source.automation_api,
        })
        .await
    }
}

fn status_str(status: ProfileStatus) -> &'static str {
    match status {
        ProfileStatus::Active => "active",
        ProfileStatus::Archived => "archived",
        ProfileStatus::Deleted => "deleted",
    }
}
fn is_real_directory(path: &Path) -> bool {
    path.is_dir()
        && std::fs::symlink_metadata(path)
            .map(|m| !m.file_type().is_symlink())
            .unwrap_or(false)
}
pub(crate) fn create_profile_dirs(profile_dir: &Path) -> Result<(), CoreError> {
    if profile_dir.exists() {
        return Err(CoreError::PathUnsafe);
    }
    std::fs::create_dir_all(profile_dir).map_err(|_| CoreError::Storage)?;
    for child in ["chrome", "downloads", "logs"] {
        std::fs::create_dir(profile_dir.join(child)).map_err(|_| CoreError::Storage)?;
    }
    Ok(())
}
fn safe_child(root: &Path, relative: PathBuf) -> Result<PathBuf, CoreError> {
    if relative.is_absolute()
        || relative
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(CoreError::PathUnsafe);
    }
    let candidate = root.join(relative);
    let mut current = root.to_path_buf();
    for component in candidate
        .strip_prefix(root)
        .map_err(|_| CoreError::PathUnsafe)?
        .components()
    {
        current.push(component);
        if current.exists()
            && std::fs::symlink_metadata(&current)
                .map_err(|_| CoreError::PathUnsafe)?
                .file_type()
                .is_symlink()
        {
            return Err(CoreError::PathUnsafe);
        }
    }
    Ok(candidate)
}
fn invalid_profile_id() -> CoreError {
    CoreError::validation(
        ErrorCode::ValidationFailed,
        "profile_id must be a valid ULID",
    )
}

fn map_profile_insert_error(error: sqlx::Error, has_proxy: bool) -> CoreError {
    if has_proxy {
        if let sqlx::Error::Database(db) = &error {
            if db.code().as_deref() == Some("787") || db.message().contains("FOREIGN KEY") {
                return CoreError::validation(ErrorCode::ProxyNotFound, "proxy binding not found");
            }
        }
    }
    map_sqlx_error(error)
}
fn profile_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Profile, CoreError> {
    let status = match row.get::<String, _>("status").as_str() {
        "active" => ProfileStatus::Active,
        "archived" => ProfileStatus::Archived,
        "deleted" => ProfileStatus::Deleted,
        _ => return Err(CoreError::StorageIntegrityFailed),
    };
    let mode = match row.get::<String, _>("mode").as_str() {
        "headful" => ProfileMode::Headful,
        "headless" => ProfileMode::Headless,
        "pseudo_headful" => ProfileMode::PseudoHeadful,
        _ => return Err(CoreError::StorageIntegrityFailed),
    };
    let deletion_error =
        row.get::<Option<String>, _>("deletion_error")
            .map(|value| match value.as_str() {
                "busy" => DeletionError::Busy,
                "permission_denied" => DeletionError::PermissionDenied,
                "io_error" => DeletionError::IoError,
                _ => DeletionError::Unknown,
            });
    Ok(Profile {
        id: ProfileId::from_string(row.get::<String, _>("id")),
        name: row.get("name"),
        status,
        fingerprint: serde_json::from_str(&row.get::<String, _>("fingerprint_json"))
            .map_err(|_| CoreError::StorageIntegrityFailed)?,
        start_url: row.get("start_url"),
        notes: row.get("notes"),
        kernel_version: row.get("kernel_version"),
        mode,
        automation_api: row.get::<i64, _>("automation_api") != 0,
        proxy_id: row.get("proxy_id"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        archived_at: row.get("archived_at"),
        deleted_at: row.get("deleted_at"),
        deletion_error,
    })
}

/// Validate a caller-provided relative path and reject traversal, absolute paths, and symlinks.
pub fn validate_relative_path(root: &Path, relative: &Path) -> Result<PathBuf, CoreError> {
    safe_child(root, relative.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::profile::CreateProfile;
    use crate::fingerprint::{spec::Platform, validate_spec};

    fn setup() -> (tokio::runtime::Runtime, SqliteProfileRepository, PathBuf) {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let path = std::env::temp_dir().join(format!("mien-core-{}", ProfileId::new().0));
        let database = path.join("db.sqlite");
        let root = path.join("data");
        std::fs::create_dir_all(&root).expect("root");
        let pool = runtime
            .block_on(crate::storage::migrations::connect(&format!(
                "sqlite://{}",
                database.display()
            )))
            .expect("pool");
        runtime
            .block_on(crate::storage::migrations::apply_all(
                &pool,
                &[
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
                ],
            ))
            .expect("migrations");
        (
            runtime,
            SqliteProfileRepository::new(pool, &root).expect("repository"),
            path,
        )
    }

    fn input(name: &str) -> CreateProfile {
        CreateProfile {
            name: name.into(),
            fingerprint: FingerprintSpec::fresh(Platform::Macos).expect("fresh fingerprint spec"),
            start_url: None,
            proxy_id: None,
            notes: None,
            kernel_version: "148".into(),
            mode: ProfileMode::Headful,
            automation_api: false,
        }
    }

    #[test]
    fn creates_and_copies_isolated_profiles() {
        let (runtime, repository, path) = setup();
        runtime.block_on(async {
            let source = repository.create(input("same")).await.expect("create");
            let copy = repository
                .copy(
                    &source.id,
                    CopyOptions {
                        copy_fingerprint: false,
                        copy_proxy: false,
                    },
                )
                .await
                .expect("copy");
            assert_ne!(source.id, copy.id);
            assert!(source.id.is_valid() && copy.id.is_valid());
            assert_ne!(source.fingerprint.seed, copy.fingerprint.seed);
            assert!(
                repository
                    .profile_dir(&source.id)
                    .expect("path")
                    .join("downloads")
                    .is_dir()
            );
            assert!(
                repository
                    .profile_dir(&copy.id)
                    .expect("path")
                    .join("downloads")
                    .is_dir()
            );
        });
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn copying_fingerprint_preserves_platform_and_generates_a_new_seed() {
        let (runtime, repository, path) = setup();
        runtime.block_on(async {
            let source = repository.create(input("source")).await.expect("create");
            let copy = repository
                .copy(
                    &source.id,
                    CopyOptions {
                        copy_fingerprint: true,
                        copy_proxy: false,
                    },
                )
                .await
                .expect("copy");

            assert_ne!(source.fingerprint.seed, copy.fingerprint.seed);
            assert_eq!(source.fingerprint.platform, copy.fingerprint.platform);
        });
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn copying_without_fingerprint_creates_a_valid_spec_with_a_new_seed() {
        let (runtime, repository, path) = setup();
        runtime.block_on(async {
            let source = repository.create(input("source")).await.expect("create");
            let copy = repository
                .copy(
                    &source.id,
                    CopyOptions {
                        copy_fingerprint: false,
                        copy_proxy: false,
                    },
                )
                .await
                .expect("copy");

            assert_ne!(source.fingerprint.seed, copy.fingerprint.seed);
            validate_spec(&copy.fingerprint).expect("valid fresh fingerprint spec");
        });
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn migration_runner_is_idempotent_and_database_is_consistent() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let path = std::env::temp_dir().join(format!("mien-migrations-{}", ProfileId::new().0));
        std::fs::create_dir_all(&path).expect("temp dir");
        let database = path.join("db.sqlite");
        let pool = runtime
            .block_on(crate::storage::migrations::connect(&format!(
                "sqlite://{}",
                database.display()
            )))
            .expect("pool");
        let migrations = [
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
        ];
        runtime
            .block_on(crate::storage::migrations::apply_all(&pool, &migrations))
            .expect("first run");
        runtime
            .block_on(crate::storage::migrations::apply_all(&pool, &migrations))
            .expect("second run");
        let foreign_key_errors = runtime
            .block_on(sqlx::query("PRAGMA foreign_key_check").fetch_all(&pool))
            .expect("foreign key check");
        assert!(foreign_key_errors.is_empty());
        let integrity = runtime
            .block_on(sqlx::query_scalar::<_, String>("PRAGMA integrity_check").fetch_one(&pool))
            .expect("integrity check");
        assert_eq!(integrity, "ok");
        let version = runtime
            .block_on(crate::storage::migrations::current_version_async_pool(
                &pool,
            ))
            .expect("version");
        assert_eq!(version, crate::storage::migrations::SchemaVersion(8));
        drop(pool);
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn lifecycle_moves_and_restores_directory() {
        let (runtime, repository, path) = setup();
        runtime.block_on(async {
            let profile = repository.create(input("lifecycle")).await.expect("create");
            let archived = repository.archive(&profile.id).await.expect("archive");
            assert_eq!(archived.status, ProfileStatus::Archived);
            let active = repository
                .restore(&profile.id)
                .await
                .expect("restore archive");
            assert_eq!(active.status, ProfileStatus::Active);
            let deleted = repository.soft_delete(&profile.id).await.expect("delete");
            assert_eq!(deleted.status, ProfileStatus::Deleted);
            assert!(repository.trash_dir(&profile.id).expect("trash").is_dir());
            let restored = repository
                .restore(&profile.id)
                .await
                .expect("restore deleted");
            assert_eq!(restored.status, ProfileStatus::Active);
            repository
                .soft_delete(&profile.id)
                .await
                .expect("delete again");
            repository.purge(&profile.id).await.expect("purge");
            assert!(!repository.trash_dir(&profile.id).expect("trash").exists());
            assert!(matches!(
                repository.get(&profile.id).await,
                Err(CoreError::NotFound)
            ));
        });
        let _ = std::fs::remove_dir_all(path);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_unsafe_paths() {
        let root = std::env::temp_dir().join(format!("mien-path-{}", ProfileId::new().0));
        std::fs::create_dir_all(&root).expect("root");
        assert!(matches!(
            validate_relative_path(&root, Path::new("../escape")),
            Err(CoreError::PathUnsafe)
        ));
        assert!(matches!(
            validate_relative_path(&root, Path::new("/absolute")),
            Err(CoreError::PathUnsafe)
        ));
        let outside = root.join("outside");
        std::fs::create_dir(&outside).expect("outside");
        std::os::unix::fs::symlink(&outside, root.join("link")).expect("symlink");
        assert!(matches!(
            validate_relative_path(&root, Path::new("link/file")),
            Err(CoreError::PathUnsafe)
        ));
        let _ = std::fs::remove_dir_all(root);
    }
}

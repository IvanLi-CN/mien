//! 档案聚合。数据模型见 `docs/ARCHITECTURE.md` §6 与 `docs/DATABASE-SCHEMA.md`。
//!
//! 占位归属：T9-A（创建/复制/导入/生命周期），启动入口由 T9-D 接入。

use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::fingerprint::spec::FingerprintSpec;

/// 档案生命周期状态；`deleted` 不得启动。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileStatus {
    Active,
    Archived,
    Deleted,
}

/// 档案标识（ULID 文本）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProfileId(pub String);

impl ProfileId {
    pub fn new() -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |v| v.as_millis() as u64);
        let counter = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut value = [0u8; 16];
        let timestamp = millis.to_be_bytes();
        value[..6].copy_from_slice(&timestamp[2..]);
        value[6..14].copy_from_slice(&counter.to_be_bytes());
        value[14..].copy_from_slice(&(std::process::id() as u16).to_be_bytes());
        let alphabet = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
        let mut encoded = String::with_capacity(26);
        for character in 0..26 {
            let mut chunk = 0u8;
            for bit in 0..5 {
                let source_bit = character * 5 + bit - 2;
                chunk <<= 1;
                if (0..128).contains(&source_bit) {
                    let byte_index = (source_bit / 8) as usize;
                    let bit_index = 7 - (source_bit % 8) as u8;
                    chunk |= (value[byte_index] >> bit_index) & 1;
                }
            }
            encoded.push(alphabet[chunk as usize] as char);
        }
        Self(encoded)
    }

    pub fn from_string(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn is_valid(&self) -> bool {
        self.0.len() == 26
            && self
                .0
                .bytes()
                .all(|b| b"0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(&b))
            && self.0.as_bytes()[0] <= b'7'
    }
}

impl Default for ProfileId {
    fn default() -> Self {
        Self::new()
    }
}

/// 档案详情。种子与数据目录由核心生成，不外露于 UI 传输之外。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub id: ProfileId,
    pub name: String,
    pub status: ProfileStatus,
    pub fingerprint: FingerprintSpec,
    pub start_url: Option<String>,
    pub notes: Option<String>,
    pub kernel_version: String,
    pub mode: ProfileMode,
    pub automation_api: bool,
    pub created_at: String,
    pub updated_at: String,
    pub archived_at: Option<String>,
    pub deleted_at: Option<String>,
    pub deletion_error: Option<DeletionError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileMode {
    Headful,
    Headless,
    PseudoHeadful,
}
impl ProfileMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Headful => "headful",
            Self::Headless => "headless",
            Self::PseudoHeadful => "pseudo_headful",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeletionError {
    Busy,
    PermissionDenied,
    IoError,
    Unknown,
}
impl DeletionError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Busy => "busy",
            Self::PermissionDenied => "permission_denied",
            Self::IoError => "io_error",
            Self::Unknown => "unknown",
        }
    }
}

/// 分页查询。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProfileQuery {
    pub cursor: Option<String>,
    pub limit: Option<u32>,
    pub search: Option<String>,
}

/// 分页结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfilePage {
    pub items: Vec<Profile>,
    pub next_cursor: Option<String>,
}

/// 创建档案输入。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateProfile {
    pub name: String,
    pub fingerprint: FingerprintSpec,
    pub start_url: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default = "default_kernel_version")]
    pub kernel_version: String,
    #[serde(default)]
    pub mode: ProfileMode,
    #[serde(default)]
    pub automation_api: bool,
}

fn default_kernel_version() -> String {
    "default".to_owned()
}
impl Default for ProfileMode {
    fn default() -> Self {
        Self::Headful
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProfilePatch {
    pub name: Option<String>,
    pub notes: Option<String>,
    pub fingerprint: Option<FingerprintSpec>,
    pub start_url: Option<String>,
    pub kernel_version: Option<String>,
    pub mode: Option<ProfileMode>,
    pub automation_api: Option<bool>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct CopyOptions {
    pub copy_fingerprint: bool,
    pub copy_proxy: bool,
}

/// 创建档案。
///
/// # Errors
/// 名称冲突、规格校验失败或存储繁忙。
pub fn create(input: CreateProfile) -> Result<Profile, CoreError> {
    if input.name.trim().is_empty() || input.name.chars().count() > 120 {
        return Err(CoreError::Validation("name".to_owned()));
    }
    let id = ProfileId::new();
    let now = crate::storage::now_utc();
    Ok(Profile {
        id,
        name: input.name,
        status: ProfileStatus::Active,
        fingerprint: input.fingerprint,
        start_url: input.start_url,
        notes: input.notes,
        kernel_version: input.kernel_version,
        mode: input.mode,
        automation_api: input.automation_api,
        created_at: now.clone(),
        updated_at: now,
        archived_at: None,
        deleted_at: None,
        deletion_error: None,
    })
}

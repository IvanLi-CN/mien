//! 主密钥加载与 AES-256-GCM 字段级加密。
//!
//! 契约：`docs/DATABASE-SCHEMA.md` §4（AAD 绑定 `proxy_id` 与字段名）与
//! `docs/SECURITY.md` §主密钥（桌面钥匙串；服务端 `<data-root>/secrets/master.key`
//! 0600 或 `MIEN_MASTER_KEY`）。
//!
//! 主密钥获取失败统一映射到 `CoreError::Storage`（`INTERNAL_UNEXPECTED`）：错误码目录
//! 已冻结且没有主密钥专用码；该类失败是宿主本地配置故障、不可重试，也不得在错误中
//! 泄露路径或密钥材料。`STORAGE_INTEGRITY_FAILED` 保留给已加密数据无法通过认证
//! （密钥不匹配或密文被搬移/篡改）的情形。

use std::fmt;
use std::path::Path;

use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::error::CoreError;

/// 主密钥环境变量；优先于密钥文件。
pub const MASTER_KEY_ENV: &str = "MIEN_MASTER_KEY";
/// 服务端主密钥相对数据根目录的路径，必须为 0600。
pub const MASTER_KEY_PATH: &str = "secrets/master.key";
/// 代理凭据整体加密时使用的 AAD 字段名。
pub const PROXY_CONFIG_FIELD: &str = "config";

const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;
/// AAD 域分隔前缀；版本化以便轮换。
const AAD_DOMAIN: &[u8] = b"mien/proxy-secret/v1";
/// 记录标识与字段名之间的分隔字节。
const AAD_SEPARATOR: u8 = 0x00;

/// 32 字节主密钥。`Debug` 只显示占位符，drop 时零化。
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct MasterKey {
    bytes: [u8; KEY_LEN],
}

impl MasterKey {
    /// 从原始 32 字节构造（测试与钥匙串/Secret 注入路径使用）。
    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
        Self { bytes }
    }

    /// 解析 64 个十六进制字符，允许 `0x` 前缀与首尾空白。
    ///
    /// # Errors
    /// 长度或字符不合法时返回 [`CoreError::Storage`]。
    pub fn parse_hex(text: &str) -> Result<Self, CoreError> {
        let trimmed = text.trim();
        let hex = trimmed
            .strip_prefix("0x")
            .or_else(|| trimmed.strip_prefix("0X"))
            .unwrap_or(trimmed);
        if hex.len() != KEY_LEN * 2 {
            return Err(CoreError::Storage);
        }
        let mut bytes = [0u8; KEY_LEN];
        for (index, pair) in hex.as_bytes().chunks_exact(2).enumerate() {
            bytes[index] = (nibble(pair[0])? << 4) | nibble(pair[1])?;
        }
        Ok(Self { bytes })
    }

    /// 读取 `MIEN_MASTER_KEY`；未设置时返回 `None`。
    ///
    /// # Errors
    /// 变量不是合法 UTF-8 或不是 64 个十六进制字符时返回 [`CoreError::Storage`]。
    pub fn from_env() -> Result<Option<Self>, CoreError> {
        let Some(raw) = std::env::var_os(MASTER_KEY_ENV) else {
            return Ok(None);
        };
        let text = raw.to_str().ok_or(CoreError::Storage)?;
        Self::parse_hex(text).map(Some)
    }

    /// 仅供派生 HMAC / `outbound_key` 使用；不得打印、序列化或写入日志。
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// 读取 `<root>/secrets/master.key`；文件必须是 0600 的普通文件。
    ///
    /// # Errors
    /// 文件缺失、权限过宽、不可读或内容不合法时返回 [`CoreError::Storage`]。
    pub fn from_file(root: &Path) -> Result<Self, CoreError> {
        let path = root.join(MASTER_KEY_PATH);
        ensure_private_file(&path)?;
        let mut bytes = std::fs::read(&path).map_err(|_| CoreError::Storage)?;
        let parsed = std::str::from_utf8(&bytes)
            .map_err(|_| CoreError::Storage)
            .and_then(Self::parse_hex);
        bytes.zeroize();
        parsed
    }

    /// 按契约顺序加载：环境变量优先，其次 `<root>/secrets/master.key`。
    ///
    /// # Errors
    /// 两条来源都不可用时返回 [`CoreError::Storage`]。
    pub fn load(root: &Path) -> Result<Self, CoreError> {
        if let Some(key) = Self::from_env()? {
            return Ok(key);
        }
        Self::from_file(root)
    }
}

impl fmt::Debug for MasterKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MasterKey")
            .field("bytes", &"<redacted>")
            .finish()
    }
}

/// AAD 组成部分；布局为 `b"mien/proxy-secret/v1" || proxy_id || 0x00 || field_name`。
#[derive(Debug, Clone, Copy)]
pub struct FieldAad<'a> {
    pub proxy_id: &'a str,
    pub field_name: &'a str,
}

impl FieldAad<'_> {
    /// 规范化后的 AAD 字节串。
    pub fn to_bytes(self) -> Vec<u8> {
        let mut out =
            Vec::with_capacity(AAD_DOMAIN.len() + self.proxy_id.len() + 1 + self.field_name.len());
        out.extend_from_slice(AAD_DOMAIN);
        out.extend_from_slice(self.proxy_id.as_bytes());
        out.push(AAD_SEPARATOR);
        out.extend_from_slice(self.field_name.as_bytes());
        out
    }
}

/// 加密一个字段：输出 `nonce(12) || ciphertext || tag(16)`。
///
/// # Errors
/// 加密失败时返回 [`CoreError::Storage`]；不会回显明文。
pub fn encrypt_field(
    key: &MasterKey,
    aad: FieldAad<'_>,
    plaintext: &[u8],
) -> Result<Vec<u8>, CoreError> {
    let cipher = Aes256Gcm::new_from_slice(&key.bytes).map_err(|_| CoreError::Storage)?;
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let aad = aad.to_bytes();
    let ciphertext = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| CoreError::Storage)?;
    let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    out.extend_from_slice(nonce.as_slice());
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// 解密一个字段；AAD 不匹配（记录或字段被搬移）即失败。
///
/// # Errors
/// 密文过短、认证失败或密钥不匹配时返回 [`CoreError::StorageIntegrityFailed`]。
pub fn decrypt_field(
    key: &MasterKey,
    aad: FieldAad<'_>,
    ciphertext: &[u8],
) -> Result<Vec<u8>, CoreError> {
    if ciphertext.len() < NONCE_LEN + TAG_LEN {
        return Err(CoreError::StorageIntegrityFailed);
    }
    let (nonce, body) = ciphertext.split_at(NONCE_LEN);
    let cipher = Aes256Gcm::new_from_slice(&key.bytes).map_err(|_| CoreError::Storage)?;
    let aad = aad.to_bytes();
    cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: body,
                aad: &aad,
            },
        )
        .map_err(|_| CoreError::StorageIntegrityFailed)
}

fn nibble(byte: u8) -> Result<u8, CoreError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(CoreError::Storage),
    }
}

#[cfg(unix)]
fn ensure_private_file(path: &Path) -> Result<(), CoreError> {
    use std::os::unix::fs::PermissionsExt;
    // 拒绝符号链接：主密钥路径不得指向数据目录之外。
    let metadata = std::fs::symlink_metadata(path).map_err(|_| CoreError::Storage)?;
    if !metadata.file_type().is_file() {
        return Err(CoreError::Storage);
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(CoreError::Storage);
    }
    Ok(())
}

#[cfg(not(unix))]
fn ensure_private_file(path: &Path) -> Result<(), CoreError> {
    // 非 unix 宿主（Windows）没有 POSIX 权限位；依赖数据目录 ACL。
    let metadata = std::fs::symlink_metadata(path).map_err(|_| CoreError::Storage)?;
    if !metadata.file_type().is_file() {
        return Err(CoreError::Storage);
    }
    Ok(())
}

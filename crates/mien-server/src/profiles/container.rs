//! `profile-encrypted-v1` 不透明容器（`REQ-PROFILE-010`）。
//!
//! 容器是一个自包含字符串：
//!
//! ```text
//! profile-encrypted-v1.<hex(header JSON)>.<hex(nonce || ciphertext+tag)>
//! ```
//!
//! `header` 只含版本、KDF 名称、Argon2id 参数与 salt；这些值都不出现在 envelope
//! 顶层。密钥由口令经 Argon2id 派生，认证加密使用 AES-256-GCM，AAD 绑定容器/envelope
//! 版本，因此改写版本号会让解密失败。
//!
//! 口令红线：口令只存在于请求体的内存中；[`Passphrase`] 不实现 `Serialize`，`Debug`
//! 永远打印占位符，drop 时零化。

use std::fmt;

use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use super::{ApiError, hex_decode, hex_encode};

/// 容器前缀；版本号同时出现在前缀与头部，二者必须一致。
pub const PREFIX: &str = "profile-encrypted-v1";
/// envelope 版本；参与 AAD（`ProfileExportEnvelope.schema_version`）。
pub const ENVELOPE_VERSION: u32 = 1;

/// 容器版本；当前只接受 v1。
const CONTAINER_VERSION: u32 = 1;
/// Argon2id 内存代价（KiB），与管理员口令哈希同量级。
const M_COST_KIB: u32 = 19 * 1024;
/// Argon2id 迭代次数。
const T_COST: u32 = 2;
/// Argon2id 并行度。
const P_COST: u32 = 1;
/// 派生密钥长度。
const KEY_LEN: usize = 32;
/// salt 长度。
const SALT_LEN: usize = 16;
/// AES-GCM nonce 长度。
const NONCE_LEN: usize = 12;
/// 解析外部容器时允许的 KDF 参数上限。
///
/// 头部来自**攻击者可控**的容器字符串，因此上限必须贴着本机导出参数
/// （[`M_COST_KIB`]/[`T_COST`]/[`P_COST`]）设定：给外部容器留出迁移余量即可，
/// 不能允许一次导入把 `argon2` 推到 GiB 级内存（那是无需认证即可触发的 DoS）。
/// 拒绝发生在任何 KDF 工作之前。
const MAX_M_COST_KIB: u32 = 4 * M_COST_KIB;
const MAX_T_COST: u32 = 4 * T_COST;
const MAX_P_COST: u32 = 4 * P_COST;

/// 口令最小/最大长度，与 `ProfileExportRequest`/`ProfileImportInput` 一致。
pub const MIN_PASSPHRASE_LEN: usize = 12;
pub const MAX_PASSPHRASE_LEN: usize = 1024;

/// 请求体中的 write-only 口令。
///
/// 不实现 `Serialize`；`Debug` 只显示占位符；drop 时零化内部字符串。
#[derive(Clone)]
pub struct Passphrase(String);

impl Passphrase {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 契约长度校验（按字符计）。
    pub fn is_valid_length(&self) -> bool {
        let length = self.0.chars().count();
        (MIN_PASSPHRASE_LEN..=MAX_PASSPHRASE_LEN).contains(&length)
    }
}

impl<'de> Deserialize<'de> for Passphrase {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self)
    }
}

impl fmt::Debug for Passphrase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Passphrase(<redacted>)")
    }
}

impl Drop for Passphrase {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    v: u32,
    envelope: u32,
    kdf: String,
    m: u32,
    t: u32,
    p: u32,
    salt: String,
}

/// 加密明文；返回 `profile-encrypted-v1` 容器字符串。
///
/// # Errors
/// 口令长度非法、随机源不可用或加密失败。
pub fn seal(passphrase: &Passphrase, plaintext: &[u8]) -> Result<String, ApiError> {
    if !passphrase.is_valid_length() {
        return Err(ApiError::validation(
            "passphrase",
            format!("must be {MIN_PASSPHRASE_LEN}-{MAX_PASSPHRASE_LEN} characters"),
        ));
    }
    let mut salt = [0_u8; SALT_LEN];
    OsRng.fill_bytes(&mut salt);
    let mut key = derive_key(passphrase.as_str(), &salt, M_COST_KIB, T_COST, P_COST)?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| internal())?;
    key.zeroize();
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let aad = aad(ENVELOPE_VERSION);
    let ciphertext = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| internal())?;
    let header = Header {
        v: CONTAINER_VERSION,
        envelope: ENVELOPE_VERSION,
        kdf: "argon2id".to_owned(),
        m: M_COST_KIB,
        t: T_COST,
        p: P_COST,
        salt: hex_encode(&salt),
    };
    let header = serde_json::to_vec(&header).map_err(|_| internal())?;
    let mut sealed = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    sealed.extend_from_slice(nonce.as_slice());
    sealed.extend_from_slice(&ciphertext);
    Ok(format!(
        "{PREFIX}.{}.{}",
        hex_encode(&header),
        hex_encode(&sealed)
    ))
}

/// 解密容器；口令错误、容器被篡改或版本不符都失败。
///
/// # Errors
/// 容器形状不合法返回 `payload` 字段错误；认证失败返回 `passphrase` 字段错误；
/// 其它内部失败返回 `INTERNAL_UNEXPECTED`。
pub fn open(passphrase: &Passphrase, container: &str) -> Result<Vec<u8>, ApiError> {
    if !passphrase.is_valid_length() {
        return Err(ApiError::validation(
            "passphrase",
            format!("must be {MIN_PASSPHRASE_LEN}-{MAX_PASSPHRASE_LEN} characters"),
        ));
    }
    let mut parts = container.split('.');
    let (Some(prefix), Some(header), Some(body), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(malformed());
    };
    if prefix != PREFIX {
        return Err(malformed());
    }
    let header = hex_decode(header).ok_or_else(malformed)?;
    let header: Header = serde_json::from_slice(&header).map_err(|_| malformed())?;
    if header.v != CONTAINER_VERSION || header.kdf != "argon2id" {
        return Err(malformed());
    }
    // 版本参与 AAD，因此未知 envelope 版本既不能解密也不能被接受。
    if header.envelope != ENVELOPE_VERSION {
        return Err(malformed());
    }
    if header.m == 0
        || header.m > MAX_M_COST_KIB
        || header.t == 0
        || header.t > MAX_T_COST
        || header.p == 0
        || header.p > MAX_P_COST
    {
        return Err(malformed());
    }
    let salt = hex_decode(&header.salt).ok_or_else(malformed)?;
    if !(8..=64).contains(&salt.len()) {
        return Err(malformed());
    }
    let sealed = hex_decode(body).ok_or_else(malformed)?;
    if sealed.len() < NONCE_LEN + 16 {
        return Err(malformed());
    }
    let mut key = derive_key(passphrase.as_str(), &salt, header.m, header.t, header.p)?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| internal())?;
    key.zeroize();
    let (nonce, ciphertext) = sealed.split_at(NONCE_LEN);
    let aad = aad(header.envelope);
    cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad: &aad,
            },
        )
        .map_err(|_| ApiError::validation("passphrase", "does not decrypt this payload"))
}

/// AAD：绑定导出域与 envelope 版本，换版本或换用途都无法通过认证。
fn aad(version: u32) -> Vec<u8> {
    format!("mien/profile-export/v{version}").into_bytes()
}

fn derive_key(
    passphrase: &str,
    salt: &[u8],
    m_cost: u32,
    t_cost: u32,
    p_cost: u32,
) -> Result<[u8; KEY_LEN], ApiError> {
    let params = Params::new(m_cost, t_cost, p_cost, Some(KEY_LEN)).map_err(|_| internal())?;
    let hasher = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0_u8; KEY_LEN];
    hasher
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|_| internal())?;
    Ok(key)
}

fn malformed() -> ApiError {
    ApiError::validation("payload", "is not a profile-encrypted-v1 container")
}

fn internal() -> ApiError {
    ApiError::internal()
}

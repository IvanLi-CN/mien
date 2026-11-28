//! API Token 的签发、校验与撤销（`REQ-API-005`）。
//!
//! 存储契约：`docs/DATABASE-SCHEMA.md` §4 与 `docs/SECURITY.md` §5.2。
//! 数据库只保存 `token_hash = HMAC-SHA-256(pepper, token)`，其中
//! `pepper = HKDF-SHA256(master_key, info="mien/api-token-pepper/v1")` 且不单独落盘；
//! `token_prefix` 只是 4–16 字符的展示前缀，不参与认证。明文 Token 只在创建响应里
//! 出现一次，绝不写入数据库、幂等缓存、审计或日志。

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use hkdf::Hkdf;
use hkdf::hmac::{Hmac, Mac};
use mien_core::secrets::MasterKey;
use rand::RngCore;
use rand::rngs::OsRng;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};

use crate::auth::{
    AuthError, BearerAuthenticator, BearerIdentity, SharedBearerAuthenticator, session,
};

/// HKDF 派生域分隔标签；版本化以便主密钥轮换后区分 pepper 代际。
pub const PEPPER_INFO: &[u8] = b"mien/api-token-pepper/v1";
/// 明文 Token 前缀；让使用者能识别凭据类型。
pub const TOKEN_SENTINEL: &str = "mien_";
/// Token 随机部分字节数（32 字节 → 64 个十六进制字符）。
const TOKEN_RANDOM_BYTES: usize = 32;
/// 展示前缀长度：`mien_` + 8 个十六进制字符 = 13，落在契约的 4–16 之间。
const PREFIX_LEN: usize = 13;
/// 幂等键保留窗口：过期后同一键可以重新使用。
const IDEMPOTENCY_RETENTION: &str = "+86400 seconds";
/// 幂等键命名空间；与表主键 `(key, scope)` 的第二列对应。
const IDEMPOTENCY_SCOPE: &str = "tokensCreate";

/// 由主密钥派生的 Token pepper。`Debug` 只显示占位符。
#[derive(Clone)]
pub struct Pepper([u8; 32]);

impl Pepper {
    /// `HKDF-SHA256(master_key, salt=None, info="mien/api-token-pepper/v1")`。
    pub fn derive(master: &MasterKey) -> Self {
        let hkdf = Hkdf::<Sha256>::new(None, master.as_bytes());
        let mut bytes = [0u8; 32];
        hkdf.expand(PEPPER_INFO, &mut bytes)
            .expect("32 bytes is a valid HKDF output length");
        Self(bytes)
    }

    /// `HMAC-SHA-256(pepper, token)`。
    fn hash(&self, token: &str) -> [u8; 32] {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.0).expect("HMAC accepts any key length");
        mac.update(token.as_bytes());
        mac.finalize().into_bytes().into()
    }

    /// 十六进制哈希；数据库比较与常量时间比较都使用该固定宽度表示。
    fn hash_hex(&self, token: &str) -> String {
        crate::auth::password::hex(&self.hash(token))
    }
}

impl fmt::Debug for Pepper {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Pepper(<redacted>)")
    }
}

/// 一条 Token 记录；不含明文也不含哈希。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TokenRecord {
    pub id: String,
    pub name: String,
    pub prefix: Option<String>,
    pub scopes: Vec<crate::tokens::scope::TokenScope>,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub revoked_at: Option<String>,
}

/// 创建结果：明文只在这里返回一次。
///
/// 故意不实现 `Serialize`：明文只允许经 `tokens::IssuedBody` 进入创建响应，避免其他
/// 序列化路径（列表、审计、事件）误带明文。
#[derive(Debug, Clone)]
pub struct Issued {
    pub record: TokenRecord,
    pub token: String,
}

/// Token 存储：数据库池 + 由主密钥派生的 pepper。
pub struct TokenStore {
    pool: SqlitePool,
    pepper: Pepper,
}

impl TokenStore {
    pub fn new(pool: SqlitePool, pepper: Pepper) -> Self {
        Self { pool, pepper }
    }

    /// 从主密钥装载：`MIEN_MASTER_KEY` 优先，其次 `<data_root>/secrets/master.key`。
    ///
    /// # Errors
    /// 主密钥不可用（缺失、权限过宽或内容非法）。
    pub fn load(
        pool: SqlitePool,
        data_root: &std::path::Path,
    ) -> Result<Self, mien_core::CoreError> {
        let master = MasterKey::load(data_root)?;
        Ok(Self::new(pool, Pepper::derive(&master)))
    }

    /// 由主密钥构造（测试注入与桌面钥匙串路径共用）。
    pub fn from_master_key(pool: SqlitePool, master: &MasterKey) -> Self {
        Self::new(pool, Pepper::derive(master))
    }

    pub fn pepper(&self) -> &Pepper {
        &self.pepper
    }

    /// 数据库池；审计事件与 Token 变更共用同一池。
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// 列出全部 Token；不含明文与哈希。
    ///
    /// # Errors
    /// 数据库查询失败。
    pub async fn list(&self) -> Result<Vec<TokenRecord>, AuthError> {
        let rows = sqlx::query(
            "SELECT id, name, token_prefix, scopes_json, created_at, last_used_at, revoked_at \
             FROM api_tokens ORDER BY created_at DESC, id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|_| AuthError::internal())?;
        rows.iter().map(record_from_row).collect()
    }

    /// 创建 Token；返回的明文是该记录唯一一次可见的完整值。
    ///
    /// `idempotency_key` 非空时先占用 `(key, "tokensCreate")`：重复键返回
    /// `409 IDEMPOTENCY_CONFLICT`，不重放明文。保留窗口外的旧键会被清理后重新使用。
    ///
    /// # Errors
    /// 幂等键冲突；数据库写入失败。
    pub async fn create(
        &self,
        name: &str,
        scopes: &[crate::tokens::scope::TokenScope],
        idempotency_key: Option<&str>,
    ) -> Result<Issued, AuthError> {
        let mut bytes = [0u8; TOKEN_RANDOM_BYTES];
        OsRng.fill_bytes(&mut bytes);
        let token = format!("{TOKEN_SENTINEL}{}", crate::auth::password::hex(&bytes));
        let prefix = token[..PREFIX_LEN].to_owned();
        let hash = self.pepper.hash_hex(&token);
        let id = mien_core::domain::profile::ProfileId::new().0;
        let created_at = session::now_timestamp();
        let scopes_json = serde_json::to_string(scopes).map_err(|_| AuthError::internal())?;
        let request_hash = crate::auth::password::hex(&Sha256::digest(
            format!("{name}\u{0}{scopes_json}").as_bytes(),
        ));

        let mut transaction = self.pool.begin().await.map_err(|_| AuthError::internal())?;
        if let Some(key) = idempotency_key {
            // 过期键可以重新使用；未过期的同键一律 409，即使请求体相同。
            sqlx::query(
                "DELETE FROM idempotency_keys \
                 WHERE key = ?1 AND scope = ?2 AND expires_at <= strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
            )
            .bind(key)
            .bind(IDEMPOTENCY_SCOPE)
            .execute(&mut *transaction)
            .await
            .map_err(|_| AuthError::internal())?;
            let inserted = sqlx::query(
                "INSERT INTO idempotency_keys (key, scope, request_hash, response_json, created_at, expires_at) \
                 VALUES (?1, ?2, ?3, NULL, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), \
                         strftime('%Y-%m-%dT%H:%M:%fZ', 'now', ?4))",
            )
            .bind(key)
            .bind(IDEMPOTENCY_SCOPE)
            .bind(request_hash)
            .bind(IDEMPOTENCY_RETENTION)
            .execute(&mut *transaction)
            .await;
            match inserted {
                Ok(_) => {}
                Err(sqlx::Error::Database(error)) if error.is_unique_violation() => {
                    return Err(idempotency_conflict());
                }
                Err(_) => return Err(AuthError::internal()),
            }
        }

        sqlx::query(
            "INSERT INTO api_tokens \
             (id, name, token_prefix, token_hash, scopes_json, created_at, last_used_at, revoked_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, NULL)",
        )
        .bind(&id)
        .bind(name)
        .bind(&prefix)
        .bind(&hash)
        .bind(&scopes_json)
        .bind(&created_at)
        .execute(&mut *transaction)
        .await
        .map_err(|_| AuthError::internal())?;
        transaction
            .commit()
            .await
            .map_err(|_| AuthError::internal())?;

        Ok(Issued {
            record: TokenRecord {
                id,
                name: name.to_owned(),
                prefix: Some(prefix),
                scopes: scopes.to_vec(),
                created_at,
                last_used_at: None,
                revoked_at: None,
            },
            token,
        })
    }

    /// 撤销 Token；已撤销的行保持 204 语义，不存在的行返回 `404 TOKEN_NOT_FOUND`。
    ///
    /// # Errors
    /// Token 不存在；数据库写入失败。
    pub async fn revoke(&self, id: &str) -> Result<(), AuthError> {
        let updated = sqlx::query(
            "UPDATE api_tokens SET revoked_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
             WHERE id = ?1 AND revoked_at IS NULL",
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|_| AuthError::internal())?;
        if updated.rows_affected() > 0 {
            return Ok(());
        }
        let exists = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM api_tokens WHERE id = ?1")
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .map_err(|_| AuthError::internal())?;
        if exists == 0 {
            Err(token_not_found())
        } else {
            Ok(())
        }
    }

    /// 按 Bearer 明文校验；命中时刷新 `last_used_at`。
    ///
    /// 哈希固定为 32 字节，因此任意长度与任意前缀的输入都走同一路径，不会 panic，
    /// 也不会按前缀或长度提前返回。
    ///
    /// # Errors
    /// 数据库查询或更新失败。
    pub async fn authenticate(&self, token: &str) -> Result<Option<BearerIdentity>, AuthError> {
        if token.is_empty() {
            return Ok(None);
        }
        let hash = self.pepper.hash_hex(token);
        let row = sqlx::query(
            "SELECT id, token_hash, scopes_json FROM api_tokens \
             WHERE token_hash = ?1 AND revoked_at IS NULL",
        )
        .bind(&hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AuthError::internal())?;
        let Some(row) = row else {
            return Ok(None);
        };
        let stored_hash: String = row
            .try_get("token_hash")
            .map_err(|_| AuthError::internal())?;
        if !constant_time_eq(stored_hash.as_bytes(), hash.as_bytes()) {
            return Ok(None);
        }
        let id: String = row.try_get("id").map_err(|_| AuthError::internal())?;
        let scopes_json: String = row
            .try_get("scopes_json")
            .map_err(|_| AuthError::internal())?;
        let scopes: Vec<crate::tokens::scope::TokenScope> =
            serde_json::from_str(&scopes_json).map_err(|_| AuthError::internal())?;

        let touched = sqlx::query(
            "UPDATE api_tokens SET last_used_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?1",
        )
        .bind(&id)
        .execute(&self.pool)
        .await;
        if let Err(error) = touched {
            tracing::warn!(error = %error, "cannot refresh the token last-used timestamp");
        }

        Ok(Some(BearerIdentity {
            subject: format!("api_token:{id}"),
            scopes: scopes
                .iter()
                .map(|scope| scope.as_str().to_owned())
                .collect(),
        }))
    }
}

impl BearerAuthenticator for TokenStore {
    fn authenticate<'a>(
        &'a self,
        token: &'a str,
    ) -> Pin<Box<dyn Future<Output = Option<BearerIdentity>> + Send + 'a>> {
        Box::pin(async move {
            match TokenStore::authenticate(self, token).await {
                Ok(identity) => identity,
                Err(error) => {
                    // 存储故障不能降级为“放行”，也不能泄露 SQL 细节。
                    tracing::warn!(code = error.code.as_str(), "token lookup failed");
                    None
                }
            }
        })
    }
}

/// 把校验器装箱，供 `AuthState::with_tokens` 挂载到认证门禁。
pub fn shared(store: std::sync::Arc<TokenStore>) -> SharedBearerAuthenticator {
    store
}

fn record_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<TokenRecord, AuthError> {
    let scopes_json: String = row
        .try_get("scopes_json")
        .map_err(|_| AuthError::internal())?;
    Ok(TokenRecord {
        id: row.try_get("id").map_err(|_| AuthError::internal())?,
        name: row.try_get("name").map_err(|_| AuthError::internal())?,
        prefix: row
            .try_get("token_prefix")
            .map_err(|_| AuthError::internal())?,
        scopes: serde_json::from_str(&scopes_json).map_err(|_| AuthError::internal())?,
        created_at: row
            .try_get("created_at")
            .map_err(|_| AuthError::internal())?,
        last_used_at: row
            .try_get("last_used_at")
            .map_err(|_| AuthError::internal())?,
        revoked_at: row
            .try_get("revoked_at")
            .map_err(|_| AuthError::internal())?,
    })
}

/// `409 IDEMPOTENCY_CONFLICT`：不返回缓存的创建响应与明文。
fn idempotency_conflict() -> AuthError {
    AuthError {
        status: axum::http::StatusCode::CONFLICT,
        code: mien_core::ErrorCode::IdempotencyConflict,
        message: "That idempotency key was already used for a token creation.",
        retryable: false,
        retry_after: None,
        details: None,
    }
}

/// `404 TOKEN_NOT_FOUND`。
fn token_not_found() -> AuthError {
    AuthError {
        status: axum::http::StatusCode::NOT_FOUND,
        code: mien_core::ErrorCode::TokenNotFound,
        message: "No such token.",
        retryable: false,
        retry_after: None,
        details: None,
    }
}

/// 常量时间字节比较；长度不同直接返回 false，不读取内容。
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0u8;
    for (a, b) in left.iter().zip(right) {
        difference |= a ^ b;
    }
    difference == 0
}

/// 请求体里的一次性秘密字段名；出现即 422。
pub const FORBIDDEN_BODY_FIELDS: &[&str] = &["token", "ticket"];

/// 递归查找 `token`/`ticket` 字段；契约要求一次性秘密不得作为请求体字段。
pub fn forbidden_secret_field(value: &serde_json::Value) -> Option<&'static str> {
    match value {
        serde_json::Value::Object(map) => {
            for (key, nested) in map {
                if let Some(field) = FORBIDDEN_BODY_FIELDS.iter().find(|field| *field == key) {
                    return Some(field);
                }
                if let Some(found) = forbidden_secret_field(nested) {
                    return Some(found);
                }
            }
            None
        }
        serde_json::Value::Array(items) => items.iter().find_map(forbidden_secret_field),
        _ => None,
    }
}

/// 请求体上限：Token 创建只接受很小的 JSON。
pub const MAX_BODY_BYTES: usize = 8 * 1024;

/// 解析请求体；未知字段、非法 JSON 与一次性秘密字段都返回 422 `VALIDATION_FAILED`。
///
/// # Errors
/// 超限、非 JSON、未知字段或出现 `token`/`ticket`。
pub fn parse_body<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, AuthError> {
    if body.len() > MAX_BODY_BYTES {
        return Err(AuthError::validation("body", "is too large"));
    }
    let value: serde_json::Value = serde_json::from_slice(body)
        .map_err(|_| AuthError::validation("body", "must be a JSON object with known fields"))?;
    if let Some(field) = forbidden_secret_field(&value) {
        return Err(AuthError::validation(
            field,
            "one-time secrets are response-only and not accepted in request bodies",
        ));
    }
    serde_json::from_value(value).map_err(|error| {
        let _ = error;
        AuthError::validation("body", "must be a JSON object with known fields")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 5869 的 HKDF-SHA256 与契约 info 组合的固定向量：
    /// master = 0x00..0x1f，pepper = 4d741180c2cc4f6e106942b64ad1d95853e6a4d6f618d8e67c2b65a1f9766605。
    #[test]
    fn pepper_derivation_matches_the_contract_vector() {
        let key = MasterKey::from_bytes(std::array::from_fn(|index| index as u8));
        let pepper = Pepper::derive(&key);
        assert_eq!(
            crate::auth::password::hex(&pepper.0),
            "4d741180c2cc4f6e106942b64ad1d95853e6a4d6f618d8e67c2b65a1f9766605"
        );
        assert_eq!(
            pepper.hash_hex("mien_0123456789abcdef"),
            "6102377c17e2b302949b3a3aaf3e54e612cf85a80c468cf8a095697568607ffb"
        );
    }

    #[test]
    fn forbidden_fields_are_found_at_any_depth() {
        assert_eq!(
            forbidden_secret_field(&serde_json::json!({"token": "x"})),
            Some("token")
        );
        assert_eq!(
            forbidden_secret_field(&serde_json::json!({"nested": [{"ticket": "x"}]})),
            Some("ticket")
        );
        assert_eq!(
            forbidden_secret_field(&serde_json::json!({"name": "ok", "scopes": []})),
            None
        );
    }

    #[test]
    fn constant_time_comparison_handles_unequal_lengths() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn parse_body_rejects_secret_fields_and_unknown_fields() {
        #[derive(Debug, serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            #[allow(dead_code)]
            name: String,
        }
        assert!(parse_body::<Input>(br#"{"name":"a"}"#).is_ok());
        let secret = parse_body::<Input>(br#"{"name":"a","token":"mien_x"}"#).expect_err("token");
        assert_eq!(secret.code, mien_core::ErrorCode::ValidationFailed);
        assert_eq!(secret.status, axum::http::StatusCode::UNPROCESSABLE_ENTITY);
        let unknown = parse_body::<Input>(br#"{"name":"a","extra":1}"#).expect_err("unknown");
        assert_eq!(unknown.code, mien_core::ErrorCode::ValidationFailed);
    }
}

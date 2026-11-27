//! 一次性 WebSocket 票据（`REQ-API-011`）。
//!
//! 票据绑定签发它的会话或 API Token，只在创建响应中出现一次；数据库只保存
//! SHA-256（`event_tickets.ticket_hash`）。升级时原子地消费票据：过期、已用或绑定
//! 对象已撤销/过期都按 `EVENT_TICKET_INVALID` 拒绝，且同一票据不可能被消费两次。

use std::time::Duration;

use rand::RngCore;
use rand::rngs::OsRng;
use sqlx::{Row, SqlitePool};

use crate::auth::{password, session};

/// 票据短 TTL：只覆盖浏览器建立升级的时间。
pub const TICKET_TTL: Duration = Duration::from_secs(60);
/// 票据明文前缀；让使用者能识别凭据类型，不参与校验。
const TICKET_SENTINEL: &str = "mien_t_";
/// 票据随机部分字节数（32 字节 → 64 个十六进制字符）。
const TICKET_BYTES: usize = 32;

/// 票据绑定的凭据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Binding {
    /// 浏览器会话；`event_tickets.session_id`。
    Session(String),
    /// API Token；`event_tickets.token_id`。
    Token(String),
}

impl Binding {
    fn split(&self) -> (Option<&str>, Option<&str>) {
        match self {
            Self::Session(id) => (Some(id), None),
            Self::Token(id) => (None, Some(id)),
        }
    }
}

/// 签发结果；`ticket` 只在这里出现一次。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issued {
    pub ticket: String,
    pub expires_at: String,
}

/// 签发一次性票据：先清理过期或已用的票据，再写入新哈希。
///
/// # Errors
/// 数据库读写失败。
pub async fn issue(
    pool: &SqlitePool,
    ttl: Duration,
    binding: &Binding,
) -> Result<Issued, sqlx::Error> {
    purge(pool).await?;
    let ticket = new_ticket();
    let expires_at: String = sqlx::query_scalar("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now', ?1)")
        .bind(format!("+{} seconds", ttl.as_secs()))
        .fetch_one(pool)
        .await?;
    let (session_id, token_id) = binding.split();
    sqlx::query(
        "INSERT INTO event_tickets (ticket_hash, session_id, token_id, expires_at, used_at, created_at) \
         VALUES (?1, ?2, ?3, ?4, NULL, strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
    )
    .bind(session::token_hash(&ticket))
    .bind(session_id)
    .bind(token_id)
    .bind(&expires_at)
    .execute(pool)
    .await?;
    Ok(Issued { ticket, expires_at })
}

/// 只校验票据仍然可用（未过期、未使用、绑定对象有效），不消费。
///
/// 升级流程先校验、通过 `after` 边界后再 [`consume`]，这样可修正的游标错误
/// 不会浪费一张票据。
///
/// # Errors
/// 数据库读取失败。
pub async fn verify(pool: &SqlitePool, ticket: &str) -> Result<bool, sqlx::Error> {
    if ticket.is_empty() {
        return Ok(false);
    }
    let hash = session::token_hash(ticket);
    let mut transaction = pool.begin().await?;
    let row = sqlx::query(
        "SELECT session_id, token_id, used_at, \
                (expires_at > strftime('%Y-%m-%dT%H:%M:%fZ','now')) AS unexpired \
         FROM event_tickets WHERE ticket_hash = ?1",
    )
    .bind(&hash)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(row) = row else {
        transaction.commit().await?;
        return Ok(false);
    };
    let usable = row.get::<Option<String>, _>("used_at").is_none()
        && row.get::<i64, _>("unexpired") == 1
        && binding_alive(
            &mut transaction,
            row.get::<Option<String>, _>("session_id").as_deref(),
            row.get::<Option<String>, _>("token_id").as_deref(),
        )
        .await?;
    transaction.commit().await?;
    Ok(usable)
}

/// 绑定对象仍然有效：会话未撤销未过期，或 Token 未撤销。
async fn binding_alive(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    session_id: Option<&str>,
    token_id: Option<&str>,
) -> Result<bool, sqlx::Error> {
    match (session_id, token_id) {
        (Some(id), None) => Ok(sqlx::query_scalar::<_, i64>(
            "SELECT 1 FROM sessions WHERE id = ?1 AND revoked_at IS NULL \
               AND expires_at > strftime('%Y-%m-%dT%H:%M:%fZ','now')",
        )
        .bind(id)
        .fetch_optional(&mut **transaction)
        .await?
        .is_some()),
        (None, Some(id)) => Ok(sqlx::query_scalar::<_, i64>(
            "SELECT 1 FROM api_tokens WHERE id = ?1 AND revoked_at IS NULL",
        )
        .bind(id)
        .fetch_optional(&mut **transaction)
        .await?
        .is_some()),
        _ => Ok(false),
    }
}

/// 原子地消费票据；只有未过期、未使用且绑定对象仍然有效时返回 `true`。
///
/// # Errors
/// 数据库读写失败。
pub async fn consume(pool: &SqlitePool, ticket: &str) -> Result<bool, sqlx::Error> {
    if ticket.is_empty() {
        return Ok(false);
    }
    let hash = session::token_hash(ticket);
    let mut transaction = pool.begin().await?;
    let row = sqlx::query(
        "SELECT session_id, token_id, used_at, \
                (expires_at > strftime('%Y-%m-%dT%H:%M:%fZ','now')) AS unexpired \
         FROM event_tickets WHERE ticket_hash = ?1",
    )
    .bind(&hash)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(row) = row else {
        transaction.commit().await?;
        return Ok(false);
    };
    if row.get::<Option<String>, _>("used_at").is_some() || row.get::<i64, _>("unexpired") != 1 {
        transaction.commit().await?;
        return Ok(false);
    }

    let bound_alive = binding_alive(
        &mut transaction,
        row.get::<Option<String>, _>("session_id").as_deref(),
        row.get::<Option<String>, _>("token_id").as_deref(),
    )
    .await?;
    if !bound_alive {
        transaction.commit().await?;
        return Ok(false);
    }

    let consumed = sqlx::query(
        "UPDATE event_tickets SET used_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') \
         WHERE ticket_hash = ?1 AND used_at IS NULL \
           AND expires_at > strftime('%Y-%m-%dT%H:%M:%fZ','now')",
    )
    .bind(&hash)
    .execute(&mut *transaction)
    .await?
    .rows_affected();
    transaction.commit().await?;
    Ok(consumed == 1)
}

/// 清理过期或已用的票据；失败只由调用方决定是否忽略。
///
/// # Errors
/// 数据库写入失败。
pub async fn purge(pool: &SqlitePool) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query(
        "DELETE FROM event_tickets WHERE used_at IS NOT NULL \
               OR expires_at <= strftime('%Y-%m-%dT%H:%M:%fZ','now')",
    )
    .execute(pool)
    .await?
    .rows_affected())
}

fn new_ticket() -> String {
    let mut bytes = [0u8; TICKET_BYTES];
    OsRng.fill_bytes(&mut bytes);
    format!("{TICKET_SENTINEL}{}", password::hex(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tickets_are_prefixed_and_hashed_for_storage() {
        let ticket = new_ticket();
        assert!(ticket.starts_with(TICKET_SENTINEL));
        assert_eq!(ticket.len(), TICKET_SENTINEL.len() + TICKET_BYTES * 2);
        assert_ne!(new_ticket(), ticket);
        assert_eq!(session::token_hash(&ticket).len(), 64);
        assert_ne!(session::token_hash(&ticket), ticket);
    }
}

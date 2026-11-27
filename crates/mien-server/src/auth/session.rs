//! 会话存储与 Cookie。
//!
//! 契约：`docs/DATABASE-SCHEMA.md`（`sessions` 只存随机标识哈希）与
//! `docs/SECURITY.md` §5.1（`HttpOnly`、`SameSite=Strict`，非回环加 `Secure`）。
//! Cookie 值是高熵随机 Token，数据库只保存它的 SHA-256；CSRF Token 由会话 Token
//! 通过 HKDF 派生（见 [`crate::auth::csrf`]），因此不额外落库。
//!
//! 时间戳由 SQLite 的 `strftime('%Y-%m-%dT%H:%M:%fZ', …)` 生成：固定宽度的 UTC
//! 文本可按字典序比较，也避免为时间处理引入额外依赖。

use std::net::SocketAddr;
use std::time::Duration;

use axum::http::HeaderMap;
use axum::http::header;
use rand::RngCore;
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use crate::auth::{AuthError, csrf};

/// 会话 Cookie 名，与 `docs/contracts/openapi.yaml#/components/securitySchemes/cookieAuth` 一致。
pub const COOKIE_NAME: &str = "mien_session";
/// 会话绝对有效期。
pub const DEFAULT_TTL: Duration = Duration::from_secs(12 * 60 * 60);
/// SQLite 侧的当前 UTC 时间表达式；与 [`crate::auth::password`] 的写入格式一致。
pub const NOW_SQL: &str = "strftime('%Y-%m-%dT%H:%M:%fZ', 'now')";
/// Cookie Token 的随机字节数。
const TOKEN_BYTES: usize = 32;
/// 会话行标识的随机字节数。
const ID_BYTES: usize = 16;

/// 当前 UTC 时间戳；只用于审计等非比较场景，格式与 [`NOW_SQL`] 相同。
pub fn now_timestamp() -> String {
    // `%f` 给出秒的小数部分；同一格式的文本可直接比较。
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default();
    unix_millis_to_iso(millis)
}

/// 把 Unix 毫秒转换为 `YYYY-MM-DDTHH:MM:SS.sssZ`。
fn unix_millis_to_iso(millis: u128) -> String {
    let seconds = i64::try_from(millis / 1000).unwrap_or(i64::MAX);
    let sub_millis = millis % 1000;
    let (days, seconds_of_day) = (seconds / 86_400, seconds % 86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3600;
    let minute = (seconds_of_day % 3600) / 60;
    let second = seconds_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{sub_millis:03}Z")
}

/// Howard Hinnant 的 `civil_from_days`：把 1970-01-01 起的天数转为公历日期。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (
        year,
        u32::try_from(m).unwrap_or(1),
        u32::try_from(d).unwrap_or(1),
    )
}

/// 会话快照；`csrf_token` 由 Cookie Token 派生，不落库。
#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub csrf_token: String,
}

/// 新建会话的结果：明文 Cookie Token 只在此刻返回一次。
#[derive(Debug, Clone)]
pub struct Issued {
    pub token: String,
    pub session: Session,
}

/// 创建会话并返回明文 Token。
///
/// # Errors
/// 数据库写入失败。
pub async fn create(pool: &SqlitePool, ttl: Duration) -> Result<Issued, AuthError> {
    let token = new_token();
    let id = new_id();
    let expires = format!("+{} seconds", ttl.as_secs());
    sqlx::query(
        "INSERT INTO sessions (id, token_hash, created_at, expires_at, last_used_at, revoked_at) \
         VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), \
                 strftime('%Y-%m-%dT%H:%M:%fZ', 'now', ?3), \
                 strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), NULL)",
    )
    .bind(&id)
    .bind(token_hash(&token))
    .bind(expires)
    .execute(pool)
    .await
    .map_err(|_| AuthError::internal())?;
    Ok(Issued {
        session: Session {
            id,
            csrf_token: csrf::derive(&token),
        },
        token,
    })
}

/// 按 Cookie Token 查找有效会话；命中时刷新 `last_used_at`。
///
/// # Errors
/// 数据库查询失败。
pub async fn find(pool: &SqlitePool, token: &str) -> Result<Option<Session>, AuthError> {
    let id = sqlx::query_scalar::<_, String>(
        "SELECT id FROM sessions \
         WHERE token_hash = ?1 AND revoked_at IS NULL \
           AND expires_at > strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
    )
    .bind(token_hash(token))
    .fetch_optional(pool)
    .await
    .map_err(|_| AuthError::internal())?;

    let Some(id) = id else {
        return Ok(None);
    };
    let _ = sqlx::query(
        "UPDATE sessions SET last_used_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?1",
    )
    .bind(&id)
    .execute(pool)
    .await;
    Ok(Some(Session {
        csrf_token: csrf::derive(token),
        id,
    }))
}

/// 撤销单个会话。
///
/// # Errors
/// 数据库写入失败。
pub async fn revoke(pool: &SqlitePool, id: &str) -> Result<(), AuthError> {
    sqlx::query(
        "UPDATE sessions SET revoked_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
         WHERE id = ?1 AND revoked_at IS NULL",
    )
    .bind(id)
    .execute(pool)
    .await
    .map_err(|_| AuthError::internal())?;
    Ok(())
}

/// 撤销除当前会话外的全部会话（改密后调用）。
///
/// # Errors
/// 数据库写入失败。
pub async fn revoke_others(pool: &SqlitePool, keep: &str) -> Result<u64, AuthError> {
    let result = sqlx::query(
        "UPDATE sessions SET revoked_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
         WHERE id <> ?1 AND revoked_at IS NULL",
    )
    .bind(keep)
    .execute(pool)
    .await
    .map_err(|_| AuthError::internal())?;
    Ok(result.rows_affected())
}

/// 清理过期会话。
///
/// # Errors
/// 数据库写入失败。
pub async fn purge_expired(pool: &SqlitePool) -> Result<u64, AuthError> {
    let result = sqlx::query(
        "DELETE FROM sessions WHERE expires_at <= strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
    )
    .execute(pool)
    .await
    .map_err(|_| AuthError::internal())?;
    Ok(result.rows_affected())
}

/// `Set-Cookie` 值：`HttpOnly`、`SameSite=Strict`、`Path=/`，非回环附加 `Secure`。
pub fn cookie_header(token: &str, ttl: Duration, secure: bool) -> String {
    let mut value = format!(
        "{COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}",
        ttl.as_secs()
    );
    if secure {
        value.push_str("; Secure");
    }
    value
}

/// 清除会话 Cookie。
pub fn clear_cookie_header(secure: bool) -> String {
    let mut value = format!("{COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0");
    if secure {
        value.push_str("; Secure");
    }
    value
}

/// 从请求头解析会话 Cookie 值；不存在或为空时返回 `None`。
pub fn parse_cookie(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    for pair in raw.split(';') {
        let pair = pair.trim();
        let Some((name, value)) = pair.split_once('=') else {
            continue;
        };
        if name.trim() == COOKIE_NAME && !value.trim().is_empty() {
            return Some(value.trim().to_owned());
        }
    }
    None
}

fn new_token() -> String {
    let mut bytes = [0u8; TOKEN_BYTES];
    OsRng.fill_bytes(&mut bytes);
    crate::auth::password::hex(&bytes)
}

fn new_id() -> String {
    let mut bytes = [0u8; ID_BYTES];
    OsRng.fill_bytes(&mut bytes);
    crate::auth::password::hex(&bytes)
}

/// 会话 Token 的存储哈希；Cookie 明文不落库。
pub fn token_hash(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    crate::auth::password::hex(&digest)
}

/// 同源/回环地址判定用的来源（scheme + host + port）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Origin {
    scheme: String,
    host: String,
    port: u16,
}

impl Origin {
    /// 解析 `Origin`/`Referer`/`MIEN_PUBLIC_URL` 的 `scheme://authority` 前缀。
    pub fn parse(value: &str) -> Option<Self> {
        let (scheme, rest) = value.split_once("://")?;
        let scheme = scheme.to_ascii_lowercase();
        let default_port = match scheme.as_str() {
            "http" => 80,
            "https" => 443,
            _ => return None,
        };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        let (host, port) = if let Some(stripped) = authority.strip_prefix('[') {
            let (host, rest) = stripped.split_once(']')?;
            let port = match rest.strip_prefix(':') {
                Some(port) => port.parse().ok()?,
                None => default_port,
            };
            (host.to_ascii_lowercase(), port)
        } else if let Some((host, port)) = authority.rsplit_once(':') {
            match port.parse() {
                Ok(port) => (host.to_ascii_lowercase(), port),
                Err(_) => (authority.to_ascii_lowercase(), default_port),
            }
        } else {
            (authority.to_ascii_lowercase(), default_port)
        };
        if host.is_empty() {
            return None;
        }
        Some(Self { scheme, host, port })
    }

    /// 回环来源：`localhost`、`127.0.0.0/8`、`::1`。
    pub fn is_loopback(&self) -> bool {
        self.host == "localhost"
            || self
                .host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    }

    /// 与另一个来源同源（忽略默认端口与大小写）。
    pub fn same_as(&self, other: &Self) -> bool {
        self.scheme == other.scheme && self.host == other.host && self.port == other.port
    }
}

/// 对端地址；没有连接信息时回退到回环，仅用于测试与本地调用。
pub fn peer_ip(peer: Option<SocketAddr>) -> std::net::IpAddr {
    peer.map_or(std::net::IpAddr::from([127, 0, 0, 1]), |address| {
        address.ip()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_round_trips_and_carries_the_required_attributes() {
        let value = cookie_header("abc", Duration::from_secs(60), false);
        assert_eq!(
            value,
            "mien_session=abc; Path=/; HttpOnly; SameSite=Strict; Max-Age=60"
        );
        assert!(cookie_header("abc", Duration::from_secs(60), true).ends_with("; Secure"));

        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            "other=1; mien_session=abc".parse().expect("cookie header"),
        );
        assert_eq!(parse_cookie(&headers).as_deref(), Some("abc"));
    }

    #[test]
    fn tokens_are_hashed_for_storage() {
        let hash = token_hash("token");
        assert_eq!(hash.len(), 64);
        assert_ne!(hash, "token");
        assert_eq!(hash, token_hash("token"));
    }

    #[test]
    fn timestamps_are_fixed_width_utc() {
        assert_eq!(unix_millis_to_iso(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(
            unix_millis_to_iso(1_700_000_000_123),
            "2023-11-14T22:13:20.123Z"
        );
        // 同一格式的文本按字典序即按时间排序。
        assert!(unix_millis_to_iso(1_700_000_000_000) < unix_millis_to_iso(1_700_000_001_000));
        let now = now_timestamp();
        assert_eq!(now.len(), 24, "timestamp: {now}");
        assert!(now.ends_with('Z'), "timestamp: {now}");
    }

    #[test]
    fn origins_are_parsed_and_compared() {
        let origin = Origin::parse("https://Mien.example.com").expect("origin");
        assert!(origin.same_as(&Origin::parse("https://mien.example.com:443").expect("origin")));
        assert!(!origin.same_as(&Origin::parse("https://mien.example.com:8443").expect("origin")));
        assert!(
            Origin::parse("http://127.0.0.1:8787")
                .expect("origin")
                .is_loopback()
        );
        assert!(
            Origin::parse("http://[::1]:8787")
                .expect("origin")
                .is_loopback()
        );
        assert!(
            !Origin::parse("http://192.0.2.10:8787")
                .expect("origin")
                .is_loopback()
        );
        assert!(Origin::parse("null").is_none());
        assert!(Origin::parse("ftp://mien.example.com").is_none());
    }
}

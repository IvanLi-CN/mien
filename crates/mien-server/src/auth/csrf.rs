//! 会话 CSRF Token 与 `Origin`/`Referer` 校验（`REQ-API-006`）。
//!
//! CSRF Token 由会话 Cookie 值经 HKDF-SHA256 派生，因此不需要额外存储，也不会
//! 随会话表泄露。写请求必须同时满足：Cookie 会话、`x-csrf-token` 与会话值一致、
//! `Origin`/`Referer` 同源（回环开发模式只接受回环来源）。Bearer 请求不使用 Cookie，
//! 因此不参与 CSRF 校验。

use axum::http::{HeaderMap, Method, header};
use hkdf::Hkdf;
use sha2::Sha256;

use crate::auth::session::Origin;
use crate::auth::{AuthError, session};

/// CSRF 请求头名，与 `docs/contracts/openapi.yaml#/components/parameters/CsrfToken` 一致。
pub const CSRF_HEADER: &str = "x-csrf-token";

/// CSRF 令牌派生域分隔标签；版本化以便轮换。
const CSRF_INFO: &[u8] = b"mien/session-csrf/v1";

/// 由会话 Cookie Token 派生 CSRF Token。
pub fn derive(token: &str) -> String {
    let hkdf = Hkdf::<Sha256>::new(Some(b"mien/csrf-salt/v1"), token.as_bytes());
    let mut output = [0u8; 32];
    hkdf.expand(CSRF_INFO, &mut output)
        .expect("32 bytes is a valid HKDF output length");
    crate::auth::password::hex(&output)
}

/// 校验 `x-csrf-token` 与会话 CSRF Token 一致（常量时间比较）。
pub fn verify(headers: &HeaderMap, session_token: &str) -> bool {
    let Some(provided) = headers
        .get(CSRF_HEADER)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    constant_time_eq(provided.as_bytes(), derive(session_token).as_bytes())
}

/// 写方法：需要 CSRF 与 Origin 校验。
pub fn is_write_method(method: &Method) -> bool {
    matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    )
}

/// 校验 `Origin`（缺失时回退 `Referer`）与配置的对外来源同源。
///
/// - 配置了 `MIEN_PUBLIC_URL`：只接受该来源。
/// - 回环开发模式（无 `MIEN_PUBLIC_URL`）：只接受回环来源。
///
/// 缺失 `Origin` 与 `Referer` 的 Cookie 写请求一律拒绝：浏览器对跨源与同源写请求
/// 都会发送 `Origin`，缺失说明不是浏览器同源调用。
///
/// # Errors
/// 跨源、来源不可解析或两者都缺失时返回 [`AuthError::origin_invalid`]。
pub fn verify_origin(headers: &HeaderMap, public_url: Option<&str>) -> Result<(), AuthError> {
    let candidate = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .or_else(|| {
            headers
                .get(header::REFERER)
                .and_then(|value| value.to_str().ok())
        })
        .ok_or_else(AuthError::origin_invalid)?;

    let Some(origin) = Origin::parse(candidate) else {
        return Err(AuthError::origin_invalid());
    };

    match public_url.map(Origin::parse) {
        Some(Some(expected)) if origin.same_as(&expected) => Ok(()),
        Some(Some(_)) => Err(AuthError::origin_invalid()),
        Some(None) => Err(AuthError::origin_invalid()),
        None if origin.is_loopback() => Ok(()),
        None => Err(AuthError::origin_invalid()),
    }
}

/// Cookie 写请求的完整防护：Origin 同源 + CSRF 头匹配会话值。
///
/// # Errors
/// 来源不合法返回 `AUTH_ORIGIN_INVALID`；CSRF 缺失或错误返回 `AUTH_CSRF_INVALID`。
pub fn verify_write(
    headers: &HeaderMap,
    public_url: Option<&str>,
    session_token: &str,
) -> Result<(), AuthError> {
    verify_origin(headers, public_url)?;
    if verify(headers, session_token) {
        Ok(())
    } else {
        Err(AuthError::csrf_invalid())
    }
}

/// 当前请求是否需要 CSRF 校验：Cookie 会话写请求。
pub fn requires_csrf(method: &Method, has_session_cookie: bool) -> bool {
    has_session_cookie && is_write_method(method)
}

/// 会话 Cookie 是否存在（不校验有效性，仅供 CSRF/Auth 判定分流）。
pub fn has_session_cookie(headers: &HeaderMap) -> bool {
    session::parse_cookie(headers).is_some()
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, value.parse().expect("header value"));
        }
        map
    }

    #[test]
    fn csrf_tokens_are_session_scoped_and_verifiable() {
        let token = "session-token";
        let csrf = derive(token);
        assert_eq!(csrf.len(), 64);
        assert_eq!(csrf, derive(token));
        assert_ne!(csrf, derive("other-session"));
        assert!(verify(&headers(&[(CSRF_HEADER, &csrf)]), token));
        assert!(!verify(&headers(&[(CSRF_HEADER, &derive("other"))]), token));
        assert!(!verify(&headers(&[]), token));
    }

    #[test]
    fn loopback_mode_accepts_only_loopback_origins() {
        assert!(verify_origin(&headers(&[("origin", "http://127.0.0.1:8787")]), None).is_ok());
        assert!(verify_origin(&headers(&[("origin", "http://localhost:5173")]), None).is_ok());
        assert!(verify_origin(&headers(&[("origin", "https://evil.example")]), None).is_err());
    }

    #[test]
    fn public_url_mode_accepts_only_that_origin() {
        let public_url = Some("https://mien.example.com");
        assert!(
            verify_origin(
                &headers(&[("origin", "https://mien.example.com")]),
                public_url
            )
            .is_ok()
        );
        assert!(
            verify_origin(
                &headers(&[("origin", "https://mien.example.com:443")]),
                public_url
            )
            .is_ok()
        );
        assert!(
            verify_origin(&headers(&[("origin", "https://evil.example")]), public_url).is_err()
        );
        assert!(
            verify_origin(&headers(&[("origin", "http://127.0.0.1:8787")]), public_url).is_err()
        );
    }

    #[test]
    fn referer_is_the_fallback_and_both_missing_is_rejected() {
        assert!(
            verify_origin(
                &headers(&[("referer", "http://127.0.0.1:8787/profiles/1")]),
                None
            )
            .is_ok()
        );
        assert!(verify_origin(&headers(&[]), None).is_err());
        assert!(verify_origin(&headers(&[("origin", "null")]), None).is_err());
    }
}

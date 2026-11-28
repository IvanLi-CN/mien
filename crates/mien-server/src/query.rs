//! 共享的查询串与游标编解码。
//!
//! 档案、代理与审计三个列表端点必须给出**同一套**错误语义（`REQ-API-009`）：
//! 未知筛选字段、重复字段、非法转义、非法游标一律 422 `VALIDATION_FAILED`。
//! 此前每个模块各复制一份解码器，第四份出现时错误语义开始分叉，因此集中在这里。

use std::collections::BTreeMap;

use axum::http::Uri;

use crate::profiles::ApiError;

/// 游标原文上限，防止任意超长输入参与解码。
pub const CURSOR_MAX_LEN: usize = 512;

/// 游标内的字段分隔符（US，不会出现在时间戳或 ULID 中）。
const CURSOR_SEPARATOR: u8 = 0x1f;

/// 一个分页游标：上一页最后一行的排序键与决胜 id。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    pub key: String,
    pub id: String,
}

/// 把查询串解析为多重映射，重复字段与非法转义都按 422 处理。
///
/// `+` 按 `application/x-www-form-urlencoded` 解释为空格。
pub fn query_map(uri: &Uri) -> Result<BTreeMap<String, String>, ApiError> {
    let mut out = BTreeMap::new();
    let Some(query) = uri.query() else {
        return Ok(out);
    };
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        let name = percent_decode(name)?;
        let value = percent_decode(value)?;
        if out.insert(name.clone(), value).is_some() {
            return Err(ApiError::validation(&name, "is repeated"));
        }
    }
    Ok(out)
}

/// 拒绝未在 `allowed` 中列出的参数。
///
/// 未知筛选字段若不拒绝，客户端拼错字段名会得到"看起来成功"的全量结果，
/// 这比报错更难排查。按字母序报告第一个未知字段，保证同一请求错误稳定。
pub fn reject_unknown(
    params: &BTreeMap<String, String>,
    allowed: &[&str],
    surface: &str,
) -> Result<(), ApiError> {
    let mut unknown: Vec<&String> = params
        .keys()
        .filter(|key| !allowed.contains(&key.as_str()))
        .collect();
    unknown.sort();
    if let Some(field) = unknown.first() {
        return Err(ApiError::validation(
            field,
            format!("is not a supported filter for {surface} listing"),
        ));
    }
    Ok(())
}

/// 解析页大小；越界是 422 且消息里写明上限，绝不静默截断。
pub fn parse_limit(
    params: &BTreeMap<String, String>,
    default: u32,
    max: u32,
) -> Result<u32, ApiError> {
    let Some(raw) = params.get("limit") else {
        return Ok(default);
    };
    let limit = raw.parse::<u32>().map_err(|_| {
        ApiError::validation("limit", format!("must be an integer between 1 and {max}"))
    })?;
    if !(1..=max).contains(&limit) {
        return Err(ApiError::validation(
            "limit",
            format!("must be an integer between 1 and {max}"),
        ));
    }
    Ok(limit)
}

/// 编码游标：`hex(key \\x1f id)`。
pub fn encode_cursor(key: &str, id: &str) -> String {
    let mut bytes = Vec::with_capacity(key.len() + id.len() + 1);
    bytes.extend_from_slice(key.as_bytes());
    bytes.push(CURSOR_SEPARATOR);
    bytes.extend_from_slice(id.as_bytes());
    hex_encode(&bytes)
}

/// 解码游标；畸形输入是 422。
///
/// `validate_id` 由调用方给出，用来按各自实体的标识形态校验决胜键
/// （档案与审计都用 ULID 形态）。
pub fn decode_cursor(text: &str, validate_id: impl Fn(&str) -> bool) -> Result<Cursor, ApiError> {
    let invalid = || ApiError::validation("cursor", "is not a valid page cursor");
    if text.is_empty() || text.len() > CURSOR_MAX_LEN {
        return Err(invalid());
    }
    let bytes = hex_decode(text).ok_or_else(invalid)?;
    let text = String::from_utf8(bytes).map_err(|_| invalid())?;
    let (key, id) = text
        .split_once(CURSOR_SEPARATOR as char)
        .ok_or_else(invalid)?;
    if key.is_empty() || !validate_id(id) {
        return Err(invalid());
    }
    Ok(Cursor {
        key: key.to_owned(),
        id: id.to_owned(),
    })
}

pub fn hex_encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

pub fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() / 2);
    for pair in text.as_bytes().chunks_exact(2) {
        let high = (pair[0] as char).to_digit(16)?;
        let low = (pair[1] as char).to_digit(16)?;
        out.push(((high << 4) | low) as u8);
    }
    Some(out)
}

fn percent_decode(raw: &str) -> Result<String, ApiError> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                let high = *bytes
                    .get(index + 1)
                    .ok_or_else(|| ApiError::validation("query", "has an invalid escape"))?;
                let low = *bytes
                    .get(index + 2)
                    .ok_or_else(|| ApiError::validation("query", "has an invalid escape"))?;
                let digit = |byte: u8| {
                    (byte as char)
                        .to_digit(16)
                        .ok_or_else(|| ApiError::validation("query", "has an invalid escape"))
                };
                out.push(((digit(high)? << 4) | digit(low)?) as u8);
                index += 3;
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            other => {
                out.push(other);
                index += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| ApiError::validation("query", "must be valid UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ulid() -> &'static str {
        "01JC5W3X6Z8N4M2P9QR7STVWXY"
    }

    #[test]
    fn cursors_round_trip_and_reject_malformed_input() {
        let text = encode_cursor("2026-11-27T10:00:00Z", ulid());
        let decoded = decode_cursor(&text, |id| id == ulid()).expect("valid cursor");
        assert_eq!(decoded.key, "2026-11-27T10:00:00Z");
        assert_eq!(decoded.id, ulid());

        for bad in ["", "zz", "not-hex", &hex_encode(b"onlykey")] {
            assert!(
                decode_cursor(bad, |id| id == ulid()).is_err(),
                "`{bad}` must be rejected"
            );
        }
        // 决胜键必须通过调用方的校验。
        let foreign = encode_cursor("2026-11-27T10:00:00Z", "nope");
        assert!(decode_cursor(&foreign, |id| id == ulid()).is_err());
    }

    #[test]
    fn query_maps_reject_repeats_and_bad_escapes() {
        let uri: Uri = "/x?a=1&b=2".parse().expect("uri");
        let map = query_map(&uri).expect("map");
        assert_eq!(map.get("a").map(String::as_str), Some("1"));
        assert_eq!(map.get("b").map(String::as_str), Some("2"));

        let repeated: Uri = "/x?a=1&a=2".parse().expect("uri");
        assert!(query_map(&repeated).is_err());

        let bad_escape: Uri = "/x?a=%zz".parse().expect("uri");
        assert!(query_map(&bad_escape).is_err());
    }

    #[test]
    fn limits_and_unknown_fields_are_validated() {
        let mut params = BTreeMap::new();
        params.insert("limit".to_owned(), "201".to_owned());
        let error = parse_limit(&params, 50, 200).expect_err("201 exceeds the cap");
        assert!(
            error.message.contains("200") || format!("{:?}", error.details).contains("200"),
            "the message must name the cap: {error:?}"
        );
        params.insert("limit".to_owned(), "0".to_owned());
        assert!(parse_limit(&params, 50, 200).is_err());
        params.insert("limit".to_owned(), "200".to_owned());
        assert_eq!(parse_limit(&params, 50, 200).expect("at the cap"), 200);

        let mut unknown = BTreeMap::new();
        unknown.insert("nope".to_owned(), "1".to_owned());
        assert!(reject_unknown(&unknown, &["limit"], "audit").is_err());
    }
}

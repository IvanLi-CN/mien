//! 分享链接解析：fixture 场景表 +  oversized/脱敏/出站键串联。

use std::collections::BTreeMap;

use mien_core::ErrorCode;
use mien_core::ports::proxy::ProxyKind;
use mien_core::proxy::keys::{OutboundKey, ProxyProtocol, Transport, outbound_key};
use mien_core::proxy::share_link::{MAX_LINK_BYTES, ProxyDraft, parse_share_link};
use serde::Deserialize;
use serde_json::{Value, json};

const FIXTURE: &str = include_str!("../../../fixtures/proxy/share-links.json");
const MASTER_KEY: &[u8] = b"share-link-master";

#[derive(Deserialize)]
struct Fixture {
    schema_version: u32,
    notes: Vec<String>,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    link: String,
    expect: Expect,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Expect {
    Draft(DraftExpect),
    Error(ErrorExpect),
}

#[derive(Deserialize)]
struct DraftExpect {
    kind: String,
    server: String,
    port: u16,
    options: BTreeMap<String, Value>,
    credential_fields: Vec<String>,
    #[serde(default)]
    warnings: Vec<String>,
}

#[derive(Deserialize)]
struct ErrorExpect {
    error: String,
    message_contains: String,
}

fn fixture() -> Fixture {
    serde_json::from_str(FIXTURE).expect("share-link fixture is valid JSON")
}

/// 草稿的线上形状：只有非秘密字段与凭据字段名，序列化与 `Debug` 都不含凭据值。
fn projection(draft: &ProxyDraft) -> Value {
    serde_json::json!({
        "kind": draft.kind().as_str(),
        "server": draft.server(),
        "port": draft.port(),
        "options": draft.options(),
        "credential_fields": draft.credential_fields(),
        "warnings": draft.warnings(),
    })
}

fn assert_case(case: &Case) {
    match &case.expect {
        Expect::Error(expected) => {
            let error = parse_share_link(&case.link)
                .map(|draft| format!("{:?}", projection(&draft)))
                .unwrap_err();
            assert_eq!(
                error.code().as_str(),
                expected.error,
                "{}: unexpected error code ({error})",
                case.name
            );
            let message = error.to_string();
            assert!(
                message.contains(&expected.message_contains),
                "{}: message {message:?} must name {:?}",
                case.name,
                expected.message_contains
            );
        }
        Expect::Draft(expected) => {
            let draft = parse_share_link(&case.link).unwrap_or_else(|error| {
                panic!("{}: rejected with {:?}: {error}", case.name, error.code())
            });
            let actual = serde_json::json!({
                "kind": expected.kind,
                "server": draft.server(),
                "port": draft.port(),
                "options": draft.options(),
                "credential_fields": draft.credential_fields(),
                "warnings": draft.warnings(),
            });
            let wanted = serde_json::json!({
                "kind": expected.kind,
                "server": expected.server,
                "port": expected.port,
                "options": expected.options,
                "credential_fields": expected.credential_fields,
                "warnings": expected.warnings,
            });
            assert_eq!(actual, wanted, "{}: draft mismatch", case.name);
        }
    }
}

#[test]
fn fixture_table_matches_the_parser() {
    let fixture = fixture();
    assert_eq!(fixture.schema_version, 1);
    assert!(!fixture.notes.is_empty(), "fixture must document itself");
    assert!(
        fixture.cases.len() >= 30,
        "fixture must cover the five protocols plus rejections, got {}",
        fixture.cases.len()
    );
    let mut names = fixture.cases.iter().map(|case| case.name.as_str());
    let unique: std::collections::BTreeSet<&str> = names.by_ref().collect();
    assert_eq!(
        unique.len(),
        fixture.cases.len(),
        "case names must be unique"
    );
    for case in &fixture.cases {
        assert_case(case);
    }
}

#[test]
fn every_supported_protocol_has_accepted_and_missing_field_cases() {
    let cases = fixture().cases;
    let required = [
        ("vless", "vless", "`id`"),
        ("vmess", "vmess", "`add`"),
        ("ss", "shadowsocks", "`method`"),
        ("trojan", "trojan", "`password`"),
        ("hysteria2", "hysteria2", "`auth`"),
    ];
    for (scheme, kind, missing_field) in required {
        let prefix = format!("{scheme}://");
        let mut accepted = 0usize;
        for case in &cases {
            if !case.link.starts_with(&prefix) {
                continue;
            }
            match &case.expect {
                Expect::Draft(expected) => {
                    assert_eq!(expected.kind, kind, "{}: kind", case.name);
                    accepted += 1;
                }
                Expect::Error(expected) => {
                    if expected.message_contains == missing_field {
                        assert_eq!(
                            expected.error, "PROXY_CONFIG_INVALID",
                            "{}: error code",
                            case.name
                        );
                    }
                }
            }
        }
        assert!(
            accepted >= 2,
            "{scheme}: needs at least two accepted shapes"
        );
        assert!(
            cases.iter().any(|case| case.link.starts_with(&prefix)
                && matches!(&case.expect, Expect::Error(error)
                    if error.message_contains == missing_field)),
            "{scheme}: needs a case rejecting a missing {missing_field}"
        );
    }
}

#[test]
fn oversized_and_malformed_links_are_rejected_without_panicking() {
    let oversized = format!(
        "vless://{}@edge.example.test:443?type=tcp",
        "a".repeat(MAX_LINK_BYTES)
    );
    assert_eq!(
        parse_share_link(&oversized).unwrap_err().code(),
        ErrorCode::ProxyConfigInvalid
    );
    let boundary = "v".repeat(MAX_LINK_BYTES - "less://".len());
    assert!(parse_share_link(&format!("vless://{boundary}")).is_err());

    for link in [
        "",
        "   ",
        "://edge.example.test:443",
        "vless://",
        "vless://id@",
        "vless://id@:443",
        "vless://id@edge.example.test:0",
        "vless://id@edge.example.test:70000",
        "vless://id@edge.example.test:443?sni=%zz",
        "vmess://%%%%",
        "vmess://",
        "ss://",
        "trojan://",
        "hysteria2://",
        "tuic://id@edge.example.test:443",
        "wireguard://key@edge.example.test:51820",
        "socks5://edge.example.test:1080",
    ] {
        let error = parse_share_link(link).expect_err(link);
        assert_eq!(error.code(), ErrorCode::ProxyConfigInvalid, "{link:?}");
    }
}

#[test]
fn base64_decoding_accepts_both_alphabet_and_padding_variants() {
    let payload = br#"{"v":"2","add":"edge.example.test","port":443,"id":"00000000-0000-4000-8000-000000000001","net":"tcp","tls":"none"}"#;
    use base64::Engine as _;
    use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
    for variant in [
        STANDARD.encode(payload),
        STANDARD_NO_PAD.encode(payload),
        URL_SAFE.encode(payload),
        URL_SAFE_NO_PAD.encode(payload),
    ] {
        let draft = parse_share_link(&format!("vmess://{variant}")).expect("variant decodes");
        assert_eq!(draft.server(), "edge.example.test");
        assert_eq!(draft.port(), 443);
    }

    // 坏 base64 只返回错误：非法字符、不完整分组、非法 UTF-8 都不 panic。
    for payload in ["!!!!", "abc", "////", "eyJhIjo"] {
        let error = parse_share_link(&format!("vmess://{payload}")).expect_err(payload);
        assert_eq!(error.code(), ErrorCode::ProxyConfigInvalid, "{payload}");
    }
    let invalid_utf8 = STANDARD_NO_PAD.encode([0xff, 0xfe, 0xfd]);
    assert!(parse_share_link(&format!("vmess://{invalid_utf8}")).is_err());
}

#[test]
fn credentials_never_leak_into_debug_or_serialized_draft() {
    let uuid = "00000000-0000-4000-8000-000000000001";
    let links = [
        format!("vless://{uuid}@edge.example.test:443?type=tcp&security=tls&sni=edge.example.test"),
        "trojan://password-test@edge.example.test:443?sni=edge.example.test".to_owned(),
        "hysteria2://auth-token@edge.example.test:8443?sni=edge.example.test".to_owned(),
        "ss://YWVzLTI1Ni1nY206cGFzc3dvcmQtdGVzdA@edge.example.test:8388".to_owned(),
    ];
    for link in links {
        let draft = parse_share_link(&link).expect("valid link");
        let debug = format!("{draft:?}");
        let json = serde_json::to_string(&draft).expect("draft serializes");
        for secret in [uuid, "password-test", "auth-token"] {
            assert!(!debug.contains(secret), "debug leaked {secret}: {debug}");
            assert!(!json.contains(secret), "json leaked {secret}: {json}");
        }
        for field in draft.credential_fields() {
            assert!(
                json.contains(&format!("\"{field}\"")),
                "field name {field} missing"
            );
        }
        assert!(
            !json.contains("credentials"),
            "credentials object must not serialize"
        );
        assert!(!json.contains(&link), "the full link must not be echoed");
    }
}

#[test]
fn drafts_feed_proxy_config_and_outbound_key() {
    let cases = fixture().cases;
    let mut keys: Vec<(String, OutboundKey)> = Vec::new();
    for case in &cases {
        let Expect::Draft(expected) = &case.expect else {
            continue;
        };
        let draft = parse_share_link(&case.link).expect(&case.name);
        let config = draft
            .proxy_config()
            .unwrap_or_else(|error| panic!("{}: {:?} {error}", case.name, error.code()));
        assert_eq!(config.server, expected.server, "{}: server", case.name);
        assert_eq!(config.port, expected.port, "{}: port", case.name);
        assert_eq!(
            config.protocol,
            match draft.kind() {
                ProxyKind::Vless => ProxyProtocol::Vless,
                ProxyKind::Vmess => ProxyProtocol::Vmess,
                ProxyKind::Shadowsocks => ProxyProtocol::Shadowsocks,
                ProxyKind::Trojan => ProxyProtocol::Trojan,
                ProxyKind::Hysteria2 => ProxyProtocol::Hysteria2,
                other => panic!("{}: non share-link kind {other:?}", case.name),
            }
        );
        if draft.kind() == ProxyKind::Hysteria2 {
            assert_eq!(config.transport, Transport::Hysteria);
        }
        let key = outbound_key(&config, MASTER_KEY).expect("outbound key");
        let text = key.as_str();
        assert!(text.starts_with("out:") && text.len() == 68, "{text}");
        assert!(
            text[4..].bytes().all(|byte| byte.is_ascii_hexdigit()),
            "{text}"
        );
        for secret in ["password-test", "auth-token", "00000000-0000-4000"] {
            assert!(!text.contains(secret), "key leaked {secret}");
        }
        keys.push((case.name.clone(), key));

        // `ProxyInput` 与 `ProxyConfig` 描述同一节点。
        let input = draft.proxy_input(case.name.clone()).expect("proxy input");
        assert_eq!(input.server, config.server);
        assert_eq!(input.port, config.port);
        assert_eq!(input.credentials.keys().collect::<Vec<_>>(), {
            let fields: Vec<&String> = config.credentials.values.keys().collect();
            fields
        });
    }
    assert!(keys.len() >= 10, "expected a broad accepted set");

    // 相同节点与凭据产生相同键；换凭据即换键。
    let base =
        parse_share_link("trojan://password-test@edge.example.test:443?sni=edge.example.test")
            .expect("base");
    let same = parse_share_link(
        "trojan://password-test@edge.example.test:443?sni=edge.example.test#renamed",
    )
    .expect("same");
    let other =
        parse_share_link("trojan://other-password@edge.example.test:443?sni=edge.example.test")
            .expect("other");
    assert_eq!(
        outbound_key(&base.proxy_config().unwrap(), MASTER_KEY).unwrap(),
        outbound_key(&same.proxy_config().unwrap(), MASTER_KEY).unwrap()
    );
    assert_ne!(
        outbound_key(&base.proxy_config().unwrap(), MASTER_KEY).unwrap(),
        outbound_key(&other.proxy_config().unwrap(), MASTER_KEY).unwrap()
    );
}

#[test]
fn hysteria2_draft_is_engine_ready() {
    let draft = parse_share_link(
        "hysteria2://auth-token@edge.example.test:8443?sni=edge.example.test&insecure=1",
    )
    .expect("hysteria2");
    let config = draft.proxy_config().expect("config");
    assert_eq!(config.transport, Transport::Hysteria);
    assert!(config.tls.allow_insecure);
    assert_eq!(config.tls.server_name.as_deref(), Some("edge.example.test"));
    assert_eq!(
        config.credentials.values.get("auth").map(String::as_str),
        Some("auth-token")
    );
    assert_eq!(draft.options().get("transport"), Some(&json!("hysteria")));
    assert_eq!(draft.options().get("security"), Some(&json!("tls")));
}

#[test]
fn rejected_links_never_echo_secret_values() {
    // 凭据存在但其他字段非法：错误消息只点名缺失/非法字段，不回显值。
    let cases = [
        (
            "vless://00000000-0000-4000-8000-000000000001@edge.example.test:443?security=xtls&sni=edge.example.test",
            "00000000-0000-4000-8000-000000000001",
        ),
        (
            "tuic://00000000-0000-4000-8000-000000000001:password-test@edge.example.test:443",
            "password-test",
        ),
        (
            "ss://:password-test@edge.example.test:8388",
            "password-test",
        ),
        (
            "hysteria2://auth-token@edge.example.test:8443?sni=edge.example.test&insecure=maybe",
            "auth-token",
        ),
    ];
    for (link, secret) in cases {
        let error = parse_share_link(link).expect_err(link);
        let message = error.to_string();
        assert!(!message.contains(secret), "{message} leaked {secret}");
        assert!(!message.contains(link), "{message} echoed the link");
        assert_eq!(error.code(), ErrorCode::ProxyConfigInvalid);
    }
}

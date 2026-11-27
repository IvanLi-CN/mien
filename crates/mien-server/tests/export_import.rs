//! 档案导出与导入验收（`REQ-API-020`、`VER-API-020`、`VER-PROFILE-009/010`）。
//!
//! 覆盖：明文导出逐键符合 `profile-export.schema.json` 且不含凭据值/`runtime.window`/
//! `password|token|uuid|cookie` 键；`passphrase` 与 `format` 的双向约束；加密往返后
//! 指纹逐字段相等；错误口令只报字段名；`validate` 零写入；`abort` 冲突零写入；
//! `new_id` 两次导入两行；第二项写入失败时整批回滚；口令不出现在日志、审计、事件与
//! 任何响应体中。
//!
//! 与真实服务一致使用**多线程**运行时（`mien_core` 的同步 gRPC 适配层在
//! `current_thread` 上会死锁）。

use std::sync::{Arc, Mutex, Once};

use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use axum::response::Response;
use mien_core::ports::proxy::ProxyRepository;
use mien_server::router;
use mien_server::state::{AppState, AuthState, SecuritySettings};
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;

const ADMIN_PASSWORD: &str = "correct horse battery staple";
const PASSPHRASE: &str = "t28-export-passphrase-9f3a";
const SECRET_UUID: &str = "3f1c2d4e-5a6b-4c7d-8e9f-0a1b2c3d4e5f";
const SECRET_PASSWORD: &str = "ss-secret-password-t28";

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime")
}

fn state() -> (TempDir, AppState) {
    let dir = TempDir::new().expect("temp dir");
    let pool = mien_core::storage::open_and_migrate(&dir.path().join("mien.db")).expect("migrate");
    let key = mien_core::secrets::MasterKey::from_bytes([7_u8; 32]);
    std::fs::create_dir_all(dir.path().join("secrets")).expect("secrets dir");
    let key_path = dir.path().join("secrets/master.key");
    std::fs::write(&key_path, hex(key.as_bytes())).expect("master key");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))
            .expect("key permissions");
    }
    let auth = AuthState::default().with_bootstrap(mien_server::auth::password::Bootstrap::inline(
        ADMIN_PASSWORD,
    ));
    let state = AppState::new(dir.path(), Some(pool))
        .with_security(SecuritySettings::loopback())
        .with_auth(auth)
        .with_allow_direct_egress(true);
    (dir, state)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

struct Session {
    cookie: String,
    csrf: String,
}

struct Call {
    method: Method,
    path: String,
    headers: HeaderMap,
    body: Option<Vec<u8>>,
}

impl Call {
    fn new(method: Method, path: impl Into<String>) -> Self {
        Self {
            method,
            path: path.into(),
            headers: HeaderMap::new(),
            body: None,
        }
    }

    fn cookie(self, session: &Session) -> Self {
        self.header(
            header::COOKIE,
            format!(
                "{}={}",
                mien_server::auth::session::COOKIE_NAME,
                session.cookie
            ),
        )
    }

    fn csrf(self, session: &Session) -> Self {
        self.header(
            header::HeaderName::from_static(mien_server::auth::csrf::CSRF_HEADER),
            session.csrf.as_str(),
        )
    }

    fn origin(self) -> Self {
        self.header(header::ORIGIN, "http://127.0.0.1:8787")
    }

    fn header(mut self, name: header::HeaderName, value: impl AsRef<str>) -> Self {
        self.headers
            .insert(name, value.as_ref().parse().expect("header value"));
        self
    }

    fn json(self, body: Value) -> Self {
        self.json_bytes(body.to_string().into_bytes())
    }

    fn json_bytes(mut self, body: Vec<u8>) -> Self {
        self.body = Some(body);
        self
    }

    async fn send(self, app: axum::Router) -> Response {
        let mut builder = Request::builder().method(self.method).uri(&self.path);
        for (name, value) in &self.headers {
            builder = builder.header(name, value);
        }
        let body = self.body.map_or_else(Body::empty, Body::from);
        app.oneshot(
            builder
                .header(header::CONTENT_TYPE, "application/json")
                .body(body)
                .expect("request"),
        )
        .await
        .expect("response")
    }
}

async fn body_text(response: Response) -> (StatusCode, String) {
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 16 * 1024 * 1024)
        .await
        .expect("body");
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn body_json(response: Response) -> (StatusCode, Value) {
    let (status, text) = body_text(response).await;
    if text.is_empty() {
        return (status, json!({}));
    }
    let parsed = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("non-JSON response {status}: {error}; body={text}"));
    (status, parsed)
}

fn set_cookie(response: &Response) -> String {
    response
        .headers()
        .get(header::SET_COOKIE)
        .expect("set-cookie")
        .to_str()
        .expect("ascii")
        .split(';')
        .next()
        .and_then(|pair| pair.split_once('='))
        .map(|(_, value)| value.to_owned())
        .expect("session cookie value")
}

async fn login(app: axum::Router) -> Session {
    let response = Call::new(Method::POST, "/api/v1/auth/login")
        .origin()
        .json(json!({ "password": ADMIN_PASSWORD }))
        .send(app.clone())
        .await;
    assert_eq!(response.status(), StatusCode::OK, "login must succeed");
    let cookie = set_cookie(&response);
    let (_, body) = body_json(response).await;
    let csrf = body["csrf_token"].as_str().expect("csrf token").to_owned();
    let session = Session { cookie, csrf };
    let changed = Call::new(Method::POST, "/api/v1/auth/password")
        .cookie(&session)
        .csrf(&session)
        .origin()
        .json(json!({
            "current_password": ADMIN_PASSWORD,
            "new_password": "a brand new passphrase",
        }))
        .send(app)
        .await;
    assert_eq!(changed.status(), StatusCode::NO_CONTENT, "password change");
    session
}

fn fingerprint() -> Value {
    json!({
        "schema_version": 1,
        "platform": "macos",
        "platform_version": "14.5",
        "brand": "Edge",
        "brand_version": "148.0.0.0",
        "hardware_concurrency": 12,
        "languages": ["en-US", "en"],
        "timezone": "America/Los_Angeles",
        "window": { "width": 1280, "height": 720 },
        "webrtc": { "disable_non_proxied_udp": true },
        "network": { "doh_mode": "disabled", "ipv6_mode": "block" },
    })
}

/// 一条真实可解密的代理行：凭据用固定值，便于断言不出现在导出中。
async fn seed_proxy(pool: &sqlx::SqlitePool) -> String {
    seed_proxy_with(pool, "node.example").await
}

async fn seed_proxy_with(pool: &sqlx::SqlitePool, server: &str) -> String {
    seed_proxy_options(pool, server, true).await
}

async fn seed_proxy_options(pool: &sqlx::SqlitePool, server: &str, with_options: bool) -> String {
    let repository = mien_core::storage::proxy::SqliteProxyRepository::new(
        pool.clone(),
        mien_core::secrets::MasterKey::from_bytes([7u8; 32]),
    );
    let view = repository
        .create(mien_core::ports::proxy::ProxyInput {
            name: "t28 proxy".to_owned(),
            kind: mien_core::ports::proxy::ProxyKind::Vless,
            server: server.to_owned(),
            port: 443,
            options: if with_options {
                [
                    ("sni".to_owned(), json!("node.example")),
                    ("method".to_owned(), json!("aes-256-gcm")),
                ]
                .into_iter()
                .collect()
            } else {
                std::collections::BTreeMap::new()
            },
            credentials: [
                ("id".to_owned(), SECRET_UUID.to_owned()),
                ("password".to_owned(), SECRET_PASSWORD.to_owned()),
            ]
            .into_iter()
            .collect(),
        })
        .await
        .expect("seed proxy");
    view.id.0
}

fn profile_input(name: &str, proxy_id: &str) -> Value {
    json!({
        "name": name,
        "fingerprint": fingerprint(),
        "proxy_id": proxy_id,
        "mode": "headful",
        "notes": "t28 notes",
        "automation_api": true,
        "start_url": "https://example.com/",
    })
}

async fn create_profile(app: axum::Router, session: &Session, body: Value) -> Value {
    let response = Call::new(Method::POST, "/api/v1/profiles")
        .cookie(session)
        .csrf(session)
        .origin()
        .json(body)
        .send(app)
        .await;
    let (status, text) = body_text(response).await;
    assert_eq!(status, StatusCode::CREATED, "create profile: {text}");
    serde_json::from_str(&text).expect("profile view")
}

async fn export(
    app: axum::Router,
    session: &Session,
    id: &str,
    body: Value,
) -> (StatusCode, Value) {
    body_json(
        Call::new(Method::POST, format!("/api/v1/profiles/{id}/export"))
            .cookie(session)
            .csrf(session)
            .origin()
            .json(body)
            .send(app)
            .await,
    )
    .await
}

async fn import(app: axum::Router, session: &Session, body: Value) -> (StatusCode, Value) {
    body_json(
        Call::new(Method::POST, "/api/v1/profiles/import")
            .cookie(session)
            .csrf(session)
            .origin()
            .json(body)
            .send(app)
            .await,
    )
    .await
}

async fn validate(app: axum::Router, session: &Session, body: Value) -> (StatusCode, Value) {
    body_json(
        Call::new(Method::POST, "/api/v1/profiles/import/validate")
            .cookie(session)
            .csrf(session)
            .origin()
            .json(body)
            .send(app)
            .await,
    )
    .await
}

async fn count_profiles(pool: &sqlx::SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM profiles")
        .fetch_one(pool)
        .await
        .expect("count")
}

fn profile_entries(data_root: &std::path::Path) -> usize {
    std::fs::read_dir(data_root.join("profiles"))
        .map(|entries| entries.count())
        .unwrap_or(0)
}

fn import_manifests(data_root: &std::path::Path) -> usize {
    std::fs::read_dir(data_root.join("runtimes/imports"))
        .map(|entries| entries.count())
        .unwrap_or(0)
}

/// 递归收集 JSON 里的全部对象键。
fn keys(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, item) in map {
                out.push(key.clone());
                keys(item, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                keys(item, out);
            }
        }
        _ => {}
    }
}

fn assert_no_forbidden_keys(value: &Value) {
    let mut all = Vec::new();
    keys(value, &mut all);
    for key in &all {
        let lowered = key.to_ascii_lowercase();
        for forbidden in [
            "password",
            "token",
            "uuid",
            "cookie",
            "passphrase",
            "secret",
        ] {
            assert!(
                !lowered.contains(forbidden),
                "payload contains a forbidden key `{key}`"
            );
        }
    }
}

fn assert_no_secrets(text: &str) {
    for secret in [SECRET_UUID, SECRET_PASSWORD, PASSPHRASE] {
        assert!(!text.contains(secret), "response leaked `{secret}`: {text}");
    }
}

/// 对 `profile-export.schema.json` 的独立实现：`additionalProperties: false`、
/// `required`、`pattern`、range、枚举与 `$defs/optionValue` 递归。
///
/// 单独实现而不是复用服务端代码，才能在测试里真正发现「两边一起错」。
fn schema_errors(payload: &Value) -> Vec<String> {
    let mut errors = Vec::new();
    let Some(object) = payload.as_object() else {
        return vec!["payload must be an object".to_owned()];
    };
    let allowed = [
        "schema_version",
        "exported_at",
        "groups",
        "tags",
        "profiles",
    ];
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            errors.push(format!("additional property `{key}`"));
        }
    }
    if object.get("schema_version").and_then(Value::as_u64) != Some(1) {
        errors.push("schema_version must be 1".to_owned());
    }
    match object.get("exported_at").and_then(Value::as_str) {
        Some(text) if is_exported_at(text) => {}
        _ => errors.push("exported_at must match the date-time pattern".to_owned()),
    }
    let profiles = object.get("profiles").and_then(Value::as_array);
    if profiles.is_some_and(Vec::is_empty) {
        errors.push("profiles must have at least one item".to_owned());
    }
    for group in object
        .get("groups")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(group) = group.as_object() else {
            errors.push("group must be an object".to_owned());
            continue;
        };
        for key in group.keys() {
            if !["name", "sort_order"].contains(&key.as_str()) {
                errors.push(format!("additional group property `{key}`"));
            }
        }
        match group.get("name").and_then(Value::as_str) {
            Some(name) if !name.is_empty() && name.chars().count() <= 120 => {}
            _ => errors.push("group.name must be 1-120 characters".to_owned()),
        }
        if let Some(order) = group.get("sort_order") {
            if order.as_i64().is_none_or(|value| value < 0) {
                errors.push("group.sort_order must be a non-negative integer".to_owned());
            }
        }
    }
    for tag in object
        .get("tags")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(tag) = tag.as_object() else {
            errors.push("tag must be an object".to_owned());
            continue;
        };
        for key in tag.keys() {
            if !["name", "color"].contains(&key.as_str()) {
                errors.push(format!("additional tag property `{key}`"));
            }
        }
        match tag.get("name").and_then(Value::as_str) {
            Some(name) if !name.is_empty() && name.chars().count() <= 64 => {}
            _ => errors.push("tag.name must be 1-64 characters".to_owned()),
        }
        if let Some(color) = tag.get("color") {
            let text = color.as_str().unwrap_or_default();
            if text.len() != 7
                || !text.starts_with('#')
                || !text[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                errors.push("tag.color must be #RRGGBB".to_owned());
            }
        }
    }
    for profile in profiles.into_iter().flatten() {
        schema_profile_errors(profile, &mut errors);
    }
    errors
}

fn schema_profile_errors(profile: &Value, errors: &mut Vec<String>) {
    let Some(profile) = profile.as_object() else {
        errors.push("profile must be an object".to_owned());
        return;
    };
    let allowed = [
        "source_profile_id",
        "name",
        "group",
        "tags",
        "notes",
        "kernel_version",
        "fingerprint",
        "proxy",
        "runtime",
    ];
    for key in profile.keys() {
        if !allowed.contains(&key.as_str()) {
            errors.push(format!("additional profile property `{key}`"));
        }
    }
    for required in [
        "source_profile_id",
        "name",
        "kernel_version",
        "fingerprint",
        "runtime",
    ] {
        if !profile.contains_key(required) {
            errors.push(format!("profile is missing `{required}`"));
        }
    }
    if !profile
        .get("source_profile_id")
        .and_then(Value::as_str)
        .is_some_and(is_ulid)
    {
        errors.push("source_profile_id must be 26 Crockford Base32 characters".to_owned());
    }
    match profile.get("name").and_then(Value::as_str) {
        Some(name) if !name.is_empty() && name.chars().count() <= 120 => {}
        _ => errors.push("profile.name must be 1-120 characters".to_owned()),
    }
    if profile
        .get("group")
        .is_some_and(|value| value.as_str().is_none_or(|text| text.chars().count() > 120))
    {
        errors.push("profile.group must be a string of at most 120 characters".to_owned());
    }
    if let Some(tags) = profile.get("tags") {
        let Some(tags) = tags.as_array() else {
            errors.push("profile.tags must be an array".to_owned());
            return;
        };
        let mut seen = Vec::new();
        for tag in tags {
            let Some(text) = tag.as_str() else {
                errors.push("profile.tags items must be strings".to_owned());
                continue;
            };
            if text.is_empty() || text.chars().count() > 64 {
                errors.push("profile.tags items must be 1-64 characters".to_owned());
            }
            if seen.contains(&text) {
                errors.push("profile.tags must be unique".to_owned());
            }
            seen.push(text);
        }
    }
    if profile.get("notes").is_some_and(|value| {
        value
            .as_str()
            .is_none_or(|text| text.chars().count() > 4000)
    }) {
        errors.push("profile.notes must be a string of at most 4000 characters".to_owned());
    }
    match profile.get("kernel_version").and_then(Value::as_str) {
        Some(version)
            if version.split('.').collect::<Vec<_>>().len() == 4
                && version.split('.').all(|part| {
                    !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
                }) => {}
        _ => errors.push("kernel_version must be four dotted numbers".to_owned()),
    }
    if let Some(fingerprint) = profile.get("fingerprint") {
        if mien_server::profiles::payload::fingerprint_spec(fingerprint).is_err() {
            errors.push("fingerprint must satisfy fingerprint-spec.schema.json".to_owned());
        }
        if fingerprint.get("spoof").is_some_and(Value::is_null) {
            errors.push("fingerprint.spoof must not be null".to_owned());
        }
    }
    if let Some(proxy) = profile.get("proxy") {
        let Some(proxy) = proxy.as_object() else {
            errors.push("profile.proxy must be an object".to_owned());
            return;
        };
        for key in proxy.keys() {
            if ![
                "kind",
                "server",
                "port",
                "options",
                "credential_fields",
                "credentials_removed",
            ]
            .contains(&key.as_str())
            {
                errors.push(format!("additional proxy property `{key}`"));
            }
        }
        for required in [
            "kind",
            "server",
            "port",
            "options",
            "credential_fields",
            "credentials_removed",
        ] {
            if !proxy.contains_key(required) {
                errors.push(format!("proxy is missing `{required}`"));
            }
        }
        if ![
            "http",
            "https",
            "socks5",
            "vless",
            "vmess",
            "shadowsocks",
            "trojan",
            "hysteria2",
            "wireguard",
        ]
        .contains(
            &proxy
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ) {
            errors.push("proxy.kind must be a documented kind".to_owned());
        }
        match proxy.get("server").and_then(Value::as_str) {
            Some(server)
                if !server.is_empty()
                    && server.chars().count() <= 255
                    && !server.chars().any(|character| {
                        character.is_whitespace() || matches!(character, '@' | '/' | '?' | '#')
                    }) => {}
            _ => errors.push("proxy.server must be a bare host or IP".to_owned()),
        }
        match proxy.get("port").and_then(Value::as_u64) {
            Some(port) if (1..=65535).contains(&port) => {}
            _ => errors.push("proxy.port must be 1-65535".to_owned()),
        }
        if let Some(options) = proxy.get("options") {
            let Some(options) = options.as_object() else {
                errors.push("proxy.options must be an object".to_owned());
                return;
            };
            for (name, value) in options {
                if secret_option_key(name) {
                    errors.push(format!("proxy.options `{name}` is a secret key"));
                }
                if !schema_option_value(value) {
                    errors.push(format!("proxy.options `{name}` has an invalid value"));
                }
            }
        }
        if let Some(fields) = proxy.get("credential_fields") {
            let Some(fields) = fields.as_array() else {
                errors.push("proxy.credential_fields must be an array".to_owned());
                return;
            };
            if fields.len() > 16 {
                errors.push("proxy.credential_fields must list at most 16 names".to_owned());
            }
            let mut seen = Vec::new();
            for name in fields {
                let text = name.as_str().unwrap_or_default();
                let valid = !text.is_empty()
                    && text.len() <= 64
                    && text.as_bytes()[0].is_ascii_lowercase()
                    && text.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'
                    });
                if !valid {
                    errors.push(
                        "proxy.credential_fields names must match ^[a-z][a-z0-9_]{0,63}$"
                            .to_owned(),
                    );
                }
                if seen.contains(&text) {
                    errors.push("proxy.credential_fields must be unique".to_owned());
                }
                seen.push(text);
            }
        }
        if proxy.get("credentials_removed") != Some(&Value::Bool(true)) {
            errors.push("proxy.credentials_removed must be true".to_owned());
        }
    }
    if let Some(runtime) = profile.get("runtime") {
        let Some(runtime) = runtime.as_object() else {
            errors.push("profile.runtime must be an object".to_owned());
            return;
        };
        for key in runtime.keys() {
            if !["start_url", "mode", "automation_api"].contains(&key.as_str()) {
                errors.push(format!("additional runtime property `{key}`"));
            }
        }
        if runtime
            .get("mode")
            .and_then(Value::as_str)
            .is_none_or(|mode| !["headful", "headless", "pseudo_headful"].contains(&mode))
        {
            errors.push("runtime.mode must be headful, headless or pseudo_headful".to_owned());
        }
        if runtime.get("start_url").is_some_and(|value| {
            value.as_str().is_none_or(|url| {
                !(url.starts_with("https://")
                    || url.starts_with("http://")
                    || url.starts_with("about:"))
            })
        }) {
            errors.push("runtime.start_url must start with https://, http:// or about:".to_owned());
        }
        if runtime
            .get("automation_api")
            .is_some_and(|value| !value.is_boolean())
        {
            errors.push("runtime.automation_api must be a boolean".to_owned());
        }
    }
}

/// 契约 `propertyNames` 的秘密键集合（大小写不敏感子串）。
fn secret_option_key(key: &str) -> bool {
    let lowered = key.to_ascii_lowercase();
    [
        "password",
        "passphrase",
        "pwd",
        "psk",
        "private",
        "secret",
        "token",
        "credential",
        "cookie",
        "uuid",
        "bearer",
    ]
    .iter()
    .any(|token| lowered.contains(token))
}

fn schema_option_value(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => true,
        Value::Array(items) => items.iter().all(schema_option_value),
        Value::Object(map) => map
            .iter()
            .all(|(key, item)| !secret_option_key(key) && schema_option_value(item)),
    }
}

fn is_ulid(text: &str) -> bool {
    text.len() == 26
        && text
            .bytes()
            .all(|byte| b"0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(&byte))
}

fn is_exported_at(text: &str) -> bool {
    let Some(rest) = text.strip_suffix('Z') else {
        return false;
    };
    let Some((date, time)) = rest.split_once('T') else {
        return false;
    };
    date.len() == 10
        && date.as_bytes()[4] == b'-'
        && date.as_bytes()[7] == b'-'
        && time.len() >= 8
        && time.as_bytes()[2] == b':'
        && time.as_bytes()[5] == b':'
        && time
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b':' || byte == b'.')
}

fn fingerprint_of(profile: &Value) -> &Value {
    profile.get("fingerprint").expect("fingerprint")
}

fn assert_same_fingerprint(left: &Value, right: &Value) {
    for field in [
        "schema_version",
        "seed",
        "platform",
        "platform_version",
        "brand",
        "brand_version",
        "hardware_concurrency",
        "languages",
        "timezone",
        "window",
        "webrtc",
        "network",
    ] {
        assert_eq!(left[field], right[field], "fingerprint field `{field}`");
    }
    assert_eq!(
        left.get("spoof"),
        right.get("spoof"),
        "fingerprint field `spoof`"
    );
}

/// 日志汇：只捕获本进程的 tracing 输出，用于断言口令未进日志。
#[derive(Clone, Default)]
struct LogSink(Arc<Mutex<Vec<u8>>>);

struct LogWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("log sink").extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogSink {
    type Writer = LogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        LogWriter(Arc::clone(&self.0))
    }
}

#[test]
fn plain_export_matches_the_schema_and_redacts_proxies() {
    let (dir, state) = state();
    let pool = state.db().expect("pool").clone();
    let app = router(state.clone());

    runtime().block_on(async {
        let proxy_id = seed_proxy(&pool).await;
        let session = login(app.clone()).await;
        let created =
            create_profile(app.clone(), &session, profile_input("export me", &proxy_id)).await;
        let profile_id = created["id"].as_str().expect("id").to_owned();

        let (status, body) = export(
            app.clone(),
            &session,
            &profile_id,
            json!({"format": "plain"}),
        )
        .await;
        let text = body.to_string();
        assert_eq!(status, StatusCode::OK, "export: {text}");
        assert_no_secrets(&text);

        // envelope：只有四个键，且 schema_version/exported_at 与 payload 一致。
        let envelope = body.as_object().expect("envelope");
        let mut envelope_keys: Vec<_> = envelope.keys().cloned().collect();
        envelope_keys.sort();
        assert_eq!(
            envelope_keys,
            vec!["exported_at", "format", "payload", "schema_version"]
        );
        assert_eq!(body["schema_version"], 1);
        assert_eq!(body["format"], "plain");

        let payload = &body["payload"];
        assert_eq!(
            schema_errors(payload),
            Vec::<String>::new(),
            "schema compliance"
        );
        assert_no_forbidden_keys(payload);
        assert!(payload.get("runtime").is_none(), "no top-level runtime");

        let payload_keys: Vec<_> = payload
            .as_object()
            .expect("payload")
            .keys()
            .cloned()
            .collect();
        for required in ["exported_at", "profiles", "schema_version"] {
            assert!(payload_keys.iter().any(|key| key == required), "{required}");
        }
        // `groups`/`tags` 是可选集合；空集合整键省略，不写空数组以外的形状。
        for key in &payload_keys {
            assert!(
                [
                    "exported_at",
                    "groups",
                    "profiles",
                    "schema_version",
                    "tags"
                ]
                .contains(&key.as_str()),
                "unexpected payload key `{key}`"
            );
        }
        assert_eq!(payload["schema_version"], 1);
        assert!(is_exported_at(
            payload["exported_at"].as_str().expect("exported_at")
        ));
        assert_eq!(body["exported_at"], payload["exported_at"]);

        let profile = &payload["profiles"][0];
        let profile_keys: Vec<_> = profile
            .as_object()
            .expect("profile")
            .keys()
            .cloned()
            .collect();
        for required in [
            "fingerprint",
            "kernel_version",
            "name",
            "runtime",
            "source_profile_id",
        ] {
            assert!(profile_keys.iter().any(|key| key == required), "{required}");
        }
        // `group`/`tags`/`notes`/`proxy` 是可选项：形状固定，空值整键省略。
        for key in &profile_keys {
            assert!(
                [
                    "fingerprint",
                    "group",
                    "kernel_version",
                    "name",
                    "notes",
                    "proxy",
                    "runtime",
                    "source_profile_id",
                    "tags"
                ]
                .contains(&key.as_str()),
                "unexpected profile key `{key}`"
            );
        }
        assert!(
            !profile_keys.iter().any(|key| key == "runtime_json"),
            "no window_json"
        );
        assert!(is_ulid(
            profile["source_profile_id"].as_str().expect("source id")
        ));
        assert_eq!(profile["source_profile_id"], profile_id);
        assert_eq!(profile["name"], "export me");
        assert_eq!(profile["notes"], "t28 notes");
        assert!(is_exported_at(payload["exported_at"].as_str().expect("ts")));
        assert_eq!(profile["runtime"]["mode"], "headful");
        assert_eq!(profile["runtime"]["automation_api"], true);
        assert_eq!(profile["runtime"]["start_url"], "https://example.com/");
        assert!(
            profile["runtime"].get("window").is_none(),
            "window belongs to the fingerprint"
        );
        assert_eq!(
            profile["fingerprint"]["window"],
            json!({"width": 1280, "height": 720})
        );

        // 代理：只有名称型 credential_fields 与非秘密 options。
        let proxy = &profile["proxy"];
        let mut proxy_keys: Vec<_> = proxy.as_object().expect("proxy").keys().cloned().collect();
        proxy_keys.sort();
        assert_eq!(
            proxy_keys,
            vec![
                "credential_fields",
                "credentials_removed",
                "kind",
                "options",
                "port",
                "server"
            ]
        );
        assert_eq!(proxy["credentials_removed"], true);
        assert_eq!(proxy["credential_fields"], json!(["id", "password"]));
        assert_eq!(proxy["options"]["method"], "aes-256-gcm");
        assert_eq!(proxy["server"], "node.example");
        assert!(proxy.get("credentials").is_none());

        // 无选项的代理也必须输出 `options: {}`：schema 把它列为 required。
        let bare_proxy = seed_proxy_options(&pool, "bare.example", false).await;
        let bare = create_profile(
            app.clone(),
            &session,
            profile_input("bare proxy", &bare_proxy),
        )
        .await;
        let bare_id = bare["id"].as_str().expect("id").to_owned();
        let (status, bare_export) =
            export(app.clone(), &session, &bare_id, json!({"format": "plain"})).await;
        assert_eq!(status, StatusCode::OK, "bare proxy export: {bare_export}");
        assert_eq!(
            schema_errors(&bare_export["payload"]),
            Vec::<String>::new(),
            "bare proxy schema compliance"
        );
        assert_eq!(
            bare_export["payload"]["profiles"][0]["proxy"]["options"],
            json!({})
        );

        // 明文 payload 自身可再被 validate 接受（同一 schema 的另一半）。
        let (status, preview) = validate(
            app.clone(),
            &session,
            json!({"format": "plain", "payload": payload}),
        )
        .await;
        let text = preview.to_string();
        assert_eq!(status, StatusCode::OK, "validate: {text}");
        assert_eq!(preview["items"][0]["status"], "conflict");
        assert!(
            preview["items"][0]["draft"]["seed_preserved"]
                .as_bool()
                .unwrap_or(false)
        );
        assert_no_secrets(&text);

        // 带 `spoof` 分类的档案：`Some(false)` 保留，未设置分类不输出 `null`。
        let mut spoofed_input = profile_input("spoofed export", &proxy_id);
        spoofed_input["fingerprint"]["spoof"] = json!({"canvas": false, "audio": true});
        let spoofed = create_profile(app.clone(), &session, spoofed_input).await;
        let spoofed_id = spoofed["id"].as_str().expect("id").to_owned();
        let (status, spoofed_export) = export(
            app.clone(),
            &session,
            &spoofed_id,
            json!({"format": "plain"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "spoof export: {spoofed_export}");
        assert_eq!(
            schema_errors(&spoofed_export["payload"]),
            Vec::<String>::new(),
            "spoof schema compliance"
        );
        assert_eq!(
            spoofed_export["payload"]["profiles"][0]["fingerprint"]["spoof"],
            json!({"canvas": false, "audio": true})
        );
        // 导入必须逐字段保留这些开关，而不是重新生成一个等价指纹。
        let (status, spoofed_import) = import(
            app.clone(),
            &session,
            json!({
                "format": "plain",
                "payload": spoofed_export["payload"],
                "conflict_strategy": "new_id",
            }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "spoof import: {spoofed_import}"
        );
        // 指纹语义以导出载荷为准：`Profile` 响应里的未设置分类是 `null`（T24 既有
        // 形状），导入后重新导出必须还原成同样的 boolean 集合。
        let imported_spoofed_id = spoofed_import["items"][0]["profile"]["id"]
            .as_str()
            .expect("id")
            .to_owned();
        let (status, spoofed_reexport) = export(
            app.clone(),
            &session,
            &imported_spoofed_id,
            json!({"format": "plain"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "spoof reexport: {spoofed_reexport}");
        assert_eq!(
            spoofed_reexport["payload"]["profiles"][0]["fingerprint"],
            spoofed_export["payload"]["profiles"][0]["fingerprint"],
            "the import must preserve the fingerprint field by field"
        );
        assert_eq!(
            spoofed_reexport["payload"]["profiles"][0]["fingerprint"]["seed"],
            spoofed_export["payload"]["profiles"][0]["fingerprint"]["seed"]
        );

        // 分组与标签：导入时按名称匹配/新建，再导出时必须原样出现在集合与档案上。
        let mut grouped = payload.clone();
        grouped["profiles"][0]["source_profile_id"] = json!("01ARZ3NDEKTSV4RRFFQ69G5FAX");
        grouped["profiles"][0]["name"] = json!("grouped profile");
        grouped["profiles"][0]["group"] = json!("work");
        grouped["profiles"][0]["tags"] = json!(["alpha", "beta"]);
        grouped["profiles"][0]["proxy"] = Value::Null;
        grouped["profiles"][0]
            .as_object_mut()
            .expect("profile")
            .remove("proxy");
        grouped["groups"] = json!([{ "name": "work", "sort_order": 3 }]);
        grouped["tags"] = json!([
            { "name": "alpha", "color": "#112233" },
            { "name": "beta", "color": "#445566" },
        ]);
        let (status, imported) = import(
            app.clone(),
            &session,
            json!({"format": "plain", "payload": grouped, "conflict_strategy": "new_id"}),
        )
        .await;
        let text = imported.to_string();
        assert_eq!(status, StatusCode::CREATED, "grouped import: {text}");
        let grouped_id = imported["items"][0]["profile"]["id"]
            .as_str()
            .expect("id")
            .to_owned();
        let (status, reexport) = export(
            app.clone(),
            &session,
            &grouped_id,
            json!({"format": "plain"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "reexport: {reexport}");
        assert_eq!(
            schema_errors(&reexport["payload"]),
            Vec::<String>::new(),
            "grouped export schema compliance"
        );
        let reexported = &reexport["payload"]["profiles"][0];
        assert_eq!(reexported["group"], "work");
        assert_eq!(reexported["tags"], json!(["alpha", "beta"]));
        assert!(
            reexported.get("proxy").is_none(),
            "direct profile has no proxy"
        );
        assert_eq!(
            reexport["payload"]["groups"],
            json!([{ "name": "work", "sort_order": 3 }])
        );
        assert_eq!(
            reexport["payload"]["tags"],
            json!([
                { "name": "alpha", "color": "#112233" },
                { "name": "beta", "color": "#445566" },
            ])
        );
        let _ = dir;
    });
}

#[test]
fn passphrase_presence_must_match_the_format() {
    let (_dir, state) = state();
    let pool = state.db().expect("pool").clone();
    let app = router(state);

    runtime().block_on(async {
        let proxy_id = seed_proxy(&pool).await;
        let session = login(app.clone()).await;
        let created = create_profile(
            app.clone(),
            &session,
            profile_input("passphrase rules", &proxy_id),
        )
        .await;
        let id = created["id"].as_str().expect("id").to_owned();

        let (status, body) = export(
            app.clone(),
            &session,
            &id,
            json!({"format": "plain", "passphrase": PASSPHRASE}),
        )
        .await;
        let text = body.to_string();
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "plain + passphrase: {text}"
        );
        assert_eq!(body["code"], "VALIDATION_FAILED");
        assert!(
            body["details"]["fields"].get("passphrase").is_some(),
            "{text}"
        );
        assert_no_secrets(&text);

        let (status, body) =
            export(app.clone(), &session, &id, json!({"format": "encrypted"})).await;
        let text = body.to_string();
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "encrypted without passphrase: {text}"
        );
        assert_eq!(body["code"], "VALIDATION_FAILED");
        assert!(
            body["details"]["fields"].get("passphrase").is_some(),
            "{text}"
        );

        // 请求体形状错误也不得回显口令字段的值（serde 的 invalid-type 文本会带原值）。
        let (status, body) = export(
            app.clone(),
            &session,
            &id,
            json!({"format": "plain", "passphrase": 3141592653589_i64}),
        )
        .await;
        let text = body.to_string();
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "malformed passphrase: {text}"
        );
        assert!(
            !text.contains("3141592653589"),
            "error echoed the passphrase field: {text}"
        );

        let (status, body) = validate(
            app.clone(),
            &session,
            json!({"format": "plain", "payload": {"schema_version": 1}, "passphrase": PASSPHRASE}),
        )
        .await;
        let text = body.to_string();
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "import plain + passphrase: {text}"
        );
        assert!(
            body["details"]["fields"].get("passphrase").is_some(),
            "{text}"
        );
        assert_no_secrets(&text);
    });
}

#[test]
fn encrypted_round_trip_preserves_the_fingerprint() {
    let (_dir, state) = state();
    let pool = state.db().expect("pool").clone();
    let app = router(state);

    runtime().block_on(async {
        let proxy_id = seed_proxy(&pool).await;
        let session = login(app.clone()).await;
        let created = create_profile(
            app.clone(),
            &session,
            profile_input("encrypted source", &proxy_id),
        )
        .await;
        let id = created["id"].as_str().expect("id").to_owned();

        let (status, envelope) = export(
            app.clone(),
            &session,
            &id,
            json!({"format": "encrypted", "passphrase": PASSPHRASE}),
        )
        .await;
        let text = envelope.to_string();
        assert_eq!(status, StatusCode::OK, "encrypted export: {text}");
        assert_no_secrets(&text);
        assert_eq!(envelope["format"], "encrypted");
        assert_eq!(envelope["schema_version"], 1);
        // KDF 参数、salt 与密文都在不透明字符串内部。
        let mut keys: Vec<_> = envelope
            .as_object()
            .expect("envelope")
            .keys()
            .cloned()
            .collect();
        keys.sort();
        assert_eq!(
            keys,
            vec!["exported_at", "format", "payload", "schema_version"]
        );
        let sealed = envelope["payload"].as_str().expect("opaque payload");
        assert!(sealed.starts_with("profile-encrypted-v1."), "{sealed}");
        for marker in ["salt", "argon2", "nonce", "ciphertext"] {
            assert!(
                !sealed.contains(marker),
                "container leaks `{marker}`: {sealed}"
            );
        }

        let (status, body) = export(app.clone(), &session, &id, json!({"format": "plain"})).await;
        assert_eq!(status, StatusCode::OK);
        let source_fingerprint = fingerprint_of(&body["payload"]["profiles"][0]).clone();
        // 导出规范形状：缺省 `spoof` 整键省略（schema 不接受 null）。
        assert!(source_fingerprint.get("spoof").is_none());

        let (status, imported) = import(
            app.clone(),
            &session,
            json!({
                "format": "encrypted",
                "payload": sealed,
                "passphrase": PASSPHRASE,
                "conflict_strategy": "new_id",
            }),
        )
        .await;
        let text = imported.to_string();
        assert_eq!(status, StatusCode::CREATED, "encrypted import: {text}");
        assert_no_secrets(&text);
        assert_eq!(imported["items"][0]["status"], "imported");
        assert_eq!(imported["items"][0]["errors"], json!([]));
        let restored = &imported["items"][0]["profile"];
        assert_ne!(restored["id"], id, "import must allocate a new ID");
        // 逐字段相等：再导出导入后的档案，两条载荷的指纹必须完全一致。
        let restored_id = restored["id"].as_str().expect("id").to_owned();
        let (status, reexport) = export(
            app.clone(),
            &session,
            &restored_id,
            json!({"format": "plain"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "re-export: {reexport}");
        assert_same_fingerprint(
            &source_fingerprint,
            fingerprint_of(&reexport["payload"]["profiles"][0]),
        );
        assert_eq!(
            source_fingerprint,
            *fingerprint_of(&reexport["payload"]["profiles"][0])
        );

        // 复用同一口令导出的另一次结果仍然可解（salt/nonce 每次独立）。
        let (status, second) = export(
            app.clone(),
            &session,
            &id,
            json!({"format": "encrypted", "passphrase": PASSPHRASE}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_ne!(second["payload"], envelope["payload"]);
    });
}

#[test]
fn wrong_passphrase_reports_only_the_field() {
    let (_dir, state) = state();
    let pool = state.db().expect("pool").clone();
    let app = router(state);

    runtime().block_on(async {
        let proxy_id = seed_proxy(&pool).await;
        let session = login(app.clone()).await;
        let created = create_profile(
            app.clone(),
            &session,
            profile_input("wrong phrase", &proxy_id),
        )
        .await;
        let id = created["id"].as_str().expect("id").to_owned();
        let (status, envelope) = export(
            app.clone(),
            &session,
            &id,
            json!({"format": "encrypted", "passphrase": PASSPHRASE}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let sealed = envelope["payload"].clone();

        let (status, body) = import(
            app.clone(),
            &session,
            json!({
                "format": "encrypted",
                "payload": sealed,
                "passphrase": "definitely-not-the-passphrase",
            }),
        )
        .await;
        let text = body.to_string();
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "wrong passphrase: {text}"
        );
        assert_eq!(body["code"], "VALIDATION_FAILED");
        assert!(
            body["details"]["fields"].get("passphrase").is_some(),
            "{text}"
        );
        assert!(!text.contains("definitely-not-the-passphrase"), "{text}");
        assert_no_secrets(&text);

        // 篡改一位密文 → 认证失败，仍是字段级错误。
        let mut tampered = sealed.as_str().expect("sealed").to_owned();
        let last = tampered.pop().expect("last char");
        tampered.push(if last == '0' { '1' } else { '0' });
        let (status, body) = import(
            app.clone(),
            &session,
            json!({"format": "encrypted", "payload": tampered, "passphrase": PASSPHRASE}),
        )
        .await;
        let text = body.to_string();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "tampered: {text}");
        assert!(
            body["details"]["fields"].get("passphrase").is_some(),
            "{text}"
        );
    });
}

#[test]
fn validate_is_a_zero_write_dry_run() {
    let (dir, state) = state();
    let pool = state.db().expect("pool").clone();
    let app = router(state.clone());

    runtime().block_on(async {
        let proxy_id = seed_proxy(&pool).await;
        let session = login(app.clone()).await;
        let created =
            create_profile(app.clone(), &session, profile_input("dry run", &proxy_id)).await;
        let id = created["id"].as_str().expect("id").to_owned();
        let (status, envelope) =
            export(app.clone(), &session, &id, json!({"format": "plain"})).await;
        assert_eq!(status, StatusCode::OK);
        let payload = envelope["payload"].clone();

        let before_rows = count_profiles(&pool).await;
        let before_entries = profile_entries(dir.path());
        let before_manifests = import_manifests(dir.path());

        let (status, preview) = validate(
            app.clone(),
            &session,
            json!({"format": "plain", "payload": payload, "conflict_strategy": "new_id"}),
        )
        .await;
        let text = preview.to_string();
        assert_eq!(status, StatusCode::OK, "validate: {text}");
        assert_eq!(preview["items"][0]["status"], "conflict");
        assert_no_secrets(&text);

        assert_eq!(
            count_profiles(&pool).await,
            before_rows,
            "validate wrote rows"
        );
        assert_eq!(
            profile_entries(dir.path()),
            before_entries,
            "validate created dirs"
        );
        assert_eq!(
            import_manifests(dir.path()),
            before_manifests,
            "validate wrote a manifest"
        );
    });
}

#[test]
fn abort_conflicts_before_writing_and_new_id_keeps_the_source() {
    let (dir, state) = state();
    let pool = state.db().expect("pool").clone();
    let app = router(state);

    runtime().block_on(async {
        let proxy_id = seed_proxy(&pool).await;
        let session = login(app.clone()).await;
        let created = create_profile(
            app.clone(),
            &session,
            profile_input("conflict source", &proxy_id),
        )
        .await;
        let id = created["id"].as_str().expect("id").to_owned();
        let (status, envelope) =
            export(app.clone(), &session, &id, json!({"format": "plain"})).await;
        assert_eq!(status, StatusCode::OK);
        let payload = envelope["payload"].clone();
        let source_id = payload["profiles"][0]["source_profile_id"].clone();

        // 第一次 new_id 导入成功。
        let (status, first) = import(
            app.clone(),
            &session,
            json!({"format": "plain", "payload": payload, "conflict_strategy": "new_id"}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "first import: {first}");
        let first_id = first["items"][0]["profile"]["id"].clone();

        // abort：冲突在写入前拒绝，行数与目录条目都不变。
        let rows = count_profiles(&pool).await;
        let entries = profile_entries(dir.path());
        let (status, body) = import(
            app.clone(),
            &session,
            json!({"format": "plain", "payload": payload, "conflict_strategy": "abort"}),
        )
        .await;
        let text = body.to_string();
        assert_eq!(status, StatusCode::CONFLICT, "abort import: {text}");
        assert_eq!(body["code"], "PROFILE_IMPORT_CONFLICT");
        assert_eq!(count_profiles(&pool).await, rows, "abort wrote rows");
        assert_eq!(profile_entries(dir.path()), entries, "abort created dirs");

        // 第二次 new_id：两行、不同 ID、相同 import_source_id。
        let (status, second) = import(
            app.clone(),
            &session,
            json!({"format": "plain", "payload": payload, "conflict_strategy": "new_id"}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "second import: {second}");
        let second_id = second["items"][0]["profile"]["id"].clone();
        assert_ne!(first_id, second_id, "each import allocates a new ULID");
        assert_eq!(count_profiles(&pool).await, rows + 1);

        let sources: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM profiles WHERE import_source_id = ?1")
                .bind(source_id.as_str().expect("source id"))
                .fetch_one(&pool)
                .await
                .expect("count sources");
        assert_eq!(sources, 2, "both imports keep the source identity");
        assert_eq!(
            import_manifests(dir.path()),
            0,
            "successful imports drop their manifest"
        );
    });
}

#[test]
fn a_failure_in_a_later_item_rolls_back_every_row_and_directory() {
    let (dir, state) = state();
    let pool = state.db().expect("pool").clone();
    let app = router(state);

    runtime().block_on(async {
        let proxy_id = seed_proxy(&pool).await;
        let session = login(app.clone()).await;
        let created = create_profile(
            app.clone(),
            &session,
            profile_input("batch source", &proxy_id),
        )
        .await;
        let id = created["id"].as_str().expect("id").to_owned();
        let (status, envelope) =
            export(app.clone(), &session, &id, json!({"format": "plain"})).await;
        assert_eq!(status, StatusCode::OK);
        let mut payload = envelope["payload"].clone();

        // 第一项合法；第二项在**写入阶段**失败：代理 options 里出现数组值，
        // 载荷 schema 允许（$defs/optionValue 递归），但代理仓储只接受标量。
        let mut second = payload["profiles"][0].clone();
        second["source_profile_id"] = json!("01ARZ3NDEKTSV4RRFFQ69G5FAW");
        second["name"] = json!("second item");
        second["proxy"]["options"] = json!({"alpn": ["h2", "http/1.1"]});
        payload["profiles"] = json!([payload["profiles"][0].clone(), second]);

        let before_rows = count_profiles(&pool).await;
        let before_entries = profile_entries(dir.path());
        let (status, body) = import(
            app.clone(),
            &session,
            json!({"format": "plain", "payload": payload, "conflict_strategy": "new_id"}),
        )
        .await;
        let text = body.to_string();
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "batch failure: {text}"
        );
        assert_no_secrets(&text);

        assert_eq!(
            count_profiles(&pool).await,
            before_rows,
            "first item's row survived"
        );
        assert_eq!(
            profile_entries(dir.path()),
            before_entries,
            "request directories survived"
        );
        assert_eq!(
            import_manifests(dir.path()),
            0,
            "failed import must not keep its manifest"
        );
    });
}

/// 401 先于 404：未认证请求不会因为资源不存在而变成 404。
#[test]
fn authentication_precedes_resource_lookup() {
    let (_dir, state) = state();
    let app = router(state);

    runtime().block_on(async {
        let anonymous = Call::new(
            Method::POST,
            "/api/v1/profiles/01ARZ3NDEKTSV4RRFFQ69G5FAV/export",
        )
        .origin()
        .json(json!({"format": "plain"}))
        .send(app.clone())
        .await;
        let (status, body) = body_json(anonymous).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "anonymous export: {body}");
        assert_eq!(body["code"], "AUTH_REQUIRED");

        let session = login(app.clone()).await;

        // 有 scope 但档案不存在：404 PROFILE_NOT_FOUND。
        let (status, body) = export(
            app.clone(),
            &session,
            "01ARZ3NDEKTSV4RRFFQ69G5FAV",
            json!({"format": "plain"}),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "missing profile: {body}");
        assert_eq!(body["code"], "PROFILE_NOT_FOUND");

        // 非法 ULID 与不存在同为 404，不暴露标识格式。
        let (status, body) = export(
            app.clone(),
            &session,
            "not-a-ulid",
            json!({"format": "plain"}),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "invalid id: {body}");
        assert_eq!(body["code"], "PROFILE_NOT_FOUND");
    });
}

#[test]
fn startup_reconciliation_removes_only_uncommitted_import_directories() {
    let (dir, state) = state();
    let pool = state.db().expect("pool").clone();

    runtime().block_on(async {
        // 手工构造一份「崩溃遗留」清单：一个孤儿目录 + 一个已提交档案的目录。
        let committed = mien_core::domain::profile::ProfileId::new();
        let orphan = mien_core::domain::profile::ProfileId::new();
        for id in [&committed, &orphan] {
            let profile_dir = dir.path().join("profiles").join(&id.0);
            std::fs::create_dir_all(profile_dir.join("downloads")).expect("dirs");
        }
        sqlx::query(
            "INSERT INTO profiles (id,name,fingerprint_json,fingerprint_schema,kernel_version,mode,status,created_at,updated_at)              VALUES (?1,'committed','{}',1,'148.0.7778.215','headful','active','2026-01-01T00:00:00.000Z','2026-01-01T00:00:00.000Z')",
        )
        .bind(&committed.0)
        .execute(&pool)
        .await
        .expect("insert profile");

        let manifests = dir.path().join("runtimes/imports");
        std::fs::create_dir_all(&manifests).expect("manifest dir");
        let manifest = manifests.join("crashed-import.json");
        std::fs::write(
            &manifest,
            serde_json::json!({
                "request_id": "crashed-import",
                "created_at": "2026-01-01T00:00:00.000Z",
                "directories": [
                    format!("profiles/{}", committed.0),
                    format!("profiles/{}", orphan.0),
                ],
            })
            .to_string(),
        )
        .expect("manifest");
        // 对账跳过一分钟内的清单；回拨修改时间模拟崩溃后的重启。
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::File::options()
            .write(true)
            .open(&manifest)
            .expect("open manifest")
            .set_modified(old)
            .expect("touch manifest");

        let cleaned = mien_core::storage::profile::reconcile_imports(&pool, dir.path())
            .await
            .expect("reconcile");
        assert_eq!(cleaned, 1, "one manifest was reconciled");
        assert!(
            dir.path().join("profiles").join(&committed.0).is_dir(),
            "a committed profile directory must never be removed"
        );
        assert!(
            !dir.path().join("profiles").join(&orphan.0).exists(),
            "the orphaned request directory is removed"
        );
        assert!(!manifest.exists(), "the stale manifest is removed");
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM profiles")
                .fetch_one(&pool)
                .await
                .expect("count"),
            1,
            "reconciliation must not restore rolled-back rows"
        );
    });
}

#[test]
fn passphrase_never_reaches_logs_audits_or_events() {
    static INIT: Once = Once::new();
    static SINK: std::sync::LazyLock<LogSink> = std::sync::LazyLock::new(LogSink::default);
    INIT.call_once(|| {
        let _ = tracing_subscriber::fmt()
            .with_writer(SINK.clone())
            .with_ansi(false)
            .with_target(false)
            .try_init();
    });

    let (dir, state) = state();
    let pool = state.db().expect("pool").clone();
    let app = router(state);

    runtime().block_on(async {
        let proxy_id = seed_proxy(&pool).await;
        let session = login(app.clone()).await;
        let created = create_profile(
            app.clone(),
            &session,
            profile_input("audit source", &proxy_id),
        )
        .await;
        let id = created["id"].as_str().expect("id").to_owned();
        let (status, envelope) = export(
            app.clone(),
            &session,
            &id,
            json!({"format": "encrypted", "passphrase": PASSPHRASE}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let sealed = envelope["payload"].clone();
        let (status, body) = import(
            app.clone(),
            &session,
            json!({"format": "encrypted", "payload": sealed, "passphrase": PASSPHRASE}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "import: {body}");
        // 错误口令路径同样不得回显口令。
        let _ = import(
            app.clone(),
            &session,
            json!({"format": "encrypted", "payload": sealed, "passphrase": "wrong-passphrase-123"}),
        )
        .await;

        let audit: Vec<String> = sqlx::query_scalar("SELECT metadata_json FROM audit_events")
            .fetch_all(&pool)
            .await
            .expect("audit rows");
        for row in &audit {
            assert!(
                !row.contains(PASSPHRASE),
                "audit metadata leaked the passphrase: {row}"
            );
            assert!(
                !row.contains(SECRET_UUID),
                "audit metadata leaked a credential: {row}"
            );
        }
        // `events.schema.json#/auditEvent` 的 `target` pattern 与 `metadata` 密钥限制。
        let targets: Vec<String> = sqlx::query_scalar("SELECT target FROM audit_events")
            .fetch_all(&pool)
            .await
            .expect("audit targets");
        for target in &targets {
            let (kind, id) = target.split_once(':').unwrap_or((target.as_str(), ""));
            assert!(
                !kind.is_empty()
                    && kind
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric()
                            || matches!(byte, b'_' | b'.' | b'-'))
                    && kind.as_bytes()[0].is_ascii_alphabetic(),
                "audit target `{target}` must be kind or kind:id"
            );
            assert!(
                !target.contains(PASSPHRASE),
                "audit target leaked the passphrase"
            );
            let _ = id;
        }
        let metadata: Vec<String> = sqlx::query_scalar("SELECT metadata_json FROM audit_events")
            .fetch_all(&pool)
            .await
            .expect("audit metadata");
        for row in &metadata {
            let parsed: Value = serde_json::from_str(row).expect("metadata json");
            for (key, value) in parsed.as_object().expect("metadata object") {
                for forbidden in [
                    "password",
                    "passphrase",
                    "token",
                    "uuid",
                    "cookie",
                    "secret",
                ] {
                    assert!(
                        !key.to_ascii_lowercase().contains(forbidden),
                        "metadata key `{key}`"
                    );
                }
                assert!(
                    !value.is_object()
                        && !value
                            .as_array()
                            .is_some_and(|items| items.iter().any(Value::is_object)),
                    "metadata values must be scalars or arrays of scalars: {row}"
                );
            }
        }

        let event_types: Vec<String> = sqlx::query_scalar("SELECT event_type FROM audit_events")
            .fetch_all(&pool)
            .await
            .expect("audit types");
        assert!(
            event_types.iter().any(|kind| kind == "profile.import"),
            "import audit row: {event_types:?}"
        );
        assert!(
            event_types
                .iter()
                .any(|kind| kind == "profile.export.encrypted"),
            "export audit row: {event_types:?}"
        );

        let events: Vec<String> = sqlx::query_scalar("SELECT payload_json FROM event_log")
            .fetch_all(&pool)
            .await
            .expect("event rows");
        for row in &events {
            assert!(
                !row.contains(PASSPHRASE),
                "event payload leaked the passphrase: {row}"
            );
        }

        let log = String::from_utf8(SINK.0.lock().expect("sink").clone()).expect("utf8 log");
        assert!(!log.is_empty(), "tracing output must be captured");
        assert!(!log.contains(PASSPHRASE), "logs leaked the passphrase");
        assert!(
            !log.contains("wrong-passphrase-123"),
            "logs leaked the wrong passphrase"
        );
        let _ = dir;
    });
}

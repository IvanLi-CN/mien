//! 明文导出载荷：`docs/contracts/profile-export.schema.json` 的严格映射。
//!
//! 导出与导入共用这一组结构：导出先构造再反序列化自检，导入直接反序列化并按
//! schema 的 pattern/range 校验。`additionalProperties: false` 由
//! `deny_unknown_fields` 保证；可省略字段用 `Option` + `present` 反序列化器，
//! 显式 `null` 会被拒绝（schema 中这些字段不是 nullable）。
//!
//! 口令与凭据永远不进入本模块：载荷只描述档案、分组、标签、指纹、脱敏代理与
//! 运行时偏好。

use std::collections::{BTreeMap, BTreeSet};

use mien_core::domain::profile::{ProfileId, ProfileMode};
use mien_core::fingerprint::json::parse_spec_json;
use mien_core::fingerprint::spec::FingerprintSpec;
use mien_core::ports::proxy::ProxyKind;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// 载荷 schema 版本。
pub const PAYLOAD_SCHEMA_VERSION: u32 = 1;
/// 档案名上限。
const MAX_NAME: usize = 120;
/// 分组名上限。
const MAX_GROUP: usize = 120;
/// 标签名上限。
const MAX_TAG: usize = 64;
/// 备注上限。
const MAX_NOTES: usize = 4000;
/// 单个档案的标签上限（与 `validate_tags` 一致）。
const MAX_TAGS: usize = 32;
/// 代理 `server` 上限。
const MAX_SERVER: usize = 255;
/// 凭据字段名上限。
const MAX_CREDENTIAL_FIELDS: usize = 16;

/// 顶层载荷。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportPayload {
    pub schema_version: u32,
    pub exported_at: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<PayloadGroup>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<PayloadTag>,
    pub profiles: Vec<PayloadProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PayloadGroup {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_order: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PayloadTag {
    pub name: String,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub color: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PayloadProfile {
    pub source_profile_id: String,
    pub name: String,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub group: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub notes: Option<String>,
    pub kernel_version: String,
    pub fingerprint: Value,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub proxy: Option<PayloadProxy>,
    pub runtime: PayloadRuntime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PayloadProxy {
    pub kind: ProxyKind,
    pub server: String,
    pub port: u16,
    /// schema 把 `options` 列为 required：空对象也必须输出，不能整键省略。
    #[serde(default)]
    pub options: BTreeMap<String, Value>,
    pub credential_fields: Vec<String>,
    pub credentials_removed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PayloadRuntime {
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub start_url: Option<String>,
    pub mode: ProfileMode,
    #[serde(default, skip_serializing_if = "is_false")]
    pub automation_api: bool,
}

/// 显式 `null` 不是这些字段的合法取值；缺省由 `#[serde(default)]` 处理。
fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// 载荷校验错误：字段路径 + 原因。路径只含 schema 字段名与索引。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayloadError {
    pub field: String,
    pub reason: String,
}

impl PayloadError {
    fn new(field: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            reason: reason.into(),
        }
    }
}

/// 反序列化并按 schema 校验；返回全部错误，调用方决定曝光方式。
///
/// # Errors
/// 未知字段、缺失必需字段、`null` 或 pattern/range 不符时给出字段级错误。
pub fn parse_payload(value: &Value) -> Result<ExportPayload, Vec<PayloadError>> {
    let payload: ExportPayload = match serde_json::from_value(value.clone()) {
        Ok(payload) => payload,
        Err(error) => {
            return Err(vec![PayloadError::new(String::new(), describe(error))]);
        }
    };
    let errors = validate_payload(&payload);
    if errors.is_empty() {
        Ok(payload)
    } else {
        Err(errors)
    }
}

fn describe(error: serde_json::Error) -> String {
    // serde 的位置与分类足够定位，且不回显字段值。
    let text = error.to_string();
    let classification = match error.classify() {
        serde_json::error::Category::Data => "is not valid for this schema",
        serde_json::error::Category::Syntax => "is not valid JSON",
        serde_json::error::Category::Eof => "is truncated",
        serde_json::error::Category::Io => "cannot be read",
    };
    if text.contains("unknown field") {
        // 只保留字段名的结论，不带 serde 的候选列表。
        "contains an unknown field".to_owned()
    } else if text.contains("missing field") {
        "is missing a required field".to_owned()
    } else {
        classification.to_owned()
    }
}

/// 逐项校验 schema 中 serde 无法表达的部分（pattern、range、枚举与唯一性）。
fn validate_payload(payload: &ExportPayload) -> Vec<PayloadError> {
    let mut errors = Vec::new();
    if payload.schema_version != PAYLOAD_SCHEMA_VERSION {
        errors.push(PayloadError::new("schema_version", "must be 1"));
    }
    if !is_exported_at(&payload.exported_at) {
        errors.push(PayloadError::new(
            "exported_at",
            "must look like 2026-01-15T12:00:00Z",
        ));
    }
    if payload.profiles.is_empty() {
        errors.push(PayloadError::new(
            "profiles",
            "must contain at least one profile",
        ));
    }
    for (index, group) in payload.groups.iter().enumerate() {
        let field = format!("groups[{index}]");
        if !valid_text(&group.name, 1, MAX_GROUP) {
            errors.push(PayloadError::new(
                format!("{field}.name"),
                "must be 1-120 characters",
            ));
        }
        if group.sort_order.is_some_and(|order| order < 0) {
            errors.push(PayloadError::new(
                format!("{field}.sort_order"),
                "must be zero or greater",
            ));
        }
    }
    let mut group_names = BTreeSet::new();
    for group in &payload.groups {
        if !group_names.insert(group.name.as_str()) {
            errors.push(PayloadError::new("groups", "contains duplicate names"));
            break;
        }
    }
    let mut tag_names = BTreeSet::new();
    for (index, tag) in payload.tags.iter().enumerate() {
        let field = format!("tags[{index}]");
        if !valid_text(&tag.name, 1, MAX_TAG) {
            errors.push(PayloadError::new(
                format!("{field}.name"),
                "must be 1-64 characters",
            ));
        }
        if let Some(color) = &tag.color
            && !is_color(color)
        {
            errors.push(PayloadError::new(
                format!("{field}.color"),
                "must be #RRGGBB",
            ));
        }
        if !tag_names.insert(tag.name.as_str()) {
            errors.push(PayloadError::new("tags", "contains duplicate names"));
            break;
        }
    }
    for (index, profile) in payload.profiles.iter().enumerate() {
        let field = format!("profiles[{index}]");
        validate_profile(profile, &field, &mut errors);
    }
    errors
}

fn validate_profile(profile: &PayloadProfile, field: &str, errors: &mut Vec<PayloadError>) {
    if !ProfileId::from_string(profile.source_profile_id.clone()).is_valid() {
        errors.push(PayloadError::new(
            format!("{field}.source_profile_id"),
            "must be a 26-character Crockford Base32 identifier",
        ));
    }
    if !valid_text(&profile.name, 1, MAX_NAME) {
        errors.push(PayloadError::new(
            format!("{field}.name"),
            "must be 1-120 characters",
        ));
    }
    if let Some(group) = &profile.group
        && group.chars().count() > MAX_GROUP
    {
        errors.push(PayloadError::new(
            format!("{field}.group"),
            "must be at most 120 characters",
        ));
    }
    if profile.tags.len() > MAX_TAGS {
        errors.push(PayloadError::new(
            format!("{field}.tags"),
            "must contain at most 32 tags",
        ));
    }
    let mut seen = BTreeSet::new();
    for tag in &profile.tags {
        if tag.chars().count() > MAX_TAG || tag.is_empty() {
            errors.push(PayloadError::new(
                format!("{field}.tags"),
                "each tag must be 1-64 characters",
            ));
            break;
        }
        if !seen.insert(tag.as_str()) {
            errors.push(PayloadError::new(
                format!("{field}.tags"),
                "must not contain duplicates",
            ));
            break;
        }
    }
    if let Some(notes) = &profile.notes
        && notes.chars().count() > MAX_NOTES
    {
        errors.push(PayloadError::new(
            format!("{field}.notes"),
            "must be at most 4000 characters",
        ));
    }
    if !is_kernel_version(&profile.kernel_version) {
        errors.push(PayloadError::new(
            format!("{field}.kernel_version"),
            "must be a four-part dotted version such as 148.0.7778.215",
        ));
    }
    if let Err(error) = fingerprint_spec(&profile.fingerprint) {
        errors.push(PayloadError::new(format!("{field}.fingerprint"), error));
    }
    if let Some(proxy) = &profile.proxy {
        validate_proxy(proxy, &format!("{field}.proxy"), errors);
    }
    validate_runtime(&profile.runtime, &format!("{field}.runtime"), errors);
}

fn validate_proxy(proxy: &PayloadProxy, field: &str, errors: &mut Vec<PayloadError>) {
    if !valid_text(&proxy.server, 1, MAX_SERVER) || !is_proxy_server(&proxy.server) {
        errors.push(PayloadError::new(
            format!("{field}.server"),
            "must be a host or IP without userinfo, path, query or spaces",
        ));
    }
    if proxy.port == 0 {
        errors.push(PayloadError::new(
            format!("{field}.port"),
            "must be between 1 and 65535",
        ));
    }
    for (name, value) in &proxy.options {
        if is_secret_key(name) {
            errors.push(PayloadError::new(
                format!("{field}.options.{name}"),
                "must not be a secret-bearing option",
            ));
        }
        if !valid_option_value(value) {
            errors.push(PayloadError::new(
                format!("{field}.options.{name}"),
                "must be a string, number, boolean, null, array or object",
            ));
        }
    }
    if proxy.credential_fields.len() > MAX_CREDENTIAL_FIELDS {
        errors.push(PayloadError::new(
            format!("{field}.credential_fields"),
            "must list at most 16 fields",
        ));
    }
    let mut seen = BTreeSet::new();
    for name in &proxy.credential_fields {
        if !is_credential_field_name(name) {
            errors.push(PayloadError::new(
                format!("{field}.credential_fields"),
                "each name must match ^[a-z][a-z0-9_]{0,63}$",
            ));
            break;
        }
        if !seen.insert(name.as_str()) {
            errors.push(PayloadError::new(
                format!("{field}.credential_fields"),
                "must not contain duplicates",
            ));
            break;
        }
    }
    if !proxy.credentials_removed {
        errors.push(PayloadError::new(
            format!("{field}.credentials_removed"),
            "must be true",
        ));
    }
}

fn validate_runtime(runtime: &PayloadRuntime, field: &str, errors: &mut Vec<PayloadError>) {
    if let Some(url) = &runtime.start_url
        && !(url.starts_with("https://") || url.starts_with("http://") || url.starts_with("about:"))
    {
        errors.push(PayloadError::new(
            format!("{field}.start_url"),
            "must start with https://, http:// or about:",
        ));
    }
}

/// 指纹必须是 schema 合法的完整规格；返回解析后的领域值以便逐字段保留。
///
/// # Errors
/// 缺字段、未知字段、类型或枚举/范围不符。
pub fn fingerprint_spec(value: &Value) -> Result<FingerprintSpec, &'static str> {
    let object = value.as_object().ok_or("must be a JSON object")?;
    // schema 里 `spoof` 是对象，不是 nullable；`Option` 只表达「缺省」。
    if object.get("spoof").is_some_and(|spoof| !spoof.is_object()) {
        return Err("spoof must be an object when present");
    }
    let text = value.to_string();
    let spec = parse_spec_json(&text).map_err(|_| "is not a valid fingerprint spec")?;
    if spec.schema_version != 1 {
        return Err("schema_version must be 1");
    }
    if spec.seed == 0 {
        return Err("seed must be non-zero");
    }
    if !matches!(spec.brand.as_str(), "Chrome" | "Edge" | "Opera" | "Vivaldi") {
        return Err("brand must be Chrome, Edge, Opera or Vivaldi");
    }
    if !is_version(&spec.platform_version) || !is_version(&spec.brand_version) {
        return Err("platform_version and brand_version must be dotted versions");
    }
    if !(1..=256).contains(&spec.hardware_concurrency) {
        return Err("hardware_concurrency must be between 1 and 256");
    }
    if spec.languages.is_empty() || !spec.languages.iter().all(|value| is_language(value)) {
        return Err("languages must contain language tags such as en-US");
    }
    if spec.languages.iter().collect::<BTreeSet<_>>().len() != spec.languages.len() {
        return Err("languages must not contain duplicates");
    }
    if !is_timezone(&spec.timezone) {
        return Err("timezone must contain only letters, digits, _ + - /");
    }
    if !(320..=7680).contains(&spec.window.width) || !(240..=4320).contains(&spec.window.height) {
        return Err("window dimensions are outside the allowed range");
    }
    // schema 的分类开关是 boolean：`null` 既不是「缺省」也不合法。
    if let Some(spoof) = object.get("spoof").and_then(Value::as_object) {
        for (category, value) in spoof {
            if !matches!(
                category.as_str(),
                "font" | "audio" | "canvas" | "clientrects" | "gpu"
            ) || !value.is_boolean()
            {
                return Err(
                    "spoof categories must be booleans named font/audio/canvas/clientrects/gpu",
                );
            }
        }
    }
    if spec.network.doh_mode != "disabled" || spec.network.ipv6_mode != "block" {
        return Err("network must disable DoH and block IPv6");
    }
    Ok(spec)
}

/// 序列化指纹为 schema 形状；`spoof` 缺省时整键省略（schema 不接受 `null`）。
///
/// # Errors
/// 序列化失败或自检不通过。
pub fn fingerprint_value(spec: &FingerprintSpec) -> Result<Value, &'static str> {
    let mut value = serde_json::to_value(spec).map_err(|_| "cannot be serialized")?;
    if let Some(object) = value.as_object_mut() {
        // `Spoof` 的字段是 `Option<bool>`：未设置的分类序列化成 `null`，而 schema
        // 只接受 boolean。未设置等于「缺省（启用）」，因此逐键删除；全空则整键省略。
        // 整个 `spoof` 缺失时 `Option<Spoof>` 序列化为 `null`，同样整键省略。
        match object.get_mut("spoof") {
            Some(Value::Object(categories)) => {
                categories.retain(|_, item| !item.is_null());
                if categories.is_empty() {
                    object.remove("spoof");
                }
            }
            Some(Value::Null) => {
                object.remove("spoof");
            }
            _ => {}
        }
    }
    fingerprint_spec(&value).map(|_| value)
}

// ---------------------------------------------------------------------------
// schema 断言
// ---------------------------------------------------------------------------

/// `^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(\.[0-9]+)?Z$` 加范围检查。
pub fn is_exported_at(text: &str) -> bool {
    let Some(rest) = text.strip_suffix('Z') else {
        return false;
    };
    let (date, time) = match rest.split_once('T') {
        Some(parts) => parts,
        None => return false,
    };
    let mut date_parts = date.split('-');
    let (Some(year), Some(month), Some(day), None) = (
        date_parts.next(),
        date_parts.next(),
        date_parts.next(),
        date_parts.next(),
    ) else {
        return false;
    };
    if year.len() != 4 || month.len() != 2 || day.len() != 2 {
        return false;
    }
    let (Some(year), Some(month), Some(day)) = (
        digits(year).and_then(|value| u32::try_from(value).ok()),
        digits(month).and_then(|value| u32::try_from(value).ok()),
        digits(day).and_then(|value| u32::try_from(value).ok()),
    ) else {
        return false;
    };
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return false;
    }
    let _ = year;
    let (clock, fraction) = match time.split_once('.') {
        Some((clock, fraction)) => (clock, Some(fraction)),
        None => (time, None),
    };
    if fraction.is_some_and(|fraction| fraction.is_empty() || digits(fraction).is_none()) {
        return false;
    }
    let mut clock_parts = clock.split(':');
    let (Some(hour), Some(minute), Some(second), None) = (
        clock_parts.next(),
        clock_parts.next(),
        clock_parts.next(),
        clock_parts.next(),
    ) else {
        return false;
    };
    if hour.len() != 2 || minute.len() != 2 || second.len() != 2 {
        return false;
    }
    matches!(
        (
            digits(hour).and_then(|value| u32::try_from(value).ok()),
            digits(minute).and_then(|value| u32::try_from(value).ok()),
            digits(second).and_then(|value| u32::try_from(value).ok()),
        ),
        (Some(hour), Some(minute), Some(second)) if hour < 24 && minute < 60 && second < 61
    )
}

fn digits(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn valid_text(text: &str, min: usize, max: usize) -> bool {
    let length = text.chars().count();
    (min..=max).contains(&length)
}

fn is_color(text: &str) -> bool {
    let mut bytes = text.bytes();
    if bytes.next() != Some(b'#') || text.len() != 7 {
        return false;
    }
    text[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_kernel_version(text: &str) -> bool {
    let parts: Vec<_> = text.split('.').collect();
    parts.len() == 4
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn is_version(text: &str) -> bool {
    let parts: Vec<_> = text.split('.').collect();
    !parts.is_empty()
        && parts.len() <= 4
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn is_language(text: &str) -> bool {
    let mut parts = text.split('-');
    let Some(language) = parts.next() else {
        return false;
    };
    (2..=3).contains(&language.len())
        && language.bytes().all(|byte| byte.is_ascii_alphabetic())
        && parts.all(|part| {
            (2..=8).contains(&part.len()) && part.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
}

fn is_timezone(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'+' | b'-' | b'/'))
        && !text.contains("//")
}

/// `^[^@/?#\s]+$`：userinfo、路径、查询与片段都不能藏进 server。
fn is_proxy_server(text: &str) -> bool {
    !text.is_empty()
        && !text.chars().any(|character| {
            character.is_whitespace() || matches!(character, '@' | '/' | '?' | '#')
        })
}

/// 与契约 `propertyNames` 等价的秘密键检测（大小写不敏感），逐层递归。
fn is_secret_key(key: &str) -> bool {
    let lowered = key.to_ascii_lowercase();
    const TOKENS: [&str; 11] = [
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
    ];
    if TOKENS.iter().any(|token| lowered.contains(token)) {
        return true;
    }
    // `api_key`、`api-key`、`apikey`、`preshared_key`、`auth`、`auth_str`。
    let compact = lowered.replace(['_', '-'], "");
    compact == "apikey" || compact == "presharedkey" || compact == "auth" || compact == "authstr"
}

fn valid_option_value(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => true,
        Value::Array(items) => items.iter().all(valid_option_value),
        Value::Object(map) => map
            .iter()
            .all(|(key, item)| !is_secret_key(key) && valid_option_value(item)),
    }
}

fn is_credential_field_name(text: &str) -> bool {
    let mut bytes = text.bytes();
    matches!(bytes.next(), Some(byte) if byte.is_ascii_lowercase())
        && text.len() <= 64
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

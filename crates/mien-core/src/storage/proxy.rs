//! SQLite 代理配置仓储：AES-256-GCM 字段级加密与脱敏读模型。
//!
//! 物理布局（`docs/DATABASE-SCHEMA.md` §2 `proxies` 没有独立 `options` 列）：
//! `config_ciphertext` 保存 JSON 信封 `{"v":1,"fields":{"options":"<hex>","<credential>":"<hex>"}}`，
//! 每个值都是 `nonce(12) || ciphertext || tag(16)`，AAD 绑定 `proxy_id` 与信封键名。
//! 因此非秘密 `options` 与凭据一样只以密文落库（`docs/ARCHITECTURE.md` §6「协议选项与凭据
//! 加密存储」），而 `credential_fields_json` 只记已设置的凭据字段名。

use std::collections::BTreeMap;

use serde_json::{Map, Value};
use sqlx::{Row, SqlitePool};
use zeroize::Zeroize;

use crate::error::{CoreError, ErrorCode};
use crate::ports::proxy::{
    ProxyId, ProxyInput, ProxyKind, ProxyPage, ProxyPatch, ProxyRepository, ProxySecret, ProxyView,
};
use crate::secrets::{FieldAad, MasterKey, decrypt_field, encrypt_field};

/// `options` 在加密信封中的键名，同时是它的 AAD 字段名。
const OPTIONS_FIELD: &str = "options";
/// 信封版本；解密时校验。
const ENVELOPE_VERSION: i64 = 1;
/// 单个标量字符串字段的上限，与 OpenAPI 一致。
const MAX_FIELD_LEN: usize = 4096;
const MAX_NAME_LEN: usize = 120;
const MAX_SERVER_LEN: usize = 255;

/// 非秘密选项键中一旦出现即视为秘密的子串（大小写不敏感），与 OpenAPI `propertyNames` 对齐。
const SECRET_KEY_TOKENS: [&str; 11] = [
    "password",
    "passphrase",
    "pwd",
    "psk",
    "private",
    "secret",
    "token",
    "uuid",
    "cookie",
    "bearer",
    "key",
];

/// SQLite 代理配置仓储。
pub struct SqliteProxyRepository {
    pool: SqlitePool,
    key: MasterKey,
}

impl SqliteProxyRepository {
    pub fn new(pool: SqlitePool, key: MasterKey) -> Self {
        Self { pool, key }
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    async fn fetch(&self, id: &ProxyId) -> Result<StoredProxy, CoreError> {
        let row = sqlx::query(
            "SELECT id,name,kind,server,port,config_ciphertext,credential_fields_json,last_check_json,created_at,updated_at FROM proxies WHERE id=?1",
        )
        .bind(id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?
        .ok_or_else(proxy_not_found)?;
        self.decode_row(&row)
    }

    fn decode_row(&self, row: &sqlx::sqlite::SqliteRow) -> Result<StoredProxy, CoreError> {
        let id = ProxyId(row.get::<String, _>("id"));
        let kind = ProxyKind::parse(&row.get::<String, _>("kind"))?;
        let ciphertext: Vec<u8> = row.get("config_ciphertext");
        let mut fields = open_envelope(&ciphertext)?;
        let sealed_options = fields
            .remove(OPTIONS_FIELD)
            .ok_or(CoreError::StorageIntegrityFailed)?;
        let options = decrypt_options(&self.key, id.as_str(), &sealed_options)?;
        let mut credentials = BTreeMap::new();
        for (name, bytes) in fields {
            let plaintext = decrypt_field(&self.key, aad(id.as_str(), &name), &bytes)?;
            let value = match String::from_utf8(plaintext) {
                Ok(value) => value,
                Err(error) => {
                    let mut bytes = error.into_bytes();
                    bytes.zeroize();
                    return Err(CoreError::StorageIntegrityFailed);
                }
            };
            credentials.insert(name, value);
        }
        let recorded = parse_credential_fields(&row.get::<String, _>("credential_fields_json"))?;
        if recorded != credentials.keys().cloned().collect::<Vec<_>>() {
            return Err(CoreError::StorageIntegrityFailed);
        }
        let last_check = row
            .get::<Option<String>, _>("last_check_json")
            .map(|text| serde_json::from_str::<Value>(&text))
            .transpose()
            .map_err(|_| CoreError::StorageIntegrityFailed)?;
        Ok(StoredProxy {
            view: ProxyView {
                id,
                name: row.get("name"),
                kind,
                server: row.get("server"),
                port: to_port(row.get::<i64, _>("port"))?,
                options,
                credential_set: !credentials.is_empty(),
                credential_fields: credentials.keys().cloned().collect(),
                last_check,
                created_at: row.get("created_at"),
                updated_at: row.get("updated_at"),
            },
            credentials,
        })
    }

    async fn write(
        &self,
        id: &ProxyId,
        state: &Validated,
        created_at: Option<&str>,
    ) -> Result<(), CoreError> {
        let ciphertext = seal(&self.key, id.as_str(), state)?;
        let fields =
            serde_json::to_string(&state.credential_fields()).map_err(|_| CoreError::Storage)?;
        let now = crate::storage::now_utc();
        match created_at {
            Some(created_at) => {
                sqlx::query(
                    "INSERT INTO proxies (id,name,kind,server,port,config_ciphertext,credential_fields_json,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?8)",
                )
                .bind(id.as_str())
                .bind(&state.name)
                .bind(state.kind.as_str())
                .bind(&state.server)
                .bind(i64::from(state.port))
                .bind(&ciphertext)
                .bind(fields)
                .bind(created_at)
                .execute(&self.pool)
                .await
                .map_err(map_sqlx_error)?;
            }
            None => {
                let rows = sqlx::query(
                    "UPDATE proxies SET name=?1,kind=?2,server=?3,port=?4,config_ciphertext=?5,credential_fields_json=?6,updated_at=?7 WHERE id=?8",
                )
                .bind(&state.name)
                .bind(state.kind.as_str())
                .bind(&state.server)
                .bind(i64::from(state.port))
                .bind(&ciphertext)
                .bind(fields)
                .bind(now)
                .bind(id.as_str())
                .execute(&self.pool)
                .await
                .map_err(map_sqlx_error)?;
                if rows.rows_affected() == 0 {
                    return Err(proxy_not_found());
                }
            }
        }
        Ok(())
    }
}

impl ProxyRepository for SqliteProxyRepository {
    async fn create(&self, input: ProxyInput) -> Result<ProxyView, CoreError> {
        let state = Validated::from_input(&input)?;
        let id = ProxyId::new();
        let created_at = crate::storage::now_utc();
        self.write(&id, &state, Some(&created_at)).await?;
        self.fetch(&id).await.map(|stored| stored.view)
    }

    async fn update(&self, id: &ProxyId, patch: ProxyPatch) -> Result<ProxyView, CoreError> {
        let current = self.fetch(id).await?;
        let state = Validated::from_patch(&current, patch)?;
        self.write(id, &state, None).await?;
        self.fetch(id).await.map(|stored| stored.view)
    }

    async fn get(&self, id: &ProxyId) -> Result<ProxyView, CoreError> {
        self.fetch(id).await.map(|stored| stored.view)
    }

    async fn list(
        &self,
        limit: Option<u32>,
        cursor: Option<String>,
    ) -> Result<ProxyPage, CoreError> {
        let limit = i64::from(limit.unwrap_or(50).clamp(1, 100));
        let rows = sqlx::query(
            "SELECT id,name,kind,server,port,config_ciphertext,credential_fields_json,last_check_json,created_at,updated_at FROM proxies WHERE (?1 IS NULL OR id > ?1) ORDER BY id LIMIT ?2",
        )
        .bind(cursor)
        .bind(limit + 1)
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)?;
        let mut items = rows
            .iter()
            .map(|row| self.decode_row(row).map(|stored| stored.view))
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = if items.len() > limit as usize {
            items.truncate(limit as usize);
            items.last().map(|view| view.id.0.clone())
        } else {
            None
        };
        Ok(ProxyPage { items, next_cursor })
    }

    async fn delete(&self, id: &ProxyId) -> Result<(), CoreError> {
        let rows = sqlx::query("DELETE FROM proxies WHERE id=?1")
            .bind(id.as_str())
            .execute(&self.pool)
            .await
            .map_err(map_delete_error)?;
        if rows.rows_affected() == 0 {
            return Err(proxy_not_found());
        }
        Ok(())
    }

    async fn load_secret(&self, id: &ProxyId) -> Result<ProxySecret, CoreError> {
        let stored = self.fetch(id).await?;
        Ok(ProxySecret {
            options: stored.view.options,
            credentials: stored.credentials,
        })
    }
}

/// 一行解密后的状态；`credentials` 只在仓储内部流转，不出现在 `view`。
struct StoredProxy {
    view: ProxyView,
    credentials: BTreeMap<String, String>,
}

/// 已校验、待加密写入的状态。
struct Validated {
    name: String,
    kind: ProxyKind,
    server: String,
    port: u16,
    options: BTreeMap<String, Value>,
    credentials: BTreeMap<String, String>,
}

impl Validated {
    fn from_input(input: &ProxyInput) -> Result<Self, CoreError> {
        let state = Self {
            name: input.name.trim().to_owned(),
            kind: input.kind,
            server: input.server.trim().to_owned(),
            port: input.port,
            options: input.options.clone(),
            credentials: input.credentials.clone(),
        };
        state.validate()?;
        Ok(state)
    }

    fn from_patch(current: &StoredProxy, patch: ProxyPatch) -> Result<Self, CoreError> {
        let state = Self {
            name: patch
                .name
                .map_or_else(|| current.view.name.clone(), |v| v.trim().to_owned()),
            kind: patch.kind.unwrap_or(current.view.kind),
            server: patch
                .server
                .map_or_else(|| current.view.server.clone(), |v| v.trim().to_owned()),
            port: patch.port.unwrap_or(current.view.port),
            options: patch
                .options
                .unwrap_or_else(|| current.view.options.clone()),
            credentials: patch
                .credentials
                .unwrap_or_else(|| current.credentials.clone()),
        };
        state.validate()?;
        Ok(state)
    }

    fn validate(&self) -> Result<(), CoreError> {
        if self.name.is_empty() || self.name.chars().count() > MAX_NAME_LEN {
            return Err(config_invalid("name must be 1-120 characters"));
        }
        if self.server.is_empty() || self.server.chars().count() > MAX_SERVER_LEN {
            return Err(config_invalid("server must be 1-255 characters"));
        }
        if self.port == 0 {
            return Err(config_invalid("port must be 1-65535"));
        }
        for (key, value) in &self.options {
            if key.is_empty() {
                return Err(config_invalid("option keys must not be empty"));
            }
            let lowered = key.to_lowercase();
            if SECRET_KEY_TOKENS
                .iter()
                .any(|token| lowered.contains(token))
            {
                return Err(config_invalid(
                    "options must not contain secret-looking keys",
                ));
            }
            let scalar_ok = match value {
                Value::String(text) => text.chars().count() <= MAX_FIELD_LEN,
                Value::Number(_) | Value::Bool(_) | Value::Null => true,
                Value::Array(_) | Value::Object(_) => false,
            };
            if !scalar_ok {
                return Err(config_invalid(
                    "option values must be scalar strings, numbers, booleans or null",
                ));
            }
        }
        for (name, value) in &self.credentials {
            if name.is_empty() || name == OPTIONS_FIELD {
                return Err(config_invalid(
                    "credential field names are reserved or empty",
                ));
            }
            if value.chars().count() > MAX_FIELD_LEN {
                return Err(config_invalid(
                    "credential values must be at most 4096 characters",
                ));
            }
        }
        Ok(())
    }

    fn credential_fields(&self) -> Vec<String> {
        self.credentials.keys().cloned().collect()
    }
}

/// 把已校验状态封进加密信封。
fn seal(key: &MasterKey, proxy_id: &str, state: &Validated) -> Result<Vec<u8>, CoreError> {
    let options = serde_json::to_vec(&state.options).map_err(|_| CoreError::Storage)?;
    let mut fields = Map::new();
    fields.insert(
        OPTIONS_FIELD.to_owned(),
        Value::String(hex(&encrypt_field(
            key,
            aad(proxy_id, OPTIONS_FIELD),
            &options,
        )?)),
    );
    for (name, value) in &state.credentials {
        let sealed = encrypt_field(key, aad(proxy_id, name), value.as_bytes())?;
        fields.insert(name.clone(), Value::String(hex(&sealed)));
    }
    let envelope = Value::Object(Map::from_iter([
        ("v".to_owned(), Value::from(ENVELOPE_VERSION)),
        ("fields".to_owned(), Value::Object(fields)),
    ]));
    serde_json::to_vec(&envelope).map_err(|_| CoreError::Storage)
}

/// 解出信封里每个字段的密文；版本不符或结构非法按完整性失败处理。
fn open_envelope(ciphertext: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, CoreError> {
    let envelope: Value =
        serde_json::from_slice(ciphertext).map_err(|_| CoreError::StorageIntegrityFailed)?;
    if envelope.get("v").and_then(Value::as_i64) != Some(ENVELOPE_VERSION) {
        return Err(CoreError::StorageIntegrityFailed);
    }
    let fields = envelope
        .get("fields")
        .and_then(Value::as_object)
        .ok_or(CoreError::StorageIntegrityFailed)?;
    let mut out = BTreeMap::new();
    for (name, value) in fields {
        let text = value.as_str().ok_or(CoreError::StorageIntegrityFailed)?;
        out.insert(name.clone(), unhex(text)?);
    }
    Ok(out)
}

/// 解密并解析 `options`；中间的 JSON 明文在返回前零化。
fn decrypt_options(
    key: &MasterKey,
    proxy_id: &str,
    sealed: &[u8],
) -> Result<BTreeMap<String, Value>, CoreError> {
    let mut plaintext = decrypt_field(key, aad(proxy_id, OPTIONS_FIELD), sealed)?;
    let parsed = serde_json::from_slice(&plaintext).map_err(|_| CoreError::StorageIntegrityFailed);
    plaintext.zeroize();
    parsed
}

fn aad<'a>(proxy_id: &'a str, field: &'a str) -> FieldAad<'a> {
    FieldAad {
        proxy_id,
        field_name: field,
    }
}

fn parse_credential_fields(text: &str) -> Result<Vec<String>, CoreError> {
    let mut names: Vec<String> =
        serde_json::from_str(text).map_err(|_| CoreError::StorageIntegrityFailed)?;
    names.sort();
    Ok(names)
}

fn to_port(value: i64) -> Result<u16, CoreError> {
    u16::try_from(value).map_err(|_| CoreError::StorageIntegrityFailed)
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit(u32::from(byte >> 4), 16).expect("nibble"));
        out.push(char::from_digit(u32::from(byte & 0x0f), 16).expect("nibble"));
    }
    out
}

fn unhex(text: &str) -> Result<Vec<u8>, CoreError> {
    if text.len() % 2 != 0 {
        return Err(CoreError::StorageIntegrityFailed);
    }
    let mut out = Vec::with_capacity(text.len() / 2);
    let mut high: Option<u8> = None;
    for byte in text.bytes() {
        let digit = (byte as char)
            .to_digit(16)
            .ok_or(CoreError::StorageIntegrityFailed)? as u8;
        match high.take() {
            Some(upper) => out.push((upper << 4) | digit),
            None => high = Some(digit),
        }
    }
    Ok(out)
}

fn proxy_not_found() -> CoreError {
    CoreError::validation(ErrorCode::ProxyNotFound, "proxy not found")
}

fn config_invalid(message: &'static str) -> CoreError {
    CoreError::validation(ErrorCode::ProxyConfigInvalid, message)
}

fn map_sqlx_error(error: sqlx::Error) -> CoreError {
    match &error {
        sqlx::Error::Database(db) if matches!(db.code().as_deref(), Some("5") | Some("6")) => {
            CoreError::StorageBusy
        }
        _ => CoreError::Storage,
    }
}

/// 删除时区分「被档案引用」与「不存在」：`787` 是 SQLite `FOREIGN KEY constraint failed`。
fn map_delete_error(error: sqlx::Error) -> CoreError {
    if let sqlx::Error::Database(db) = &error
        && (db.code().as_deref() == Some("787") || db.message().contains("FOREIGN KEY"))
    {
        return CoreError::validation(ErrorCode::ResourceInUse, "proxy is still referenced");
    }
    map_sqlx_error(error)
}

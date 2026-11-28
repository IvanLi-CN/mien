//! 代理配置仓储端口与传输 DTO。
//!
//! 契约：`docs/DATABASE-SCHEMA.md` §2（`proxies`）、§4（加密边界）与
//! `docs/contracts/openapi.yaml` 的 `Proxy`/`ProxyInput`/`ProxyPatch`/`ProxyPage`。
//!
//! 脱敏边界：出参只有 [`ProxyView`]，绝不含明文凭据；[`ProxySecret`] 只供进程内运行时
//! 装配出站使用，`Debug` 只列字段名且 drop 时零化。

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use zeroize::Zeroize;

use crate::domain::profile::ProfileId;
use crate::error::{CoreError, ErrorCode};

/// `proxies.kind` 的取值集合，与 DDL CHECK 枚举逐项一致；不含 v1 拒绝的 `tuic`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyKind {
    Http,
    Https,
    Socks5,
    Vless,
    Vmess,
    Shadowsocks,
    Trojan,
    Hysteria2,
    Wireguard,
}

impl ProxyKind {
    /// 全部合法取值，顺序同 DDL。
    pub const ALL: [Self; 9] = [
        Self::Http,
        Self::Https,
        Self::Socks5,
        Self::Vless,
        Self::Vmess,
        Self::Shadowsocks,
        Self::Trojan,
        Self::Hysteria2,
        Self::Wireguard,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
            Self::Socks5 => "socks5",
            Self::Vless => "vless",
            Self::Vmess => "vmess",
            Self::Shadowsocks => "shadowsocks",
            Self::Trojan => "trojan",
            Self::Hysteria2 => "hysteria2",
            Self::Wireguard => "wireguard",
        }
    }

    /// 解析 `kind`；DDL 枚举外的值（含 `tuic`）返回 `PROXY_CONFIG_INVALID`。
    ///
    /// # Errors
    /// 未知种类返回 [`ErrorCode::ProxyConfigInvalid`]。
    pub fn parse(value: &str) -> Result<Self, CoreError> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == value)
            .ok_or_else(|| {
                CoreError::validation(
                    ErrorCode::ProxyConfigInvalid,
                    "kind must be one of the documented proxy kinds",
                )
            })
    }
}

/// 代理标识（ULID 文本，与档案主键同一格式）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProxyId(pub String);

impl ProxyId {
    /// 生成新标识；复用档案的 ULID 生成器，避免第二套标识编码。
    pub fn new() -> Self {
        Self(ProfileId::new().0)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for ProxyId {
    fn default() -> Self {
        Self::new()
    }
}

/// 创建输入；`credentials` 只写入加密列，永不回显。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyInput {
    pub name: String,
    pub kind: ProxyKind,
    pub server: String,
    pub port: u16,
    #[serde(default)]
    pub options: BTreeMap<String, Value>,
    #[serde(default)]
    pub credentials: BTreeMap<String, String>,
}

/// 部分更新；未给出的字段保持原值，给出 `credentials` 则整体替换凭据集合。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProxyPatch {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub kind: Option<ProxyKind>,
    #[serde(default)]
    pub server: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub options: Option<BTreeMap<String, Value>>,
    #[serde(default)]
    pub credentials: Option<BTreeMap<String, String>>,
}

/// 脱敏视图：非秘密字段 + 凭据字段名，绝不含明文。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyView {
    pub id: ProxyId,
    pub name: String,
    pub kind: ProxyKind,
    pub server: String,
    pub port: u16,
    pub options: BTreeMap<String, Value>,
    pub credential_set: bool,
    pub credential_fields: Vec<String>,
    pub last_check: Option<Value>,
    pub created_at: String,
    pub updated_at: String,
}

/// 分页结果；`next_cursor` 是最后一条的 `id`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyPage {
    pub items: Vec<ProxyView>,
    pub next_cursor: Option<String>,
}

/// 进程内明文配置；只用于装配引擎出站，不序列化、不写日志。
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ProxySecret {
    pub options: BTreeMap<String, Value>,
    pub credentials: BTreeMap<String, String>,
}

impl ProxySecret {
    /// 已设置的凭据字段名（升序）。
    pub fn credential_fields(&self) -> Vec<String> {
        self.credentials.keys().cloned().collect()
    }
}

impl fmt::Debug for ProxySecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProxySecret")
            .field("options", &self.options.keys().collect::<Vec<_>>())
            .field("credential_fields", &self.credential_fields())
            .finish()
    }
}

impl Drop for ProxySecret {
    fn drop(&mut self) {
        for value in self.credentials.values_mut() {
            value.zeroize();
        }
    }
}

/// 代理配置仓储。
pub trait ProxyRepository: Send + Sync {
    /// 新增代理；凭据加密后写入 `config_ciphertext`。
    ///
    /// # Errors
    /// 字段非法返回 `PROXY_CONFIG_INVALID`；存储失败返回 `STORAGE_*`。
    fn create(
        &self,
        input: ProxyInput,
    ) -> impl Future<Output = Result<ProxyView, CoreError>> + Send;

    /// 部分更新；未提及字段不变。
    ///
    /// # Errors
    /// 不存在返回 `PROXY_NOT_FOUND`；字段非法返回 `PROXY_CONFIG_INVALID`。
    fn update(
        &self,
        id: &ProxyId,
        patch: ProxyPatch,
    ) -> impl Future<Output = Result<ProxyView, CoreError>> + Send;

    /// 读取脱敏视图。
    ///
    /// # Errors
    /// 不存在返回 `PROXY_NOT_FOUND`。
    fn get(&self, id: &ProxyId) -> impl Future<Output = Result<ProxyView, CoreError>> + Send;

    /// 按 `id` 升序分页。
    ///
    /// # Errors
    /// 存储或解密失败。
    fn list(
        &self,
        limit: Option<u32>,
        cursor: Option<String>,
    ) -> impl Future<Output = Result<ProxyPage, CoreError>> + Send;

    /// 删除代理行。
    ///
    /// # Errors
    /// 不存在返回 `PROXY_NOT_FOUND`；仍被档案引用返回 `RESOURCE_IN_USE`。
    fn delete(&self, id: &ProxyId) -> impl Future<Output = Result<(), CoreError>> + Send;

    /// 解密读取配置；仅在进程内用于装配引擎，调用方不得记录结果。
    ///
    /// # Errors
    /// 不存在返回 `PROXY_NOT_FOUND`；密文认证失败返回 `STORAGE_INTEGRITY_FAILED`。
    fn load_secret(
        &self,
        id: &ProxyId,
    ) -> impl Future<Output = Result<ProxySecret, CoreError>> + Send;
}

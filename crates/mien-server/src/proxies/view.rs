//! `ProxyCheck` 响应形状与检查错误分类（`docs/contracts/openapi.yaml`）。
//!
//! 同一形状三处复用：`POST /api/v1/proxies/{id}/check` 的 200 响应、
//! `proxies.last_check_json` 与 `proxy.check.completed` 事件的 payload。
//! `error_code` 只能取 [`CheckErrorCode`] 的六个值；控制面错误走 HTTP 错误信封，
//! 绝不出现在这里。

use serde::Serialize;

/// `ProxyCheck.error_code` 的六个契约取值。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CheckErrorCode {
    ProxyConnectionRefused,
    ProxyAuthFailed,
    ProxyTlsFailed,
    ProxyTimeout,
    ProxyConnectionFailed,
    ProxyDnsFailed,
}

impl CheckErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProxyConnectionRefused => "PROXY_CONNECTION_REFUSED",
            Self::ProxyAuthFailed => "PROXY_AUTH_FAILED",
            Self::ProxyTlsFailed => "PROXY_TLS_FAILED",
            Self::ProxyTimeout => "PROXY_TIMEOUT",
            Self::ProxyConnectionFailed => "PROXY_CONNECTION_FAILED",
            Self::ProxyDnsFailed => "PROXY_DNS_FAILED",
        }
    }

    /// 稳定、脱敏的人读说明；绝不含节点地址、URL 或凭据。
    pub const fn message(self) -> &'static str {
        match self {
            Self::ProxyConnectionRefused => "The proxy node refused the connection.",
            Self::ProxyAuthFailed => "The proxy node rejected the credentials.",
            Self::ProxyTlsFailed => "The TLS or REALITY handshake with the proxy node failed.",
            Self::ProxyTimeout => "The proxy check timed out.",
            Self::ProxyConnectionFailed => "The proxy node could not be reached.",
            Self::ProxyDnsFailed => "The proxy node host could not be resolved.",
        }
    }
}

/// `ProxyCheck`（`docs/contracts/openapi.yaml#/components/schemas/ProxyCheck`）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProxyCheck {
    pub ok: bool,
    pub latency_ms: Option<i64>,
    pub exit_ip: Option<String>,
    pub region: Option<String>,
    pub error_code: Option<CheckErrorCode>,
    pub message: Option<String>,
    pub checked_at: String,
}

impl ProxyCheck {
    /// 成功：延迟必有，出口 IP/地区只在回显端点给出时出现。
    pub fn success(latency_ms: i64, exit_ip: Option<String>, region: Option<String>) -> Self {
        Self {
            ok: true,
            latency_ms: Some(latency_ms),
            exit_ip,
            region,
            error_code: None,
            message: None,
            checked_at: crate::auth::session::now_timestamp(),
        }
    }

    /// 节点/网络失败：`ok=false`，稳定错误码，脱敏说明。
    pub fn failure(code: CheckErrorCode) -> Self {
        Self {
            ok: false,
            latency_ms: None,
            exit_ip: None,
            region: None,
            error_code: Some(code),
            message: Some(code.message().to_owned()),
            checked_at: crate::auth::session::now_timestamp(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_serialize_to_the_contract_strings() {
        let codes = [
            CheckErrorCode::ProxyConnectionRefused,
            CheckErrorCode::ProxyAuthFailed,
            CheckErrorCode::ProxyTlsFailed,
            CheckErrorCode::ProxyTimeout,
            CheckErrorCode::ProxyConnectionFailed,
            CheckErrorCode::ProxyDnsFailed,
        ];
        for code in codes {
            assert_eq!(
                serde_json::to_value(code).expect("serialize"),
                serde_json::Value::String(code.as_str().to_owned())
            );
        }
    }

    #[test]
    fn failed_check_carries_null_measurements_and_a_redacted_message() {
        let check = ProxyCheck::failure(CheckErrorCode::ProxyTimeout);
        let value = serde_json::to_value(&check).expect("serialize");
        assert_eq!(value["ok"], false);
        assert_eq!(value["error_code"], "PROXY_TIMEOUT");
        assert!(value["latency_ms"].is_null());
        assert!(value["exit_ip"].is_null());
        assert!(!check.message.expect("message").contains("://"));
    }
}

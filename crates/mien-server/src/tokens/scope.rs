//! API Token scope 枚举与端点映射。
//!
//! 契约：`docs/contracts/openapi.yaml#/components/schemas/TokenScope`（11 项，冻结）与每个
//! 接受 `bearerAuth` 的操作上的 `x-required-scope`；`docs/specs/control-api/SPEC.md`
//! `REQ-API-005`。这里的 [`ROUTES`] 是该契约的镜像：测试逐项比对 OpenAPI，任何一侧漂移
//! 都会失败。仅在 Cookie/ticket 认证的操作（`auth*`、`eventsWebSocket`）与公开端点不在表内。

use std::fmt;

use axum::http::Method;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Token 授权范围；字符串值与 OpenAPI `TokenScope` 一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TokenScope {
    ProfilesRead,
    ProfilesWrite,
    RuntimeControl,
    ProxiesRead,
    ProxiesWrite,
    AuditRead,
    EventsRead,
    TokensManage,
    KernelsManage,
    EnginesManage,
    ViewerAccess,
}

impl TokenScope {
    /// 全部范围，顺序与 OpenAPI 枚举一致。
    pub const ALL: &'static [Self] = &[
        Self::ProfilesRead,
        Self::ProfilesWrite,
        Self::RuntimeControl,
        Self::ProxiesRead,
        Self::ProxiesWrite,
        Self::AuditRead,
        Self::EventsRead,
        Self::TokensManage,
        Self::KernelsManage,
        Self::EnginesManage,
        Self::ViewerAccess,
    ];

    /// 稳定字符串；持久化与契约都使用它。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProfilesRead => "profiles.read",
            Self::ProfilesWrite => "profiles.write",
            Self::RuntimeControl => "runtime.control",
            Self::ProxiesRead => "proxies.read",
            Self::ProxiesWrite => "proxies.write",
            Self::AuditRead => "audit.read",
            Self::EventsRead => "events.read",
            Self::TokensManage => "tokens.manage",
            Self::KernelsManage => "kernels.manage",
            Self::EnginesManage => "engines.manage",
            Self::ViewerAccess => "viewer.access",
        }
    }

    /// 解析稳定字符串；未知值返回 `None`，调用方必须拒绝而不是降级。
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|scope| scope.as_str() == value)
    }
}

impl fmt::Display for TokenScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for TokenScope {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for TokenScope {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ScopeVisitor;

        impl Visitor<'_> for ScopeVisitor {
            type Value = TokenScope;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a TokenScope value such as \"profiles.read\"")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                TokenScope::parse(value).ok_or_else(|| E::unknown_variant(value, &[]))
            }
        }

        deserializer.deserialize_str(ScopeVisitor)
    }
}

/// 一个接受 Bearer Token 的端点及其所需范围。
#[derive(Debug, Clone, Copy)]
pub struct RouteScope {
    pub method: &'static str,
    pub path: &'static str,
    pub operation: &'static str,
    pub scope: TokenScope,
}

/// `x-required-scope` 的完整镜像；顺序与 OpenAPI 文档一致，测试逐项比对。
pub const ROUTES: &[RouteScope] = &[
    RouteScope {
        method: "GET",
        path: "/api/v1/profiles",
        operation: "profilesList",
        scope: TokenScope::ProfilesRead,
    },
    RouteScope {
        method: "POST",
        path: "/api/v1/profiles",
        operation: "profilesCreate",
        scope: TokenScope::ProfilesWrite,
    },
    RouteScope {
        method: "GET",
        path: "/api/v1/profiles/{profile_id}",
        operation: "profilesGet",
        scope: TokenScope::ProfilesRead,
    },
    RouteScope {
        method: "PATCH",
        path: "/api/v1/profiles/{profile_id}",
        operation: "profilesUpdate",
        scope: TokenScope::ProfilesWrite,
    },
    RouteScope {
        method: "DELETE",
        path: "/api/v1/profiles/{profile_id}",
        operation: "profilesDelete",
        scope: TokenScope::ProfilesWrite,
    },
    RouteScope {
        method: "POST",
        path: "/api/v1/profiles/{profile_id}/duplicate",
        operation: "profilesDuplicate",
        scope: TokenScope::ProfilesWrite,
    },
    RouteScope {
        method: "POST",
        path: "/api/v1/profiles/{profile_id}/archive",
        operation: "profilesArchive",
        scope: TokenScope::ProfilesWrite,
    },
    RouteScope {
        method: "POST",
        path: "/api/v1/profiles/{profile_id}/restore",
        operation: "profilesRestore",
        scope: TokenScope::ProfilesWrite,
    },
    RouteScope {
        method: "POST",
        path: "/api/v1/profiles/import",
        operation: "profilesImport",
        scope: TokenScope::ProfilesWrite,
    },
    RouteScope {
        method: "POST",
        path: "/api/v1/profiles/{profile_id}/export",
        operation: "profilesExport",
        scope: TokenScope::ProfilesRead,
    },
    RouteScope {
        method: "POST",
        path: "/api/v1/profiles/{profile_id}/start",
        operation: "profilesStart",
        scope: TokenScope::RuntimeControl,
    },
    RouteScope {
        method: "POST",
        path: "/api/v1/profiles/{profile_id}/stop",
        operation: "profilesStop",
        scope: TokenScope::RuntimeControl,
    },
    RouteScope {
        method: "GET",
        path: "/api/v1/profiles/{profile_id}/runtime",
        operation: "profilesRuntime",
        scope: TokenScope::ProfilesRead,
    },
    RouteScope {
        method: "POST",
        path: "/api/v1/profiles/{profile_id}/viewer",
        operation: "profilesViewer",
        scope: TokenScope::ViewerAccess,
    },
    RouteScope {
        method: "GET",
        path: "/api/v1/groups",
        operation: "groupsList",
        scope: TokenScope::ProfilesRead,
    },
    RouteScope {
        method: "POST",
        path: "/api/v1/groups",
        operation: "groupsCreate",
        scope: TokenScope::ProfilesWrite,
    },
    RouteScope {
        method: "PATCH",
        path: "/api/v1/groups/{group_id}",
        operation: "groupsUpdate",
        scope: TokenScope::ProfilesWrite,
    },
    RouteScope {
        method: "DELETE",
        path: "/api/v1/groups/{group_id}",
        operation: "groupsDelete",
        scope: TokenScope::ProfilesWrite,
    },
    RouteScope {
        method: "GET",
        path: "/api/v1/tags",
        operation: "tagsList",
        scope: TokenScope::ProfilesRead,
    },
    RouteScope {
        method: "POST",
        path: "/api/v1/tags",
        operation: "tagsCreate",
        scope: TokenScope::ProfilesWrite,
    },
    RouteScope {
        method: "PATCH",
        path: "/api/v1/tags/{tag_id}",
        operation: "tagsUpdate",
        scope: TokenScope::ProfilesWrite,
    },
    RouteScope {
        method: "DELETE",
        path: "/api/v1/tags/{tag_id}",
        operation: "tagsDelete",
        scope: TokenScope::ProfilesWrite,
    },
    RouteScope {
        method: "GET",
        path: "/api/v1/proxies",
        operation: "proxiesList",
        scope: TokenScope::ProxiesRead,
    },
    RouteScope {
        method: "POST",
        path: "/api/v1/proxies",
        operation: "proxiesCreate",
        scope: TokenScope::ProxiesWrite,
    },
    RouteScope {
        method: "POST",
        path: "/api/v1/proxies/import-link",
        operation: "proxiesImportLink",
        scope: TokenScope::ProxiesWrite,
    },
    RouteScope {
        method: "PATCH",
        path: "/api/v1/proxies/{proxy_id}",
        operation: "proxiesUpdate",
        scope: TokenScope::ProxiesWrite,
    },
    RouteScope {
        method: "DELETE",
        path: "/api/v1/proxies/{proxy_id}",
        operation: "proxiesDelete",
        scope: TokenScope::ProxiesWrite,
    },
    RouteScope {
        method: "POST",
        path: "/api/v1/proxies/{proxy_id}/check",
        operation: "proxiesCheck",
        scope: TokenScope::ProxiesRead,
    },
    RouteScope {
        method: "GET",
        path: "/api/v1/kernels",
        operation: "kernelsList",
        scope: TokenScope::KernelsManage,
    },
    RouteScope {
        method: "POST",
        path: "/api/v1/kernels/{version}/activate",
        operation: "kernelsActivate",
        scope: TokenScope::KernelsManage,
    },
    RouteScope {
        method: "GET",
        path: "/api/v1/engines",
        operation: "enginesList",
        scope: TokenScope::EnginesManage,
    },
    RouteScope {
        method: "POST",
        path: "/api/v1/engines/{version}/activate",
        operation: "enginesActivate",
        scope: TokenScope::EnginesManage,
    },
    RouteScope {
        method: "GET",
        path: "/api/v1/tokens",
        operation: "tokensList",
        scope: TokenScope::TokensManage,
    },
    RouteScope {
        method: "POST",
        path: "/api/v1/tokens",
        operation: "tokensCreate",
        scope: TokenScope::TokensManage,
    },
    RouteScope {
        method: "DELETE",
        path: "/api/v1/tokens/{token_id}",
        operation: "tokensRevoke",
        scope: TokenScope::TokensManage,
    },
    RouteScope {
        method: "GET",
        path: "/api/v1/audit/events",
        operation: "auditList",
        scope: TokenScope::AuditRead,
    },
    RouteScope {
        method: "POST",
        path: "/api/v1/events/ticket",
        operation: "eventsTicket",
        scope: TokenScope::EventsRead,
    },
    RouteScope {
        method: "POST",
        path: "/api/v1/profiles/import/validate",
        operation: "profilesImportValidate",
        scope: TokenScope::ProfilesWrite,
    },
    RouteScope {
        method: "GET",
        path: "/api/v1/profiles/{profile_id}/logs",
        operation: "profilesLogs",
        scope: TokenScope::ProfilesRead,
    },
    RouteScope {
        method: "POST",
        path: "/api/v1/profiles/{profile_id}/migrate-kernel",
        operation: "profilesMigrateKernel",
        scope: TokenScope::ProfilesWrite,
    },
];

/// 请求路径所需的 scope；未登记的方法/路径返回 `None`（由路由层给 404）。
///
/// 匹配在认证之后、任何资源存在性查询之前进行（`REQ-API-003` 优先级）。
pub fn required(method: &Method, path: &str) -> Option<TokenScope> {
    ROUTES
        .iter()
        .find(|route| route.method == method.as_str() && path_matches(route.path, path))
        .map(|route| route.scope)
}

/// 路径模板匹配：`{param}` 只匹配一个非空段，其余按字面量比较。
fn path_matches(pattern: &str, path: &str) -> bool {
    let mut pattern_segments = pattern.split('/');
    let mut path_segments = path.split('/');
    loop {
        match (pattern_segments.next(), path_segments.next()) {
            (None, None) => return true,
            (Some(template), Some(segment)) => {
                let parameter = template.starts_with('{') && template.ends_with('}');
                if !parameter && template != segment {
                    return false;
                }
                if parameter && segment.is_empty() {
                    return false;
                }
            }
            _ => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_round_trip_and_reject_unknown_values() {
        for scope in TokenScope::ALL {
            assert_eq!(TokenScope::parse(scope.as_str()), Some(*scope));
            assert_eq!(
                serde_json::to_value(scope).expect("serialize"),
                serde_json::Value::String(scope.as_str().to_owned())
            );
            let decoded: TokenScope =
                serde_json::from_value(serde_json::json!(scope.as_str())).expect("deserialize");
            assert_eq!(decoded, *scope);
        }
        assert_eq!(TokenScope::ALL.len(), 11);
        assert_eq!(TokenScope::parse("profiles.delete"), None);
        assert_eq!(TokenScope::parse("Profiles.Read"), None);
        assert!(serde_json::from_value::<TokenScope>(serde_json::json!("nope")).is_err());
    }

    #[test]
    fn route_lookup_matches_literals_and_parameters() {
        assert_eq!(
            required(&Method::GET, "/api/v1/tokens"),
            Some(TokenScope::TokensManage)
        );
        assert_eq!(
            required(&Method::DELETE, "/api/v1/tokens/01ARZ3NDEKTSV4RRFFQ69G5FAV"),
            Some(TokenScope::TokensManage)
        );
        assert_eq!(
            required(&Method::POST, "/api/v1/profiles/abc/start"),
            Some(TokenScope::RuntimeControl)
        );
        assert_eq!(
            required(&Method::GET, "/api/v1/profiles/abc"),
            Some(TokenScope::ProfilesRead)
        );
        // 空参数段、额外段、错误方法与公开端点都不使用 Bearer 授权表。
        assert_eq!(required(&Method::GET, "/api/v1/profiles/"), None);
        assert_eq!(required(&Method::GET, "/api/v1/profiles/abc/def"), None);
        assert_eq!(required(&Method::PUT, "/api/v1/tokens"), None);
        assert_eq!(required(&Method::GET, "/api/v1/auth/session"), None);
        assert_eq!(required(&Method::GET, "/api/v1/nope"), None);
    }
}

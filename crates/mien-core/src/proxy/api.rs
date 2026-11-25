//! Xray v26.11.9 gRPC API adapter and idempotent tag reconciliation.
use super::keys::{MuxSettings, Transport};
use super::xray::{
    app::{
        proxyman::{
            MultiplexingConfig, ReceiverConfig, SenderConfig, SniffingConfig,
            command::{
                AddInboundRequest, AddOutboundRequest, ListInboundsRequest, ListOutboundsRequest,
                RemoveInboundRequest, RemoveOutboundRequest,
                handler_service_client::HandlerServiceClient,
            },
        },
        router::{
            Config as RouterConfig, RoutingRule,
            command::{
                AddRuleRequest, ListRuleRequest, RemoveRuleRequest,
                routing_service_client::RoutingServiceClient,
            },
        },
    },
    common::{
        net::{IpOrDomain, PortList, PortRange, ip_or_domain::Address as IpAddress},
        protocol::{ServerEndpoint, User},
        serial::TypedMessage,
    },
    core::{InboundHandlerConfig, OutboundHandlerConfig},
    proxy::{
        http::{
            Account as HttpAccount, ClientConfig as HttpClientConfig,
            ServerConfig as HttpServerConfig,
        },
        vless::{Account as VlessAccount, outbound::Config as VlessConfig},
    },
};
use crate::error::{CoreError, ErrorCode};
use prost::Message;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fmt, future::Future, net::SocketAddr, sync::Arc};
use tokio::sync::Mutex;
use tonic::transport::{Channel, Endpoint};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiErrorCode {
    AlreadyExists,
    NotFound,
    Unavailable,
    Invalid,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub code: ApiErrorCode,
    pub message: String,
}
impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.code {
            ApiErrorCode::AlreadyExists => "Xray resource already exists",
            ApiErrorCode::NotFound => "Xray resource not found",
            ApiErrorCode::Unavailable => "Xray management API unavailable",
            ApiErrorCode::Invalid => "invalid Xray API request",
        })
    }
}
impl std::error::Error for ApiError {}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Inbound {
    pub tag: String,
    pub listen: String,
    pub port: u16,
    pub protocol: String,
    #[serde(default)]
    pub settings: Value,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Outbound {
    pub tag: String,
    pub protocol: String,
    #[serde(default)]
    pub mux: MuxSettings,
    #[serde(default)]
    pub transport: Transport,
    #[serde(default)]
    pub settings: Value,
    #[serde(default)]
    pub stream_settings: Value,
}
impl fmt::Debug for Outbound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Outbound")
            .field("tag", &self.tag)
            .field("protocol", &self.protocol)
            .field("settings", &"<redacted>")
            .finish()
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    pub tag: String,
    pub inbound_tag: String,
    pub outbound_tag: String,
}
pub trait XrayApi: Send + Sync {
    fn list_inbounds(&self) -> Result<Vec<Inbound>, ApiError>;
    fn list_outbounds(&self) -> Result<Vec<Outbound>, ApiError>;
    fn list_rules(&self) -> Result<Vec<Rule>, ApiError>;
    fn add_inbound(&self, x: Inbound) -> Result<(), ApiError>;
    fn remove_inbound(&self, t: &str) -> Result<(), ApiError>;
    fn add_outbound(&self, x: Outbound) -> Result<(), ApiError>;
    fn remove_outbound(&self, t: &str) -> Result<(), ApiError>;
    fn add_rule(&self, x: Rule, a: bool) -> Result<(), ApiError>;
    fn remove_rule(&self, t: &str) -> Result<(), ApiError>;
}
#[derive(Clone)]
pub struct GrpcXrayApi {
    endpoint: String,
    handler: Arc<Mutex<HandlerServiceClient<Channel>>>,
    router: Arc<Mutex<RoutingServiceClient<Channel>>>,
}
impl fmt::Debug for GrpcXrayApi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GrpcXrayApi")
            .field("endpoint", &self.endpoint)
            .finish()
    }
}
impl GrpcXrayApi {
    pub async fn connect(a: SocketAddr) -> Result<Self, ApiError> {
        if !a.ip().is_loopback() {
            return Err(invalid("loopback required"));
        }
        let e = format!("http://{a}");
        let c = Endpoint::from_shared(e.clone())
            .map_err(transport_error)?
            .connect()
            .await
            .map_err(transport_error)?;
        Ok(Self {
            endpoint: e,
            handler: Arc::new(Mutex::new(HandlerServiceClient::new(c.clone()))),
            router: Arc::new(Mutex::new(RoutingServiceClient::new(c))),
        })
    }
    pub async fn probe(&self) -> Result<(), ApiError> {
        self.handler
            .lock()
            .await
            .list_inbounds(ListInboundsRequest { is_only_tags: true })
            .await
            .map_err(status_error)?;
        self.router
            .lock()
            .await
            .list_rule(ListRuleRequest {})
            .await
            .map_err(status_error)?;
        Ok(())
    }
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}
impl XrayApi for GrpcXrayApi {
    fn list_inbounds(&self) -> Result<Vec<Inbound>, ApiError> {
        sync_call(async {
            Ok(self
                .handler
                .lock()
                .await
                .list_inbounds(ListInboundsRequest {
                    is_only_tags: false,
                })
                .await
                .map_err(status_error)?
                .into_inner()
                .inbounds
                .into_iter()
                .map(|x| Inbound {
                    tag: x.tag,
                    listen: String::new(),
                    port: 0,
                    protocol: String::new(),
                    settings: Value::Null,
                })
                .collect())
        })
    }
    fn list_outbounds(&self) -> Result<Vec<Outbound>, ApiError> {
        sync_call(async {
            Ok(self
                .handler
                .lock()
                .await
                .list_outbounds(ListOutboundsRequest {})
                .await
                .map_err(status_error)?
                .into_inner()
                .outbounds
                .into_iter()
                .map(|x| Outbound {
                    tag: x.tag,
                    protocol: String::new(),
                    mux: MuxSettings::default(),
                    transport: Transport::Tcp,
                    settings: Value::Null,
                    stream_settings: Value::Null,
                })
                .collect())
        })
    }
    fn list_rules(&self) -> Result<Vec<Rule>, ApiError> {
        sync_call(async {
            Ok(self
                .router
                .lock()
                .await
                .list_rule(ListRuleRequest {})
                .await
                .map_err(status_error)?
                .into_inner()
                .rules
                .into_iter()
                .map(|x| Rule {
                    tag: if x.rule_tag.is_empty() {
                        x.tag
                    } else {
                        x.rule_tag
                    },
                    inbound_tag: String::new(),
                    outbound_tag: String::new(),
                })
                .collect())
        })
    }
    fn add_inbound(&self, x: Inbound) -> Result<(), ApiError> {
        let x = to_inbound(x)?;
        sync_call(async {
            self.handler
                .lock()
                .await
                .add_inbound(AddInboundRequest { inbound: Some(x) })
                .await
                .map_err(status_error)?;
            Ok(())
        })
    }
    fn remove_inbound(&self, t: &str) -> Result<(), ApiError> {
        let t = t.into();
        sync_call(async {
            self.handler
                .lock()
                .await
                .remove_inbound(RemoveInboundRequest { tag: t })
                .await
                .map_err(status_error)?;
            Ok(())
        })
    }
    fn add_outbound(&self, x: Outbound) -> Result<(), ApiError> {
        let x = to_outbound(x)?;
        sync_call(async {
            self.handler
                .lock()
                .await
                .add_outbound(AddOutboundRequest { outbound: Some(x) })
                .await
                .map_err(status_error)?;
            Ok(())
        })
    }
    fn remove_outbound(&self, t: &str) -> Result<(), ApiError> {
        let t = t.into();
        sync_call(async {
            self.handler
                .lock()
                .await
                .remove_outbound(RemoveOutboundRequest { tag: t })
                .await
                .map_err(status_error)?;
            Ok(())
        })
    }
    fn add_rule(&self, x: Rule, a: bool) -> Result<(), ApiError> {
        let x = to_rule(&x)?;
        sync_call(async {
            self.router
                .lock()
                .await
                .add_rule(AddRuleRequest {
                    config: Some(x),
                    should_append: a,
                })
                .await
                .map_err(status_error)?;
            Ok(())
        })
    }
    fn remove_rule(&self, t: &str) -> Result<(), ApiError> {
        let t = t.into();
        sync_call(async {
            self.router
                .lock()
                .await
                .remove_rule(RemoveRuleRequest { rule_tag: t })
                .await
                .map_err(status_error)?;
            Ok(())
        })
    }
}
fn sync_call<T: Send, F: Future<Output = Result<T, ApiError>> + Send>(f: F) -> Result<T, ApiError> {
    let run = move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(transport_error)?
            .block_on(f)
    };
    if tokio::runtime::Handle::try_current().is_ok() {
        std::thread::scope(|s| {
            s.spawn(run)
                .join()
                .unwrap_or_else(|_| Err(invalid("gRPC worker failed")))
        })
    } else {
        run()
    }
}
fn to_rule(r: &Rule) -> Result<TypedMessage, ApiError> {
    let route = RoutingRule {
        target_tag: Some(super::xray::app::router::routing_rule::TargetTag::Tag(
            r.outbound_tag.clone(),
        )),
        rule_tag: r.tag.clone(),
        inbound_tag: vec![r.inbound_tag.clone()],
        ..Default::default()
    };
    let cfg = RouterConfig {
        domain_strategy: 0,
        rule: vec![route],
        balancing_rule: vec![],
        script: String::new(),
    };
    Ok(TypedMessage {
        r#type: "xray.app.router.Config".into(),
        value: cfg.encode_to_vec(),
    })
}
fn to_inbound(x: Inbound) -> Result<InboundHandlerConfig, ApiError> {
    if x.listen != "127.0.0.1" || x.port == 0 {
        return Err(invalid("loopback/fixed port required"));
    }
    let recv = ReceiverConfig {
        port_list: Some(PortList {
            range: vec![PortRange {
                from: x.port.into(),
                to: x.port.into(),
            }],
        }),
        listen: Some(IpOrDomain {
            address: Some(IpAddress::Ip(
                std::net::Ipv4Addr::LOCALHOST.octets().to_vec(),
            )),
        }),
        stream_settings: None,
        receive_original_destination: false,
        sniffing_settings: Some(SniffingConfig {
            enabled: false,
            destination_override: vec![],
            metadata_only: false,
            route_only: false,
        }),
    };
    let cfg = HttpServerConfig {
        accounts: Default::default(),
        allow_transparent: false,
        user_level: 0,
    };
    Ok(InboundHandlerConfig {
        tag: x.tag,
        receiver_settings: Some(TypedMessage {
            r#type: "xray.app.proxyman.ReceiverConfig".into(),
            value: recv.encode_to_vec(),
        }),
        proxy_settings: Some(TypedMessage {
            r#type: "xray.proxy.http.ServerConfig".into(),
            value: cfg.encode_to_vec(),
        }),
    })
}
fn to_outbound(x: Outbound) -> Result<OutboundHandlerConfig, ApiError> {
    let (t, v) = match x.protocol.as_str() {
        "vless" => {
            let s = x.settings["vnext"]
                .get(0)
                .ok_or_else(|| invalid("bad VLESS settings"))?;
            let u = s["users"].get(0).ok_or_else(|| invalid("bad VLESS user"))?;
            let a = VlessAccount {
                id: u["id"]
                    .as_str()
                    .ok_or_else(|| invalid("missing VLESS id"))?
                    .into(),
                flow: u["flow"].as_str().unwrap_or_default().into(),
                encryption: u["encryption"].as_str().unwrap_or("none").into(),
                xor_mode: 0,
                seconds: 0,
                padding: String::new(),
                reverse: None,
                testpre: 0,
                testseed: vec![],
            };
            let user = User {
                level: 0,
                email: String::new(),
                account: Some(TypedMessage {
                    r#type: "xray.proxy.vless.Account".into(),
                    value: a.encode_to_vec(),
                }),
            };
            let cfg = VlessConfig {
                vnext: Some(ServerEndpoint {
                    address: Some(address(
                        s["address"]
                            .as_str()
                            .ok_or_else(|| invalid("missing address"))?,
                    )),
                    port: s["port"].as_u64().ok_or_else(|| invalid("missing port"))? as u32,
                    user: Some(user),
                }),
            };
            ("xray.proxy.vless.outbound.Config", cfg.encode_to_vec())
        }
        "http" => {
            let s = x.settings["servers"]
                .get(0)
                .ok_or_else(|| invalid("bad HTTP settings"))?;
            let a = HttpAccount {
                username: s["users"]
                    .get(0)
                    .and_then(|u| u["user"].as_str())
                    .unwrap_or_default()
                    .into(),
                password: s["users"]
                    .get(0)
                    .and_then(|u| u["pass"].as_str())
                    .unwrap_or_default()
                    .into(),
            };
            let user = User {
                level: 0,
                email: String::new(),
                account: Some(TypedMessage {
                    r#type: "xray.proxy.http.Account".into(),
                    value: a.encode_to_vec(),
                }),
            };
            let cfg = HttpClientConfig {
                server: Some(ServerEndpoint {
                    address: Some(address(
                        s["address"]
                            .as_str()
                            .ok_or_else(|| invalid("missing address"))?,
                    )),
                    port: s["port"].as_u64().ok_or_else(|| invalid("missing port"))? as u32,
                    user: Some(user),
                }),
                header: vec![],
            };
            ("xray.proxy.http.ClientConfig", cfg.encode_to_vec())
        }
        _ => return Err(invalid("unsupported outbound protocol")),
    };
    let m = x.mux.effective();
    let stream = super::xray::transport::internet::StreamConfig {
        address: None,
        port: 0,
        protocol_name: match x.transport {
            Transport::Tcp => "tcp",
            Transport::Ws => "ws",
            Transport::Grpc => "grpc",
            Transport::Quic => "quic",
            Transport::Hysteria => "hysteria",
        }
        .into(),
        transport_settings: vec![],
        security_type: String::new(),
        security_settings: vec![],
    };
    let sender = SenderConfig {
        via: None,
        stream_settings: Some(stream),
        multiplex_settings: Some(MultiplexingConfig {
            enabled: m.enabled,
            concurrency: m.concurrency as i32,
            xudp_concurrency: 0,
            xudp_proxy_udp443: "reject".into(),
        }),
        via_cidr: String::new(),
        target_strategy: 0,
    };
    Ok(OutboundHandlerConfig {
        tag: x.tag,
        sender_settings: Some(TypedMessage {
            r#type: "xray.app.proxyman.SenderConfig".into(),
            value: sender.encode_to_vec(),
        }),
        proxy_settings: Some(TypedMessage {
            r#type: t.into(),
            value: v,
        }),
        expire: 0,
        comment: String::new(),
    })
}
fn address(s: &str) -> IpOrDomain {
    let a = if let Ok(ip) = s.parse::<std::net::IpAddr>() {
        IpAddress::Ip(match ip {
            std::net::IpAddr::V4(v) => v.octets().to_vec(),
            std::net::IpAddr::V6(v) => v.octets().to_vec(),
        })
    } else {
        IpAddress::Domain(s.into())
    };
    IpOrDomain { address: Some(a) }
}
fn invalid(s: &str) -> ApiError {
    ApiError {
        code: ApiErrorCode::Invalid,
        message: s.into(),
    }
}
fn transport_error<E: fmt::Display>(_: E) -> ApiError {
    ApiError {
        code: ApiErrorCode::Unavailable,
        message: String::new(),
    }
}
fn status_error(s: tonic::Status) -> ApiError {
    ApiError {
        code: match s.code() {
            tonic::Code::AlreadyExists => ApiErrorCode::AlreadyExists,
            tonic::Code::NotFound => ApiErrorCode::NotFound,
            tonic::Code::Unavailable => ApiErrorCode::Unavailable,
            _ => ApiErrorCode::Invalid,
        },
        message: s.message().into(),
    }
}
pub fn ensure_inbound(a: &dyn XrayApi, x: Inbound) -> Result<(), CoreError> {
    if a.list_inbounds()
        .map_err(api_error)?
        .iter()
        .any(|i| i.tag == x.tag)
    {
        return Ok(());
    }
    match a.add_inbound(x) {
        Ok(())
        | Err(ApiError {
            code: ApiErrorCode::AlreadyExists,
            ..
        }) => Ok(()),
        Err(e) => Err(api_error(e)),
    }
}
pub fn ensure_outbound(a: &dyn XrayApi, x: Outbound) -> Result<(), CoreError> {
    if a.list_outbounds()
        .map_err(api_error)?
        .iter()
        .any(|i| i.tag == x.tag)
    {
        return Ok(());
    }
    match a.add_outbound(x) {
        Ok(())
        | Err(ApiError {
            code: ApiErrorCode::AlreadyExists,
            ..
        }) => Ok(()),
        Err(e) => Err(api_error(e)),
    }
}
pub fn ensure_rule(a: &dyn XrayApi, x: Rule) -> Result<(), CoreError> {
    if a.list_rules()
        .map_err(api_error)?
        .iter()
        .any(|i| i.tag == x.tag)
    {
        return Ok(());
    }
    match a.add_rule(x, true) {
        Ok(())
        | Err(ApiError {
            code: ApiErrorCode::AlreadyExists,
            ..
        }) => Ok(()),
        Err(e) => Err(api_error(e)),
    }
}
pub fn remove_inbound(a: &dyn XrayApi, t: &str) -> Result<(), CoreError> {
    if !a
        .list_inbounds()
        .map_err(api_error)?
        .iter()
        .any(|i| i.tag == t)
    {
        return Ok(());
    }
    match a.remove_inbound(t) {
        Ok(())
        | Err(ApiError {
            code: ApiErrorCode::NotFound,
            ..
        }) => Ok(()),
        Err(e) => Err(api_error(e)),
    }
}
pub fn remove_outbound(a: &dyn XrayApi, t: &str) -> Result<(), CoreError> {
    if !a
        .list_outbounds()
        .map_err(api_error)?
        .iter()
        .any(|i| i.tag == t)
    {
        return Ok(());
    }
    match a.remove_outbound(t) {
        Ok(())
        | Err(ApiError {
            code: ApiErrorCode::NotFound,
            ..
        }) => Ok(()),
        Err(e) => Err(api_error(e)),
    }
}
pub fn remove_rule(a: &dyn XrayApi, t: &str) -> Result<(), CoreError> {
    if !a
        .list_rules()
        .map_err(api_error)?
        .iter()
        .any(|i| i.tag == t)
    {
        return Ok(());
    }
    match a.remove_rule(t) {
        Ok(())
        | Err(ApiError {
            code: ApiErrorCode::NotFound,
            ..
        }) => Ok(()),
        Err(e) => Err(api_error(e)),
    }
}
fn api_error(e: ApiError) -> CoreError {
    // Every branch must use a catalogued code; `docs/ERROR-CODES.md` has no
    // dedicated reconcile code, so unexpected API state is reported as an
    // unavailable API (retryable) rather than inventing a new code.
    let (code, fallback) = match e.code {
        ApiErrorCode::Unavailable => (
            ErrorCode::ProxyEngineApiUnavailable,
            "proxy engine API is unavailable",
        ),
        ApiErrorCode::AlreadyExists | ApiErrorCode::NotFound => (
            ErrorCode::ProxyEngineApiUnavailable,
            "proxy engine API state could not be reconciled",
        ),
        ApiErrorCode::Invalid => (
            ErrorCode::ProxyConfigInvalid,
            "proxy engine rejected the configuration as invalid",
        ),
    };
    let message = if e.message.is_empty() {
        fallback.to_owned()
    } else {
        format!("{fallback}: {}", e.message)
    };
    CoreError::validation(code, message)
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Map;
    use std::sync::Mutex;
    #[derive(Default)]
    struct Fake {
        ins: Mutex<Vec<Inbound>>,
        adds: Mutex<usize>,
        rems: Mutex<usize>,
    }
    impl XrayApi for Fake {
        fn list_inbounds(&self) -> Result<Vec<Inbound>, ApiError> {
            Ok(self.ins.lock().unwrap().clone())
        }
        fn list_outbounds(&self) -> Result<Vec<Outbound>, ApiError> {
            Ok(vec![])
        }
        fn list_rules(&self) -> Result<Vec<Rule>, ApiError> {
            Ok(vec![])
        }
        fn add_inbound(&self, x: Inbound) -> Result<(), ApiError> {
            *self.adds.lock().unwrap() += 1;
            self.ins.lock().unwrap().push(x);
            Ok(())
        }
        fn remove_inbound(&self, t: &str) -> Result<(), ApiError> {
            *self.rems.lock().unwrap() += 1;
            self.ins.lock().unwrap().retain(|x| x.tag != t);
            Ok(())
        }
        fn add_outbound(&self, _: Outbound) -> Result<(), ApiError> {
            Ok(())
        }
        fn remove_outbound(&self, _: &str) -> Result<(), ApiError> {
            Ok(())
        }
        fn add_rule(&self, _: Rule, a: bool) -> Result<(), ApiError> {
            assert!(a);
            Ok(())
        }
        fn remove_rule(&self, _: &str) -> Result<(), ApiError> {
            Ok(())
        }
    }
    fn i() -> Inbound {
        Inbound {
            tag: "rt:a:in".into(),
            listen: "127.0.0.1".into(),
            port: 40000,
            protocol: "http".into(),
            settings: Value::Object(Map::new()),
        }
    }
    #[test]
    fn idempotent() {
        let a = Fake::default();
        ensure_inbound(&a, i()).unwrap();
        ensure_inbound(&a, i()).unwrap();
        assert_eq!(*a.adds.lock().unwrap(), 1);
        remove_inbound(&a, "rt:a:in").unwrap();
        remove_inbound(&a, "rt:a:in").unwrap();
        assert_eq!(*a.rems.lock().unwrap(), 1);
        remove_inbound(&a, "unknown").unwrap()
    }
    #[test]
    fn loopback_only() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        assert_eq!(
            rt.block_on(GrpcXrayApi::connect("192.0.2.1:1080".parse().unwrap()))
                .err()
                .unwrap()
                .code,
            ApiErrorCode::Invalid
        )
    }
}

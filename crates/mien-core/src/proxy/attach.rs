//! Production [`ProxyAttach`] over the shared Xray engine.
//!
//! The engine owns one Xray process per host. Each supervised runtime gets its
//! own loopback HTTP inbound and route rule, while the outbound is shared by
//! `outbound_key`. Credentials stay inside the in-memory `OutboundSpec` and the
//! gRPC request body; nothing is written to disk or argv.

use std::net::TcpListener;
use std::sync::Arc;

use crate::error::{CoreError, ErrorCode};
use crate::runtime::{ProxyAttach, ProxyAttachment};

use super::api::XrayApi;
use super::engine::{EngineController, RuntimeBinding};
use super::keys::OutboundSpec;

/// Adapter that binds one runtime to the shared engine.
pub struct GatewayAttach<A: XrayApi> {
    controller: Arc<EngineController<A>>,
    spec: OutboundSpec,
}

impl<A: XrayApi> GatewayAttach<A> {
    /// Creates the adapter for one proxy configuration.
    pub fn new(controller: Arc<EngineController<A>>, spec: OutboundSpec) -> Self {
        Self { controller, spec }
    }

    /// The shared outbound key the adapter reuses.
    pub fn outbound_key(&self) -> &super::keys::OutboundKey {
        &self.spec.key
    }
}

impl<A: XrayApi + 'static> ProxyAttach for GatewayAttach<A> {
    fn attach(&self, runtime_id: &str) -> Result<ProxyAttachment, CoreError> {
        let port = free_loopback_port()?;
        let binding = RuntimeBinding {
            runtime_id: runtime_id.to_owned(),
            outbound_key: self.spec.key.clone(),
            inbound_tag: super::gateway::inbound_tag(runtime_id),
            rule_tag: super::gateway::rule_tag(runtime_id),
            proxy_port: port,
        };
        self.controller.acquire(binding, &self.spec)?;
        Ok(ProxyAttachment {
            port,
            outbound_key: runtime_id.to_owned(),
        })
    }

    fn detach(&self, runtime_id: &str) -> Result<(), CoreError> {
        self.controller.release(runtime_id).map(|_| ())
    }
}

fn free_loopback_port() -> Result<u16, CoreError> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|_| {
        CoreError::validation(
            ErrorCode::ProxyInboundConflict,
            "no loopback port available",
        )
    })?;
    listener
        .local_addr()
        .map(|address| address.port())
        .map_err(|_| CoreError::validation(ErrorCode::ProxyInboundConflict, "no loopback port"))
}

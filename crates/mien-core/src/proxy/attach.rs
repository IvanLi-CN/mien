//! Production [`ProxyAttach`] over the shared Xray engine.
//!
//! The engine owns one Xray process per host. Each supervised runtime gets its
//! own loopback HTTP inbound and route rule, while the outbound is shared by
//! `outbound_key`. Credentials stay inside the in-memory `OutboundSpec` and the
//! gRPC request body; nothing is written to disk or argv.
//!
//! A host may register the engine binary so a crash can be rebuilt on the same
//! API port with bounded backoff (REQ-PROXY-022). Which runtimes to tear down
//! when the window is exhausted is the caller's decision: it holds the failure
//! listener.

use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use crate::error::{CoreError, ErrorCode};
use crate::runtime::{ProxyAttach, ProxyAttachment};

use super::api::XrayApi;
use super::engine::{EngineBinary, EngineController, EngineProcess, RuntimeBinding};
use super::keys::OutboundSpec;

/// Adapter that binds one runtime to the shared engine.
pub struct GatewayAttach<A: XrayApi> {
    controller: Arc<EngineController<A>>,
    spec: OutboundSpec,
    /// Runtimes to tear down once a rebuild window is exhausted.
    failures: Mutex<Vec<String>>,
    /// Rebuild serialization: one recovery at a time per controller.
    recovery: tokio::sync::Mutex<()>,
}

impl<A: XrayApi> GatewayAttach<A> {
    /// Creates the adapter for one proxy configuration.
    pub fn new(controller: Arc<EngineController<A>>, spec: OutboundSpec) -> Self {
        Self {
            controller,
            spec,
            failures: Mutex::new(Vec::new()),
            recovery: tokio::sync::Mutex::new(()),
        }
    }

    /// The shared outbound key the adapter reuses.
    pub fn outbound_key(&self) -> &super::keys::OutboundKey {
        &self.spec.key
    }

    /// Registers the binary every rebuild spawns, and the API port to keep.
    pub fn register_engine(&self, binary: EngineBinary) {
        self.controller.register_binary(binary);
    }

    /// Tells the controller which runtime owns a freshly started engine.
    pub async fn adopt_holder(&self, process: EngineProcess, runtime_id: &str) {
        self.controller.adopt_engine(process, runtime_id).await;
    }

    /// Takes over an engine started for this runtime, unless one is already live.
    pub async fn claim_slot(&self, runtime_id: &str) {
        if let Some(process) = self.controller.detach_engine(runtime_id).await {
            self.adopt_holder(process, runtime_id).await;
        }
    }

    /// Runtimes the last exhausted rebuild asked to fail.
    pub fn take_failed(&self) -> Vec<String> {
        std::mem::take(&mut *self.failures.lock().expect("failures lock"))
    }

    /// Handles a live-engine crash: fail closed, then rebuild within the bounded
    /// window on the original ports. A successful rebuild leaves bound kernels
    /// running; an exhausted window lists them in [`Self::take_failed`].
    pub async fn recover(&self) {
        let _guard = self.recovery.lock().await;
        let Some(mut crashed) = self.controller.crash().await else {
            return;
        };
        let _ = crashed.stop(std::time::Duration::from_secs(1)).await;
        if self.controller.rebuild().await.is_ok() {
            return;
        }
        let bound: Vec<String> = self
            .controller
            .bindings()
            .into_iter()
            .map(|b| b.runtime_id)
            .collect();
        self.failures.lock().expect("failures lock").extend(bound);
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

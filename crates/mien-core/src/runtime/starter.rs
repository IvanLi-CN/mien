//! Profile start use case.
//!
//! Wires the admitted inputs into one supervised launch: the persisted profile,
//! the deterministic fingerprint [`LaunchPlan`], and the shared proxy engine.
//! Fingerprint flags are the kernel contract; network controls are a separate
//! runtime/proxy contract and are surfaced as disclosures instead of being
//! silently dropped.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::domain::profile::{Profile, ProfileMode, ProfileStatus};
use crate::error::{CoreError, ErrorCode};
use crate::fingerprint::{CapabilityManifest, Enforcement, LaunchPlan, build_launch_argv};
use crate::proxy::gateway::proxy_server_arg;

use super::supervisor::{RuntimeOptions, RuntimeSnapshot, RuntimeSupervisor};

/// Proxy attachment returned by the shared engine for one runtime.
#[derive(Clone, PartialEq, Eq)]
pub struct ProxyAttachment {
    /// Loopback HTTP inbound port the kernel must use.
    pub port: u16,
    /// Stable key used to release the shared outbound.
    pub outbound_key: String,
}

impl std::fmt::Debug for ProxyAttachment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyAttachment")
            .field("port", &self.port)
            .field("outbound_key", &"<redacted>")
            .finish()
    }
}

/// Engine side of the start path.
///
/// `Send + Sync` because a start request is awaited on a host async runtime and
/// the attachment is borrowed across awaits.
pub trait ProxyAttach: Send + Sync {
    fn attach(&self, runtime_id: &str) -> Result<ProxyAttachment, CoreError>;
    fn detach(&self, runtime_id: &str) -> Result<(), CoreError>;
}

/// One start request. `proxy` is required when the profile has a proxy binding.
pub struct StartProfile<'a> {
    pub profile: &'a Profile,
    pub binary: PathBuf,
    pub binary_sha256: String,
    pub manifest: &'a CapabilityManifest,
    pub data_root: PathBuf,
    pub sandbox_kind: String,
    pub proxy: Option<&'a dyn ProxyAttach>,
    pub stop_timeout: Duration,
}

impl std::fmt::Debug for StartProfile<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StartProfile")
            .field("profile", &self.profile.id)
            .field("binary", &self.binary)
            .field("direct", &self.profile.proxy_id.is_none())
            .finish()
    }
}

/// Outcome of a successful start.
#[derive(Debug, Clone)]
pub struct StartedProfile {
    /// Supervised runtime snapshot.
    pub snapshot: RuntimeSnapshot,
    /// Deterministic launch plan the runtime used.
    pub launch: LaunchPlan,
    /// Proxy binding, when the profile is not direct.
    pub proxy: Option<ProxyAttachment>,
    /// Reduced-assurance reasons the UI/API must disclose.
    pub disclosures: Vec<String>,
}

impl StartProfile<'_> {
    /// Validate the profile, attach the proxy, and supervise the kernel.
    ///
    /// # Errors
    /// Non-active profiles, unsupported modes, unverified fingerprints, a
    /// missing proxy engine, or a kernel that fails to start.
    pub async fn start(
        &self,
        supervisor: &RuntimeSupervisor,
        runtime_id: &str,
    ) -> Result<StartedProfile, CoreError> {
        if self.profile.status != ProfileStatus::Active {
            return Err(CoreError::InvalidTransition);
        }
        if self.profile.mode != ProfileMode::Headful {
            return Err(CoreError::validation(
                ErrorCode::RuntimeModeUnsupported,
                "only headful mode is implemented for the macOS slice",
            ));
        }
        if self.profile.proxy_id.is_some() && self.proxy.is_none() {
            return Err(CoreError::validation(
                ErrorCode::ProxyEngineNotReady,
                "the profile requires a proxy engine binding",
            ));
        }

        let data_dir = profile_data_dir(&self.data_root, &self.profile.id.0);
        std::fs::create_dir_all(&data_dir).map_err(|_| CoreError::Storage)?;

        let mut launch = build_launch_argv(&self.profile.fingerprint, self.manifest, &data_dir)?;
        let mut disclosures = Vec::new();
        for (name, status) in [
            ("doh_mode", launch.network.doh_mode.status),
            ("ipv6_mode", launch.network.ipv6_mode.status),
            (
                "disable_non_proxied_udp",
                launch.network.disable_non_proxied_udp.status,
            ),
        ] {
            match status {
                Enforcement::Declared => {}
                Enforcement::Unverified => disclosures.push(format!(
                    "{name} is unverified; the runtime fails closed and reports reduced assurance"
                )),
                Enforcement::Unsupported => {
                    return Err(CoreError::validation(
                        ErrorCode::KernelUnsupported,
                        format!("{name} cannot be enforced on this kernel"),
                    ));
                }
            }
        }

        let proxy = match (self.profile.proxy_id.as_deref(), self.proxy) {
            (Some(_), Some(engine)) => Some(engine.attach(runtime_id)?),
            _ => None,
        };

        let mut options = RuntimeOptions::new(
            runtime_id,
            self.profile.id.clone(),
            self.binary.clone(),
            self.binary_sha256.clone(),
            data_dir,
        );
        options.manifest_sandbox_kind.clone_from(&self.sandbox_kind);
        options.mode = self.profile.mode;
        options.automation_api = self.profile.automation_api;
        options.stop_timeout = self.stop_timeout;
        options.launch_args.clone_from(&launch.argv);
        options.direct = proxy.is_none();
        options.proxy_ready = proxy.is_some();
        match &proxy {
            Some(attachment) => {
                options.proxy_port = Some(attachment.port);
                let proxy_arg = format!("--proxy-server={}", proxy_server_arg(attachment.port));
                options.launch_args.push(proxy_arg.clone());
                launch.argv.push(proxy_arg);
            }
            None => disclosures
                .push("direct mode: no proxy-level DNS or fail-closed guarantee".to_owned()),
        }

        match supervisor.start(options).await {
            Ok(snapshot) => Ok(StartedProfile {
                snapshot,
                launch,
                proxy,
                disclosures,
            }),
            Err(error) => {
                if let (Some(engine), Some(_)) = (self.proxy, &proxy) {
                    let _ = engine.detach(runtime_id);
                }
                Err(error)
            }
        }
    }

    /// Stop the runtime and release its proxy binding.
    ///
    /// # Errors
    /// Unknown runtime or an engine that fails to release the binding.
    pub async fn stop(
        &self,
        supervisor: &RuntimeSupervisor,
        runtime_id: &str,
    ) -> Result<RuntimeSnapshot, CoreError> {
        let snapshot = supervisor.stop(runtime_id).await?;
        if let Some(engine) = self.proxy {
            if self.profile.proxy_id.is_some() {
                engine.detach(runtime_id)?;
            }
        }
        Ok(snapshot)
    }
}

/// The profile's data root, confined under the configured base directory.
fn profile_data_dir(data_root: &Path, profile_id: &str) -> PathBuf {
    data_root.join("profiles").join(profile_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fingerprint::load_pinned_manifest;

    fn profile(proxy_id: Option<&str>) -> Profile {
        use crate::fingerprint::spec::{FingerprintSpec, Platform};
        Profile {
            id: crate::domain::profile::ProfileId::new(),
            name: "slice".into(),
            status: ProfileStatus::Active,
            fingerprint: FingerprintSpec::fresh(Platform::Macos).expect("fresh spec"),
            start_url: None,
            notes: None,
            kernel_version: "148".into(),
            mode: ProfileMode::Headful,
            automation_api: false,
            proxy_id: proxy_id.map(str::to_owned),
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            archived_at: None,
            deleted_at: None,
            deletion_error: None,
        }
    }

    #[test]
    fn proxy_binding_requires_an_engine() {
        let manifest = load_pinned_manifest().expect("manifest");
        let profile = profile(Some("proxy-1"));
        let request = StartProfile {
            profile: &profile,
            binary: "/nonexistent".into(),
            binary_sha256: String::new(),
            manifest: &manifest,
            data_root: std::env::temp_dir(),
            sandbox_kind: "seatbelt".into(),
            proxy: None,
            stop_timeout: Duration::from_secs(1),
        };
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let supervisor = RuntimeSupervisor::new(std::env::temp_dir());
        let error = runtime
            .block_on(request.start(&supervisor, "rt-1"))
            .expect_err("must fail closed");
        assert_eq!(error.code(), ErrorCode::ProxyEngineNotReady);
    }
}

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
use crate::fingerprint::{CapabilityManifest, Enforcement, LaunchPlan, build_launch_argv_for_mode};
use crate::proxy::gateway::proxy_server_arg;

use super::state::RuntimeState;
use super::supervisor::{RestartPolicy, RuntimeOptions, RuntimeSnapshot, RuntimeSupervisor};

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
    /// 崩溃后的有界重启策略；`RestartPolicy::none()` 是最保守的选择。
    pub restart: RestartPolicy,
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
    /// Runtime mode actually requested; never silently rewritten.
    pub mode: ProfileMode,
    /// UA scope the launch may claim; only `pseudo_headful` claims `chrome`.
    pub ua_scope: Option<&'static str>,
    /// Whether the response must disclose reduced assurance.
    pub reduced_assurance: bool,
    /// Proxy binding, when the profile is not direct.
    pub proxy: Option<ProxyAttachment>,
    /// Reduced-assurance reasons the UI/API must disclose.
    pub disclosures: Vec<String>,
}

impl StartProfile<'_> {
    /// Validate the profile, attach the proxy, and supervise the kernel.
    ///
    /// # Errors
    /// Non-active profiles, a mode the kernel manifest cannot support
    /// (`RUNTIME_MODE_UNSUPPORTED`), unverified fingerprints, a missing proxy
    /// engine, or a kernel that fails to start.
    pub async fn start(
        &self,
        supervisor: &RuntimeSupervisor,
        runtime_id: &str,
    ) -> Result<StartedProfile, CoreError> {
        if self.profile.status != ProfileStatus::Active {
            return Err(CoreError::InvalidTransition);
        }
        if self.profile.proxy_id.is_some() && self.proxy.is_none() {
            return Err(CoreError::validation(
                ErrorCode::ProxyEngineNotReady,
                "the profile requires a proxy engine binding",
            ));
        }

        let data_dir = profile_data_dir(&self.data_root, &self.profile.id.0);
        std::fs::create_dir_all(&data_dir).map_err(|_| CoreError::Storage)?;

        // Mode resolution runs before any process or proxy resource exists: an
        // unsupported mode must not leave a half-built runtime behind.
        let mut launch = build_launch_argv_for_mode(
            &self.profile.fingerprint,
            self.manifest,
            &data_dir,
            self.profile.mode,
        )?;
        let mut disclosures = Vec::new();
        if let Some(limitation) = launch.limitation {
            disclosures.push(limitation.to_owned());
        }
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
        options.headless_chrome_ua_patch = self.manifest.headless_chrome_ua_patch;
        options.automation_api = self.profile.automation_api;
        options.stop_timeout = self.stop_timeout;
        options.launch_args.clone_from(&launch.argv);
        options.restart = self.restart.clone();
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
            Ok(snapshot) => {
                let mode = launch.mode;
                let ua_scope = launch.ua_scope;
                let reduced_assurance = snapshot.reduced_assurance;
                Ok(StartedProfile {
                    snapshot,
                    launch,
                    mode,
                    ua_scope,
                    reduced_assurance,
                    proxy,
                    disclosures,
                })
            }
            Err(error) => {
                if let (Some(engine), Some(_)) = (self.proxy, &proxy) {
                    let _ = engine.detach(runtime_id);
                }
                Err(error)
            }
        }
    }

    /// Tear down a runtime whose shared engine rebuild failed (REQ-RUNTIME-019);
    /// a direct runtime is untouched. Runs in three phases so the state machine
    /// reaches `stopping` before the proxy resources are dropped and only writes
    /// `failed` after the process group is confirmed gone.
    ///
    /// # Errors
    /// The teardown could not confirm that the kernel exited.
    pub async fn engine_teardown(
        &self,
        supervisor: &RuntimeSupervisor,
        runtime_id: &str,
    ) -> Result<RuntimeSnapshot, CoreError> {
        let record = supervisor.record(runtime_id).ok_or(CoreError::NotFound)?;
        let Some(engine) = self.proxy.filter(|_| self.profile.proxy_id.is_some()) else {
            return supervisor.stop(runtime_id).await;
        };
        match record.persistable_state() {
            RuntimeState::Stopped | RuntimeState::Failed => return Ok(record.snapshot()),
            RuntimeState::Crashed => {
                // Repeated crashes still sit on this same runtime id.
                record
                    .transition_check(RuntimeState::Starting, None, None)
                    .or_else(|_| record.transition_check(RuntimeState::Stopping, None, None))?;
            }
            _ => {}
        }
        if record.persistable_state() != RuntimeState::Stopping {
            record.mark_stop_requested();
            record.transition_check(RuntimeState::Stopping, None, None)?;
        } else {
            record.mark_stop_requested();
        }
        // Phase 1: mark `stopping` so a control-plane kill here re-reconciles
        // the row instead of trusting a `running` claim.
        record.persist_state(RuntimeState::Stopping);
        // Phase 2: drop the proxy binding before the process group dies. A stop
        // racing this teardown may have released it already, which is fine.
        let _ = engine.detach(runtime_id);
        record.stop_process(self.stop_timeout).await?;
        record.transition_check(
            RuntimeState::Failed,
            Some(ErrorCode::ProxyEngineCrashed.as_str().to_owned()),
            Some("shared proxy engine rebuild window exhausted".into()),
        )?;
        record.release_permit();
        Ok(record.snapshot())
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
            restart: RestartPolicy::none(),
        };
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let supervisor = RuntimeSupervisor::new(std::env::temp_dir());
        let error = runtime
            .block_on(request.start(&supervisor, "rt-1"))
            .expect_err("must fail closed");
        assert_eq!(error.code(), ErrorCode::ProxyEngineNotReady);
    }
}

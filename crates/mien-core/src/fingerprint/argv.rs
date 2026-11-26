//! Deterministic, typed Chromium launch argument construction.

use std::path::Path;

use crate::{
    domain::profile::ProfileMode,
    error::{CoreError, ErrorCode},
};

use super::capability::{self, CapabilityManifest};
use super::spec::{FingerprintSpec, Network, ValidatedSpec, Window, validate_spec_with_manifest};

/// Runtime/proxy network controls preserved beside browser argv.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkControls {
    pub doh_mode: NetworkControl<String>,
    pub ipv6_mode: NetworkControl<String>,
    pub disable_non_proxied_udp: NetworkControl<bool>,
}

/// Network enforcement value and evidence status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkControl<T> {
    pub value: T,
    pub status: Enforcement,
}

/// Evidence level available to the launch/runtime consumer.
///
/// `Declared` is verified by the capability manifest. `Unverified` requires
/// runtime fail-closed behavior and disclosure. `Unsupported` means execution
/// cannot be guaranteed and must be rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enforcement {
    Declared,
    Unverified,
    Unsupported,
}

/// Complete launch plan. `argv` is safe to pass directly to a process API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    pub argv: Vec<String>,
    pub window: Window,
    pub network: NetworkControls,
    /// Runtime mode this argv was built for.
    pub mode: ProfileMode,
    /// UA scope the launch may claim; only `pseudo_headful` claims `chrome`.
    pub ua_scope: Option<&'static str>,
    /// Guarantee limitation the start response must disclose, when the mode has one.
    pub limitation: Option<&'static str>,
}

/// The kernel argument shared by every Headless mode.
pub const HEADLESS_ARG: &str = "--headless=new";

/// `headless` promises no user-agent result at all.
pub const HEADLESS_LIMITATION: &str = "headless makes no user-agent claim";

/// `pseudo_headful` buys the UA patch and nothing else.
pub const PSEUDO_HEADFUL_LIMITATION: &str =
    "pseudo_headful changes UA only; other Headless characteristics remain";

/// Mode-specific argv entries and the guarantee they carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModeArgs {
    /// Append [`HEADLESS_ARG`] to the fingerprint flags.
    pub headless: bool,
    /// UA scope the mode may claim.
    pub ua_scope: Option<&'static str>,
    /// Disclosure the start response must surface.
    pub limitation: Option<&'static str>,
}

impl ModeArgs {
    /// Resolve `mode` against the installed kernel capability flag.
    ///
    /// `pseudo_headful` requires the manifest to declare `headless_chrome_ua_patch=true`;
    /// without it the launch is refused rather than silently degraded to `headless` or
    /// `headful`. The UA string itself is never synthesised here: the patch is the
    /// kernel's claim, and passing a UA flag would forge it.
    pub fn resolve(mode: ProfileMode, headless_chrome_ua_patch: bool) -> Result<Self, CoreError> {
        Ok(match mode {
            ProfileMode::Headful => Self {
                headless: false,
                ua_scope: None,
                limitation: None,
            },
            ProfileMode::Headless => Self {
                headless: true,
                ua_scope: None,
                limitation: Some(HEADLESS_LIMITATION),
            },
            ProfileMode::PseudoHeadful => {
                if !headless_chrome_ua_patch {
                    return Err(CoreError::validation(
                        ErrorCode::RuntimeModeUnsupported,
                        "pseudo_headful requires the kernel manifest to declare headless_chrome_ua_patch=true",
                    ));
                }
                Self {
                    headless: true,
                    ua_scope: Some("chrome"),
                    limitation: Some(PSEUDO_HEADFUL_LIMITATION),
                }
            }
        })
    }
}

/// Build deterministic Chromium argv from a spec, manifest, and data directory.
///
/// The array order is profile directory, seed, platform fields, brand fields,
/// hardware, locale, timezone, window, then merged spoof toggles. Network
/// controls and credentials are excluded from argv; the caller passes the array
/// directly to a process API without shell interpretation. `window` describes
/// browser window geometry only; it makes no `screen.*` claim. Pseudo-headful
/// mode likewise is not a headful-equivalence claim.
pub fn build_launch_argv(
    spec: &FingerprintSpec,
    manifest: &CapabilityManifest,
    data_dir: &Path,
) -> Result<LaunchPlan, CoreError> {
    build_launch_argv_for_mode(spec, manifest, data_dir, ProfileMode::Headful)
}

/// Mode-aware [`build_launch_argv`].
///
/// The fingerprint flags keep their fixed order; the mode contributes at most one
/// trailing [`HEADLESS_ARG`], so headful argv is byte-identical to [`build_launch_argv`].
///
/// # Errors
/// Invalid/admitted spec failures, or `RUNTIME_MODE_UNSUPPORTED` when `pseudo_headful`
/// runs against a kernel without the `headless_chrome_ua_patch` capability.
pub fn build_launch_argv_for_mode(
    spec: &FingerprintSpec,
    manifest: &CapabilityManifest,
    data_dir: &Path,
    mode: ProfileMode,
) -> Result<LaunchPlan, CoreError> {
    let mode_args = ModeArgs::resolve(mode, manifest.headless_chrome_ua_patch)?;
    let validated = validate_spec_with_manifest(spec, manifest)?;
    let mut plan = build_from_validated(&validated, manifest, data_dir)?;
    if mode_args.headless {
        plan.argv.push(HEADLESS_ARG.to_owned());
    }
    plan.mode = mode;
    plan.ua_scope = mode_args.ua_scope;
    plan.limitation = mode_args.limitation;
    Ok(plan)
}

fn build_from_validated(
    validated: &ValidatedSpec,
    manifest: &CapabilityManifest,
    data_dir: &Path,
) -> Result<LaunchPlan, CoreError> {
    let data_dir = data_dir
        .to_str()
        .ok_or_else(|| capability::invalid("data directory is not valid UTF-8"))?;
    if data_dir.is_empty() || data_dir.contains('\0') {
        return Err(capability::invalid("data directory is invalid"));
    }
    let spec = &validated.explicit;
    let mut argv = Vec::with_capacity(13);
    argv.push(format!("--user-data-dir={data_dir}"));
    push_flag(
        &mut argv,
        manifest,
        "fingerprint",
        &spec.seed.to_string(),
        spec,
    )?;
    push_flag(
        &mut argv,
        manifest,
        "fingerprint-platform",
        spec.platform.as_str(),
        spec,
    )?;
    push_flag(
        &mut argv,
        manifest,
        "fingerprint-platform-version",
        &spec.platform_version,
        spec,
    )?;
    push_flag(&mut argv, manifest, "fingerprint-brand", &spec.brand, spec)?;
    push_flag(
        &mut argv,
        manifest,
        "fingerprint-brand-version",
        &spec.brand_version,
        spec,
    )?;
    push_flag(
        &mut argv,
        manifest,
        "fingerprint-hardware-concurrency",
        &spec.hardware_concurrency.to_string(),
        spec,
    )?;
    push_flag(&mut argv, manifest, "lang", &spec.languages[0], spec)?;
    push_flag(
        &mut argv,
        manifest,
        "accept-lang",
        &spec.languages.join(","),
        spec,
    )?;
    push_flag(&mut argv, manifest, "timezone", &spec.timezone, spec)?;
    push_flag(
        &mut argv,
        manifest,
        "window-size",
        &format!("{},{}", spec.window.width, spec.window.height),
        spec,
    )?;

    let mut disabled = Vec::new();
    if let Some(spoof) = &spec.spoof {
        for (category, enabled) in [
            ("font", spoof.font),
            ("audio", spoof.audio),
            ("canvas", spoof.canvas),
            ("clientrects", spoof.clientrects),
            ("gpu", spoof.gpu),
        ] {
            if enabled == Some(false) {
                disabled.push(category);
            }
        }
    }
    if !disabled.is_empty() {
        argv.push(format!("--disable-spoofing={}", disabled.join(",")));
    }

    Ok(LaunchPlan {
        argv,
        window: spec.window,
        network: NetworkControls::from_spec(&spec.network, spec.webrtc.disable_non_proxied_udp),
        mode: ProfileMode::Headful,
        ua_scope: None,
        limitation: None,
    })
}

impl NetworkControls {
    fn from_spec(network: &Network, disable_non_proxied_udp: bool) -> Self {
        Self {
            doh_mode: NetworkControl {
                value: network.doh_mode.clone(),
                status: Enforcement::Declared,
            },
            ipv6_mode: NetworkControl {
                value: network.ipv6_mode.clone(),
                // 实测（docs/spikes/network）：pinned 148 没有任何 IPv6 阻断开关，
                // 直连下 IPv6 走真实出口。清单声明是期望，不是已执行的控制，
                // 因此只能是 Unverified；代理模式下的阻断由代理层负责。
                status: Enforcement::Unverified,
            },
            disable_non_proxied_udp: NetworkControl {
                value: disable_non_proxied_udp,
                status: Enforcement::Unverified,
            },
        }
    }
}

fn push_flag(
    argv: &mut Vec<String>,
    manifest: &CapabilityManifest,
    name: &str,
    value: &str,
    spec: &FingerprintSpec,
) -> Result<(), CoreError> {
    match name {
        "fingerprint" => manifest.flag(name).and_then(|flag| {
            if flag.value_type.as_deref() != Some("uint32") {
                return Err(capability::unsupported(format!(
                    "flag `{name}` has an incompatible type"
                )));
            }
            Ok(())
        })?,
        "fingerprint-hardware-concurrency" => {
            manifest.supports_integer(name, spec.hardware_concurrency, spec.platform)?
        }
        "window-size" => manifest.supports_window(spec.window, spec.platform)?,
        _ => manifest.supports_string(name, value, spec.platform)?,
    }
    argv.push(format!("--{name}={value}"));
    Ok(())
}

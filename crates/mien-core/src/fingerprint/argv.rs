//! Deterministic, typed Chromium launch argument construction.

use std::path::Path;

use crate::error::CoreError;

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
}

/// Build deterministic Chromium argv from a spec, manifest, and data directory.
pub fn build_launch_argv(
    spec: &FingerprintSpec,
    manifest: &CapabilityManifest,
    data_dir: &Path,
) -> Result<LaunchPlan, CoreError> {
    let validated = validate_spec_with_manifest(spec, manifest)?;
    build_from_validated(&validated, manifest, data_dir)
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
                status: Enforcement::Declared,
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

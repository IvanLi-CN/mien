//! Installed fingerprint-kernel capability manifest consumption.
//!
//! Fingerprint flags are emitted only when the manifest declares the flag and
//! the value is in its `flagSpec` allow-list. Network controls are represented
//! separately: their declaration is consumed by the runtime/proxy layer, not
//! translated into unverified Chromium argv flags.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::CoreError;

use super::spec::{Platform, Window};

const PINNED_MANIFEST: &str = include_str!(
    "../../../../docs/contracts/manifests/fingerprint-chromium-148.0.7778.215-macos-arm64.json"
);

/// One manifest flag specification (`flagSpec` in the contract).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlagSpec {
    #[serde(rename = "type")]
    pub value_type: Option<String>,
    #[serde(default)]
    pub values: Vec<Value>,
    #[serde(default)]
    pub per_platform_values: BTreeMap<String, Vec<Value>>,
    #[serde(default)]
    pub description: Option<String>,
}

/// Network controls declared by an installed kernel.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkCapabilities {
    pub doh_mode: NetworkDohMode,
    pub ipv6_mode: NetworkIpv6Mode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkDohMode {
    Disabled,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkIpv6Mode {
    Block,
    Unsupported,
}

/// Chromium kernel manifest used for capability admission.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityManifest {
    pub kernel_version: String,
    pub platform: String,
    pub sha256: String,
    pub flags: BTreeMap<String, FlagSpec>,
    pub seed_derived: Vec<String>,
    pub known_issues: Vec<String>,
    #[serde(default)]
    pub limitations: Vec<String>,
    pub sandbox_kind: String,
    pub network_controls: NetworkCapabilities,
    pub headless_chrome_ua_patch: bool,
}

impl CapabilityManifest {
    /// Parse and validate a JSON capability manifest.
    pub fn from_json_str(text: &str) -> Result<Self, CoreError> {
        let manifest: Self = serde_json::from_str(text)
            .map_err(|error| CoreError::Validation(format!("KERNEL_MANIFEST_INVALID: {error}")))?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Read and validate a JSON capability manifest from disk.
    pub fn from_path(path: &Path) -> Result<Self, CoreError> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| CoreError::Validation(format!("KERNEL_MANIFEST_INVALID: {error}")))?;
        Self::from_json_str(&text)
    }

    /// Load the pinned macOS ARM64 manifest shipped with this source tree.
    pub fn pinned() -> Result<Self, CoreError> {
        Self::from_json_str(PINNED_MANIFEST)
    }

    /// Validate manifest invariants required by fingerprint admission.
    pub fn validate(&self) -> Result<(), CoreError> {
        if !self.flags.contains_key("window-size") {
            return Err(manifest_error("window-size flag is required"));
        }
        let window = self.flags.get("window-size").expect("checked above");
        if window.value_type.as_deref() != Some("string") {
            return Err(manifest_error("window-size must have string type"));
        }
        if window.values.is_empty() && window.per_platform_values.is_empty() {
            return Err(manifest_error("window-size must declare candidates"));
        }
        if !window.per_platform_values.is_empty() {
            for platform in ["windows", "macos", "linux"] {
                if window
                    .per_platform_values
                    .get(platform)
                    .is_none_or(Vec::is_empty)
                {
                    return Err(manifest_error(
                        "window-size per_platform_values must cover every platform",
                    ));
                }
            }
        }

        let hardware = self
            .flags
            .get("fingerprint-hardware-concurrency")
            .ok_or_else(|| manifest_error("fingerprint-hardware-concurrency flag is required"))?;
        if hardware.values.is_empty() && hardware.per_platform_values.is_empty() {
            return Err(manifest_error(
                "fingerprint-hardware-concurrency must declare candidates",
            ));
        }
        if !hardware.per_platform_values.is_empty() {
            for platform in ["windows", "macos", "linux"] {
                if hardware
                    .per_platform_values
                    .get(platform)
                    .is_none_or(Vec::is_empty)
                {
                    return Err(manifest_error(
                        "hardware per_platform_values must cover every platform",
                    ));
                }
            }
        }
        Ok(())
    }

    /// Return the flag specification, or a stable unsupported-capability error.
    pub fn flag(&self, name: &str) -> Result<&FlagSpec, CoreError> {
        self.flags
            .get(name)
            .ok_or_else(|| unsupported(format!("flag `{name}` is not declared")))
    }

    /// Return values authoritative for the selected persona platform.
    pub fn values_for<'a>(&'a self, flag: &'a FlagSpec, platform: Platform) -> &'a [Value] {
        flag.per_platform_values
            .get(platform.as_str())
            .map(Vec::as_slice)
            .unwrap_or(flag.values.as_slice())
    }

    /// Check a string flag against its declared type and candidates.
    pub fn supports_string(
        &self,
        name: &str,
        value: &str,
        platform: Platform,
    ) -> Result<(), CoreError> {
        let flag = self.flag(name)?;
        if flag
            .value_type
            .as_deref()
            .is_some_and(|kind| kind != "string")
        {
            return Err(unsupported(format!("flag `{name}` is not a string flag")));
        }
        let candidates = self.values_for(flag, platform);
        if !candidates.is_empty()
            && !candidates
                .iter()
                .any(|candidate| candidate.as_str() == Some(value))
        {
            return Err(unsupported(format!(
                "flag `{name}` does not allow `{value}`"
            )));
        }
        Ok(())
    }

    /// Check an integer flag against its authoritative candidate set.
    pub fn supports_integer(
        &self,
        name: &str,
        value: u32,
        platform: Platform,
    ) -> Result<(), CoreError> {
        let flag = self.flag(name)?;
        if flag
            .value_type
            .as_deref()
            .is_some_and(|kind| kind != "integer")
        {
            return Err(unsupported(format!("flag `{name}` is not an integer flag")));
        }
        let candidates = self.values_for(flag, platform);
        if !candidates
            .iter()
            .any(|candidate| candidate.as_u64() == Some(u64::from(value)))
        {
            return Err(unsupported(format!(
                "flag `{name}` does not allow `{value}`"
            )));
        }
        Ok(())
    }

    /// Check a canonical window size against the manifest candidates.
    pub fn supports_window(&self, window: Window, platform: Platform) -> Result<(), CoreError> {
        let value = format!("{},{}", window.width, window.height);
        self.supports_string("window-size", &value, platform)
    }
}

/// Load the pinned macOS ARM64 manifest.
pub fn load_pinned_manifest() -> Result<CapabilityManifest, CoreError> {
    CapabilityManifest::pinned()
}

fn manifest_error(detail: impl Into<String>) -> CoreError {
    CoreError::Validation(format!("KERNEL_MANIFEST_INVALID: {}", detail.into()))
}

pub(crate) fn unsupported(detail: impl Into<String>) -> CoreError {
    CoreError::Validation(format!("KERNEL_UNSUPPORTED: {}", detail.into()))
}

pub(crate) fn invalid(detail: impl Into<String>) -> CoreError {
    CoreError::Validation(format!("FINGERPRINT_INVALID: {}", detail.into()))
}

pub(crate) fn required(detail: impl Into<String>) -> CoreError {
    CoreError::Validation(format!("FINGERPRINT_REQUIRED_FIELD: {}", detail.into()))
}

pub(crate) fn schema(detail: impl Into<String>) -> CoreError {
    CoreError::Validation(format!("FINGERPRINT_SCHEMA_VERSION: {}", detail.into()))
}

pub(crate) fn unsafe_network(detail: impl Into<String>) -> CoreError {
    CoreError::Validation(format!("FINGERPRINT_NETWORK_UNSAFE: {}", detail.into()))
}

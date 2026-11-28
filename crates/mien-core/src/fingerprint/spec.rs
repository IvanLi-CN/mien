//! Versioned fingerprint specification and schema validation.

use std::fs::File;
use std::io::Read;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, ErrorCode};

use super::capability::{self, CapabilityManifest, NetworkDohMode, NetworkIpv6Mode};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Windows,
    Macos,
    Linux,
}

impl Platform {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Macos => "macos",
            Self::Linux => "linux",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Window {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spoof {
    pub font: Option<bool>,
    pub audio: Option<bool>,
    pub canvas: Option<bool>,
    pub clientrects: Option<bool>,
    pub gpu: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Webrtc {
    pub disable_non_proxied_udp: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Network {
    pub doh_mode: String,
    pub ipv6_mode: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FingerprintSpec {
    pub schema_version: u32,
    pub seed: u32,
    pub platform: Platform,
    pub platform_version: String,
    pub brand: String,
    pub brand_version: String,
    pub hardware_concurrency: u32,
    pub languages: Vec<String>,
    pub timezone: String,
    pub window: Window,
    #[serde(default)]
    pub spoof: Option<Spoof>,
    pub webrtc: Webrtc,
    pub network: Network,
}

impl FingerprintSpec {
    pub fn fresh(platform: Platform) -> Result<Self, CoreError> {
        let spec = Self {
            schema_version: SCHEMA_VERSION,
            seed: generate_seed()?,
            platform,
            platform_version: match platform {
                Platform::Windows => "10.0.0",
                Platform::Macos => "14.5",
                Platform::Linux => "6.8.0",
            }
            .to_owned(),
            brand: "Edge".to_owned(),
            brand_version: "148.0.0.0".to_owned(),
            hardware_concurrency: 12,
            languages: vec!["en-US".to_owned(), "en".to_owned()],
            timezone: "America/Los_Angeles".to_owned(),
            window: Window {
                width: 1280,
                height: 720,
            },
            spoof: None,
            webrtc: Webrtc {
                disable_non_proxied_udp: true,
            },
            network: Network {
                doh_mode: "disabled".to_owned(),
                ipv6_mode: "block".to_owned(),
            },
        };
        validate_spec(&spec)?;
        Ok(spec)
    }

    /// Duplicate every explicit field and draw a seed that differs from the source.
    ///
    /// Derived traits are never stored on [`FingerprintSpec`]; consumers obtain
    /// them only through [`derive_traits`] on the copy's new seed, so a duplicate
    /// cannot inherit the source's canvas/audio/WebGL/memory/GPU values. The copy
    /// is validated against the pinned manifest before it is returned, so a
    /// duplicate is admissible by construction.
    pub fn duplicate(&self) -> Result<Self, CoreError> {
        let mut copy = self.clone();
        loop {
            copy.seed = generate_seed()?;
            if copy.seed != self.seed {
                break;
            }
        }
        validate_spec(&copy)?;
        Ok(copy)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DerivedTraits {
    pub canvas: u64,
    pub audio: u64,
    pub webgl: u64,
    pub device_memory: u8,
    pub gpu_model: u8,
}

#[cfg(unix)]
pub fn generate_seed() -> Result<u32, CoreError> {
    let mut source = File::open("/dev/urandom").map_err(|error| {
        CoreError::validation(
            ErrorCode::FingerprintInvalid,
            format!("unable to read OS randomness: {error}"),
        )
    })?;
    loop {
        let mut bytes = [0_u8; 4];
        source.read_exact(&mut bytes).map_err(|error| {
            CoreError::validation(
                ErrorCode::FingerprintInvalid,
                format!("unable to read OS randomness: {error}"),
            )
        })?;
        let seed = u32::from_ne_bytes(bytes);
        if seed != 0 {
            return Ok(seed);
        }
    }
}

#[cfg(not(unix))]
pub fn generate_seed() -> Result<u32, CoreError> {
    Err(CoreError::validation(
        ErrorCode::FingerprintInvalid,
        "operating-system CSPRNG unavailable",
    ))
}

pub fn validate_spec(spec: &FingerprintSpec) -> Result<(), CoreError> {
    let manifest = CapabilityManifest::pinned()?;
    validate_spec_with_manifest(spec, &manifest).map(|_| ())
}

pub fn validate_spec_with_manifest(
    spec: &FingerprintSpec,
    manifest: &CapabilityManifest,
) -> Result<ValidatedSpec, CoreError> {
    manifest.validate()?;
    if spec.schema_version != SCHEMA_VERSION {
        return Err(capability::schema(format!(
            "expected {}, got {}",
            SCHEMA_VERSION, spec.schema_version
        )));
    }
    if spec.seed == 0 {
        return Err(capability::invalid("seed must be non-zero"));
    }
    if !valid_version(&spec.platform_version) {
        return Err(capability::invalid("platform_version has invalid format"));
    }
    if spec.brand.is_empty() || !valid_version(&spec.brand_version) {
        return Err(capability::invalid(
            "brand or brand_version has invalid format",
        ));
    }
    if spec.languages.is_empty() {
        return Err(capability::required("languages is required"));
    }
    if spec
        .languages
        .iter()
        .any(|language| !valid_language(language))
    {
        return Err(capability::invalid("languages contains an invalid value"));
    }
    if spec
        .languages
        .iter()
        .enumerate()
        .any(|(index, language)| spec.languages[..index].contains(language))
    {
        return Err(capability::invalid("languages must not contain duplicates"));
    }
    if spec.timezone.is_empty() || !valid_timezone(&spec.timezone) {
        return Err(capability::invalid("timezone has invalid format"));
    }
    if !(320..=7680).contains(&spec.window.width) || !(240..=4320).contains(&spec.window.height) {
        return Err(capability::invalid(
            "window dimensions are outside the allowed range",
        ));
    }

    manifest.supports_string(
        "fingerprint-platform",
        spec.platform.as_str(),
        spec.platform,
    )?;
    manifest.supports_string(
        "fingerprint-platform-version",
        &spec.platform_version,
        spec.platform,
    )?;
    manifest.supports_string("fingerprint-brand", &spec.brand, spec.platform)?;
    let kernel_major = manifest
        .kernel_version
        .split('.')
        .next()
        .and_then(|part| part.parse::<u32>().ok())
        .ok_or_else(|| capability::invalid("manifest kernel version is invalid"))?;
    let brand_major = spec
        .brand_version
        .split('.')
        .next()
        .and_then(|part| part.parse::<u32>().ok())
        .ok_or_else(|| capability::invalid("brand_version has invalid format"))?;
    if brand_major > kernel_major {
        return Err(capability::invalid(
            "brand_version cannot exceed kernel major version",
        ));
    }
    manifest.supports_string(
        "fingerprint-brand-version",
        &spec.brand_version,
        spec.platform,
    )?;
    if spec.hardware_concurrency == 0 || spec.hardware_concurrency > 256 {
        return Err(capability::invalid(
            "hardware_concurrency must be between 1 and 256",
        ));
    }
    manifest.supports_integer(
        "fingerprint-hardware-concurrency",
        spec.hardware_concurrency,
        spec.platform,
    )?;
    manifest.supports_string("lang", &spec.languages[0], spec.platform)?;
    manifest.supports_string("accept-lang", &spec.languages.join(","), spec.platform)?;
    manifest.supports_string("timezone", &spec.timezone, spec.platform)?;
    manifest.supports_window(spec.window, spec.platform)?;

    if let Some(spoof) = &spec.spoof {
        let flag = manifest.flag("disable-spoofing")?;
        for (category, value) in [
            ("font", spoof.font),
            ("audio", spoof.audio),
            ("canvas", spoof.canvas),
            ("clientrects", spoof.clientrects),
            ("gpu", spoof.gpu),
        ] {
            if value.is_some()
                && !flag
                    .values
                    .iter()
                    .any(|candidate| candidate.as_str() == Some(category))
            {
                return Err(capability::unsupported(format!(
                    "spoof category `{category}` is not verified by the manifest"
                )));
            }
        }
    }
    if !spec.webrtc.disable_non_proxied_udp {
        return Err(capability::unsafe_network(
            "disable_non_proxied_udp=false requires audited confirmation",
        ));
    }
    if spec.network.doh_mode != "disabled" {
        return Err(capability::invalid("network.doh_mode must be disabled"));
    }
    if spec.network.ipv6_mode != "block" {
        return Err(capability::invalid("network.ipv6_mode must be block"));
    }
    if manifest.network_controls.doh_mode == NetworkDohMode::Unsupported
        || manifest.network_controls.ipv6_mode == NetworkIpv6Mode::Unsupported
    {
        return Err(capability::unsupported(
            "manifest cannot enforce required network controls",
        ));
    }
    Ok(ValidatedSpec {
        explicit: spec.clone(),
        derived: derive_traits(spec.seed),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedSpec {
    pub explicit: FingerprintSpec,
    pub derived: DerivedTraits,
}

pub fn derive_traits(seed: u32) -> DerivedTraits {
    let mut state = u64::from(seed);
    let canvas = splitmix64(&mut state);
    let audio = splitmix64(&mut state);
    let webgl = splitmix64(&mut state);
    let device_memory = [4, 8, 16, 32][(splitmix64(&mut state) % 4) as usize];
    let gpu_model = (splitmix64(&mut state) % 16) as u8;
    DerivedTraits {
        canvas,
        audio,
        webgl,
        device_memory,
        gpu_model,
    }
}

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut value = *state;
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}
fn valid_version(value: &str) -> bool {
    let parts: Vec<_> = value.split('.').collect();
    !parts.is_empty()
        && parts.len() <= 4
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}
fn valid_language(value: &str) -> bool {
    let mut parts = value.split('-');
    let Some(language) = parts.next() else {
        return false;
    };
    (2..=3).contains(&language.len())
        && language.bytes().all(|byte| byte.is_ascii_alphabetic())
        && parts.all(|part| {
            (2..=8).contains(&part.len()) && part.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
}
fn valid_timezone(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'+' | b'-' | b'/'))
        && !value.contains("//")
}

//! Fingerprint specification admission and launch planning.
//!
//! Fingerprint flags are the kernel contract. Network controls are a separate
//! runtime/proxy contract and remain explicit in [`LaunchPlan`] rather than
//! becoming guessed Chromium argv entries. Enforcement status is three-valued:
//! `Declared` means verified by the manifest, `Unverified` means the required
//! runtime control must fail closed and be disclosed, and `Unsupported` means
//! it cannot be performed and the launch is rejected.
//!
//! `window` affects browser window geometry only, never `screen.*`; pseudo-headful
//! mode adjusts UA only and does not claim headful equivalence.

pub mod argv;
pub mod capability;
#[cfg(test)]
mod golden_tests;
pub mod json;
pub mod spec;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod window_tests;

pub use argv::{Enforcement, LaunchPlan, NetworkControl, NetworkControls, build_launch_argv};
pub use capability::{CapabilityManifest, load_pinned_manifest};
pub use json::parse_spec_json;
pub use spec::{FingerprintSpec, validate_spec, validate_spec_with_manifest};

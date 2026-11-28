use super::capability::CapabilityManifest;
use super::spec::{
    FingerprintSpec, Network, Platform, Webrtc, Window, validate_spec_with_manifest,
};
use crate::error::ErrorCode;

#[test]
fn rejects_window_dimensions_outside_manifest_candidates() {
    let spec = FingerprintSpec {
        schema_version: 1,
        seed: 148001,
        platform: Platform::Macos,
        platform_version: "14.5".to_owned(),
        brand: "Edge".to_owned(),
        brand_version: "148.0.0.0".to_owned(),
        hardware_concurrency: 12,
        languages: vec!["en-US".to_owned()],
        timezone: "America/Los_Angeles".to_owned(),
        window: Window {
            width: 800,
            height: 600,
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
    let manifest = CapabilityManifest::pinned().expect("pinned manifest");
    let error = validate_spec_with_manifest(&spec, &manifest).unwrap_err();
    assert_eq!(error.code(), ErrorCode::KernelUnsupported);
}

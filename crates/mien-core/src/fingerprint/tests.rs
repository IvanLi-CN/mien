use super::argv::{Enforcement, build_launch_argv};
use super::capability::CapabilityManifest;
use super::spec::{
    FingerprintSpec, Network, Platform, Spoof, Webrtc, Window, derive_traits,
    validate_spec_with_manifest,
};

fn valid_spec() -> FingerprintSpec {
    FingerprintSpec {
        schema_version: 1,
        seed: 148001,
        platform: Platform::Macos,
        platform_version: "14.5".to_owned(),
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
    }
}
fn manifest() -> CapabilityManifest {
    CapabilityManifest::pinned().expect("pinned manifest")
}

#[test]
fn validates_verified_spec_and_keeps_derived_traits_separate() {
    let spec = valid_spec();
    let validated = validate_spec_with_manifest(&spec, &manifest()).expect("valid spec");
    assert_eq!(validated.explicit, spec);
    assert_ne!(validated.derived, derive_traits(spec.seed.wrapping_add(1)));
}

#[test]
fn rejects_schema_version_with_stable_code() {
    let mut spec = valid_spec();
    spec.schema_version = 2;
    let error = validate_spec_with_manifest(&spec, &manifest()).unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with("validation failed: FINGERPRINT_SCHEMA_VERSION:")
    );
}

#[test]
fn rejects_unverified_manifest_values() {
    for mutation in 0_u8..4 {
        let mut spec = valid_spec();
        match mutation {
            0 => spec.brand = "Chrome".to_owned(),
            1 => spec.brand = "Vivaldi".to_owned(),
            2 => spec.hardware_concurrency = 8,
            _ => {
                spec.spoof = Some(Spoof {
                    font: Some(false),
                    ..Spoof::default()
                })
            }
        }
        let error = validate_spec_with_manifest(&spec, &manifest()).unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("validation failed: KERNEL_UNSUPPORTED:")
        );
    }
    let mut spec = valid_spec();
    spec.spoof = Some(Spoof {
        clientrects: Some(false),
        ..Spoof::default()
    });
    assert!(
        validate_spec_with_manifest(&spec, &manifest())
            .unwrap_err()
            .to_string()
            .starts_with("validation failed: KERNEL_UNSUPPORTED:")
    );
}

#[test]
fn rejects_unsafe_network_settings_with_stable_codes() {
    let mut spec = valid_spec();
    spec.network.doh_mode = "auto".to_owned();
    assert!(
        validate_spec_with_manifest(&spec, &manifest())
            .unwrap_err()
            .to_string()
            .starts_with("validation failed: FINGERPRINT_INVALID:")
    );
    let mut spec = valid_spec();
    spec.network.ipv6_mode = "allow".to_owned();
    assert!(
        validate_spec_with_manifest(&spec, &manifest())
            .unwrap_err()
            .to_string()
            .starts_with("validation failed: FINGERPRINT_INVALID:")
    );
    let mut spec = valid_spec();
    spec.webrtc.disable_non_proxied_udp = false;
    assert!(
        validate_spec_with_manifest(&spec, &manifest())
            .unwrap_err()
            .to_string()
            .starts_with("validation failed: FINGERPRINT_NETWORK_UNSAFE:")
    );
}

#[test]
fn argv_is_deterministic_and_network_controls_are_explicit_but_not_flags() {
    let spec = valid_spec();
    let caps = manifest();
    let first =
        build_launch_argv(&spec, &caps, std::path::Path::new("/synthetic/profile")).expect("plan");
    for _ in 0..3 {
        let next = build_launch_argv(&spec, &caps, std::path::Path::new("/synthetic/profile"))
            .expect("plan");
        assert_eq!(next.argv, first.argv);
    }
    assert_eq!(
        first.argv,
        [
            "--user-data-dir=/synthetic/profile",
            "--fingerprint=148001",
            "--fingerprint-platform=macos",
            "--fingerprint-platform-version=14.5",
            "--fingerprint-brand=Edge",
            "--fingerprint-brand-version=148.0.0.0",
            "--fingerprint-hardware-concurrency=12",
            "--lang=en-US",
            "--accept-lang=en-US,en",
            "--timezone=America/Los_Angeles",
            "--window-size=1280,720",
        ]
    );
    assert_eq!(first.network.doh_mode.value, "disabled");
    assert_eq!(first.network.doh_mode.status, Enforcement::Declared);
    assert_eq!(first.network.ipv6_mode.value, "block");
    assert_eq!(first.network.ipv6_mode.status, Enforcement::Declared);
    assert!(first.network.disable_non_proxied_udp.value);
    assert_eq!(
        first.network.disable_non_proxied_udp.status,
        Enforcement::Unverified
    );
    assert_ne!(derive_traits(148001), derive_traits(148002));
}

#[test]
fn rejects_unknown_credentials_and_missing_window() {
    let mut value = serde_json::to_value(valid_spec()).expect("serialize");
    value["proxy_password"] = serde_json::json!("secret");
    assert!(
        serde_json::from_value::<FingerprintSpec>(value)
            .unwrap_err()
            .to_string()
            .contains("unknown field")
    );
    let mut value = serde_json::to_value(valid_spec()).expect("serialize");
    value.as_object_mut().unwrap().remove("window");
    assert!(
        serde_json::from_value::<FingerprintSpec>(value)
            .unwrap_err()
            .to_string()
            .contains("missing field `window`")
    );
}

#[test]
fn generated_seed_is_cryptographically_sourced_and_nonzero() {
    assert_ne!(super::spec::generate_seed().expect("system CSPRNG"), 0);
}

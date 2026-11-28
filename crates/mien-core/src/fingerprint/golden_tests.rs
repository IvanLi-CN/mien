use super::Enforcement;
use super::argv::build_launch_argv;
use super::capability::CapabilityManifest;
use super::json::parse_spec_json;
use super::spec::{FingerprintSpec, Spoof, derive_traits, validate_spec_with_manifest};
use crate::error::ErrorCode;
use serde::Deserialize;

#[derive(Deserialize)]
struct Golden {
    case: String,
    spec: FingerprintSpec,
    expected_argv: Vec<String>,
    expected_network: ExpectedNetwork,
}
#[derive(Deserialize)]
struct ExpectedNetwork {
    doh_mode: String,
    ipv6_mode: String,
    disable_non_proxied_udp: bool,
}
fn fixtures() -> Vec<Golden> {
    [
        include_str!("../../../../fixtures/fingerprint/golden-argv/macos-headful-default.json"),
        include_str!("../../../../fixtures/fingerprint/golden-argv/linux-spoof-toggles.json"),
    ]
    .into_iter()
    .map(|json| serde_json::from_str(json).expect("valid golden fixture"))
    .collect()
}
fn manifest() -> CapabilityManifest {
    CapabilityManifest::pinned().expect("pinned manifest")
}

#[test]
fn golden_argv_matches_exactly_and_builds_repeatably() {
    let caps = manifest();
    for golden in fixtures() {
        let plan = build_launch_argv(
            &golden.spec,
            &caps,
            std::path::Path::new("/synthetic/profile"),
        )
        .expect(&golden.case);
        assert_eq!(plan.argv, golden.expected_argv, "{}", golden.case);
        for _ in 0..3 {
            assert_eq!(
                build_launch_argv(
                    &golden.spec,
                    &caps,
                    std::path::Path::new("/synthetic/profile")
                )
                .unwrap()
                .argv,
                golden.expected_argv,
                "{}",
                golden.case
            );
        }
        assert_eq!(
            plan.network.doh_mode.value,
            golden.expected_network.doh_mode
        );
        assert_eq!(plan.network.doh_mode.status, Enforcement::Declared);
        assert_eq!(
            plan.network.ipv6_mode.value,
            golden.expected_network.ipv6_mode
        );
        // 没有任何 IPv6 开关，清单声明只是期望；实测见 docs/spikes/network。
        assert_eq!(plan.network.ipv6_mode.status, Enforcement::Unverified);
        assert_eq!(
            plan.network.disable_non_proxied_udp.value,
            golden.expected_network.disable_non_proxied_udp
        );
        assert_eq!(
            plan.network.disable_non_proxied_udp.status,
            Enforcement::Unverified
        );
        assert!(plan.argv.iter().all(|arg| !arg.contains("doh")
            && !arg.contains("ipv6")
            && !arg.contains("non-proxied-udp")));
    }
}

#[test]
fn rejects_each_negative_capability_case_with_stable_error_code() {
    let base = fixtures().remove(0).spec;
    let mut cases: Vec<(&str, FingerprintSpec)> = Vec::new();
    let mut spec = base.clone();
    spec.brand = "Chrome".to_owned();
    cases.push(("Chrome", spec));
    let mut spec = base.clone();
    spec.brand = "Vivaldi".to_owned();
    cases.push(("Vivaldi", spec));
    let mut spec = base.clone();
    spec.hardware_concurrency = 8;
    cases.push(("hardware-8", spec));
    let mut spec = base.clone();
    spec.spoof = Some(Spoof {
        font: Some(false),
        ..Spoof::default()
    });
    cases.push(("font", spec));
    let mut spec = base.clone();
    spec.spoof = Some(Spoof {
        clientrects: Some(false),
        ..Spoof::default()
    });
    cases.push(("clientrects", spec));
    for (name, spec) in cases {
        let error = validate_spec_with_manifest(&spec, &manifest()).unwrap_err();
        assert_eq!(
            error.code(),
            ErrorCode::KernelUnsupported,
            "{name}: {error}"
        );
    }
    let error = manifest().flag("proxy-server").unwrap_err();
    assert_eq!(error.code(), ErrorCode::KernelUnsupported);
    let mut spec = base.clone();
    spec.network.doh_mode = "auto".to_owned();
    assert_eq!(
        validate_spec_with_manifest(&spec, &manifest())
            .unwrap_err()
            .code(),
        ErrorCode::FingerprintInvalid
    );
    let mut spec = base.clone();
    spec.network.ipv6_mode = "allow".to_owned();
    assert_eq!(
        validate_spec_with_manifest(&spec, &manifest())
            .unwrap_err()
            .code(),
        ErrorCode::FingerprintInvalid
    );
    let mut spec = base;
    spec.webrtc.disable_non_proxied_udp = false;
    assert_eq!(
        validate_spec_with_manifest(&spec, &manifest())
            .unwrap_err()
            .code(),
        ErrorCode::FingerprintInvalid
    );
}

#[test]
fn missing_required_json_fields_and_unknown_credentials_have_stable_codes() {
    let missing: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../fixtures/fingerprint/golden-argv/macos-headful-default.json"
    ))
    .expect("golden json");
    let mut spec = missing["spec"].clone();
    spec.as_object_mut().unwrap().remove("window");
    let error = parse_spec_json(&spec.to_string()).unwrap_err();
    assert_eq!(error.code(), ErrorCode::FingerprintInvalid);
    let mut spec = missing["spec"].clone();
    spec["proxy_password"] = serde_json::json!("secret");
    let error = parse_spec_json(&spec.to_string()).unwrap_err();
    assert_eq!(error.code(), ErrorCode::FingerprintInvalid);
}

#[test]
fn seed_derivation_is_deterministic_and_changes_with_seed() {
    let original = derive_traits(148001);
    assert_eq!(original, derive_traits(148001));
    assert_ne!(original, derive_traits(148002));
}

#[test]
fn manifest_without_runtime_network_controls_is_rejected() {
    let mut json: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../docs/contracts/manifests/fingerprint-chromium-148.0.7778.215-macos-arm64.json"
    ))
    .expect("manifest json");
    json.as_object_mut().unwrap().remove("network_controls");
    let error = CapabilityManifest::from_json_str(&json.to_string()).unwrap_err();
    assert_eq!(error.code(), ErrorCode::KernelUnsupported);
}

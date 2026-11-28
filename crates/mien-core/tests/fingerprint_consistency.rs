//! Fingerprint consistency and seed contract evidence.
//!
//! Proves three contract claims as re-runnable evidence against the pinned
//! capability manifest: one stored profile reproduces byte-identical launch
//! argv across repeated launches, two profiles with different seeds differ
//! only in the seed flag, and a duplicate profile always regenerates its seed
//! instead of inheriting the source seed or its derived traits. All specs are
//! synthetic; no credentials, browser profiles, or kernel binaries are used.

use std::collections::BTreeSet;
use std::path::Path;

use mien_core::ErrorCode;
use mien_core::domain::profile::{CopyOptions, CreateProfile, ProfileMode};
use mien_core::fingerprint::spec::{
    FingerprintSpec, Network, Platform, Spoof, Webrtc, Window, derive_traits,
};
use mien_core::fingerprint::{
    Enforcement, build_launch_argv, load_pinned_manifest, validate_spec_with_manifest,
};
use mien_core::ports::repository::ProfileRepository;
use mien_core::storage::{open_and_migrate, sqlite::SqliteProfileRepository};
use serde::Deserialize;

const CONSISTENCY_FIXTURE: &str =
    include_str!("../../../fixtures/fingerprint/golden-argv/macos-headful-consistency.json");

/// Synthetic data directory; argv construction never touches the filesystem.
const PROFILE_DIR: &str = "/synthetic/profile";

fn spec(seed: u32) -> FingerprintSpec {
    FingerprintSpec {
        schema_version: 1,
        seed,
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

#[derive(Deserialize)]
struct ConsistencyGolden {
    case: String,
    spec: FingerprintSpec,
    /// One argv vector per launch; every entry must be byte-identical.
    expected_argv_runs: Vec<Vec<String>>,
    expected_network: ExpectedNetwork,
}

#[derive(Deserialize)]
struct ExpectedNetwork {
    doh_mode: ExpectedControl<String>,
    ipv6_mode: ExpectedControl<String>,
    disable_non_proxied_udp: ExpectedControl<bool>,
}

#[derive(Deserialize)]
struct ExpectedControl<T> {
    value: T,
    status: String,
}

fn status(value: &str) -> Enforcement {
    match value {
        "declared" => Enforcement::Declared,
        "unverified" => Enforcement::Unverified,
        "unsupported" => Enforcement::Unsupported,
        other => panic!("unknown enforcement status `{other}`"),
    }
}

#[test]
fn fresh_specs_never_repeat_a_seed_within_one_process() {
    let manifest = load_pinned_manifest().expect("pinned manifest");
    let mut seeds = BTreeSet::new();
    for draw in 0..16 {
        let fresh = FingerprintSpec::fresh(Platform::Macos).expect("fresh spec");
        assert_ne!(fresh.seed, 0, "draw {draw}: generate_seed must skip zero");
        assert!(
            seeds.insert(fresh.seed),
            "draw {draw}: fresh seed {} repeated within 16 draws",
            fresh.seed
        );
        validate_spec_with_manifest(&fresh, &manifest).expect("fresh spec is admissible");
    }
    assert_eq!(seeds.len(), 16);
}

#[test]
fn derive_traits_is_a_pure_function_of_the_seed() {
    let first = derive_traits(148_001);
    assert_eq!(first, derive_traits(148_001));
    for seed in [148_002, 420_042, 1, u32::MAX] {
        assert_ne!(
            first,
            derive_traits(seed),
            "seed {seed} produced the same derived traits as 148001"
        );
    }
}

#[test]
fn same_seed_different_explicit_fields_share_derived_traits() {
    let manifest = load_pinned_manifest().expect("pinned manifest");
    let base = spec(148_001);
    let derived = validate_spec_with_manifest(&base, &manifest)
        .expect("base spec")
        .derived;
    assert_eq!(derived, derive_traits(base.seed));

    let mut other_brand = base.clone();
    other_brand.brand = "Opera".to_owned();
    let mut other_window = base.clone();
    other_window.window = Window {
        width: 1024,
        height: 768,
    };
    let mut other_spoof = base.clone();
    other_spoof.spoof = Some(Spoof {
        audio: Some(false),
        canvas: Some(false),
        gpu: Some(false),
        ..Spoof::default()
    });

    for (name, candidate) in [
        ("brand", other_brand),
        ("window", other_window),
        ("spoof", other_spoof),
    ] {
        let validated = validate_spec_with_manifest(&candidate, &manifest)
            .unwrap_or_else(|error| panic!("{name} variant must validate: {error}"));
        assert_ne!(
            validated.explicit, base,
            "{name} must change explicit fields"
        );
        assert_eq!(
            validated.explicit.seed, base.seed,
            "{name} changed the seed"
        );
        assert_eq!(
            validated.derived, derived,
            "{name} changed derived traits, so they are not seed-only"
        );
    }
}

#[test]
fn zero_seed_is_rejected_as_fingerprint_invalid() {
    let manifest = load_pinned_manifest().expect("pinned manifest");
    let mut invalid = spec(148_001);
    invalid.seed = 0;
    let error = validate_spec_with_manifest(&invalid, &manifest).unwrap_err();
    assert_eq!(error.code(), ErrorCode::FingerprintInvalid, "{error}");
    assert!(
        error.to_string().contains("seed"),
        "error must name the seed field: {error}"
    );
}

#[test]
fn repeated_launches_of_one_profile_produce_byte_identical_argv() {
    let manifest = load_pinned_manifest().expect("pinned manifest");
    let profile = spec(148_001);
    let first = build_launch_argv(&profile, &manifest, Path::new(PROFILE_DIR)).expect("launch");
    let first_bytes = first.argv.join("\u{0}").into_bytes();

    for launch in 1..3 {
        let next = build_launch_argv(&profile, &manifest, Path::new(PROFILE_DIR))
            .unwrap_or_else(|error| panic!("launch {launch}: {error}"));
        assert_eq!(next.argv.len(), first.argv.len(), "launch {launch}");
        assert_eq!(
            next.argv.join("\u{0}").into_bytes(),
            first_bytes,
            "launch {launch} argv is not byte-identical"
        );
        assert_eq!(next.window, first.window, "launch {launch} window");
        assert_eq!(next.network, first.network, "launch {launch} network");
    }
    assert_eq!(first.window, profile.window);
    assert!(first.argv.iter().any(|arg| arg == "--fingerprint=148001"));
}

#[test]
fn different_seeds_change_only_the_fingerprint_flag_value() {
    let manifest = load_pinned_manifest().expect("pinned manifest");
    let low =
        build_launch_argv(&spec(148_001), &manifest, Path::new(PROFILE_DIR)).expect("launch plan");
    let high =
        build_launch_argv(&spec(148_002), &manifest, Path::new(PROFILE_DIR)).expect("launch plan");

    assert_eq!(low.argv.len(), high.argv.len());
    let seed_index = low
        .argv
        .iter()
        .position(|arg| arg == "--fingerprint=148001")
        .expect("seed flag present");
    let differing: Vec<usize> = low
        .argv
        .iter()
        .zip(&high.argv)
        .enumerate()
        .filter(|(_, (left, right))| left != right)
        .map(|(index, _)| index)
        .collect();
    assert_eq!(
        differing,
        vec![seed_index],
        "only the seed flag may differ between profiles"
    );
    assert_eq!(high.argv[seed_index], "--fingerprint=148002");
    assert_eq!(low.window, high.window);
    assert_eq!(low.network, high.network);
}

#[test]
fn duplicate_redraws_the_seed_and_drops_derived_traits() {
    let manifest = load_pinned_manifest().expect("pinned manifest");
    let source = spec(148_001);
    let copy = source.duplicate().expect("duplicate spec");

    assert_ne!(copy.seed, source.seed, "duplicate must redraw the seed");
    assert_ne!(copy.seed, 0);
    let mut explicit = copy.clone();
    explicit.seed = source.seed;
    assert_eq!(explicit, source, "every explicit field must be preserved");

    let validated = validate_spec_with_manifest(&copy, &manifest).expect("copy is admissible");
    assert_eq!(validated.derived, derive_traits(copy.seed));
    assert_ne!(
        validated.derived,
        derive_traits(source.seed),
        "duplicate inherited the source derived traits"
    );

    let second = copy.duplicate().expect("second duplicate");
    assert_ne!(second.seed, copy.seed);
    assert_ne!(second.seed, source.seed);
}

/// The production copy path must satisfy the same invariant as
/// [`FingerprintSpec::duplicate`], including the derived-trait reset.
#[test]
fn repository_copy_regenerates_the_seed_for_both_copy_modes() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let root = std::env::temp_dir().join(format!(
        "mien-fingerprint-copy-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).expect("temp root");
    let pool = open_and_migrate(&root.join("db.sqlite")).expect("migrated database");
    let repository = SqliteProfileRepository::new(pool, &root).expect("repository");

    runtime.block_on(async {
        for copy_fingerprint in [false, true] {
            let source = repository
                .create(CreateProfile {
                    name: format!("source-{copy_fingerprint}"),
                    fingerprint: FingerprintSpec::fresh(Platform::Macos).expect("fresh spec"),
                    start_url: None,
                    proxy_id: None,
                    notes: None,
                    kernel_version: "148".to_owned(),
                    mode: ProfileMode::Headful,
                    automation_api: false,
                })
                .await
                .expect("create source");
            let copy = repository
                .copy(
                    &source.id,
                    CopyOptions {
                        copy_fingerprint,
                        copy_proxy: false,
                    },
                )
                .await
                .expect("copy profile");

            assert_ne!(copy.id, source.id);
            assert_ne!(
                copy.fingerprint.seed, source.fingerprint.seed,
                "copy_fingerprint={copy_fingerprint} reused the source seed"
            );
            assert_eq!(
                copy.fingerprint.platform, source.fingerprint.platform,
                "copy_fingerprint={copy_fingerprint} changed the platform"
            );
            if copy_fingerprint {
                let mut explicit = copy.fingerprint.clone();
                explicit.seed = source.fingerprint.seed;
                assert_eq!(
                    explicit, source.fingerprint,
                    "copy_fingerprint=true must preserve every explicit field"
                );
            }
            let derived = validate_spec_with_manifest(
                &copy.fingerprint,
                &load_pinned_manifest().expect("pinned manifest"),
            )
            .expect("copied spec is admissible")
            .derived;
            assert_eq!(derived, derive_traits(copy.fingerprint.seed));
            assert_ne!(
                derived,
                derive_traits(source.fingerprint.seed),
                "copy_fingerprint={copy_fingerprint} kept the source derived traits"
            );
        }
    });

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn golden_consistency_fixture_matches_all_three_launches() {
    let golden: ConsistencyGolden =
        serde_json::from_str(CONSISTENCY_FIXTURE).expect("valid consistency golden fixture");
    let manifest = load_pinned_manifest().expect("pinned manifest");
    assert_eq!(golden.expected_argv_runs.len(), 3, "{}", golden.case);

    for expected in &golden.expected_argv_runs {
        let plan = build_launch_argv(&golden.spec, &manifest, Path::new(PROFILE_DIR))
            .unwrap_or_else(|error| panic!("{}: {error}", golden.case));
        assert_eq!(&plan.argv, expected, "{}", golden.case);
        assert_eq!(
            plan.network.doh_mode.value, golden.expected_network.doh_mode.value,
            "{}",
            golden.case
        );
        assert_eq!(
            plan.network.doh_mode.status,
            status(&golden.expected_network.doh_mode.status)
        );
        assert_eq!(
            plan.network.ipv6_mode.value, golden.expected_network.ipv6_mode.value,
            "{}",
            golden.case
        );
        assert_eq!(
            plan.network.ipv6_mode.status,
            status(&golden.expected_network.ipv6_mode.status)
        );
        assert_eq!(
            plan.network.disable_non_proxied_udp.value,
            golden.expected_network.disable_non_proxied_udp.value
        );
        assert_eq!(
            plan.network.disable_non_proxied_udp.status,
            status(&golden.expected_network.disable_non_proxied_udp.status)
        );
        assert!(
            plan.argv.iter().all(|arg| !arg.contains("doh")
                && !arg.contains("ipv6")
                && !arg.contains("non-proxied-udp")),
            "{} leaked network controls into argv",
            golden.case
        );
    }

    let first = &golden.expected_argv_runs[0];
    for run in &golden.expected_argv_runs[1..] {
        assert_eq!(run, first, "{} recorded divergent launches", golden.case);
    }
}

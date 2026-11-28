// PKG-009: release policy rules, and the rehearsal that proves each one rejects.
//
// Two layers are tested here:
//   1. the pure rules (`release-policy.mjs`): target parsing, trusted baseline,
//      collision, reservation binding, source/main-green, artifact completeness,
//      attestation coverage, identity record and the no-Apple-credential rule;
//   2. `rehearsal.mjs`, which runs the whole gate sequence against fixtures and
//      must fail on each specific violation.
//
// `docs/TEST-PLAN.md:322` says PKG-009's pre-release result comes from a
// controlled rehearsal, not a real public release. These fixtures are that
// controlled rehearsal: nothing here touches GitHub or builds an artifact.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { REHEARSAL_STEPS, rehearse } from "./rehearsal.mjs";
import {
  ARTIFACT_REQUIREMENTS,
  ATTESTATION_SUBJECTS,
  artifactManifestErrors,
  attestationErrors,
  bindReservation,
  buildIdentityRecord,
  collisionErrors,
  compareCore,
  compareSemVer,
  expectedAttestationSubjects,
  formatIdentityBlock,
  identityRecordErrors,
  POLICY_ID,
  parseLsRemoteTags,
  parseReleaseTarget,
  parseReservationTag,
  parseVersion,
  planRelease,
  readBaseline,
  reservationTagName,
  resolveCanonicalVersion,
  secretPolicyErrors,
  sourceGateErrors,
} from "./release-policy.mjs";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const SCRIPT = path.join(HERE, "release-policy.mjs");
const SOURCE_SHA = "0123456789abcdef0123456789abcdef01234567";
const OLD_SHA = "b".repeat(40);
const DIGEST = "a".repeat(64);
const OCI_DIGEST = `sha256:${"c".repeat(64)}`;

const digestFor = (name) => {
  // Distinct, deterministic, shape-valid digests per artifact so a mismatch is
  // visible in the error message instead of hiding behind equal hashes.
  let hash = 0;
  for (const char of name) hash = (hash * 31 + char.charCodeAt(0)) % 0xffffffff;
  return hash.toString(16).padStart(8, "0").repeat(8).slice(0, 64);
};

const has = (errors, code) =>
  errors.some((error) => error.startsWith(`${code}:`)) ||
  errors.some((error) => error.includes(code));

function assertError(errors, code) {
  assert.ok(has(errors, code), `expected a '${code}' violation, got:\n${errors.join("\n")}`);
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/** A complete, valid artifact manifest for 0.2.0. */
function validManifest(overrides = {}) {
  const version = overrides.canonicalVersion ?? "0.2.0";
  const files = [
    { name: "Mien_0.2.0_aarch64.dmg", sha256: digestFor("dmg") },
    { name: "mien-app.spdx.json", sha256: digestFor("sbom") },
    { name: "LICENSE", sha256: digestFor("LICENSE") },
    { name: "NOTICE", sha256: digestFor("NOTICE") },
    { name: "LICENSES/Apache-2.0.txt", sha256: digestFor("licenses") },
    { name: `mien-seccomp-chromium-${version}.json`, sha256: digestFor("seccomp") },
    { name: "mien-chromium.apparmor", sha256: digestFor("apparmor") },
    { name: "release-identity.json", sha256: digestFor("identity") },
    { name: "entitlements.plist", sha256: digestFor("entitlements") },
    { name: "latest.json", sha256: digestFor("updater") },
  ];
  if (overrides.remove) {
    const remove = new Set(overrides.remove);
    for (let index = files.length - 1; index >= 0; index -= 1) {
      if (remove.has(files[index].name)) files.splice(index, 1);
    }
  }
  const sums = {};
  for (const file of files) sums[file.name] = file.sha256;
  if (!overrides.remove?.includes("SHA256SUMS")) {
    sums.SHA256SUMS = digestFor("SHA256SUMS");
    files.push({ name: "SHA256SUMS", sha256: digestFor("SHA256SUMS") });
  }
  const manifest = {
    canonical_version: version,
    files,
    sums,
    docker: { image: "ghcr.io/ivan/mien-server", digest: OCI_DIGEST },
    binaries: {
      kernel: { sha256: digestFor("kernel") },
      xray: { sha256: digestFor("xray") },
    },
  };
  if (overrides.sums) manifest.sums = { ...manifest.sums, ...overrides.sums };
  if (overrides.dockerDigest !== undefined)
    manifest.docker = { ...manifest.docker, digest: overrides.dockerDigest };
  if (overrides.binaries) manifest.binaries = overrides.binaries;
  return manifest;
}

/** An attestation covering exactly the manifest's subjects. */
function validAttestation(manifest = validManifest()) {
  const expected = expectedAttestationSubjects(manifest);
  return { subjects: { ...expected } };
}

/** A valid main state for SOURCE_SHA. */
function greenMain(overrides = {}) {
  return {
    dispatch_ref: "refs/heads/main",
    head_sha: SOURCE_SHA,
    merged_pr: { number: 91, merge_commit_sha: SOURCE_SHA, pr_required_conclusion: "success" },
    ci_main: {
      head_sha: SOURCE_SHA,
      conclusion: "success",
      job: "verify",
      job_conclusion: "success",
    },
    ...overrides,
  };
}

const reservationRelease = ({
  id = 77,
  version = "0.2.0",
  target = "minor",
  sha = SOURCE_SHA,
  runId = 4242,
  draft = true,
} = {}) => ({
  id,
  tag_name: `mien-reservation-${version}-${runId}`,
  draft,
  target_commitish: sha,
  body: `## Reservation\n\n${formatIdentityBlock({
    policy_id: POLICY_ID,
    release_target: target,
    canonical_version: version,
    source_sha: sha,
    reservation_id: id,
  })}`,
});

// ---------------------------------------------------------------------------
// Version parsing and ordering
// ---------------------------------------------------------------------------

test("explicit v-prefixed target resolves to a canonical version without the prefix", () => {
  const target = parseReleaseTarget("v0.2.0");
  assert.equal(target.kind, "explicit");
  assert.equal(target.version, "0.2.0");
  assert.equal(target.raw, "v0.2.0");
});

test("symbolic targets are only patch, minor and major", () => {
  assert.deepEqual(parseReleaseTarget("minor"), { kind: "symbolic", bump: "minor", raw: "minor" });
  for (const bad of ["Patch", "build", "latest", ""]) {
    assert.throws(() => parseReleaseTarget(bad), /target-(invalid|missing)/);
  }
});

test("a missing target fails closed instead of defaulting", () => {
  assert.throws(() => parseReleaseTarget(undefined), /target-missing/);
  assert.throws(() => parseReleaseTarget("   "), /target-missing/);
});

test("prerelease and build metadata targets are rejected by mien-semver-v1", () => {
  for (const bad of ["1.0.0-rc.1", "1.0.0+meta", "v0.2.0-beta.2"]) {
    assert.throws(() => parseReleaseTarget(bad), /target-prerelease/);
  }
});

test("numeric identifiers with leading zeroes are not valid versions", () => {
  for (const bad of ["01.2.3", "1.02.3", "1.2.03"]) {
    assert.throws(() => parseVersion(bad), /version-invalid/);
  }
  assert.throws(() => parseVersion("1.2"), /version-invalid/);
});

test("core comparison ignores prerelease and build metadata", () => {
  assert.equal(compareCore("1.0.0-rc.1", "1.0.0"), 0);
  assert.equal(compareCore("1.0.0+build.9", "1.0.0"), 0);
  assert.equal(compareCore("0.1.0", "0.2.0"), -1);
});

test("SemVer precedence orders prereleases below the release", () => {
  assert.equal(compareSemVer("1.0.0-rc.1", "1.0.0"), -1);
  assert.equal(compareSemVer("1.0.0-alpha", "1.0.0-beta"), -1);
  assert.equal(compareSemVer("1.0.0-alpha.1", "1.0.0-alpha"), 1);
  assert.equal(compareSemVer("1.0.0-2", "1.0.0-10"), -1);
  assert.equal(compareSemVer("1.0.0+build", "1.0.0"), 0);
});

// ---------------------------------------------------------------------------
// Trusted baseline
// ---------------------------------------------------------------------------

test("baseline is the highest core across tags and releases, prereleases included", () => {
  const { baseline, occupied } = readBaseline([
    "v0.1.0",
    { tag_name: "v0.3.0-rc.1" },
    "v0.2.9",
    "not-a-version",
  ]);
  assert.equal(baseline, "0.3.0");
  assert.deepEqual(occupied, ["0.1.0", "0.2.9", "0.3.0"]);
});

test("a v-prefixed tag that is not SemVer fails closed instead of being skipped", () => {
  assert.throws(() => readBaseline(["v1.0"]), /baseline-invalid/);
  assert.throws(() => readBaseline(["v1.0.0.0"]), /baseline-invalid/);
});

test("no baseline means only explicit 0.1.0", () => {
  assert.equal(resolveCanonicalVersion({ releaseTarget: "0.1.0" }).canonical, "0.1.0");
  assert.throws(() => resolveCanonicalVersion({ releaseTarget: "0.2.0" }), /first-release-version/);
  for (const symbolic of ["patch", "minor", "major"]) {
    assert.throws(
      () => resolveCanonicalVersion({ releaseTarget: symbolic, baselineVersions: [] }),
      /baseline-missing/,
    );
  }
});

test("symbolic targets bump the baseline's core", () => {
  const baselineVersions = ["v0.1.1"];
  assert.equal(
    resolveCanonicalVersion({ releaseTarget: "patch", baselineVersions }).canonical,
    "0.1.2",
  );
  assert.equal(
    resolveCanonicalVersion({ releaseTarget: "minor", baselineVersions }).canonical,
    "0.2.0",
  );
  assert.equal(
    resolveCanonicalVersion({ releaseTarget: "major", baselineVersions }).canonical,
    "1.0.0",
  );
});

test("an explicit target must be strictly greater than the baseline", () => {
  assert.equal(
    resolveCanonicalVersion({ releaseTarget: "0.3.0", baselineVersions: ["v0.2.0"] }).canonical,
    "0.3.0",
  );
  assert.throws(
    () => resolveCanonicalVersion({ releaseTarget: "0.2.0", baselineVersions: ["v0.2.0"] }),
    /version-not-greater-than-baseline/,
  );
  assert.throws(
    () => resolveCanonicalVersion({ releaseTarget: "0.1.0", baselineVersions: ["v0.2.0"] }),
    /version-not-greater-than-baseline/,
  );
});

test("a prerelease tag still occupies its core number", () => {
  assert.throws(
    () => resolveCanonicalVersion({ releaseTarget: "1.0.0", baselineVersions: ["v1.0.0-rc.1"] }),
    /version-not-greater-than-baseline/,
  );
});

// ---------------------------------------------------------------------------
// Reservation identity
// ---------------------------------------------------------------------------

test("reservation tag names never start with v and embed version and run id", () => {
  assert.equal(reservationTagName("0.2.0", 4242), "mien-reservation-0.2.0-4242");
  assert.deepEqual(parseReservationTag("mien-reservation-0.2.0-4242"), {
    canonical: "0.2.0",
    runId: "4242",
  });
  assert.equal(parseReservationTag("v0.2.0"), null);
  assert.throws(() => reservationTagName("0.2.0", ""), /run-id-missing/);
});

test("a matching reservation is a recovery, and its version/source win over main", () => {
  const plan = planRelease({
    release_target: "minor",
    source_sha: OLD_SHA,
    run_id: 9,
    tags: [{ name: "v0.2.0", commit_sha: SOURCE_SHA }],
    releases: [reservationRelease({ version: "0.2.0", sha: SOURCE_SHA })],
  });
  assert.equal(plan.ok, true, plan.errors.join("\n"));
  assert.equal(plan.resolution.recovery, true);
  assert.equal(plan.resolution.canonical_version, "0.2.0");
  assert.equal(plan.resolution.source_sha, SOURCE_SHA);
  assert.equal(plan.resolution.action, "reuse");
});

test("a matching reservation is a recovery even when main has moved", () => {
  const plan = planRelease({
    release_target: "minor",
    source_sha: OLD_SHA,
    run_id: 9,
    tags: [],
    releases: [reservationRelease({ version: "0.2.0", sha: SOURCE_SHA })],
  });
  // A recovery keeps the reservation's SHA; the dispatch SHA is not a conflict.
  assert.equal(plan.ok, true, plan.errors.join("\n"));
  assert.equal(plan.resolution.source_sha, SOURCE_SHA);
});

test("an unread baseline is a rejection, not an empty baseline", () => {
  const plan = planRelease({
    release_target: "0.2.0",
    source_sha: SOURCE_SHA,
    run_id: 9,
    tags: null,
    releases: null,
  });
  assert.equal(plan.ok, false);
  assertError(plan.errors, "baseline-unreadable");
});

test("the same version claimed from a different source is a conflict", () => {
  const plan = planRelease({
    release_target: "0.2.0",
    source_sha: OLD_SHA,
    run_id: 9,
    tags: ["v0.1.0"],
    releases: [reservationRelease({ version: "0.2.0", sha: SOURCE_SHA })],
  });
  assert.equal(plan.ok, false);
  assertError(plan.errors, "reservation-conflict");
});

test("binding requires the exact target, not just the version", () => {
  const plan = planRelease({
    release_target: "0.4.0",
    source_sha: SOURCE_SHA,
    run_id: 9,
    tags: ["v0.1.0"],
    releases: [reservationRelease({ version: "0.2.0", target: "minor" })],
  });
  // No reservation for '0.4.0', so this is a new identity; the 0.2.0 reservation
  // is below the target and does not collide.
  assert.equal(plan.ok, true, plan.errors.join("\n"));
  assert.equal(plan.resolution.recovery, false);
});

test("two reservations for one target with different versions are ambiguous", () => {
  const plan = planRelease({
    release_target: "minor",
    source_sha: SOURCE_SHA,
    run_id: 9,
    tags: [],
    releases: [
      reservationRelease({ id: 1, version: "0.2.0", runId: 1 }),
      reservationRelease({ id: 2, version: "0.3.0", runId: 2 }),
    ],
  });
  assert.equal(plan.ok, false);
  assertError(plan.errors, "reservation-ambiguous");
});

test("bindReservation rejects a reservation whose body disagrees with the plan", () => {
  const bound = bindReservation({
    releaseTarget: "minor",
    canonicalVersion: "0.2.0",
    sourceSha: OLD_SHA,
    runId: 4242,
    action: "reuse",
    releases: [reservationRelease({ version: "0.2.0", sha: SOURCE_SHA })],
  });
  assert.equal(bound.ok, false);
  assertError(bound.errors, "reservation-source-mismatch");
});

test("bindReservation reports an absent reservation as such", () => {
  const bound = bindReservation({
    releaseTarget: "minor",
    canonicalVersion: "0.2.0",
    sourceSha: OLD_SHA,
    runId: 4242,
    action: "reuse",
    releases: [],
  });
  assert.equal(bound.ok, false);
  assertError(bound.errors, "reservation-missing");
});

test("bindReservation accepts a create only when the tag is free", () => {
  const ok = bindReservation({
    releaseTarget: "minor",
    canonicalVersion: "0.2.0",
    sourceSha: SOURCE_SHA,
    runId: 11,
    action: "create",
    releases: [],
  });
  assert.equal(ok.ok, true, ok.errors.join("\n"));
  assert.equal(ok.reservation_tag, "mien-reservation-0.2.0-11");

  const taken = bindReservation({
    releaseTarget: "minor",
    canonicalVersion: "0.2.0",
    sourceSha: SOURCE_SHA,
    runId: 11,
    action: "create",
    releases: [reservationRelease({ version: "0.2.0", runId: 11 })],
  });
  assert.equal(taken.ok, false);
  assertError(taken.errors, "reservation-tag-in-use");
});

test("a non-draft reservation is not a reservation", () => {
  const plan = planRelease({
    release_target: "minor",
    source_sha: SOURCE_SHA,
    run_id: 9,
    tags: [],
    releases: [reservationRelease({ version: "0.2.0", draft: false })],
  });
  assert.equal(plan.ok, false);
  assertError(plan.errors, "reservation-not-draft");
});

// ---------------------------------------------------------------------------
// Collision rules
// ---------------------------------------------------------------------------

test("an existing tag at the target version blocks a foreign identity", () => {
  const errors = collisionErrors({
    canonicalVersion: "0.2.0",
    identity: {
      policy_id: POLICY_ID,
      release_target: "minor",
      canonical_version: "0.2.0",
      source_sha: SOURCE_SHA,
    },
    tags: [{ name: "v0.2.0", commit_sha: OLD_SHA }],
    releases: [],
  });
  assertError(errors, "tag-source-mismatch");
});

test("a higher existing version blocks a lower target", () => {
  const errors = collisionErrors({
    canonicalVersion: "0.2.0",
    identity: null,
    tags: ["v0.3.0"],
    releases: [],
  });
  assertError(errors, "collision");
});

test("the current identity's own tag and reservation are excluded", () => {
  const identity = {
    policy_id: POLICY_ID,
    release_target: "minor",
    canonical_version: "0.2.0",
    source_sha: SOURCE_SHA,
  };
  const errors = collisionErrors({
    canonicalVersion: "0.2.0",
    identity,
    tags: [{ name: "v0.2.0", commit_sha: SOURCE_SHA }],
    releases: [
      { id: 5, tag_name: "v0.2.0", body: formatIdentityBlock({ ...identity, reservation_id: 5 }) },
      reservationRelease({ id: 5 }),
    ],
    chosenReservationId: 5,
  });
  assert.deepEqual(errors, []);
});

test("a foreign release at the same version still collides even on recovery", () => {
  const identity = {
    policy_id: POLICY_ID,
    release_target: "minor",
    canonical_version: "0.2.0",
    source_sha: SOURCE_SHA,
  };
  const errors = collisionErrors({
    canonicalVersion: "0.2.0",
    identity,
    tags: [],
    releases: [{ id: 8, tag_name: "v0.2.0", body: "no identity block" }],
    chosenReservationId: null,
  });
  assertError(errors, "collision");
});

test("git ls-remote output is peeled so the commit comparison uses the target commit", () => {
  const tags = parseLsRemoteTags(
    [
      "1111111111111111111111111111111111111111\trefs/tags/v0.1.0",
      "2222222222222222222222222222222222222222\trefs/tags/v0.1.0^{}",
      "3333333333333333333333333333333333333333\trefs/tags/mien-reservation-0.2.0-1",
    ].join("\n"),
  );
  assert.deepEqual(tags, [
    { name: "mien-reservation-0.2.0-1", commit_sha: "3333333333333333333333333333333333333333" },
    { name: "v0.1.0", commit_sha: "2222222222222222222222222222222222222222" },
  ]);
});

// ---------------------------------------------------------------------------
// main-green gate
// ---------------------------------------------------------------------------

test("a green main for the exact SHA passes", () => {
  assert.deepEqual(sourceGateErrors({ sourceSha: SOURCE_SHA, main: greenMain() }), []);
});

test("a stale main rejects the release", () => {
  const errors = sourceGateErrors({ sourceSha: OLD_SHA, main: greenMain() });
  assertError(errors, "stale-main");
});

test("a missing CI Main run rejects the release", () => {
  const main = greenMain();
  delete main.ci_main;
  assertError(sourceGateErrors({ sourceSha: SOURCE_SHA, main }), "main-not-green");
});

test("a CI Main run for a different SHA does not count", () => {
  const main = greenMain({ ci_main: { head_sha: OLD_SHA, conclusion: "success" } });
  assertError(sourceGateErrors({ sourceSha: SOURCE_SHA, main }), "main-not-green");
});

test("missing CI PR / required evidence rejects the release", () => {
  const main = greenMain();
  delete main.merged_pr;
  assertError(sourceGateErrors({ sourceSha: SOURCE_SHA, main }), "main-not-merged-from-pr");

  const failed = greenMain();
  failed.merged_pr.pr_required_conclusion = "failure";
  assertError(sourceGateErrors({ sourceSha: SOURCE_SHA, main: failed }), "main-not-green");
});

test("a release dispatched from a branch or tag is rejected", () => {
  const main = greenMain({ dispatch_ref: "refs/heads/topic" });
  assertError(sourceGateErrors({ sourceSha: SOURCE_SHA, main }), "source-not-main");
});

test("recovery may keep an old SHA after main advances, but still needs CI Main on it", () => {
  // `main` has moved on, so the merged PR and CI Main run that count are the
  // ones that produced the bound recovery SHA, not the current head.
  const advanced = greenMain({
    head_sha: "f".repeat(40),
    merged_pr: { number: 77, merge_commit_sha: OLD_SHA, pr_required_conclusion: "success" },
    ci_main: { head_sha: OLD_SHA, conclusion: "success" },
  });
  assert.deepEqual(sourceGateErrors({ sourceSha: OLD_SHA, main: advanced, recovery: true }), []);

  const noGreen = greenMain({
    head_sha: "f".repeat(40),
    merged_pr: { number: 77, merge_commit_sha: OLD_SHA, pr_required_conclusion: "success" },
    ci_main: null,
  });
  assertError(
    sourceGateErrors({ sourceSha: OLD_SHA, main: noGreen, recovery: true }),
    "main-not-green",
  );
});

// ---------------------------------------------------------------------------
// Artifact manifest completeness
// ---------------------------------------------------------------------------

test("a complete manifest passes", () => {
  assert.deepEqual(artifactManifestErrors(validManifest(), { canonicalVersion: "0.2.0" }), []);
});

test("a missing SHA256SUMS is rejected by name", () => {
  const manifest = validManifest({ remove: ["SHA256SUMS"] });
  assertError(
    artifactManifestErrors(manifest, { canonicalVersion: "0.2.0" }),
    "missing-sha256sums",
  );
});

test("a missing host-side seccomp artifact is rejected", () => {
  const manifest = validManifest({ remove: ["mien-seccomp-chromium-0.2.0.json"] });
  assertError(artifactManifestErrors(manifest, { canonicalVersion: "0.2.0" }), "missing-seccomp");
});

test("a seccomp artifact for another version is not the release's seccomp artifact", () => {
  const manifest = validManifest();
  manifest.files = manifest.files.map((file) =>
    file.name === "mien-seccomp-chromium-0.2.0.json"
      ? { ...file, name: "mien-seccomp-chromium-0.1.0.json" }
      : file,
  );
  const renamed = manifest.files.find((file) => file.name === "mien-seccomp-chromium-0.1.0.json");
  delete manifest.sums["mien-seccomp-chromium-0.2.0.json"];
  manifest.sums[renamed.name] = renamed.sha256;
  assertError(
    artifactManifestErrors(manifest, { canonicalVersion: "0.2.0" }),
    "seccomp-version-mismatch",
  );
});

test("every required artifact has a specific violation code", () => {
  for (const requirement of ARTIFACT_REQUIREMENTS) {
    const manifest = validManifest();
    const removed = manifest.files.filter((file) => requirement.matches(file.name, "0.2.0"));
    assert.ok(removed.length > 0, `fixture does not contain ${requirement.description}`);
    const errors = artifactManifestErrors(
      validManifest({ remove: removed.map((file) => file.name) }),
      { canonicalVersion: "0.2.0" },
    );
    assertError(errors, requirement.code);
  }
});

test("a checksum that does not match the artifact bytes is rejected", () => {
  const manifest = validManifest();
  manifest.sums["Mien_0.2.0_aarch64.dmg"] = digestFor("something-else");
  assertError(artifactManifestErrors(manifest, { canonicalVersion: "0.2.0" }), "checksum-mismatch");
});

test("SHA256SUMS must list every artifact and nothing else", () => {
  const missing = validManifest();
  delete missing.sums["latest.json"];
  assertError(artifactManifestErrors(missing, { canonicalVersion: "0.2.0" }), "checksum-missing");

  const extra = validManifest();
  extra.sums["ghost.dmg"] = DIGEST;
  assertError(artifactManifestErrors(extra, { canonicalVersion: "0.2.0" }), "checksum-unknown");
});

test("the container digest, kernel and Xray hashes must be recorded", () => {
  assertError(
    artifactManifestErrors(validManifest({ dockerDigest: null }), { canonicalVersion: "0.2.0" }),
    "missing-docker-digest",
  );
  assertError(
    artifactManifestErrors(validManifest({ binaries: { kernel: {} } }), {
      canonicalVersion: "0.2.0",
    }),
    "missing-binary-digest",
  );
  assertError(
    artifactManifestErrors(validManifest(), { canonicalVersion: "0.3.0" }),
    "manifest-version-mismatch",
  );
});

test("a malformed digest is rejected rather than compared", () => {
  const manifest = validManifest();
  manifest.files[0].sha256 = "not-a-digest";
  assertError(artifactManifestErrors(manifest, { canonicalVersion: "0.2.0" }), "invalid-digest");
});

// ---------------------------------------------------------------------------
// Attestation coverage
// ---------------------------------------------------------------------------

test("an attestation covering every subject passes", () => {
  const manifest = validManifest();
  assert.deepEqual(attestationErrors(validAttestation(manifest), manifest), []);
});

test("PKG-009's four named subjects are all required", () => {
  for (const role of ["dmg", "container", "sbom", "seccomp"]) {
    assert.ok(ATTESTATION_SUBJECTS.includes(role), `${role} must be an attestation subject`);
  }
});

test("dropping any attestation subject is rejected by name", () => {
  const manifest = validManifest();
  for (const role of ATTESTATION_SUBJECTS) {
    const attestation = validAttestation(manifest);
    delete attestation.subjects[role];
    assertError(attestationErrors(attestation, manifest), `missing-attestation-subject`);
  }
});

test("an attestation covering different bytes is rejected", () => {
  const manifest = validManifest();
  const attestation = validAttestation(manifest);
  attestation.subjects.dmg = DIGEST;
  assertError(attestationErrors(attestation, manifest), "attestation-digest-mismatch");
});

test("no attestation at all is a rejection, not a skip", () => {
  assertError(attestationErrors(null, validManifest()), "missing-attestation");
  assertError(attestationErrors({ subjects: {} }, validManifest()), "missing-attestation-subject");
});

// ---------------------------------------------------------------------------
// release-identity.json
// ---------------------------------------------------------------------------

test("the identity record binds version, source, reservation and digests", () => {
  const manifest = validManifest();
  const resolution = {
    policy_id: POLICY_ID,
    release_target: "minor",
    canonical_version: "0.2.0",
    source_sha: SOURCE_SHA,
    reservation_id: 77,
  };
  const record = buildIdentityRecord({ resolution, manifest, publication: { tag: "v0.2.0" } });
  assert.equal(record.canonical_version, "0.2.0");
  assert.equal(record.source_sha, SOURCE_SHA);
  assert.equal(record.reservation_id, 77);
  assert.equal(record.artifacts.seccomp.name, "mien-seccomp-chromium-0.2.0.json");
  assert.deepEqual(identityRecordErrors(record, { resolution, manifest }), []);
});

test("a rebuilt DMG with a different digest cannot overwrite the identity", () => {
  const manifest = validManifest();
  const resolution = {
    policy_id: POLICY_ID,
    release_target: "minor",
    canonical_version: "0.2.0",
    source_sha: SOURCE_SHA,
    reservation_id: 77,
  };
  const record = buildIdentityRecord({ resolution, manifest });
  record.artifacts.dmg.sha256 = DIGEST;
  assertError(identityRecordErrors(record, { resolution, manifest }), "identity-digest-mismatch");
});

test("an identity record for another source is rejected", () => {
  const manifest = validManifest();
  const resolution = {
    policy_id: POLICY_ID,
    release_target: "minor",
    canonical_version: "0.2.0",
    source_sha: SOURCE_SHA,
    reservation_id: 77,
  };
  const record = buildIdentityRecord({ resolution, manifest });
  record.source_sha = OLD_SHA;
  assertError(identityRecordErrors(record, { resolution, manifest }), "identity-record-source");
});

// ---------------------------------------------------------------------------
// Credentials
// ---------------------------------------------------------------------------

test("only the updater key is allowed in the release environment", () => {
  assert.deepEqual(
    secretPolicyErrors({
      secretNames: ["TAURI_SIGNING_PRIVATE_KEY", "TAURI_SIGNING_PRIVATE_KEY_PASSWORD"],
      signingMode: "adhoc",
    }),
    [],
  );
});

test("no Apple certificate or notarization secret may exist", () => {
  for (const name of [
    "APPLE_CERTIFICATE",
    "APPLE_CERTIFICATE_PASSWORD",
    "APPLE_ID",
    "APPLE_TEAM_ID",
    "APPLE_APP_SPECIFIC_PASSWORD",
    "APPLE_API_KEY",
    "APPLE_API_ISSUER",
  ]) {
    assertError(secretPolicyErrors({ secretNames: [name] }), "apple-credential-present");
  }
});

test("an unknown release secret is a failure, not a warning", () => {
  assertError(secretPolicyErrors({ secretNames: ["SOME_OTHER_KEY"] }), "unexpected-release-secret");
  assertError(secretPolicyErrors({ signingMode: "developer-id" }), "signing-mode-not-adhoc");
});

// ---------------------------------------------------------------------------
// Rehearsal: the whole sequence, and each violation named
// ---------------------------------------------------------------------------

test("the rehearsal reports every step it exercises", () => {
  const report = rehearse();
  assert.equal(report.ok, true, report.errors.join("\n"));
  assert.deepEqual(
    report.steps.map((step) => step.step),
    REHEARSAL_STEPS,
  );
});

test("the rehearsal rejects a stale main", () => {
  const report = rehearse({ main: greenMain({ head_sha: OLD_SHA }) });
  assert.equal(report.ok, false);
  assertError(report.errors, "stale-main");
});

test("the rehearsal rejects a version not greater than the baseline", () => {
  const report = rehearse({ releaseTarget: "0.1.0" });
  assert.equal(report.ok, false);
  assertError(report.errors, "version-not-greater-than-baseline");
});

test("the rehearsal rejects a missing SHA256SUMS", () => {
  const report = rehearse({ removeArtifacts: ["SHA256SUMS"] });
  assert.equal(report.ok, false);
  assertError(report.errors, "missing-sha256sums");
});

test("the rehearsal rejects a missing seccomp artifact", () => {
  const report = rehearse({ removeArtifacts: ["mien-seccomp-chromium-0.2.0.json"] });
  assert.equal(report.ok, false);
  assertError(report.errors, "missing-seccomp");
});

test("the rehearsal rejects a missing attestation subject", () => {
  const report = rehearse({ dropAttestationSubjects: ["dmg"] });
  assert.equal(report.ok, false);
  assertError(report.errors, "missing-attestation-subject");
});

test("the rehearsal rejects a reservation bound to another source", () => {
  const report = rehearse({ reservationSourceSha: OLD_SHA });
  assert.equal(report.ok, false);
  assertError(report.errors, "reservation-source-mismatch");
});

test("the rehearsal rejects an Apple credential in the release environment", () => {
  const report = rehearse({ secretNames: ["APPLE_CERTIFICATE"] });
  assert.equal(report.ok, false);
  assertError(report.errors, "apple-credential-present");
});

test("the rehearsal stops before artifacts when the target is invalid", () => {
  const report = rehearse({ releaseTarget: "0.2.0-rc.1" });
  assert.equal(report.ok, false);
  assertError(report.errors, "target-prerelease");
  assert.equal(report.resolution, null);
});

// ---------------------------------------------------------------------------
// CLI contract: the workflow runs these, so their exit codes are the gate
// ---------------------------------------------------------------------------

function runCli(args, files = {}) {
  const dir = mkdtempSync(path.join(tmpdir(), "mien-release-policy-"));
  try {
    for (const [name, value] of Object.entries(files)) {
      writeFileSync(path.join(dir, name), JSON.stringify(value, null, 2));
    }
    try {
      const stdout = execFileSync(process.execPath, [SCRIPT, ...args], {
        cwd: dir,
        encoding: "utf8",
        stdio: ["ignore", "pipe", "pipe"],
      });
      return { code: 0, stdout };
    } catch (error) {
      return { code: error.status, stdout: error.stdout ?? "", stderr: error.stderr ?? "" };
    }
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

const validResolution = () => ({
  policy_id: POLICY_ID,
  release_target: "minor",
  canonical_version: "0.2.0",
  source_sha: SOURCE_SHA,
  reservation_id: 77,
});

function happyPathCli() {
  const manifest = validManifest();
  const resolution = validResolution();
  return {
    dir: null,
    files: {
      "tags.json": ["v0.1.0"],
      "releases.json": [],
      "main.json": greenMain(),
      "manifest.json": manifest,
      "attestation.json": validAttestation(manifest),
      "resolved.json": resolution,
      "identity.json": buildIdentityRecord({ resolution, manifest }),
      "secrets.json": ["TAURI_SIGNING_PRIVATE_KEY"],
    },
  };
}

test("resolve exits 0 with a canonical version for a green main", () => {
  const { files } = happyPathCli();
  const result = runCli(
    [
      "resolve",
      "--target",
      "minor",
      "--source-sha",
      SOURCE_SHA,
      "--run-id",
      "4242",
      "--tags",
      "tags.json",
      "--releases",
      "releases.json",
      "--main",
      "main.json",
    ],
    files,
  );
  assert.equal(result.code, 0, result.stderr + result.stdout);
  const payload = JSON.parse(result.stdout);
  assert.equal(payload.canonical_version, "0.2.0");
  assert.equal(payload.reservation_tag, "mien-reservation-0.2.0-4242");
  assert.equal(payload.recovery, false);
});

test("resolve exits 1 when main evidence is omitted, so the check cannot be skipped", () => {
  const { files } = happyPathCli();
  const result = runCli(
    [
      "resolve",
      "--target",
      "minor",
      "--source-sha",
      SOURCE_SHA,
      "--run-id",
      "4242",
      "--tags",
      "tags.json",
      "--releases",
      "releases.json",
    ],
    files,
  );
  assert.equal(result.code, 1);
  assert.match(result.stdout, /main-not-green/);
});

test("resolve exits 1 for a stale main", () => {
  const { files } = happyPathCli();
  files["main.json"] = greenMain({ head_sha: OLD_SHA });
  const result = runCli(
    [
      "resolve",
      "--target",
      "minor",
      "--source-sha",
      SOURCE_SHA,
      "--run-id",
      "4242",
      "--tags",
      "tags.json",
      "--releases",
      "releases.json",
      "--main",
      "main.json",
    ],
    files,
  );
  assert.equal(result.code, 1);
  assert.match(result.stdout, /stale-main/);
});

test("check-manifest exits 1 when seccomp is missing", () => {
  const { files } = happyPathCli();
  files["manifest.json"] = validManifest({ remove: ["mien-seccomp-chromium-0.2.0.json"] });
  const result = runCli(
    ["check-manifest", "--manifest", "manifest.json", "--resolved", "resolved.json"],
    files,
  );
  assert.equal(result.code, 1);
  assert.match(result.stdout, /missing-seccomp/);
});

test("check-attestation exits 0 only with full coverage", () => {
  const { files } = happyPathCli();
  const ok = runCli(
    ["check-attestation", "--manifest", "manifest.json", "--attestation", "attestation.json"],
    files,
  );
  assert.equal(ok.code, 0, ok.stderr + ok.stdout);

  const attestation = validAttestation(validManifest());
  delete attestation.subjects.container;
  files["attestation.json"] = attestation;
  const bad = runCli(
    ["check-attestation", "--manifest", "manifest.json", "--attestation", "attestation.json"],
    files,
  );
  assert.equal(bad.code, 1);
  assert.match(bad.stdout, /missing-attestation-subject/);
});

test("check-identity exits 0 only when the record matches this build", () => {
  const { files } = happyPathCli();
  const ok = runCli(
    [
      "check-identity",
      "--record",
      "identity.json",
      "--resolved",
      "resolved.json",
      "--manifest",
      "manifest.json",
    ],
    files,
  );
  assert.equal(ok.code, 0, ok.stderr + ok.stdout);

  const record = buildIdentityRecord({ resolution: validResolution(), manifest: validManifest() });
  record.canonical_version = "0.3.0";
  files["identity.json"] = record;
  const bad = runCli(
    [
      "check-identity",
      "--record",
      "identity.json",
      "--resolved",
      "resolved.json",
      "--manifest",
      "manifest.json",
    ],
    files,
  );
  assert.equal(bad.code, 1);
  assert.match(bad.stdout, /identity-record-version/);
});

test("check-secrets exits 1 for an Apple certificate", () => {
  const { files } = happyPathCli();
  files["secrets.json"] = ["TAURI_SIGNING_PRIVATE_KEY", "APPLE_CERTIFICATE"];
  const result = runCli(
    ["check-secrets", "--secrets", "secrets.json", "--signing", "adhoc"],
    files,
  );
  assert.equal(result.code, 1);
  assert.match(result.stdout, /apple-credential-present/);
});

test("check-secrets exits 1 when the declaration is absent or malformed", () => {
  const missing = runCli(["check-secrets"], {});
  assert.equal(missing.code, 1);
  assert.match(missing.stdout, /secrets-declaration-missing/);

  const malformed = runCli(["check-secrets", "--secrets", "secrets.json"], { "secrets.json": {} });
  assert.equal(malformed.code, 1);
  assert.match(malformed.stdout, /secrets-declaration-invalid/);
});

test("an unknown subcommand is a usage error, not a pass", () => {
  const result = runCli(["publish"]);
  assert.equal(result.code, 2);
});

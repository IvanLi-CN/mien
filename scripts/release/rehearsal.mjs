#!/usr/bin/env node
// PKG-009 rehearsal: the gate sequence, end to end, with no network and no build.
//
// `docs/TEST-PLAN.md:322` says PKG-009's pre-release result comes from a
// controlled rehearsal of the workflow/policy, explicitly not from a real public
// release. This module is that rehearsal. It walks the ordered sequence — target
// resolves, main is green, the reservation binds, the artifact manifest is
// complete, the identity record matches, attestation covers every subject, the
// release environment has no Apple credential — and reports which rule rejected
// each stage.
//
// It is *not* a simulation of publishing. It never creates a tag, a release, a
// registry tag or an artifact; the fixtures are checked against the same policy
// functions the workflow calls, so a hole in the policy is a hole here too.
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

import {
  artifactManifestErrors,
  attestationErrors,
  bindReservation,
  buildIdentityRecord,
  expectedAttestationSubjects,
  formatIdentityBlock,
  identityRecordErrors,
  POLICY_ID,
  parseIdentityBlock,
  planRelease,
  secretPolicyErrors,
  sourceGateErrors,
} from "./release-policy.mjs";

/** The gate order `docs/CI.md` §5 fixes. Every step runs on every rehearsal. */
export const REHEARSAL_STEPS = [
  "resolve-target",
  "main-green",
  "reserve",
  "artifact-manifest",
  "identity-record",
  "attestation",
  "release-environment",
];

const SOURCE_SHA = "0123456789abcdef0123456789abcdef01234567";
const OCI_DIGEST = `sha256:${"c".repeat(64)}`;

/** Distinct, deterministic, shape-valid digests per artifact name. */
const digestFor = (name) => {
  let hash = 0;
  for (const char of name) hash = (hash * 31 + char.charCodeAt(0)) % 0xffffffff;
  return hash.toString(16).padStart(8, "0").repeat(8).slice(0, 64);
};

/** The artifact set PKG-009 requires for one release candidate. */
export function rehearsalManifest({ version = "0.2.0", remove = [] } = {}) {
  const removed = new Set(remove);
  const files = [
    { name: `Mien_${version}_aarch64.dmg`, sha256: digestFor("dmg") },
    { name: "mien-app.spdx.json", sha256: digestFor("sbom") },
    { name: "LICENSE", sha256: digestFor("LICENSE") },
    { name: "NOTICE", sha256: digestFor("NOTICE") },
    { name: "LICENSES/Apache-2.0.txt", sha256: digestFor("licenses") },
    { name: `mien-seccomp-chromium-${version}.json`, sha256: digestFor("seccomp") },
    { name: "mien-chromium.apparmor", sha256: digestFor("apparmor") },
    { name: "release-identity.json", sha256: digestFor("identity") },
    { name: "entitlements.plist", sha256: digestFor("entitlements") },
    { name: "latest.json", sha256: digestFor("updater") },
  ].filter((file) => !removed.has(file.name));

  const sums = {};
  for (const file of files) sums[file.name] = file.sha256;
  if (!removed.has("SHA256SUMS")) {
    sums.SHA256SUMS = digestFor("SHA256SUMS");
    files.push({ name: "SHA256SUMS", sha256: digestFor("SHA256SUMS") });
  }

  return {
    canonical_version: version,
    files,
    sums,
    docker: { image: "ghcr.io/ivan/mien-server", digest: OCI_DIGEST },
    binaries: { kernel: { sha256: digestFor("kernel") }, xray: { sha256: digestFor("xray") } },
  };
}

function rehearsalMain(overrides = {}) {
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

function rehearsalReservation({
  version = "0.2.0",
  target = "minor",
  sha = SOURCE_SHA,
  runId = 4242,
  id = 77,
  draft = true,
} = {}) {
  return {
    id,
    tag_name: `mien-reservation-${version}-${runId}`,
    draft,
    target_commitish: sha,
    body: `## Draft reservation\n\n${formatIdentityBlock({
      policy_id: POLICY_ID,
      release_target: target,
      canonical_version: version,
      source_sha: sha,
      reservation_id: id,
    })}`,
  };
}

/**
 * Run the gate sequence against fixtures.
 *
 * Each option breaks one rule so a test can assert the specific violation:
 * `releaseTarget`, `tags`, `main`, `reservationSourceSha`, `reservationTarget`,
 * `removeArtifacts`, `dropAttestationSubjects`, `secretNames`, `signingMode`,
 * `identityVersion`.
 */
export function rehearse(options = {}) {
  const {
    releaseTarget = "minor",
    sourceSha = SOURCE_SHA,
    runId = 4242,
    tags = ["v0.1.0"],
    main = rehearsalMain(),
    reservationSourceSha = SOURCE_SHA,
    reservationTarget = "minor",
    removeArtifacts = [],
    dropAttestationSubjects = [],
    secretNames = ["TAURI_SIGNING_PRIVATE_KEY", "TAURI_SIGNING_PRIVATE_KEY_PASSWORD"],
    signingMode = "adhoc",
    identityVersion = null,
  } = options;

  const steps = [];
  const errors = [];
  const record = (step, stepErrors) => {
    steps.push({ step, ok: stepErrors.length === 0, errors: stepErrors });
    for (const error of stepErrors) errors.push(error);
  };

  // 1. The target resolves against the trusted baseline read from tags. A first
  //    rehearsal has no reservation, so this is always a new identity.
  const plan = planRelease({
    release_target: releaseTarget,
    source_sha: sourceSha,
    run_id: runId,
    tags,
    releases: [],
  });
  record("resolve-target", plan.errors);
  if (plan.resolution === null) {
    return { ok: false, errors, steps, resolution: null };
  }
  const resolution = { ...plan.resolution };

  // 2. `main` is green for exactly this SHA (or, on recovery, for the bound SHA).
  record(
    "main-green",
    sourceGateErrors({ sourceSha: resolution.source_sha, main, recovery: resolution.recovery }),
  );

  // 3. The draft reservation binds version and source. The fixture is what
  //    `reserve` would have written; the deviation knobs break one field each.
  const releases = [
    rehearsalReservation({
      version: resolution.canonical_version,
      target: reservationTarget,
      sha: reservationSourceSha,
      runId,
    }),
  ];
  const bound = bindReservation({
    releaseTarget: resolution.release_target,
    canonicalVersion: resolution.canonical_version,
    sourceSha: resolution.source_sha,
    runId,
    releases,
    action: "reuse",
    reservationId: 77,
  });
  record("reserve", bound.errors);

  const reservation = releases[0];
  const draftIdentity = parseIdentityBlock(reservation.body);
  resolution.reservation_id = draftIdentity?.reservation_id ?? 77;

  // 4. Every required artifact is present and SHA256SUMS covers it.
  const manifest = rehearsalManifest({
    version: resolution.canonical_version,
    remove: removeArtifacts,
  });
  record(
    "artifact-manifest",
    artifactManifestErrors(manifest, { canonicalVersion: resolution.canonical_version }),
  );

  // 5. release-identity.json binds the same tuple and digests as this build.
  const identity = buildIdentityRecord({
    resolution,
    manifest,
    publication: { tag: resolution.tag_name },
  });
  if (identityVersion !== null) identity.canonical_version = identityVersion;
  record("identity-record", identityRecordErrors(identity, { resolution, manifest }));

  // 6. Attestation covers the DMG, container, SBOM, seccomp and the rest.
  const attestation = { subjects: { ...expectedAttestationSubjects(manifest) } };
  for (const role of dropAttestationSubjects) delete attestation.subjects[role];
  record("attestation", attestationErrors(attestation, manifest));

  // 7. The release environment holds the updater key and no Apple credential.
  record("release-environment", secretPolicyErrors({ secretNames, signingMode }));

  return {
    ok: errors.length === 0,
    errors,
    steps,
    resolution,
    reservation: { tag: reservation.tag_name, id: reservation.id },
    manifest,
    identity,
    attestation,
  };
}

const USAGE = `usage:
  rehearsal.mjs [--out <report.json>]

Runs the PKG-009 gate sequence against fixtures and prints a JSON report.
Exit 0 means every rule held; 1 means at least one rule rejected the release.`;

function parseArgs(argv) {
  const options = {};
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (!arg.startsWith("--")) continue;
    const key = arg.slice(2);
    const next = argv[index + 1];
    if (next === undefined || next.startsWith("--")) {
      options[key] = true;
      continue;
    }
    options[key] = next;
    index += 1;
  }
  return options;
}

export function main(argv = process.argv.slice(2)) {
  const options = parseArgs(argv);
  if (options.help || options.h) {
    console.log(USAGE);
    return 0;
  }
  const report = rehearse();
  const text = `${JSON.stringify(report, null, 2)}\n`;
  console.log(text);
  if (typeof options.out === "string") {
    const target = path.resolve(options.out);
    mkdirSync(path.dirname(target), { recursive: true });
    writeFileSync(target, text);
  }
  return report.ok ? 0 : 1;
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === path.resolve(fileURLToPath(import.meta.url))
) {
  process.exitCode = main();
}

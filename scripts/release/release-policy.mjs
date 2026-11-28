#!/usr/bin/env node
// PKG-009: the release policy layer that `.github/workflows/release.yml` runs.
//
// `docs/RELEASE-POLICY.md` is the specification; this module is the executable
// subset: `release_target` parsing, the trusted baseline, canonical version
// resolution, collision rules, reservation identity binding, the `main` green
// gate, and the artifact/attestation completeness PKG-009 requires. It makes no
// network calls — every GitHub fact arrives as JSON — so the rules are testable
// without a token and the workflow keeps only the API plumbing.
//
// Violations are returned as `"<code>: <message>"` strings. The code is stable
// so tests and the rehearsal can name the rule that rejected a release.
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

/** `docs/RELEASE-POLICY.md` §1: policy revisions are explicit, never silent. */
export const POLICY_ID = "mien-semver-v1";

/** The only symbolic targets. There is no default. */
const SYMBOLIC_TARGETS = ["patch", "minor", "major"];

/** A release without a trusted baseline must be exactly this version. */
const FIRST_RELEASE_VERSION = "0.1.0";

/** Reservation tag names never start with `v`, so they never occupy the baseline. */
const RESERVATION_TAG_PREFIX = "mien-reservation-";

const IDENTITY_BLOCK_START = "<!-- mien-release-identity";
const IDENTITY_BLOCK_END = "-->";
const VERSION_RE = /^(v)?(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+([0-9A-Za-z.-]+))?$/;
const NUMERIC_RE = /^(0|[1-9]\d*)$/;
const SHA_RE = /^[0-9a-f]{40}$/;
const SHA256_RE = /^[0-9a-f]{64}$/;
const OCI_DIGEST_RE = /^sha256:[0-9a-f]{64}$/;

const USAGE = `usage:
  release-policy.mjs tags --input <git-ls-remote.txt> [--out <tags.json>]
  release-policy.mjs resolve --target <release_target> --source-sha <sha> --run-id <id>
                             --tags <tags.json> --releases <releases.json>
                             [--main <main.json>] [--out <resolved.json>]
  release-policy.mjs bind-reservation --target <release_target> --canonical <X.Y.Z>
                             --source-sha <sha> --run-id <id> --releases <releases.json>
                             --action <create|reuse> [--reservation-id <id>] [--out <bound.json>]
  release-policy.mjs check-manifest --manifest <manifest.json> [--resolved <resolved.json>]
                             [--identity-record <release-identity.json>] [--report <out.json>]
  release-policy.mjs check-attestation --manifest <manifest.json> --attestation <attestation.json>
                             [--report <out.json>]
  release-policy.mjs check-identity --record <release-identity.json> --resolved <resolved.json>
                             --manifest <manifest.json> [--report <out.json>]
  release-policy.mjs check-secrets --secrets <names.json> [--signing adhoc] [--report <out.json>]

Exit code 0 means every policy rule passed; 1 means the release is rejected; 2 is bad usage.`;

/** A policy rule violation. `message` already reads `code: reason`. */
class PolicyError extends Error {
  constructor(code, message) {
    super(`${code}: ${message}`);
    this.name = "PolicyError";
    this.code = code;
  }
}

const messageOf = (error) =>
  error instanceof PolicyError ? error.message : `policy-error: ${error?.message ?? String(error)}`;

// ---------------------------------------------------------------------------
// SemVer
// ---------------------------------------------------------------------------

/** Parse `vX.Y.Z[-pre][+build]`. Leading zeroes and empty identifiers fail. */
export function parseVersion(text) {
  const value = typeof text === "string" ? text.trim() : "";
  const match = VERSION_RE.exec(value);
  if (!match) throw new PolicyError("version-invalid", `'${value}' is not a SemVer version`);
  const [major, minor, patch] = [match[2], match[3], match[4]];
  for (const part of [major, minor, patch]) {
    if (!NUMERIC_RE.test(part)) {
      throw new PolicyError(
        "version-invalid",
        `'${value}' has a numeric identifier with a leading zero`,
      );
    }
  }
  const prerelease = match[5] ?? null;
  const build = match[6] ?? null;
  if (prerelease !== null) {
    for (const identifier of prerelease.split(".")) {
      if (identifier === "") {
        throw new PolicyError("version-invalid", `'${value}' has an empty prerelease identifier`);
      }
      if (NUMERIC_RE.test(identifier) === false && /^\d+$/.test(identifier)) {
        throw new PolicyError(
          "version-invalid",
          `'${value}' has a prerelease identifier with a leading zero`,
        );
      }
    }
  }
  return {
    major: Number(major),
    minor: Number(minor),
    patch: Number(patch),
    prerelease,
    build,
    canonical: `${Number(major)}.${Number(minor)}.${Number(patch)}`,
  };
}

const asVersion = (value) => (value && typeof value === "object" ? value : parseVersion(value));

/** Compare only `MAJOR.MINOR.PATCH`, ignoring prerelease and build metadata. */
export function compareCore(left, right) {
  const a = asVersion(left);
  const b = asVersion(right);
  for (const key of ["major", "minor", "patch"]) {
    if (a[key] !== b[key]) return a[key] < b[key] ? -1 : 1;
  }
  return 0;
}

const comparePrereleaseIdentifier = (left, right) => {
  const leftNumeric = /^\d+$/.test(left);
  const rightNumeric = /^\d+$/.test(right);
  if (leftNumeric && rightNumeric)
    return Number(left) === Number(right) ? 0 : Number(left) < Number(right) ? -1 : 1;
  if (leftNumeric) return -1;
  if (rightNumeric) return 1;
  if (left === right) return 0;
  return left < right ? -1 : 1;
};

/** SemVer 2.0.0 precedence. Build metadata is not part of precedence. */
export function compareSemVer(left, right) {
  const a = asVersion(left);
  const b = asVersion(right);
  const core = compareCore(a, b);
  if (core !== 0) return core;
  if (a.prerelease === b.prerelease) return 0;
  if (a.prerelease === null) return 1;
  if (b.prerelease === null) return -1;
  const leftIds = a.prerelease.split(".");
  const rightIds = b.prerelease.split(".");
  for (let index = 0; index < Math.min(leftIds.length, rightIds.length); index += 1) {
    const order = comparePrereleaseIdentifier(leftIds[index], rightIds[index]);
    if (order !== 0) return order;
  }
  if (leftIds.length === rightIds.length) return 0;
  return leftIds.length < rightIds.length ? -1 : 1;
}

function bumpCore(core, bump) {
  const version = parseVersion(core);
  switch (bump) {
    case "major":
      return `${version.major + 1}.0.0`;
    case "minor":
      return `${version.major}.${version.minor + 1}.0`;
    case "patch":
      return `${version.major}.${version.minor}.${version.patch + 1}`;
    default:
      throw new PolicyError("target-invalid", `unknown symbolic bump '${bump}'`);
  }
}

// ---------------------------------------------------------------------------
// release_target
// ---------------------------------------------------------------------------

/**
 * `patch` / `minor` / `major` are symbolic; `X.Y.Z` (or `vX.Y.Z`) is explicit.
 * `raw` is preserved because the release identity records the owner's input.
 */
export function parseReleaseTarget(raw) {
  if (raw === undefined || raw === null || String(raw).trim() === "") {
    throw new PolicyError(
      "target-missing",
      "release_target is required; there is no default and it is never derived",
    );
  }
  const text = String(raw).trim();
  if (SYMBOLIC_TARGETS.includes(text)) return { kind: "symbolic", bump: text, raw: text };
  let version;
  try {
    version = parseVersion(text);
  } catch {
    throw new PolicyError(
      "target-invalid",
      `release_target '${text}' is neither patch/minor/major nor an explicit X.Y.Z`,
    );
  }
  if (!/^(v)?\d+\.\d+\.\d+$/.test(text)) {
    throw new PolicyError(
      "target-prerelease",
      `release_target '${text}' carries prerelease or build metadata; ${POLICY_ID} publishes stable SemVer only`,
    );
  }
  return { kind: "explicit", version: version.canonical, raw: text };
}

// ---------------------------------------------------------------------------
// Trusted baseline
// ---------------------------------------------------------------------------

/**
 * The trusted baseline is the highest core `MAJOR.MINOR.PATCH` of every existing
 * `vX.Y.Z` tag and GitHub Release, whatever its draft/prerelease/invalidated
 * state. Prerelease and build metadata occupy the same core tuple, so
 * `v1.0.0-rc.1` blocks a new `1.0.0`.
 *
 * A `v`-prefixed name that cannot be parsed fails closed rather than being
 * skipped: silently ignoring it could hide an occupied version.
 */
export function readBaseline(entries = []) {
  const occupied = new Map();
  for (const entry of entries) {
    const name = typeof entry === "string" ? entry : (entry?.name ?? entry?.tag_name);
    if (typeof name !== "string" || name.trim() === "") {
      throw new PolicyError("baseline-invalid", "a baseline entry has no version name");
    }
    const text = name.trim();
    if (!/^v\d/.test(text)) continue;
    let parsed;
    try {
      parsed = parseVersion(text);
    } catch (error) {
      throw new PolicyError(
        "baseline-invalid",
        `'${text}' looks like a version tag but is not valid SemVer (${error.message}); refusing to guess the baseline`,
      );
    }
    const existing = occupied.get(parsed.canonical);
    if (!existing || compareSemVer(parsed, existing) > 0) occupied.set(parsed.canonical, parsed);
  }
  const cores = [...occupied.keys()].sort(compareCore);
  return { baseline: cores.length === 0 ? null : cores[cores.length - 1], occupied: cores };
}

/**
 * Resolve the canonical version. A symbolic target with no baseline fails
 * closed; the first release must be explicit `0.1.0`; every other new identity
 * must be strictly greater than the baseline.
 */
export function resolveCanonicalVersion({ releaseTarget, baselineVersions = [] } = {}) {
  const target = parseReleaseTarget(releaseTarget);
  const { baseline, occupied } = readBaseline(baselineVersions);
  if (target.kind === "symbolic") {
    if (baseline === null) {
      throw new PolicyError(
        "baseline-missing",
        `release_target '${target.raw}' needs a trusted baseline, but no vX.Y.Z tag or release exists`,
      );
    }
    return { target, canonical: bumpCore(baseline, target.bump), baseline, occupied };
  }
  if (baseline === null) {
    if (target.version !== FIRST_RELEASE_VERSION) {
      throw new PolicyError(
        "first-release-version",
        `the first release must be explicit ${FIRST_RELEASE_VERSION}; got '${target.version}'`,
      );
    }
    return { target, canonical: target.version, baseline: null, occupied };
  }
  if (compareCore(target.version, baseline) <= 0) {
    throw new PolicyError(
      "version-not-greater-than-baseline",
      `explicit ${target.version} must be strictly greater than trusted baseline ${baseline}`,
    );
  }
  return { target, canonical: target.version, baseline, occupied };
}

// ---------------------------------------------------------------------------
// Reservation identity
// ---------------------------------------------------------------------------

export function reservationTagName(canonical, runId) {
  const id = runId === undefined || runId === null ? "" : String(runId).trim();
  if (!/^\d+$/.test(id)) {
    throw new PolicyError(
      "run-id-missing",
      `reservation tag needs the numeric workflow run id; got '${id}'`,
    );
  }
  const version = parseVersion(canonical);
  return `${RESERVATION_TAG_PREFIX}${version.canonical}-${id}`;
}

export function parseReservationTag(name) {
  const match = /^mien-reservation-(\d+\.\d+\.\d+)-(\d+)$/.exec(
    typeof name === "string" ? name : "",
  );
  return match ? { canonical: match[1], runId: match[2] } : null;
}

/** The machine-readable identity block written into a draft release body. */
export function formatIdentityBlock(identity) {
  return `${IDENTITY_BLOCK_START}\n${JSON.stringify(identity, null, 2)}\n${IDENTITY_BLOCK_END}`;
}

export function parseIdentityBlock(body) {
  if (typeof body !== "string") return null;
  const start = body.indexOf(IDENTITY_BLOCK_START);
  if (start === -1) return null;
  const from = start + IDENTITY_BLOCK_START.length;
  const end = body.indexOf(IDENTITY_BLOCK_END, from);
  if (end === -1) return null;
  try {
    const parsed = JSON.parse(body.slice(from, end));
    return parsed && typeof parsed === "object" && !Array.isArray(parsed) ? parsed : null;
  } catch {
    return null;
  }
}

/** A full identity match requires policy, target, version and source to agree. */
function sameIdentity(left, right) {
  if (!left || !right) return false;
  return (
    left.policy_id === right.policy_id &&
    left.release_target === right.release_target &&
    left.canonical_version === right.canonical_version &&
    left.source_sha === right.source_sha
  );
}

/**
 * Binding checks for one reservation: its identity block must agree with the
 * expected tuple, its tag name must embed that version, and its GitHub id must
 * match the id recorded in the body.
 */
function reservationBindingErrors(entry, expected) {
  const errors = [];
  const identity = entry.identity;
  if (!identity) {
    return [`reservation-identity-missing: ${entry.tagName} has no mien-release-identity block`];
  }
  if (identity.policy_id !== POLICY_ID) {
    errors.push(
      `reservation-policy-mismatch: ${entry.tagName} was reserved under ${JSON.stringify(identity.policy_id)}, expected ${POLICY_ID}`,
    );
  }
  if (identity.release_target !== expected.releaseTarget) {
    errors.push(
      `reservation-target-mismatch: ${entry.tagName} was reserved for release_target ${JSON.stringify(identity.release_target)}, expected ${JSON.stringify(expected.releaseTarget)}`,
    );
  }
  if (identity.canonical_version !== expected.canonicalVersion) {
    errors.push(
      `reservation-version-mismatch: ${entry.tagName} binds ${JSON.stringify(identity.canonical_version)}, expected ${expected.canonicalVersion}`,
    );
  }
  if (identity.source_sha !== expected.sourceSha) {
    errors.push(
      `reservation-source-mismatch: ${entry.tagName} binds ${JSON.stringify(identity.source_sha)}, expected ${expected.sourceSha}`,
    );
  }
  const parsedTag = parseReservationTag(entry.tagName);
  if (!parsedTag) {
    errors.push(
      `reservation-tag-invalid: '${entry.tagName}' is not ${RESERVATION_TAG_PREFIX}<X.Y.Z>-<run_id>`,
    );
  } else if (parsedTag.canonical !== identity.canonical_version) {
    errors.push(
      `reservation-tag-invalid: '${entry.tagName}' does not embed the reserved version ${identity.canonical_version}`,
    );
  }
  if (identity.reservation_id !== undefined && identity.reservation_id !== null) {
    if (String(identity.reservation_id) !== String(entry.release.id)) {
      errors.push(
        `reservation-id-mismatch: body records reservation ${identity.reservation_id}, GitHub release id is ${entry.release.id}`,
      );
    }
  }
  if (entry.release.draft !== true) {
    errors.push(
      `reservation-not-draft: ${entry.tagName} is not a draft; a published release is not a reservation`,
    );
  }
  if (SHA_RE.test(String(entry.release.target_commitish ?? ""))) {
    if (entry.release.target_commitish !== identity.source_sha) {
      errors.push(
        `reservation-source-mismatch: ${entry.tagName} targets ${entry.release.target_commitish}, identity binds ${identity.source_sha}`,
      );
    }
  }
  return errors;
}

/** Normalize `git ls-remote --tags` output, peeling annotated tags. */
export function parseLsRemoteTags(text) {
  const direct = new Map();
  const peeled = new Map();
  for (const line of String(text).split("\n")) {
    const trimmed = line.trim();
    if (trimmed === "") continue;
    const [sha, ref] = trimmed.split(/\s+/);
    if (!ref?.startsWith("refs/tags/")) continue;
    const name = ref.slice("refs/tags/".length);
    if (name.endsWith("^{}")) {
      peeled.set(name.slice(0, -3), sha);
      continue;
    }
    direct.set(name, sha);
  }
  return [...direct.entries()]
    .map(([name, sha]) => ({ name, commit_sha: peeled.get(name) ?? sha }))
    .sort((left, right) => left.name.localeCompare(right.name));
}

// ---------------------------------------------------------------------------
// Collision rules
// ---------------------------------------------------------------------------

function versionOfName(name) {
  if (typeof name !== "string" || !/^v\d/.test(name)) return null;
  try {
    return parseVersion(name).canonical;
  } catch {
    return null;
  }
}

function normalizeTags(tags) {
  return tags
    .map((tag) =>
      typeof tag === "string"
        ? { name: tag, commit: null, identity: null, version: versionOfName(tag) }
        : {
            name: tag?.name ?? "",
            commit: tag?.commit_sha ?? tag?.commit ?? null,
            identity: tag?.identity ?? null,
            version: versionOfName(tag?.name),
          },
    )
    .filter((tag) => tag.name !== "");
}

/**
 * Existing versions always occupy their number. The current identity's full
 * matches are excluded from the collision set: its own reservation, and any tag
 * or release whose identity block is identical (policy, target, version,
 * source). Everything else at or above the target fails closed.
 */
export function collisionErrors({
  canonicalVersion,
  identity,
  tags = [],
  releases = [],
  chosenReservationId = null,
}) {
  const errors = [];
  const matches = (candidate) => sameIdentity(candidate, identity);

  for (const tag of normalizeTags(tags)) {
    if (tag.version === null) continue;
    const order = compareCore(tag.version, canonicalVersion);
    if (order < 0) continue;
    if (identity && order === 0) {
      // The only same-version tag this identity may keep is its own: the peeled
      // commit must be the bound source and any identity block must match.
      if (tag.commit === identity.source_sha && (!tag.identity || matches(tag.identity))) continue;
      if (tag.commit !== null && tag.commit !== identity.source_sha) {
        errors.push(
          `tag-source-mismatch: tag ${tag.name} points at ${tag.commit}, identity binds ${identity.source_sha}`,
        );
        continue;
      }
    }
    errors.push(
      `collision: tag ${tag.name} occupies ${tag.version}; target is ${canonicalVersion}`,
    );
  }

  for (const release of releases) {
    const name = typeof release?.tag_name === "string" ? release.tag_name : "";
    const version = versionOfName(name);
    if (version === null) continue;
    if (compareCore(version, canonicalVersion) < 0) continue;
    const releaseIdentity = parseIdentityBlock(release.body);
    if (identity && version === canonicalVersion && matches(releaseIdentity)) continue;
    errors.push(`collision: release ${name} occupies ${version}; target is ${canonicalVersion}`);
  }

  for (const release of releases) {
    const name = typeof release?.tag_name === "string" ? release.tag_name : "";
    if (!name.startsWith(RESERVATION_TAG_PREFIX)) continue;
    if (chosenReservationId !== null && String(release.id) === String(chosenReservationId))
      continue;
    const reservationIdentity = parseIdentityBlock(release.body);
    if (!reservationIdentity) continue;
    if (reservationIdentity.canonical_version !== canonicalVersion) continue;
    if (identity && matches(reservationIdentity)) continue;
    errors.push(
      `reservation-conflict: ${name} already reserves ${canonicalVersion} for release_target ${JSON.stringify(reservationIdentity.release_target)} from ${reservationIdentity.source_sha}`,
    );
  }

  return errors;
}

// ---------------------------------------------------------------------------
// Source and main-green gate
// ---------------------------------------------------------------------------

/**
 * A release starts from a normal PR merge on `main`, and that exact SHA must
 * have a green `CI Main` run. A new identity must be dispatched from the current
 * `main` head; a recovery keeps the reservation's SHA even after `main` moves,
 * without lowering the `CI Main` requirement.
 */
export function sourceGateErrors({ sourceSha, main, recovery = false }) {
  const errors = [];
  if (!SHA_RE.test(String(sourceSha ?? ""))) {
    errors.push(
      `source-invalid: source SHA must be a 40-character lowercase hex commit; got ${JSON.stringify(sourceSha)}`,
    );
  }
  if (!main || typeof main !== "object") {
    errors.push("main-not-green: no main/CI Main state was read");
    return errors;
  }
  if (main.dispatch_ref !== undefined && main.dispatch_ref !== "refs/heads/main") {
    errors.push(
      `source-not-main: releases are dispatched from refs/heads/main; got ${JSON.stringify(main.dispatch_ref)}`,
    );
  }
  if (!recovery && main.head_sha !== sourceSha) {
    errors.push(
      `stale-main: main head is ${JSON.stringify(main.head_sha)}, but the dispatched source is ${sourceSha}; re-dispatch from the current main`,
    );
  }
  const pr = main.merged_pr;
  if (!pr || typeof pr !== "object") {
    errors.push(`main-not-merged-from-pr: no merged pull request produced ${sourceSha}`);
  } else if (pr.merge_commit_sha !== sourceSha) {
    errors.push(
      `main-not-merged-from-pr: pull request #${pr.number ?? "?"} merged as ${JSON.stringify(pr.merge_commit_sha)}, not ${sourceSha}`,
    );
  } else if (pr.pr_required_conclusion !== "success") {
    errors.push(
      `main-not-green: CI PR / required for pull request #${pr.number ?? "?"} concluded ${JSON.stringify(pr.pr_required_conclusion)}`,
    );
  }
  const ci = main.ci_main;
  if (!ci || typeof ci !== "object" || ci.head_sha !== sourceSha) {
    errors.push(`main-not-green: no CI Main run for ${sourceSha}`);
  } else if (ci.conclusion !== "success") {
    errors.push(
      `main-not-green: CI Main for ${sourceSha} concluded ${JSON.stringify(ci.conclusion)}`,
    );
  } else if (ci.job !== undefined && (ci.job !== "verify" || ci.job_conclusion !== "success")) {
    errors.push(
      `main-not-green: CI Main / verify for ${sourceSha} concluded ${JSON.stringify(ci.job_conclusion)}`,
    );
  }
  return errors;
}

// ---------------------------------------------------------------------------
// planRelease: target -> baseline -> recovery -> collision
// ---------------------------------------------------------------------------

function readReservations(releases) {
  const reservations = [];
  for (const release of releases) {
    const tagName = typeof release?.tag_name === "string" ? release.tag_name : "";
    if (!tagName.startsWith(RESERVATION_TAG_PREFIX)) continue;
    reservations.push({ release, tagName, identity: parseIdentityBlock(release.body) });
  }
  return reservations;
}

/**
 * Resolve one dispatch. A reservation for the same policy and `release_target`
 * is a recovery: its canonical version and source SHA win, even when `main` has
 * moved. Otherwise the dispatch is a new identity resolved from the baseline.
 */
export function planRelease(input = {}) {
  const errors = [];
  let target;
  try {
    target = parseReleaseTarget(input.release_target);
  } catch (error) {
    return { ok: false, errors: [messageOf(error)], resolution: null };
  }

  const tags = Array.isArray(input.tags) ? input.tags : [];
  if (!Array.isArray(input.tags) || !Array.isArray(input.releases)) {
    // An unread baseline is not an empty baseline: with no tags, a symbolic target
    // would fail as "first release" and an explicit one would be accepted as new.
    errors.push(
      "baseline-unreadable: tags and releases must both be read; an API failure is not an empty baseline",
    );
  }
  const releases = Array.isArray(input.releases) ? input.releases : [];
  const reservations = readReservations(releases);
  for (const entry of reservations) {
    if (!entry.identity) {
      errors.push(
        `reservation-identity-missing: ${entry.tagName} has no parseable mien-release-identity block`,
      );
    }
  }

  const matching = reservations.filter(
    (entry) =>
      entry.identity?.policy_id === POLICY_ID && entry.identity?.release_target === target.raw,
  );
  const distinct = new Set(
    matching.map((entry) => `${entry.identity.canonical_version}@${entry.identity.source_sha}`),
  );
  if (distinct.size > 1) {
    errors.push(
      `reservation-ambiguous: ${matching.length} reservations for release_target '${target.raw}' disagree on version/source: ${[...distinct].join(", ")}`,
    );
  }

  let recovery = false;
  let canonical = null;
  let sourceSha = null;
  let identity = null;
  let action = null;
  let reservationTag = null;
  let reservationId = null;
  let baseline = null;
  let chosen = null;

  if (matching.length > 0) {
    recovery = true;
    chosen = matching[0];
    canonical = chosen.identity.canonical_version;
    sourceSha = chosen.identity.source_sha;
    identity = chosen.identity;
    action = "reuse";
    reservationTag = chosen.tagName;
    reservationId = chosen.release.id ?? null;
    errors.push(
      ...reservationBindingErrors(chosen, {
        releaseTarget: target.raw,
        canonicalVersion: canonical,
        sourceSha,
      }),
    );
  } else {
    if (!SHA_RE.test(String(input.source_sha ?? ""))) {
      errors.push(
        `source-invalid: source_sha must be a 40-character lowercase hex commit; got ${JSON.stringify(input.source_sha)}`,
      );
    }
    sourceSha = input.source_sha ?? null;
    try {
      const resolved = resolveCanonicalVersion({
        releaseTarget: target.raw,
        baselineVersions: [...tags, ...releases],
      });
      canonical = resolved.canonical;
      baseline = resolved.baseline;
    } catch (error) {
      errors.push(messageOf(error));
    }
    if (canonical !== null) {
      try {
        reservationTag = reservationTagName(canonical, input.run_id);
      } catch (error) {
        errors.push(messageOf(error));
      }
      identity = {
        policy_id: POLICY_ID,
        release_target: target.raw,
        canonical_version: canonical,
        source_sha: sourceSha,
        reservation_id: null,
      };
      action = "create";
    }
  }

  if (canonical === null || identity === null) {
    return { ok: false, errors, resolution: null };
  }

  errors.push(
    ...collisionErrors({
      canonicalVersion: canonical,
      identity,
      tags,
      releases,
      chosenReservationId: chosen?.release.id ?? null,
    }),
  );

  const resolution = {
    policy_id: POLICY_ID,
    release_target: target.raw,
    target_kind: target.kind,
    canonical_version: canonical,
    source_sha: sourceSha,
    baseline,
    recovery,
    action,
    reservation_tag: reservationTag,
    reservation_id: reservationId,
    tag_name: `v${canonical}`,
    identity,
  };
  return { ok: errors.length === 0, errors, resolution };
}

/**
 * The `reserve` job re-reads the draft and must find the same binding the
 * resolution recorded. For a new identity it validates the planned tag and
 * checks that the version is not already reserved by another identity.
 */
export function bindReservation({
  releaseTarget,
  canonicalVersion,
  sourceSha,
  runId,
  releases = [],
  action = "create",
  reservationId = null,
} = {}) {
  const errors = [];
  const reservations = readReservations(releases);
  let reservationTag = null;

  if (action === "reuse") {
    // Select by the version being claimed, not by full identity equality: a
    // reservation claiming this version from another source is a source
    // conflict, not an absent reservation.
    const sameVersion = reservations.filter(
      (entry) => entry.identity?.canonical_version === canonicalVersion,
    );
    if (sameVersion.length === 0) {
      errors.push(
        `reservation-missing: no draft claims ${canonicalVersion}; deleting a reservation abandons the identity`,
      );
    } else {
      const chosen =
        sameVersion.find(
          (entry) =>
            entry.identity.release_target === releaseTarget &&
            entry.identity.source_sha === sourceSha,
        ) ?? sameVersion[0];
      reservationTag = chosen.tagName;
      errors.push(
        ...reservationBindingErrors(chosen, { releaseTarget, canonicalVersion, sourceSha }),
      );
      if (
        reservationId !== null &&
        reservationId !== "" &&
        String(chosen.release.id) !== String(reservationId)
      ) {
        errors.push(
          `reservation-id-mismatch: resolved reservation ${reservationId}, GitHub release id is ${chosen.release.id}`,
        );
      }
    }
  } else {
    try {
      reservationTag = reservationTagName(canonicalVersion, runId);
    } catch (error) {
      errors.push(messageOf(error));
    }
    if (
      reservationTag !== null &&
      releases.some((release) => release?.tag_name === reservationTag)
    ) {
      errors.push(
        `reservation-tag-in-use: ${reservationTag} already exists; reuse it instead of inventing a new identity`,
      );
    }
    const planned = {
      policy_id: POLICY_ID,
      release_target: releaseTarget,
      canonical_version: canonicalVersion,
      source_sha: sourceSha,
    };
    for (const entry of reservations) {
      if (!entry.identity) continue;
      if (entry.identity.canonical_version !== canonicalVersion) continue;
      if (sameIdentity(entry.identity, planned)) continue;
      errors.push(
        `reservation-conflict: ${entry.tagName} already reserves ${canonicalVersion} for release_target ${JSON.stringify(entry.identity.release_target)} from ${entry.identity.source_sha}`,
      );
    }
  }

  return {
    ok: errors.length === 0,
    errors,
    action,
    reservation_tag: reservationTag,
    policy_id: POLICY_ID,
    release_target: releaseTarget,
    canonical_version: canonicalVersion,
    source_sha: sourceSha,
  };
}

// ---------------------------------------------------------------------------
// Artifact manifest and attestation coverage
// ---------------------------------------------------------------------------

/**
 * PKG-009 / RELEASE.md §2: a release candidate is only complete when these
 * artifacts exist. The seccomp profile is a host-side release artifact — the
 * container cannot enforce it from inside.
 */
export const ARTIFACT_REQUIREMENTS = [
  {
    key: "dmg",
    code: "missing-dmg",
    description: "macOS DMG",
    matches: (name) => /^Mien_.+_aarch64\.dmg$/.test(name),
  },
  {
    key: "sums",
    code: "missing-sha256sums",
    description: "SHA256SUMS",
    matches: (name) => name === "SHA256SUMS",
  },
  {
    key: "sbom",
    code: "missing-sbom",
    description: "SBOM",
    matches: (name) => /\.(spdx|cdx)\.json$/.test(name),
  },
  {
    key: "license",
    code: "missing-license",
    description: "LICENSE",
    matches: (name) => name === "LICENSE",
  },
  {
    key: "notice",
    code: "missing-notice",
    description: "NOTICE",
    matches: (name) => name === "NOTICE",
  },
  {
    key: "licenses",
    code: "missing-licenses-dir",
    description: "LICENSES/ bundle",
    matches: (name) => name.startsWith("LICENSES/"),
  },
  {
    key: "seccomp",
    code: "missing-seccomp",
    description: "host-side seccomp release artifact",
    matches: (name, canonical) => name === `mien-seccomp-chromium-${canonical}.json`,
  },
  {
    key: "apparmor",
    code: "missing-apparmor",
    description: "mien-chromium AppArmor profile",
    matches: (name) => name === "mien-chromium.apparmor",
  },
  {
    key: "identity",
    code: "missing-identity",
    description: "release-identity.json",
    matches: (name) => name === "release-identity.json",
  },
  {
    key: "entitlements",
    code: "missing-entitlements",
    description: "entitlements.plist",
    matches: (name) => name === "entitlements.plist",
  },
  {
    key: "updater",
    code: "missing-updater-manifest",
    description: "updater manifest (latest.json)",
    matches: (name) => name === "latest.json",
  },
];

/**
 * Validate the collected artifact manifest. Digests must be shaped correctly,
 * SHA256SUMS must cover every artifact byte-for-byte, and the container digest
 * and kernel/Xray hashes must be recorded.
 */
export function artifactManifestErrors(manifest, { canonicalVersion } = {}) {
  if (!manifest || typeof manifest !== "object") {
    return ["manifest-invalid: no artifact manifest was recorded"];
  }
  const errors = [];
  if (manifest.canonical_version !== canonicalVersion) {
    errors.push(
      `manifest-version-mismatch: manifest records ${JSON.stringify(manifest.canonical_version)}, release is ${canonicalVersion}`,
    );
  }
  const files = (Array.isArray(manifest.files) ? manifest.files : []).filter(
    (file) => typeof file?.name === "string" && file.name !== "",
  );
  if (files.length === 0) errors.push("manifest-empty: no artifact files were recorded");
  for (const file of files) {
    if (!SHA256_RE.test(String(file.sha256 ?? ""))) {
      errors.push(`invalid-digest: ${file.name} sha256 must be 64 lowercase hex characters`);
    }
  }
  for (const requirement of ARTIFACT_REQUIREMENTS) {
    const found = files.some((file) => requirement.matches(file.name, canonicalVersion));
    if (found) continue;
    if (
      requirement.key === "seccomp" &&
      files.some((file) => /^mien-seccomp-chromium-.*\.json$/.test(file.name))
    ) {
      errors.push(
        `seccomp-version-mismatch: no mien-seccomp-chromium-${canonicalVersion}.json; the host-side profile must match the release version`,
      );
      continue;
    }
    errors.push(`${requirement.code}: ${requirement.description} is missing`);
  }
  if (!OCI_DIGEST_RE.test(String(manifest.docker?.digest ?? ""))) {
    errors.push(
      "missing-docker-digest: the container image digest is not recorded as sha256:<64 hex>",
    );
  }
  for (const binary of ["kernel", "xray"]) {
    if (!SHA256_RE.test(String(manifest.binaries?.[binary]?.sha256 ?? ""))) {
      errors.push(`missing-binary-digest: ${binary} SHA-256 is not recorded`);
    }
  }
  const sums = manifest.sums;
  if (!sums || typeof sums !== "object" || Object.keys(sums).length === 0) {
    errors.push("missing-sha256sums-content: SHA256SUMS was not parsed into per-file digests");
    return errors;
  }
  const names = new Set(files.map((file) => file.name));
  for (const file of files) {
    if (file.name === "SHA256SUMS") continue;
    const recorded = sums[file.name];
    if (recorded === undefined) {
      errors.push(`checksum-missing: ${file.name} is not listed in SHA256SUMS`);
    } else if (recorded !== file.sha256) {
      errors.push(
        `checksum-mismatch: SHA256SUMS says ${recorded} for ${file.name}, artifact is ${file.sha256}`,
      );
    }
  }
  for (const name of Object.keys(sums)) {
    if (name !== "SHA256SUMS" && !names.has(name)) {
      errors.push(`checksum-unknown: SHA256SUMS lists ${name}, which is not an artifact`);
    }
  }
  return errors;
}

/**
 * Attestation subjects required by RELEASE.md §5.6: the DMG, the kernel and
 * Xray binaries, the container digest, the SBOM, the license bundle, the seccomp
 * file and the AppArmor profile. `dmg`, `container`, `sbom` and `seccomp` are
 * also named directly by PKG-009.
 */
export const ATTESTATION_SUBJECTS = [
  "dmg",
  "container",
  "sbom",
  "seccomp",
  "kernel",
  "xray",
  "licenses",
  "apparmor",
];

/** The digest each attestation subject must cover, derived from the manifest. */
export function expectedAttestationSubjects(manifest) {
  const files = Array.isArray(manifest?.files) ? manifest.files : [];
  const digestOf = (matches) => {
    const file = files.find((entry) => typeof entry?.name === "string" && matches(entry.name));
    return typeof file?.sha256 === "string" ? file.sha256 : null;
  };
  return {
    dmg: digestOf((name) => /^Mien_.+_aarch64\.dmg$/.test(name)),
    container: typeof manifest?.docker?.digest === "string" ? manifest.docker.digest : null,
    sbom: digestOf((name) => /\.(spdx|cdx)\.json$/.test(name)),
    seccomp: digestOf((name) => /^mien-seccomp-chromium-.*\.json$/.test(name)),
    kernel:
      typeof manifest?.binaries?.kernel?.sha256 === "string"
        ? manifest.binaries.kernel.sha256
        : null,
    xray:
      typeof manifest?.binaries?.xray?.sha256 === "string" ? manifest.binaries.xray.sha256 : null,
    licenses: digestOf((name) => name.startsWith("LICENSES/")),
    apparmor: digestOf((name) => name === "mien-chromium.apparmor"),
  };
}

/**
 * Every subject must exist and cover the exact bytes in the manifest. There is
 * no "repository does not support attestations" branch: a missing subject stops
 * the release.
 */
export function attestationErrors(attestation, manifest) {
  const expected = expectedAttestationSubjects(manifest);
  const subjects = attestation?.subjects;
  if (!subjects || typeof subjects !== "object") {
    return ["missing-attestation: no attestation subject digests were recorded"];
  }
  const errors = [];
  for (const role of ATTESTATION_SUBJECTS) {
    const recorded = subjects[role];
    if (recorded === undefined || recorded === null || recorded === "") {
      errors.push(`missing-attestation-subject: ${role}`);
      continue;
    }
    if (expected[role] === null) {
      errors.push(`missing-attestation-digest: ${role} has no artifact digest to attest`);
      continue;
    }
    if (recorded !== expected[role]) {
      errors.push(
        `attestation-digest-mismatch: ${role} attestation covers ${recorded}, artifact is ${expected[role]}`,
      );
    }
  }
  return errors;
}

/** Build the `release-identity.json` payload from the resolution and manifest. */
export function buildIdentityRecord({
  resolution,
  manifest,
  publication = null,
  generatedAt = null,
}) {
  const expected = expectedAttestationSubjects(manifest);
  const fileName = (matches) =>
    (Array.isArray(manifest?.files) ? manifest.files : []).find(
      (entry) => typeof entry?.name === "string" && matches(entry.name),
    )?.name ?? null;
  return {
    policy_id: POLICY_ID,
    release_target: resolution.release_target,
    canonical_version: resolution.canonical_version,
    source_sha: resolution.source_sha,
    reservation_id: resolution.reservation_id ?? null,
    artifacts: {
      dmg: { name: fileName((name) => /^Mien_.+_aarch64\.dmg$/.test(name)), sha256: expected.dmg },
      docker: { image: manifest?.docker?.image ?? null, digest: expected.container },
      sbom: { name: fileName((name) => /\.(spdx|cdx)\.json$/.test(name)), sha256: expected.sbom },
      seccomp: {
        name: fileName(
          (name) => name === `mien-seccomp-chromium-${resolution.canonical_version}.json`,
        ),
        sha256: expected.seccomp,
      },
    },
    publication,
    generated_at: generatedAt,
  };
}

/**
 * `release-identity.json` must bind the same tuple and the same digests as this
 * build. A stage that rebuilds a different DMG cannot quietly overwrite the
 * identity: the digest comparison fails.
 */
export function identityRecordErrors(record, { resolution, manifest } = {}) {
  if (!record || typeof record !== "object") {
    return ["missing-identity-record: the release-identity.json content is absent"];
  }
  const errors = [];
  if (record.policy_id !== POLICY_ID) {
    errors.push(
      `identity-record-policy: record says ${JSON.stringify(record.policy_id)}, expected ${POLICY_ID}`,
    );
  }
  if (record.release_target !== resolution?.release_target) {
    errors.push(
      `identity-record-target: record says ${JSON.stringify(record.release_target)}, expected ${JSON.stringify(resolution?.release_target)}`,
    );
  }
  if (record.canonical_version !== resolution?.canonical_version) {
    errors.push(
      `identity-record-version: record says ${JSON.stringify(record.canonical_version)}, expected ${resolution?.canonical_version}`,
    );
  }
  if (record.source_sha !== resolution?.source_sha) {
    errors.push(
      `identity-record-source: record says ${JSON.stringify(record.source_sha)}, expected ${resolution?.source_sha}`,
    );
  }
  if (
    resolution?.reservation_id !== null &&
    resolution?.reservation_id !== undefined &&
    String(record.reservation_id) !== String(resolution.reservation_id)
  ) {
    errors.push(
      `identity-record-reservation: record says ${JSON.stringify(record.reservation_id)}, expected ${resolution.reservation_id}`,
    );
  }
  const expected = expectedAttestationSubjects(manifest);
  const artifacts = record.artifacts ?? {};
  const pairs = [
    ["dmg", artifacts.dmg?.sha256, expected.dmg],
    ["container", artifacts.docker?.digest, expected.container],
    ["sbom", artifacts.sbom?.sha256, expected.sbom],
    ["seccomp", artifacts.seccomp?.sha256, expected.seccomp],
  ];
  for (const [role, recorded, digest] of pairs) {
    if (recorded === undefined || recorded === null) {
      errors.push(`identity-digest-missing: ${role}`);
      continue;
    }
    if (digest !== null && recorded !== digest) {
      errors.push(
        `identity-digest-mismatch: ${role} record has ${recorded}, artifact is ${digest}`,
      );
    }
  }
  return errors;
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

function readJson(file) {
  return JSON.parse(readFileSync(file, "utf8"));
}
/**
 * A missing input file is a policy rejection (exit 1), not a usage error: the
 * release cannot proceed without it, and `gate-release` must fail closed on an
 * absent manifest or attestation rather than on a crashed CLI.
 */
function readJsonIfPresent(file) {
  try {
    return readJson(file);
  } catch (error) {
    if (error?.code === "ENOENT") return null;
    throw error;
  }
}

/**
 * Apple credentials must not exist at all (`docs/RELEASE.md` §3.9, PKG-009): the
 * release environment holds only the Tauri updater signing key.
 */
const FORBIDDEN_APPLE_SECRETS = [
  "APPLE_CERTIFICATE",
  "APPLE_CERTIFICATE_PASSWORD",
  "APPLE_SIGNING_IDENTITY",
  "APPLE_ID",
  "APPLE_TEAM_ID",
  "APPLE_PASSWORD",
  "APPLE_APP_SPECIFIC_PASSWORD",
  "APPLE_API_KEY",
  "APPLE_API_KEY_ID",
  "APPLE_API_ISSUER",
  "APPLE_NOTARY_KEY",
  "NOTARYTOOL_PASSWORD",
];

/**
 * Release secrets are allow-listed. Anything else in the release environment is
 * a stale credential nobody validated, so it fails rather than being ignored.
 */
const ALLOWED_RELEASE_SECRETS = ["TAURI_SIGNING_PRIVATE_KEY", "TAURI_SIGNING_PRIVATE_KEY_PASSWORD"];

/** Reject Apple signing/notarization credentials and unknown release secrets. */
export function secretPolicyErrors({ secretNames = [], signingMode } = {}) {
  const errors = [];
  const names = secretNames.map((name) => String(name).trim().toUpperCase()).filter(Boolean);
  for (const name of names) {
    if (FORBIDDEN_APPLE_SECRETS.includes(name)) {
      errors.push(
        `apple-credential-present: ${name} must not exist; macOS distribution is AdHoc only`,
      );
    } else if (!ALLOWED_RELEASE_SECRETS.includes(name)) {
      errors.push(
        `unexpected-release-secret: ${name} is not one of ${ALLOWED_RELEASE_SECRETS.join(", ")}`,
      );
    }
  }
  if (signingMode !== undefined && signingMode !== "adhoc") {
    errors.push(
      `signing-mode-not-adhoc: ${JSON.stringify(signingMode)} would require an Apple certificate`,
    );
  }
  return errors;
}

function writeJson(file, value) {
  const resolved = path.resolve(file);
  mkdirSync(path.dirname(resolved), { recursive: true });
  writeFileSync(resolved, `${JSON.stringify(value, null, 2)}\n`);
}

function parseArgs(argv) {
  const options = {};
  const positional = [];
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (!arg.startsWith("--")) {
      positional.push(arg);
      continue;
    }
    const key = arg.slice(2);
    const next = argv[index + 1];
    if (next === undefined || next.startsWith("--")) {
      options[key] = true;
      continue;
    }
    options[key] = next;
    index += 1;
  }
  return { command: positional[0] ?? null, options };
}

function emit(payload, options) {
  console.log(JSON.stringify(payload, null, 2));
  if (typeof options.out === "string") writeJson(options.out, payload);
  if (typeof options.report === "string") writeJson(options.report, payload);
  return payload.ok ? 0 : 1;
}

function runResolve(options) {
  const input = {
    release_target: options.target,
    source_sha: options["source-sha"],
    run_id: options["run-id"],
    tags: typeof options.tags === "string" ? readJson(options.tags) : null,
    releases: typeof options.releases === "string" ? readJson(options.releases) : null,
  };
  const plan = planRelease(input);
  const errors = [...plan.errors];
  // The green-main gate has no skip branch: `--main` is required, and its absence
  // is a rejection rather than a weaker check. `sourceGateErrors` reports a null
  // document as `main-not-green`, so an unreadable file also fails closed.
  if (typeof options.main !== "string") {
    errors.push(
      "main-not-green: --main is required; a release has no path that skips the CI Main check",
    );
  } else {
    errors.push(
      ...sourceGateErrors({
        sourceSha: plan.resolution?.source_sha ?? input.source_sha,
        main: readJsonIfPresent(options.main),
        recovery: plan.resolution?.recovery === true,
      }),
    );
  }
  const resolution = plan.resolution;
  return emit(
    {
      ok: errors.length === 0,
      errors,
      policy_id: POLICY_ID,
      release_target: resolution?.release_target ?? input.release_target ?? null,
      canonical_version: resolution?.canonical_version ?? null,
      source_sha: resolution?.source_sha ?? null,
      baseline: resolution?.baseline ?? null,
      recovery: resolution?.recovery ?? false,
      action: resolution?.action ?? null,
      reservation_tag: resolution?.reservation_tag ?? null,
      reservation_id: resolution?.reservation_id ?? null,
      tag_name: resolution?.tag_name ?? null,
    },
    options,
  );
}

function runTags(options) {
  if (typeof options.input !== "string") throw new PolicyError("usage", "tags needs --input");
  const tags = parseLsRemoteTags(readFileSync(options.input, "utf8"));
  console.log(JSON.stringify(tags, null, 2));
  if (typeof options.out === "string") writeJson(options.out, tags);
  return 0;
}

function runBind(options) {
  const bound = bindReservation({
    releaseTarget: options.target,
    canonicalVersion: options.canonical,
    sourceSha: options["source-sha"],
    runId: options["run-id"],
    releases: typeof options.releases === "string" ? readJson(options.releases) : [],
    action: options.action ?? "create",
    reservationId: options["reservation-id"] ?? null,
  });
  return emit(bound, options);
}

function runCheckManifest(options) {
  if (typeof options.manifest !== "string")
    throw new PolicyError("usage", "check-manifest needs --manifest");
  const manifest = readJsonIfPresent(options.manifest);
  const resolved =
    typeof options.resolved === "string" ? readJsonIfPresent(options.resolved) : null;
  const canonicalVersion = options.canonical ?? resolved?.canonical_version;
  const errors = artifactManifestErrors(manifest, { canonicalVersion });
  if (typeof options["identity-record"] === "string") {
    if (!resolved) throw new PolicyError("usage", "--identity-record needs --resolved");
    errors.push(
      ...identityRecordErrors(readJson(options["identity-record"]), {
        resolution: resolved,
        manifest,
      }),
    );
  }
  return emit(
    { ok: errors.length === 0, errors, canonical_version: canonicalVersion ?? null },
    options,
  );
}

function runCheckAttestation(options) {
  if (typeof options.manifest !== "string" || typeof options.attestation !== "string") {
    throw new PolicyError("usage", "check-attestation needs --manifest and --attestation");
  }
  const errors = attestationErrors(
    readJsonIfPresent(options.attestation),
    readJsonIfPresent(options.manifest),
  );
  return emit({ ok: errors.length === 0, errors }, options);
}

function runCheckIdentity(options) {
  if (
    typeof options.record !== "string" ||
    typeof options.resolved !== "string" ||
    typeof options.manifest !== "string"
  ) {
    throw new PolicyError("usage", "check-identity needs --record, --resolved and --manifest");
  }
  const errors = identityRecordErrors(readJsonIfPresent(options.record), {
    resolution: readJsonIfPresent(options.resolved),
    manifest: readJsonIfPresent(options.manifest),
  });
  return emit({ ok: errors.length === 0, errors }, options);
}

function runCheckSecrets(options) {
  if (typeof options.secrets !== "string") {
    // Without the declaration there is nothing to check against, and "no evidence"
    // must not read as "no forbidden credential".
    return emit(
      {
        ok: false,
        errors: [
          "secrets-declaration-missing: pass --secrets with the release environment secret names",
        ],
      },
      options,
    );
  }
  const names = readJsonIfPresent(options.secrets);
  if (!Array.isArray(names)) {
    return emit(
      {
        ok: false,
        errors: [
          `secrets-declaration-invalid: ${options.secrets} is not a JSON array of secret names`,
        ],
      },
      options,
    );
  }
  const errors = secretPolicyErrors({ secretNames: names, signingMode: options.signing });
  return emit({ ok: errors.length === 0, errors }, options);
}

export function main(argv = process.argv.slice(2)) {
  const { command, options } = parseArgs(argv);
  try {
    switch (command) {
      case "tags":
        return runTags(options);
      case "resolve":
        return runResolve(options);
      case "bind-reservation":
        return runBind(options);
      case "check-manifest":
        return runCheckManifest(options);
      case "check-attestation":
        return runCheckAttestation(options);
      case "check-identity":
        return runCheckIdentity(options);
      case "check-secrets":
        return runCheckSecrets(options);
      default:
        console.error(USAGE);
        return 2;
    }
  } catch (error) {
    console.error(messageOf(error));
    return 2;
  }
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === path.resolve(fileURLToPath(import.meta.url))
) {
  process.exitCode = main();
}

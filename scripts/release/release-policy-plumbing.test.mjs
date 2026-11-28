// PKG-009 plumbing: the selectors that turn GitHub API JSON into policy facts, and
// the manifest builder that recomputes digests from the bytes on disk.
//
// The selectors matter because the policy trusts whatever shape they emit. A
// selector that quietly picks the newest `CI Main` run, or a PR that was not
// merged, would make the green-main rule vacuous even though the policy itself
// is correct. So each selector is tested for the record it must *not* pick.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";

import {
  buildMainRecord,
  selectCiMainRun,
  selectCiPrRun,
  selectJob,
  selectMergedPr,
} from "./github-facts.mjs";
import { buildManifest, listAssets, parseSums, sha256File } from "./release-manifest.mjs";

const SHA = "0123456789abcdef0123456789abcdef01234567";
const OTHER = "b".repeat(40);

const run = (overrides = {}) => ({
  id: 1,
  head_sha: SHA,
  event: "push",
  head_branch: "main",
  status: "completed",
  conclusion: "success",
  ...overrides,
});

// ---------------------------------------------------------------------------
// Selectors
// ---------------------------------------------------------------------------

test("only a merged pull request counts as the release source", () => {
  const unmerged = { number: 1, merge_commit_sha: null, merged_at: null };
  const merged = { number: 2, merge_commit_sha: SHA, merged_at: "2026-11-27T00:00:00Z" };
  assert.equal(selectMergedPr([unmerged]), null);
  assert.equal(selectMergedPr([]), null);
  assert.equal(selectMergedPr([unmerged, merged]).number, 2);
});

test("CI Main must be a successful push on main for that exact SHA", () => {
  assert.equal(selectCiMainRun([run({ conclusion: "failure" })], SHA), null);
  assert.equal(selectCiMainRun([run({ head_sha: OTHER })], SHA), null);
  assert.equal(selectCiMainRun([run({ event: "workflow_dispatch" })], SHA), null);
  assert.equal(selectCiMainRun([run({ head_branch: "topic" })], SHA), null);
  assert.equal(selectCiMainRun([run({ status: "in_progress", conclusion: null })], SHA), null);
  assert.equal(selectCiMainRun([run()], SHA).id, 1);
});

test("a canceled or skipped CI Main is not green", () => {
  for (const conclusion of ["cancelled", "skipped", "timed_out", "neutral", null]) {
    assert.equal(selectCiMainRun([run({ conclusion })], SHA), null);
  }
});

test("CI PR accepts pull_request or merge_group but not push", () => {
  assert.equal(selectCiPrRun([run({ event: "push" })], SHA), null);
  assert.equal(selectCiPrRun([run({ event: "pull_request" })], SHA).id, 1);
  assert.equal(selectCiPrRun([run({ event: "merge_group" })], SHA).id, 1);
  assert.equal(selectCiPrRun([run({ event: "pull_request", conclusion: "failure" })], SHA), null);
});

test("a named job is selected by exact name, not by position", () => {
  const jobs = [
    { name: "lint", conclusion: "success" },
    { name: "verify", conclusion: "failure" },
  ];
  assert.equal(selectJob(jobs, "verify").conclusion, "failure");
  assert.equal(selectJob(jobs, "package-smoke"), null);
});

test("the assembled record has a null for every missing fact", () => {
  const record = buildMainRecord({
    headSha: SHA,
    pulls: [],
    ciMainRun: null,
    ciMainJobs: [],
    ciPrRun: null,
    ciPrJobs: [],
    dispatchRef: "refs/heads/main",
  });
  assert.equal(record.merged_pr, null);
  assert.equal(record.ci_main, null);
});

test("the assembled record keeps a job's own conclusion, not the run's", () => {
  const record = buildMainRecord({
    headSha: SHA,
    pulls: [{ number: 9, merge_commit_sha: SHA, merged_at: "2026-11-27T00:00:00Z" }],
    ciPrRun: { id: 100 },
    ciPrJobs: [{ name: "required", conclusion: "success" }],
    ciMainRun: run({ id: 200 }),
    ciMainJobs: [{ name: "verify", conclusion: "failure" }],
    dispatchRef: "refs/heads/main",
  });
  assert.equal(record.merged_pr.pr_required_conclusion, "success");
  assert.equal(record.ci_main.conclusion, "success");
  // The policy checks `job_conclusion` separately, so a failed verify is not hidden
  // behind a successful run.
  assert.equal(record.ci_main.job_conclusion, "failure");
});

// ---------------------------------------------------------------------------
// Manifest builder
// ---------------------------------------------------------------------------

const digestOf = (content) => createHash("sha256").update(content).digest("hex");

function withAssets(build) {
  const dir = mkdtempSync(path.join(tmpdir(), "mien-manifest-"));
  try {
    return build(dir);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

test("sums parsing accepts binary markers and strips ./", () => {
  const sums = parseSums(
    [
      `${"a".repeat(64)}  Mien_0.2.0_aarch64.dmg`,
      `${"b".repeat(64)} *LICENSE`,
      "# a comment",
      "",
      "not a checksum line",
    ].join("\n"),
  );
  assert.deepEqual(sums, {
    "Mien_0.2.0_aarch64.dmg": "a".repeat(64),
    LICENSE: "b".repeat(64),
  });
});

test("the manifest digests are recomputed from the bytes", () => {
  withAssets((dir) => {
    writeFileSync(path.join(dir, "LICENSE"), "license text\n");
    writeFileSync(path.join(dir, "SHA256SUMS"), `${digestOf("license text\n")}  LICENSE\n`);
    const manifest = buildManifest({ assetsDir: dir, canonicalVersion: "0.2.0" });
    const license = manifest.files.find((file) => file.name === "LICENSE");
    assert.equal(license.sha256, digestOf("license text\n"));
    assert.equal(manifest.sums.LICENSE, digestOf("license text\n"));
  });
});

test("a SHA256SUMS that lies about the bytes is visible to the policy", () => {
  withAssets((dir) => {
    writeFileSync(path.join(dir, "LICENSE"), "license text\n");
    const sums = `${"0".repeat(64)}  LICENSE\n`;
    writeFileSync(path.join(dir, "SHA256SUMS"), sums);
    const manifest = buildManifest({ assetsDir: dir, canonicalVersion: "0.2.0" });
    assert.equal(manifest.sums.LICENSE, "0".repeat(64));
    assert.notEqual(manifest.files.find((file) => file.name === "LICENSE").sha256, "0".repeat(64));
  });
});

test("nested license files keep their release-relative path", () => {
  withAssets((dir) => {
    mkdirSync(path.join(dir, "LICENSES"));
    writeFileSync(path.join(dir, "LICENSES", "Apache-2.0.txt"), "apache\n");
    const names = listAssets(dir).map((asset) => asset.name);
    assert.deepEqual(names, ["LICENSES/Apache-2.0.txt"]);
    assert.equal(sha256File(path.join(dir, "LICENSES", "Apache-2.0.txt")), digestOf("apache\n"));
  });
});

test("an empty asset directory produces an incomplete manifest, not a fake one", () => {
  withAssets((dir) => {
    const manifest = buildManifest({ assetsDir: dir, canonicalVersion: "0.2.0" });
    assert.deepEqual(manifest.files, []);
    assert.deepEqual(manifest.sums, {});
    assert.deepEqual(manifest.docker, {});
    assert.deepEqual(manifest.binaries, {});
  });
});

test("build records are carried through untouched", () => {
  withAssets((dir) => {
    const manifest = buildManifest({
      assetsDir: dir,
      canonicalVersion: "0.2.0",
      dockerRecord: { image: "ghcr.io/ivan/mien-server", digest: `sha256:${"c".repeat(64)}` },
      kernelRecord: { sha256: "d".repeat(64) },
      xrayRecord: { sha256: "e".repeat(64) },
    });
    assert.equal(manifest.docker.digest, `sha256:${"c".repeat(64)}`);
    assert.equal(manifest.binaries.kernel.sha256, "d".repeat(64));
    assert.equal(manifest.binaries.xray.sha256, "e".repeat(64));
  });
});

test("manifest generation is deterministic for the same bytes", () => {
  const build = () =>
    withAssets((dir) => {
      writeFileSync(path.join(dir, "NOTICE"), "notice\n");
      return buildManifest({ assetsDir: dir, canonicalVersion: "0.2.0" });
    });
  const first = build();
  const second = build();
  assert.equal(first.asset_count, second.asset_count);
  assert.deepEqual(
    first.files.map((file) => file.sha256),
    second.files.map((file) => file.sha256),
  );
});

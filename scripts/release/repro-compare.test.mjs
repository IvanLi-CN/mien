// Tests for the reproducibility comparison. They run without Docker: the two
// build records are the only inputs, so every verdict the workflow can reach is
// exercised here, including the diagnostics that a failing build must print.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import process from "node:process";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { compare } from "./repro-compare.mjs";

const script = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "repro-compare.mjs");

const LAYERS = [
  {
    mediaType: "application/vnd.oci.image.layer.v1.tar+gzip",
    digest: "sha256:1111111111111111111111111111111111111111111111111111111111111111",
    size: 28247960,
  },
  {
    mediaType: "application/vnd.oci.image.layer.v1.tar+gzip",
    digest: "sha256:2222222222222222222222222222222222222222222222222222222222222222",
    size: 4096,
  },
  {
    mediaType: "application/vnd.oci.image.layer.v1.tar+gzip",
    digest: "sha256:3333333333333333333333333333333333333333333333333333333333333333",
    size: 16384,
  },
];

const record = (overrides = {}) => ({
  source_sha: "3607fd4be44385103efe022a70a75c013c9e5988",
  source_date_epoch: 1795827569,
  platform: "linux/amd64",
  frontend:
    "docker/dockerfile:1.29.0@sha256:572cebbc2b15e6f84c077742952674889cb63cd5e9b80abdffcbe587b6f19183",
  buildkit_image:
    "moby/buildkit:v0.35.1@sha256:baf6138eb8c37644800c7c913fba051e840ebb9ba13eaf92d92ef9a1712d0b4c",
  buildx_version: "v0.39.1",
  compression: "gzip",
  image_digest: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  oci_manifest_digest: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  config_digest: "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
  layers: LAYERS,
  diff_ids: [
    "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
    "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
    "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
  ],
  ...overrides,
});

test("identical records from two builders are equal", () => {
  const result = compare(record(), record());
  assert.equal(result.verdict, "equal");
  assert.equal(result.first_different_layer, null);
  assert.deepEqual(
    result.checks.filter((check) => !check.ok),
    [],
  );
});

test("different layer digests at the same index name that index", () => {
  const changed = structuredClone(LAYERS);
  changed[2] = { ...changed[2], digest: `sha256:${"9".repeat(64)}` };
  const result = compare(record(), record({ layers: changed }));
  assert.equal(result.verdict, "different");
  assert.equal(result.first_different_layer.index, 2);
  assert.equal(result.first_different_layer.a.digest, LAYERS[2].digest);
  assert.equal(result.first_different_layer.b.digest, `sha256:${"9".repeat(64)}`);
});

test("an extra layer is reported even when all shared layers match", () => {
  const longer = [
    ...LAYERS,
    {
      mediaType: "application/vnd.oci.image.layer.v1.tar+gzip",
      digest: `sha256:${"7".repeat(64)}`,
      size: 1024,
    },
  ];
  const result = compare(record(), record({ layers: longer }));
  assert.equal(result.verdict, "different");
  assert.equal(result.first_different_layer, null);
  const layerCheck = result.checks.find((check) => check.name === "layers");
  assert.equal(layerCheck.ok, false);
});

test("equal digests with a different SOURCE_DATE_EPOCH still fail", () => {
  const result = compare(record(), record({ source_date_epoch: 1795827570 }));
  assert.equal(result.verdict, "different");
  assert.equal(result.digestsAgree, true);
  assert.equal(result.parametersAgree, false);
});

test("equal digests from different source commits are rejected", () => {
  const result = compare(record(), record({ source_sha: "0".repeat(40) }));
  assert.equal(result.verdict, "different");
});

test("a record whose archive digest disagrees with buildx metadata is rejected", () => {
  const result = compare(record(), record({ oci_manifest_digest: `sha256:${"5".repeat(64)}` }));
  assert.equal(result.verdict, "different");
  const check = result.checks.find((entry) => entry.name === "archive_matches_metadata_b");
  assert.equal(check.ok, false);
});

test("a malformed record fails closed", () => {
  assert.throws(() => compare(record(), record({ image_digest: "not-a-digest" })), /image_digest/);
  assert.throws(
    () => compare(record(), record({ source_date_epoch: "1795827569" })),
    /source_date_epoch/,
  );
});

test("the CLI reports PASS, writes a report, and exits 0 when equal", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "mien-repro-compare-"));
  try {
    const a = path.join(dir, "a.json");
    const b = path.join(dir, "b.json");
    const report = path.join(dir, "report.json");
    const summary = path.join(dir, "summary.md");
    writeFileSync(a, JSON.stringify(record()));
    writeFileSync(b, JSON.stringify(record()));
    const stdout = execFileSync(
      process.execPath,
      [script, "--a", a, "--b", b, "--report", report, "--summary", summary],
      { encoding: "utf8" },
    );
    assert.match(stdout, /repro-compare: PASS/);
    assert.equal(JSON.parse(readFileSync(report, "utf8")).verdict, "equal");
    assert.match(readFileSync(summary, "utf8"), /verdict: \*\*equal\*\*/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("the CLI exits 1 and names the first differing layer", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "mien-repro-compare-"));
  try {
    const a = path.join(dir, "a.json");
    const b = path.join(dir, "b.json");
    const changed = structuredClone(LAYERS);
    changed[1] = { ...changed[1], digest: `sha256:${"8".repeat(64)}` };
    writeFileSync(a, JSON.stringify(record()));
    writeFileSync(b, JSON.stringify(record({ layers: changed })));
    let outcome = null;
    try {
      execFileSync(process.execPath, [script, "--a", a, "--b", b], { encoding: "utf8" });
    } catch (error) {
      outcome = error;
    }
    assert.ok(outcome, "expected a non-zero exit");
    assert.equal(outcome.status, 1);
    assert.match(outcome.stderr, /first differing layer: index 1/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

// GATE-001 校验器测试：`node --test scripts/release/gate.test.mjs`。
//
// 每个用例都从一份全 pass 的报告出发，只改动被测字段，避免一份"恰好合法"的 fixture 掩盖其他规则。
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { buildSkeleton, main, REQUIRED_IDS, validateGate } from "./gate.mjs";

const NOW = new Date("2026-11-28T00:00:00Z");
const SOURCE_SHA = "0123456789abcdef0123456789abcdef01234567";
const DIGEST = "a".repeat(64);

const evidence = (name) => ({
  path: `release-evidence/${name}.json`,
  sha256: DIGEST,
});

/** 全 pass 的合法报告；waivers 为空。 */
function passingGate() {
  return {
    schema_version: 1,
    source_sha: SOURCE_SHA,
    generated_at: "2026-11-27T12:00:00Z",
    entries: REQUIRED_IDS.map((id) => ({
      id,
      status: "pass",
      evidence: [evidence(id.toLowerCase())],
      ran_at: "2026-11-27T11:00:00Z",
      command: `bun run test -- ${id}`,
    })),
    waivers: [],
  };
}

function entryOf(document, id) {
  const entry = document.entries.find((candidate) => candidate.id === id);
  assert.ok(entry, `fixture is missing ${id}`);
  return entry;
}

function check(document, options = {}) {
  return validateGate(document, { now: NOW, sourceSha: SOURCE_SHA, ...options });
}

function assertRejected(document, matcher, options) {
  const result = check(document, options);
  assert.equal(result.ok, false, "expected the gate to fail closed");
  assert.ok(
    result.errors.some((error) => matcher.test(error)),
    `no error matched ${matcher}:\n${result.errors.join("\n")}`,
  );
  return result;
}

const waiver = (id, overrides = {}) => ({
  id,
  severity: "P1",
  blocking_class: false,
  accepted_by: "Ivan Li",
  scope: `single finding ${id}`,
  expires_at: "2026-12-31T00:00:00Z",
  fix_plan: "fix in the next patch release",
  ...overrides,
});

test("the required set is the fixed 169 IDs from docs/TEST-PLAN.md:320", () => {
  const counts = {};
  for (const id of REQUIRED_IDS) {
    const prefix = id.replace(/-\d{3}$/, "");
    counts[prefix] = (counts[prefix] ?? 0) + 1;
  }
  assert.deepEqual(counts, {
    FP: 13,
    KERNEL: 10,
    ENGINE: 9,
    PROXY: 28,
    EXPORT: 6,
    UI: 22,
    RT: 26,
    API: 20,
    SEC: 9,
    "PRD-ACC": 10,
    PKG: 9,
    PERF: 7,
  });
  assert.equal(REQUIRED_IDS.length, 169);
  assert.equal(new Set(REQUIRED_IDS).size, 169);
  for (const id of ["PROXY-007", "PROXY-030", "PERF-008"]) assert.ok(!REQUIRED_IDS.includes(id));
});

test("an all-pass report passes", () => {
  const document = passingGate();
  const result = check(document);
  assert.deepEqual(result.errors, []);
  assert.equal(result.ok, true);
  assert.equal(result.counts.pass, 169);
});
test("the schema alone rejects PRD-ACC accepted", () => {
  const document = passingGate();
  entryOf(document, "PRD-ACC-002").status = "accepted";
  const errors = check(document).errors;
  assert.ok(
    errors.some(
      (error) => error.includes("/entries/") && error.includes("must be equal to constant"),
    ),
    `expected a schema violation, got:\n${errors.join("\n")}`,
  );
  assert.ok(errors.some((error) => /^PRD-ACC-002: PRD-ACC-001\.\.010 must be pass/.test(error)));
});

test("PRD-ACC-001 as accepted is rejected and the error names the ID", () => {
  const document = passingGate();
  entryOf(document, "PRD-ACC-001").status = "accepted";
  document.waivers.push(waiver("PRD-ACC-001"));
  const result = assertRejected(document, /^PRD-ACC-001: .*must be pass/);
  assert.equal(result.ok, false);
});

test("PRD-ACC-005 as missing is rejected", () => {
  const document = passingGate();
  entryOf(document, "PRD-ACC-005").status = "missing";
  assertRejected(document, /^PRD-ACC-005: status=missing blocks release/);
});

test("a pass entry with empty evidence is rejected", () => {
  const document = passingGate();
  entryOf(document, "RT-002").evidence = [];
  assertRejected(document, /^RT-002: status=pass requires at least one evidence item/);
});

test("a source_sha that differs from the target commit is rejected", () => {
  const document = passingGate();
  document.source_sha = "f".repeat(40);
  assertRejected(document, /source_sha .* does not match the target commit/);
});

test("a gate without a target commit fails closed", () => {
  assertRejected(passingGate(), /cannot determine the target commit/, { sourceSha: undefined });
});

test("an expired waiver is rejected", () => {
  const document = passingGate();
  entryOf(document, "RT-007").status = "accepted";
  document.waivers.push(waiver("RT-007", { expires_at: "2026-11-27T00:00:00Z" }));
  assertRejected(document, /^RT-007: waiver expired/);
});

test("a P0 waiver is rejected", () => {
  const document = passingGate();
  entryOf(document, "RT-008").status = "accepted";
  document.waivers.push(waiver("RT-008", { severity: "P0" }));
  assertRejected(document, /^RT-008: waiver severity must be P1/);
});

test("a blocking_class waiver is rejected", () => {
  const document = passingGate();
  entryOf(document, "RT-009").status = "accepted";
  document.waivers.push(waiver("RT-009", { blocking_class: true }));
  assertRejected(document, /^RT-009: waiver blocking_class must be false/);
});

test("accepted without a waiver record is rejected", () => {
  const document = passingGate();
  entryOf(document, "API-003").status = "accepted";
  assertRejected(document, /^API-003: status=accepted requires a waiver record/);
});

test("a named, unexpired P1 waiver makes a non-ACC accepted entry pass", () => {
  const document = passingGate();
  entryOf(document, "UI-014").status = "accepted";
  document.waivers.push(waiver("UI-014"));
  const result = check(document);
  assert.deepEqual(result.errors, []);
  assert.equal(result.ok, true);
  assert.equal(result.counts.accepted, 1);
});
test("a waiver without a matching accepted entry is rejected", () => {
  const document = passingGate();
  document.waivers.push(waiver("RT-011"));
  assertRejected(document, /^RT-011: waiver has no accepted entry/);
});

test("a waiver on a pass entry is rejected", () => {
  const document = passingGate();
  document.waivers.push(waiver("RT-011"));
  assertRejected(document, /^RT-011: waiver has no accepted entry \(entry status is pass\)/);
});

test("a waiver for a non-gate ID is rejected", () => {
  const document = passingGate();
  document.waivers.push(waiver("PROXY-030"));
  assertRejected(document, /^PROXY-030: not a GATE-001 ID; waivers cannot cover/);
});

test("a waiver for an unknown ID is rejected", () => {
  const document = passingGate();
  document.waivers.push(waiver("REG-004"));
  assertRejected(document, /^REG-004: unknown ID; waivers must name a GATE-001 ID/);
});

test("a waiver for a PRD-ACC ID is rejected even when the entry is accepted", () => {
  const document = passingGate();
  entryOf(document, "PRD-ACC-010").status = "accepted";
  document.waivers.push(waiver("PRD-ACC-010"));
  assertRejected(document, /^PRD-ACC-010: PRD-ACC-001\.\.010 have no waiver path/);
});

test("an unknown ID is rejected", () => {
  const document = passingGate();
  document.entries.push({
    id: "FOO-001",
    status: "pass",
    evidence: [evidence("foo")],
    ran_at: "2026-11-27T11:00:00Z",
    command: "true",
  });
  assertRejected(document, /^FOO-001: unknown ID/);
});

for (const id of ["PERF-008", "PROXY-007", "PROXY-030"]) {
  test(`${id} cannot stand in for a Tier A gate result`, () => {
    const document = passingGate();
    document.entries.push({
      id,
      status: "pass",
      evidence: [evidence(id.toLowerCase())],
      ran_at: "2026-11-27T11:00:00Z",
      command: "true",
    });
    assertRejected(document, new RegExp(`^${id}: not a GATE-001 ID`));
  });
}

test("a missing required ID is rejected and listed", () => {
  const document = passingGate();
  document.entries = document.entries.filter((entry) => entry.id !== "KERNEL-004");
  const result = assertRejected(document, /missing required IDs: KERNEL-004/);
  assert.deepEqual(result.missingIds, ["KERNEL-004"]);
});

test("PRD-ACC-009 fails while any PERF-001..007 is not pass, and names the ID", () => {
  const document = passingGate();
  entryOf(document, "PERF-003").status = "fail";
  assertRejected(document, /PRD-ACC-009: requires PERF-001\.\.007 to pass; not passing: PERF-003/);
});

test("PRD-ACC-009 fails when a PERF entry is missing entirely", () => {
  const document = passingGate();
  document.entries = document.entries.filter((entry) => entry.id !== "PERF-006");
  const result = assertRejected(document, /not passing: PERF-006/);
  assert.ok(result.errors.some((error) => /missing required IDs: PERF-006/.test(error)));
});

test("only PRD-ACC-009 carries the PERF dependency", () => {
  const document = passingGate();
  entryOf(document, "PERF-002").status = "fail";
  const result = check(document);
  assert.ok(result.errors.some((error) => /^PERF-002: status=fail/.test(error)));
  const dependency = result.errors.filter((error) => error.includes("requires PERF-001..007"));
  assert.deepEqual(dependency, [
    "PRD-ACC-009: requires PERF-001..007 to pass; not passing: PERF-002",
  ]);
  assert.ok(!result.errors.some((error) => /^PRD-ACC-00[1-8]|^PRD-ACC-010/.test(error)));
});

test("--init writes a skeleton whose 169 entries are all missing and which fails the gate", () => {
  const directory = mkdtempSync(path.join(tmpdir(), "mien-release-gate-"));
  const out = path.join(directory, "release-gate.json");
  const report = path.join(directory, "gate-report.json");
  try {
    assert.equal(
      main(["--init", "--out", out, "--source-sha", SOURCE_SHA]),
      0,
      "skeleton generation is not a gate decision",
    );
    const skeleton = JSON.parse(readFileSync(out, "utf8"));
    assert.equal(skeleton.entries.length, 169);
    assert.equal(skeleton.waivers.length, 0);
    assert.equal(skeleton.source_sha, SOURCE_SHA);
    assert.deepEqual(
      skeleton.entries.map((entry) => entry.id),
      REQUIRED_IDS,
    );
    assert.ok(skeleton.entries.every((entry) => entry.status === "missing"));
    assert.ok(skeleton.entries.every((entry) => entry.evidence.length === 0));

    assert.equal(
      main(["--check", out, "--source-sha", SOURCE_SHA, "--report", report]),
      1,
      "an all-missing skeleton must not pass",
    );
    const conclusion = JSON.parse(readFileSync(report, "utf8"));
    assert.equal(conclusion.ok, false);
    assert.equal(conclusion.status_counts.missing, 169);
    assert.equal(conclusion.missing_ids.length, 0);
    assert.equal(conclusion.blocking.length, 169);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("--check fails closed when the report file does not exist", () => {
  const directory = mkdtempSync(path.join(tmpdir(), "mien-release-gate-"));
  try {
    const missing = path.join(directory, "release-gate.json");
    assert.equal(main(["--check", missing, "--source-sha", SOURCE_SHA]), 1);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("--check writes a machine-readable report for a legal gate", () => {
  const directory = mkdtempSync(path.join(tmpdir(), "mien-release-gate-"));
  const gateFile = path.join(directory, "release-gate.json");
  const reportFile = path.join(directory, "gate-report.json");
  try {
    writeFileSync(gateFile, JSON.stringify(passingGate(), null, 2));
    assert.equal(
      main(["--check", gateFile, "--source-sha", SOURCE_SHA, "--report", reportFile]),
      0,
    );
    const conclusion = JSON.parse(readFileSync(reportFile, "utf8"));
    assert.equal(conclusion.ok, true);
    assert.equal(conclusion.gate, "GATE-001");
    assert.equal(conclusion.required_count, 169);
    assert.equal(conclusion.source_sha, SOURCE_SHA);
    assert.equal(conclusion.expected_source_sha, SOURCE_SHA);
    assert.deepEqual(conclusion.errors, []);
    assert.deepEqual(conclusion.blocking, []);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("the skeleton builder shares the required ID list with the checker", () => {
  const skeleton = buildSkeleton({ sourceSha: SOURCE_SHA, generatedAt: NOW });
  assert.equal(skeleton.entries.length, REQUIRED_IDS.length);
  assert.deepEqual(
    skeleton.entries.map((entry) => entry.id),
    REQUIRED_IDS,
  );
});

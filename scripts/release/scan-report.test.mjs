// SBOM 与扫描结论测试：`node --test scripts/release/scan-report.test.mjs`。
//
// 用例覆盖三条不可退让的规则：SBOM 必须真的覆盖 OS 层；例外必须有理由/到期/责任人，
// 过期即失败；未覆盖的 HIGH/CRITICAL 一定阻断。合成 SBOM 只保留被测字段。
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { buildSummary, filterFindings, loadExceptions, validateSbom } from "./scan-report.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const NOW = new Date("2026-11-28T00:00:00Z");

const spdx = (names) => ({
  spdxVersion: "SPDX-2.3",
  packages: names.map((name) => ({ name, SPDXID: `SPDXRef-${name}` })),
});
const cyclonedx = (names) => ({
  bomFormat: "CycloneDX",
  specVersion: "1.7",
  components: names.map((name) => ({ name, type: "library" })),
});

const vulnerability = (id, pkg, severity, extra = {}) => ({
  VulnerabilityID: id,
  PkgName: pkg,
  InstalledVersion: "1.0-1",
  Severity: severity,
  Status: "affected",
  ...extra,
});

const trivyReport = (...vulnerabilities) => ({
  SchemaVersion: 2,
  Results: [
    {
      Target: `debian@sha256:${"a".repeat(64)}`,
      Class: "os-pkgs",
      Type: "debian",
      Vulnerabilities: vulnerabilities,
    },
  ],
});

const exceptionYaml = (entries) =>
  `exceptions:\n${entries
    .map(
      (entry) =>
        `  - id: ${entry.id}\n    reason: ${entry.reason}\n    owner: ${entry.owner}\n    expires: ${entry.expires}\n${entry.package ? `    package: ${entry.package}\n` : ""}`,
    )
    .join("")}`;

const goodException = {
  id: "CVE-2024-0001",
  reason:
    "No fixed version exists in the pinned Debian suite; the binary is not reachable from the container.",
  owner: "Ivan Li",
  expires: "2027-01-31",
};

test("a complete SBOM passes and reports its package count", () => {
  const result = validateSbom(spdx(["dpkg", "libc6", "xvfb"]), {
    format: "spdx-json",
    filename: "spdx.json",
    requiredPackages: ["libc6", "xvfb"],
  });
  assert.deepEqual(result.errors, []);
  assert.equal(result.count, 3);
  assert.equal(result.present, 2);
});

test("an SBOM without dpkg cannot prove the OS layer was scanned", () => {
  const result = validateSbom(spdx(["mien-server"]), {
    format: "spdx-json",
    filename: "spdx.json",
    requiredPackages: [],
  });
  assert.ok(result.errors.some((error) => error.includes("no dpkg package")));
});

test("an SBOM missing an apt package fails with the package name", () => {
  const result = validateSbom(cyclonedx(["dpkg", "libc6"]), {
    format: "cyclonedx-json",
    filename: "cdx.json",
    requiredPackages: ["libc6", "libgtk-3-0", "xvfb"],
  });
  assert.ok(result.errors.some((error) => error.includes("libgtk-3-0") && error.includes("xvfb")));
  assert.equal(result.present, 1);
});

test("an empty SBOM is rejected", () => {
  const result = validateSbom(
    { spdxVersion: "SPDX-2.3", packages: [] },
    {
      format: "spdx-json",
      filename: "spdx.json",
      requiredPackages: [],
    },
  );
  assert.ok(result.errors.some((error) => error.includes("contains no packages")));
});

test("a CycloneDX document without bomFormat is rejected", () => {
  const result = validateSbom(
    { components: [{ name: "dpkg" }] },
    {
      format: "cyclonedx-json",
      filename: "cdx.json",
      requiredPackages: [],
    },
  );
  assert.ok(result.errors.some((error) => error.includes("bomFormat")));
  assert.ok(result.errors.some((error) => error.includes("specVersion")));
});

test("the shipped exception list is valid and empty", () => {
  const { exceptions, errors } = loadExceptions(
    readFileSync(path.join(root, "deploy/docker/security-exceptions.yaml"), "utf8"),
    { now: NOW, filename: "security-exceptions.yaml" },
  );
  assert.deepEqual(errors, []);
  assert.deepEqual(exceptions, []);
});

test("an expired exception fails even when it matches", () => {
  const { exceptions, errors } = loadExceptions(
    exceptionYaml([{ ...goodException, expires: "2026-11-27" }]),
    { now: NOW },
  );
  assert.equal(exceptions.length, 1);
  assert.ok(errors.some((error) => error.includes("expired on 2026-11-27")));
});

test("an exception without owner or reason is rejected", () => {
  const text = `exceptions:\n  - id: CVE-2024-0001\n    reason: ""\n    expires: 2027-01-31\n`;
  const { errors } = loadExceptions(text, { now: NOW });
  assert.ok(errors.some((error) => error.includes("owner must be a non-empty string")));
  assert.ok(errors.some((error) => error.includes("reason must be a non-empty string")));
});

test("an empty file is not the same as an explicit empty list", () => {
  const { errors } = loadExceptions("", { now: NOW });
  assert.ok(errors.some((error) => error.includes("empty file")));
});

test("exception fields are strict", () => {
  const text = `exceptions:\n  - id: CVE-2024-0001\n    reason: r\n    owner: o\n    expires: 2027-02-30\n    severity: low\n`;
  const { errors } = loadExceptions(text, { now: NOW });
  assert.ok(errors.some((error) => error.includes("expires must be a YYYY-MM-DD date")));
  assert.ok(errors.some((error) => error.includes("unknown key(s): severity")));
});

test("CVE exceptions suppress only the named CVE", () => {
  const { exceptions } = loadExceptions(exceptionYaml([goodException]), { now: NOW });
  const filtered = filterFindings(
    trivyReport(
      vulnerability("CVE-2024-0001", "libc6", "HIGH"),
      vulnerability("CVE-2024-9999", "libc6", "HIGH"),
      vulnerability("CVE-2024-0001", "zlib1g", "CRITICAL"),
    ),
    exceptions,
  );
  // A CVE exception without `package` covers that CVE in every package, but
  // never a different CVE: CVE-2024-9999 still blocks.
  assert.equal(filtered.findings.length, 3);
  assert.equal(filtered.excepted.length, 2);
  assert.deepEqual(
    filtered.blocking.map((finding) => finding.VulnerabilityID),
    ["CVE-2024-9999"],
  );
  assert.equal(filtered.perException.get("CVE-2024-0001").length, 2);
});

test("a package-scoped CVE exception does not hide the same CVE in another package", () => {
  const { exceptions } = loadExceptions(exceptionYaml([{ ...goodException, package: "libc6" }]), {
    now: NOW,
  });
  const filtered = filterFindings(
    trivyReport(
      vulnerability("CVE-2024-0001", "libc6", "HIGH"),
      vulnerability("CVE-2024-0001", "zlib1g", "HIGH"),
    ),
    exceptions,
  );
  assert.equal(filtered.excepted.length, 1);
  assert.equal(filtered.excepted[0].PkgName, "libc6");
  assert.deepEqual(
    filtered.blocking.map((finding) => finding.PkgName),
    ["zlib1g"],
  );
});

test("a package-name exception suppresses every finding of that package", () => {
  const { exceptions } = loadExceptions(exceptionYaml([{ ...goodException, id: "libc6" }]), {
    now: NOW,
  });
  const filtered = filterFindings(
    trivyReport(
      vulnerability("CVE-2024-0001", "libc6", "HIGH"),
      vulnerability("CVE-2024-0002", "libc6", "CRITICAL"),
      vulnerability("CVE-2024-0003", "zlib1g", "HIGH"),
    ),
    exceptions,
  );
  assert.equal(filtered.excepted.length, 2);
  assert.deepEqual(
    filtered.blocking.map((finding) => finding.VulnerabilityID),
    ["CVE-2024-0003"],
  );
});

test("medium and low findings never block", () => {
  const filtered = filterFindings(
    trivyReport(
      vulnerability("CVE-2024-0001", "libc6", "MEDIUM"),
      vulnerability("CVE-2024-0002", "libc6", "LOW"),
    ),
    [],
  );
  assert.equal(filtered.findings.length, 0);
});

test("the summary mirrors trivy and keeps gate-ready evidence entries", () => {
  const { exceptions } = loadExceptions(exceptionYaml([goodException]), { now: NOW });
  const filtered = filterFindings(
    trivyReport(
      vulnerability("CVE-2024-0001", "libc6", "HIGH"),
      vulnerability("CVE-2024-0002", "zlib1g", "CRITICAL"),
    ),
    exceptions,
  );
  const summary = buildSummary({
    image: { ref: "mien-server:runtime-base", id: `sha256:${"b".repeat(64)}` },
    platform: "linux/amd64",
    generatedAt: NOW.toISOString(),
    toolVersions: { syft: "1.57.2", trivy: "0.76.2" },
    sbom: [
      {
        format: "spdx-json",
        path: "artifacts/sbom/x.spdx.json",
        sha256: "c".repeat(64),
        packages: 120,
        apt_packages_present: 38,
      },
      {
        format: "cyclonedx-json",
        path: "artifacts/sbom/x.cdx.json",
        sha256: "d".repeat(64),
        packages: 4000,
        apt_packages_present: 38,
      },
    ],
    filtered,
    exceptions,
    aptPackagesRequired: Array.from({ length: 38 }, (_, index) => `pkg-${index}`),
  });
  assert.equal(summary.vulnerabilities.total, 2);
  assert.equal(summary.vulnerabilities.excepted, 1);
  assert.equal(summary.vulnerabilities.blocking, 1);
  assert.deepEqual(summary.vulnerabilities.blocking_ids, ["CVE-2024-0002"]);
  assert.equal(summary.exceptions[0].status, "applied");
  assert.equal(summary.exceptions[0].matches, 1);
  assert.equal(summary.ok, false);
  assert.deepEqual(
    summary.release_gate_evidence.map((entry) => entry.sha256),
    ["c".repeat(64), "d".repeat(64)],
  );
  assert.deepEqual(summary.gate_ids, ["SEC-004", "PKG-006", "PKG-007"]);
  assert.equal(summary.sbom[0].apt_packages_required, 38);
});

test("an unmatched exception is reported but does not fail by itself", () => {
  const { exceptions } = loadExceptions(exceptionYaml([goodException]), { now: NOW });
  const filtered = filterFindings(trivyReport(), exceptions);
  const summary = buildSummary({
    image: { ref: "x" },
    platform: "linux/amd64",
    generatedAt: NOW.toISOString(),
    toolVersions: {},
    sbom: [],
    filtered,
    exceptions,
  });
  assert.equal(summary.exceptions[0].status, "unmatched");
  assert.equal(summary.ok, true);
});

#!/usr/bin/env node
// 镜像 SBOM 与漏洞扫描结论：校验 SBOM 完整性、应用已批准例外并生成可供门禁引用的摘要。
//
// `deploy/docker/scan-image.sh` 只负责调用 syft/trivy；数据语义都在这里，因此例外清单、
// SBOM 包覆盖和退出码都能用 `node --test` 在没有 Docker 的机器上覆盖。
//
// 退出码：0 = 无未接受的高危/严重；1 = 存在阻断项或 SBOM/例外清单不合规；2 = 用法错误。
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { parse as parseYaml } from "yaml";
import { aptPackages, parseDockerfile } from "./dockerfile.mjs";

const MODULE_DIR = path.dirname(fileURLToPath(import.meta.url));
const DEFAULT_ROOT = path.resolve(MODULE_DIR, "../..");
const DEFAULT_DOCKERFILE = "deploy/docker/Dockerfile.server";
const DEFAULT_EXCEPTIONS = "deploy/docker/security-exceptions.yaml";
const SEVERITIES = ["CRITICAL", "HIGH"];

const USAGE = `用法：
  scan-report.mjs --spdx <file> --cyclonedx <file> --trivy <file> [--exceptions <file>]
                  [--summary <file>] [--root DIR] [--dockerfile PATH]
                  [--image REF] [--image-id ID] [--platform PLATFORM]
                  [--generated-at RFC3339]

--spdx/--cyclonedx/--trivy 读取扫描产物；--exceptions 默认为 deploy/docker/security-exceptions.yaml。
SBOM 必须非空、格式正确、含基础镜像与 apt 安装的全部包；例外必须有 id/reason/expires/owner，
过期或未命中都会被打印，过期即失败；剩余 HIGH/CRITICAL 非零即退出 1。`;

function fail(message) {
  const error = new Error(message);
  error.usage = true;
  return error;
}

function sha256File(filename) {
  return createHash("sha256").update(readFileSync(filename)).digest("hex");
}

function readJson(filename, label) {
  if (!existsSync(filename)) throw new Error(`${label} missing: ${filename}`);
  const bytes = readFileSync(filename);
  if (bytes.length === 0) throw new Error(`${label} is empty: ${filename}`);
  try {
    return { document: JSON.parse(bytes.toString("utf8")), bytes: bytes.length, sha256: sha256File(filename) };
  } catch (error) {
    throw new Error(`${label} is not valid JSON (${filename}): ${error.message}`);
  }
}

/**
 * 校验一份 SBOM：非空、格式标识存在、包数量 > 0，且覆盖 dpkg（基础镜像的包数据库）
 * 与 Dockerfile 里 apt 安装的每个包。
 */
export function validateSbom(document, { format, filename, requiredPackages = [] }) {
  const errors = [];
  const names = new Set();
  let count = 0;
  if (format === "spdx-json") {
    if (typeof document.spdxVersion !== "string" || document.spdxVersion === "")
      errors.push(`${filename}: spdxVersion is missing`);
    const packages = Array.isArray(document.packages) ? document.packages : [];
    count = packages.length;
    for (const pkg of packages) if (typeof pkg?.name === "string") names.add(pkg.name);
  } else {
    if (document.bomFormat !== "CycloneDX") errors.push(`${filename}: bomFormat is not CycloneDX`);
    if (typeof document.specVersion !== "string" || document.specVersion === "")
      errors.push(`${filename}: specVersion is missing`);
    const components = Array.isArray(document.components) ? document.components : [];
    count = components.length;
    for (const component of components) if (typeof component?.name === "string") names.add(component.name);
  }
  if (count === 0) errors.push(`${filename}: contains no packages`);
  if (count > 0 && !names.has("dpkg"))
    errors.push(`${filename}: no dpkg package; the image OS layer (base image and apt packages) is missing`);
  const missing = requiredPackages.filter((name) => !names.has(name));
  if (missing.length > 0)
    errors.push(
      `${filename}: ${missing.length} apt package(s) from ${DEFAULT_DOCKERFILE} are absent: ${missing.join(", ")}`,
    );
  return { errors, count, present: requiredPackages.length - missing.length, missing };
}

function isDate(value) {
  if (typeof value !== "string" || !/^\d{4}-\d{2}-\d{2}$/.test(value)) return false;
  const parsed = new Date(`${value}T00:00:00Z`);
  return !Number.isNaN(parsed.getTime()) && parsed.toISOString().startsWith(value);
}

/**
 * 读取并校验例外清单。字段固定为 id/reason/expires/owner，可选 package 收窄 CVE 匹配范围。
 * 过期例外不是“已接受”，直接进 errors。
 */
export function loadExceptions(text, { now = new Date(), filename = DEFAULT_EXCEPTIONS } = {}) {
  const errors = [];
  let document;
  try {
    document = parseYaml(text);
  } catch (error) {
    return { exceptions: [], errors: [`${filename}: not valid YAML (${error.message})`] };
  }
  const raw = document?.exceptions;
  if (raw === undefined || raw === null) {
    if (document === null || document === undefined || Object.keys(document).length === 0)
      return { exceptions: [], errors: [`${filename}: empty file; write "exceptions: []" instead of leaving it blank`] };
    return { exceptions: [], errors: [`${filename}: top-level "exceptions" list is missing`] };
  }
  if (!Array.isArray(raw)) return { exceptions: [], errors: [`${filename}: "exceptions" must be a list`] };
  const seen = new Set();
  const exceptions = [];
  const today = now.toISOString().slice(0, 10);
  for (const [index, entry] of raw.entries()) {
    const where = `${filename}: exceptions[${index}]`;
    if (!entry || typeof entry !== "object" || Array.isArray(entry)) {
      errors.push(`${where}: must be a mapping`);
      continue;
    }
    const unknown = Object.keys(entry).filter(
      (key) => !["id", "reason", "expires", "owner", "package", "note"].includes(key),
    );
    if (unknown.length > 0) errors.push(`${where}: unknown key(s): ${unknown.join(", ")}`);
    for (const field of ["id", "reason", "owner"])
      if (typeof entry[field] !== "string" || entry[field].trim() === "")
        errors.push(`${where}: ${field} must be a non-empty string`);
    if (typeof entry.id === "string") {
      if (!/^[A-Za-z0-9:+._/-]+$/.test(entry.id))
        errors.push(`${where}: id must be a CVE id or package name, got ${entry.id}`);
      if (seen.has(entry.id)) errors.push(`${where}: duplicate id ${entry.id}`);
      seen.add(entry.id);
    }
    if (!isDate(entry.expires)) errors.push(`${where}: expires must be a YYYY-MM-DD date`);
    else if (entry.expires <= today) errors.push(`${where}: exception for ${entry.id} expired on ${entry.expires}`);
    if (entry.package !== undefined && (typeof entry.package !== "string" || entry.package.trim() === ""))
      errors.push(`${where}: package must be a non-empty string when present`);
    exceptions.push({
      id: entry.id,
      reason: entry.reason,
      owner: entry.owner,
      expires: entry.expires,
      package: entry.package,
      note: entry.note,
      line: index,
    });
  }
  return { exceptions, errors };
}

/** 例外是否覆盖这条发现：CVE 按 id 匹配（可用 package 收窄），否则按包名匹配。 */
export function exceptionMatches(exception, finding) {
  if (!exception?.id || !finding) return false;
  const byCve = exception.id.toUpperCase().startsWith("CVE-");
  const idHit = byCve
    ? exception.id === finding.VulnerabilityID
    : exception.id === finding.PkgName;
  if (!idHit) return false;
  return exception.package === undefined || exception.package === finding.PkgName;
}

function findingKey(finding) {
  return `${finding.VulnerabilityID}\t${finding.PkgName}@${finding.InstalledVersion ?? "?"}`;
}

/**
 * 把 trivy JSON 拆成“被例外覆盖”与“仍然阻断”两组，并逐条例外统计命中数。
 * `findings` 只保留 HIGH/CRITICAL；其他严重度不参与判定。
 */
export function filterFindings(trivy, exceptions) {
  const findings = [];
  for (const result of trivy?.Results ?? []) {
    for (const vulnerability of result?.Vulnerabilities ?? []) {
      if (!SEVERITIES.includes(vulnerability?.Severity)) continue;
      findings.push({
        VulnerabilityID: vulnerability.VulnerabilityID,
        PkgName: vulnerability.PkgName,
        InstalledVersion: vulnerability.InstalledVersion,
        FixedVersion: vulnerability.FixedVersion,
        Severity: vulnerability.Severity,
        Status: vulnerability.Status,
        Target: result.Target,
        Class: result.Class ?? null,
        Type: result.Type ?? null,
        Key: findingKey(vulnerability),
      });
    }
  }
  const perException = new Map(exceptions.map((exception) => [exception.id, []]));
  const excepted = [];
  const blocking = [];
  for (const finding of findings) {
    const hit = exceptions.find((exception) => exceptionMatches(exception, finding));
    if (hit) {
      excepted.push(finding);
      perException.get(hit.id).push(finding);
    } else blocking.push(finding);
  }
  return { findings, excepted, blocking, perException };
}

function bySeverity(findings) {
  const counts = { CRITICAL: 0, HIGH: 0 };
  for (const finding of findings) counts[finding.Severity] += 1;
  return counts;
}

function uniqueSorted(values) {
  return [...new Set(values)].sort();
}

/** 人类可读报告：先说明扫描对象，再逐条例外，最后才是仍阻断的发现。 */
export function renderText(report) {
  const lines = [];
  lines.push(`image: ${report.image.ref}${report.image.id ? ` (id ${report.image.id})` : ""}`);
  lines.push(`tools: syft ${report.tools.syft ?? "unknown"}, trivy ${report.tools.trivy ?? "unknown"}`);
  for (const sbom of report.sbom)
    lines.push(
      `sbom: ${sbom.path} format=${sbom.format} packages=${sbom.packages} apt_covered=${sbom.apt_packages_present}/${sbom.apt_packages_required} sha256=${sbom.sha256}`,
    );
  lines.push(
    `vulnerabilities: HIGH/CRITICAL total=${report.vulnerabilities.total} (CRITICAL ${report.vulnerabilities.by_severity.CRITICAL}, HIGH ${report.vulnerabilities.by_severity.HIGH}) excepted=${report.vulnerabilities.excepted} blocking=${report.vulnerabilities.blocking}`,
  );
  if (report.exceptions.length === 0) lines.push("exceptions: none declared");
  for (const exception of report.exceptions) {
    const scope = exception.package ? ` package=${exception.package}` : "";
    lines.push(
      `exception: ${exception.id}${scope} expires=${exception.expires} owner=${exception.owner} matches=${exception.matches} status=${exception.status}`,
    );
    lines.push(`  reason: ${exception.reason}`);
    for (const key of exception.sample) lines.push(`  suppressed: ${key}`);
  }
  if (report.blocking.length > 0) {
    lines.push("blocking findings (not covered by any exception):");
    for (const finding of report.blocking)
      lines.push(
        `  ${finding.severity}\t${finding.id}\t${finding.package}@${finding.installed_version}\tfixed=${finding.fixed_version ?? "-"}\tstatus=${finding.status ?? "-"}`,
      );
  }
  return `${lines.join("\n")}\n`;
}

/** 组装机器可读摘要；`release_gate_evidence` 的 path/sha256 形状与 GATE-001 证据一致。 */
export function buildSummary({
  image,
  platform,
  generatedAt,
  toolVersions,
  sbom,
  filtered,
  exceptions,
  dockerfile = DEFAULT_DOCKERFILE,
  aptPackagesRequired = [],
}) {
  const report = {
    schema_version: 1,
    gate_ids: ["SEC-004", "PKG-006", "PKG-007"],
    generated_at: generatedAt,
    dockerfile,
    image: { ref: image.ref, id: image.id ?? null, platform: platform ?? null },
    tools: toolVersions,
    sbom: sbom.map((entry) => ({
      format: entry.format,
      path: entry.path,
      sha256: entry.sha256,
      packages: entry.packages,
      apt_packages_required: aptPackagesRequired.length,
      apt_packages_present: entry.apt_packages_present,
    })),
    vulnerabilities: {
      total: filtered.findings.length,
      by_severity: bySeverity(filtered.findings),
      excepted: filtered.excepted.length,
      blocking: filtered.blocking.length,
      blocking_ids: uniqueSorted(filtered.blocking.map((finding) => finding.VulnerabilityID)),
      blocking_packages: uniqueSorted(filtered.blocking.map((finding) => finding.PkgName)),
    },
    exceptions: exceptions.map((exception) => {
      const matches = filtered.perException.get(exception.id) ?? [];
      return {
        id: exception.id,
        package: exception.package ?? null,
        reason: exception.reason,
        owner: exception.owner,
        expires: exception.expires,
        matches: matches.length,
        status: matches.length === 0 ? "unmatched" : "applied",
        sample: uniqueSorted(matches.map((finding) => finding.Key)).slice(0, 10),
      };
    }),
    blocking: filtered.blocking.map((finding) => ({
      id: finding.VulnerabilityID,
      package: finding.PkgName,
      installed_version: finding.InstalledVersion,
      fixed_version: finding.FixedVersion ?? null,
      severity: finding.Severity,
      status: finding.Status ?? null,
      target: finding.Target,
      class: finding.Class,
      type: finding.Type,
    })),
    release_gate_evidence: sbom.map((entry) => ({ path: entry.path, sha256: entry.sha256 })),
    ok: filtered.blocking.length === 0,
  };
  return report;
}

export function parseArgs(argv) {
  const options = {
    spdx: null,
    cyclonedx: null,
    trivy: null,
    exceptions: DEFAULT_EXCEPTIONS,
    summary: null,
    root: DEFAULT_ROOT,
    dockerfile: DEFAULT_DOCKERFILE,
    image: "unknown",
    imageId: null,
    platform: null,
    generatedAt: new Date().toISOString(),
    help: false,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    const next = () => {
      const value = argv[index + 1];
      if (value === undefined) throw fail(`${arg} requires a value`);
      index += 1;
      return value;
    };
    if (arg === "--help" || arg === "-h") options.help = true;
    else if (arg === "--spdx") options.spdx = next();
    else if (arg === "--cyclonedx") options.cyclonedx = next();
    else if (arg === "--trivy") options.trivy = next();
    else if (arg === "--exceptions") options.exceptions = next();
    else if (arg === "--summary") options.summary = next();
    else if (arg === "--root") options.root = path.resolve(next());
    else if (arg === "--dockerfile") options.dockerfile = next();
    else if (arg === "--image") options.image = next();
    else if (arg === "--image-id") options.imageId = next();
    else if (arg === "--platform") options.platform = next();
    else if (arg === "--generated-at") options.generatedAt = next();
    else throw fail(`unknown argument ${arg}`);
  }
  if (options.help) return options;
  for (const field of ["spdx", "cyclonedx", "trivy"])
    if (!options[field]) throw fail(`--${field} is required`);
  return options;
}

function aptCoverage(options) {
  const dockerfile = path.resolve(options.root, options.dockerfile);
  if (!existsSync(dockerfile)) throw new Error(`${options.dockerfile}: Dockerfile not found`);
  return aptPackages(parseDockerfile(readFileSync(dockerfile, "utf8")));
}

/** 证据路径相对仓库打印；仓库外的文件给出绝对路径，避免一串 `../`。 */
function displayPath(root, record) {
  const absolute = path.resolve(root, record);
  const relative = path.relative(root, absolute);
  return relative.startsWith("..") ? absolute : relative;
}

/** 供测试直接传入文档；`main` 负责读文件与计算摘要。 */
export function analyse({ spdx, cyclonedx, trivy, exceptions, requiredPackages, root, filenames }) {
  const errors = [];
  const sbom = [];
  for (const [format, document, record] of [
    ["spdx-json", spdx, filenames.spdx],
    ["cyclonedx-json", cyclonedx, filenames.cyclonedx],
  ]) {
    const validation = validateSbom(document, {
      format,
      filename: displayPath(root, record),
      requiredPackages,
    });
    errors.push(...validation.errors);
    sbom.push({
      format,
      path: displayPath(root, record),
      sha256: null,
      packages: validation.count,
      apt_packages_present: validation.present,
    });
  }
  const filtered = filterFindings(trivy, exceptions);
  return { errors, sbom, filtered };
}

export function main(argv = process.argv.slice(2)) {
  let options;
  try {
    options = parseArgs(argv);
  } catch (error) {
    console.error(`scan-report: ${error.message}\n\n${USAGE}`);
    return 2;
  }
  if (options.help) {
    console.log(USAGE);
    return 0;
  }

  try {
    const requiredPackages = aptCoverage(options);
    const spdx = readJson(options.spdx, "SPDX SBOM");
    const cyclonedx = readJson(options.cyclonedx, "CycloneDX SBOM");
    const trivy = readJson(options.trivy, "trivy report");
    const exceptionsText = existsSync(options.exceptions)
      ? readFileSync(options.exceptions, "utf8")
      : null;
    if (exceptionsText === null)
      throw new Error(
        `${options.exceptions}: exception list missing; the list is required even when it is empty`,
      );
    const { exceptions, errors: exceptionErrors } = loadExceptions(exceptionsText, {
      filename: options.exceptions,
    });

    const { errors, sbom, filtered } = analyse({
      spdx: spdx.document,
      cyclonedx: cyclonedx.document,
      trivy: trivy.document,
      exceptions,
      requiredPackages,
      root: options.root,
      filenames: { spdx: options.spdx, cyclonedx: options.cyclonedx },
    });
    for (const entry of sbom) {
      const record = entry.format === "spdx-json" ? spdx : cyclonedx;
      entry.sha256 = record.sha256;
    }
    const allErrors = [...exceptionErrors, ...errors];
    const report = buildSummary({
      image: { ref: options.image, id: options.imageId },
      platform: options.platform,
      generatedAt: options.generatedAt,
      toolVersions: {
        syft: process.env.MIEN_SYFT_VERSION ?? null,
        trivy: process.env.MIEN_TRIVY_VERSION ?? null,
        trivy_db: process.env.MIEN_TRIVY_DB_UPDATED_AT ?? null,
      },
      sbom,
      filtered,
      exceptions,
      dockerfile: options.dockerfile,
      aptPackagesRequired: requiredPackages,
    });
    for (const entry of report.exceptions) {
      if (entry.status === "unmatched")
        console.error(
          `scan-report: warning: exception ${entry.id} matched no HIGH/CRITICAL finding; remove it or confirm the scope`,
        );
    }
    if (options.summary) {
      mkdirSync(path.dirname(path.resolve(options.summary)), { recursive: true });
      writeFileSync(path.resolve(options.summary), `${JSON.stringify(report, null, 2)}\n`);
    }
    process.stdout.write(renderText(report));
    if (allErrors.length > 0) {
      console.error("scan-report: FAIL (evidence is not usable)");
      for (const error of allErrors) console.error(`  - ${error}`);
      return 1;
    }
    if (!report.ok) {
      console.error(
        `scan-report: FAIL (${report.vulnerabilities.blocking} HIGH/CRITICAL finding(s) are not excepted)`,
      );
      return 1;
    }
    console.log("scan-report: PASS (no unexcepted HIGH/CRITICAL findings)");
    return 0;
  } catch (error) {
    console.error(`scan-report: ${error.message}`);
    return 1;
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === path.resolve(fileURLToPath(import.meta.url)))
  process.exitCode = main();

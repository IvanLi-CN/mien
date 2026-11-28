#!/usr/bin/env bun
// GATE-001：校验或生成 `release-gate.json`。
//
// 规范见 `docs/TEST-PLAN.md` §11（门禁）与 §12（证据）、`docs/CI.md` §8、`docs/RELEASE.md` §2。
// 报告结构与 `docs/contracts/release-gate.schema.json` 对应；本文件额外实现 schema 无法表达的
// 语义：固定必需 ID 集合、PRD-ACC 只允许 pass、waiver 严重级别与到期、ACC-009 对 PERF 的依赖、
// 以及 `source_sha` 必须等于目标提交。缺报告、未知 ID、非门禁 ID 都是失败，没有 skip 分支。
// 不做网络请求，也不读 GitHub API；证据只校验存在性与 sha256 形状，不访问证据本身。
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import Ajv2020 from "ajv/dist/2020.js";
import addFormats from "ajv-formats";

const moduleRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const schemaPath = path.join(moduleRoot, "docs/contracts/release-gate.schema.json");

/** `docs/TEST-PLAN.md:320` 的固定 required 范围，写死防止 Tier B 或草稿条目混入门禁。 */
const ID_RANGES = [
  ["FP", 1, 13],
  ["KERNEL", 1, 10],
  ["ENGINE", 1, 9],
  ["PROXY", 1, 6],
  ["PROXY", 8, 29],
  ["EXPORT", 1, 6],
  ["UI", 1, 22],
  ["RT", 1, 26],
  ["API", 1, 20],
  ["SEC", 1, 9],
  ["PRD-ACC", 1, 10],
  ["PKG", 1, 9],
  ["PERF", 1, 7],
];

const pad = (value) => String(value).padStart(3, "0");

/** 门禁必需 ID，共 169 条（见 ID_RANGES 求和）。 */
export const REQUIRED_IDS = ID_RANGES.flatMap(([prefix, first, last]) =>
  Array.from({ length: last - first + 1 }, (_, index) => `${prefix}-${pad(first + index)}`),
);

/** 非门禁 ID：Tier B 协议矩阵与 PERF-008，可以记录但不能出现在门禁报告里。 */
export const NON_GATE_IDS = ["PERF-008", "PROXY-007", "PROXY-030"];

/** PRD-ACC-001..010 只允许 pass，不存在 waiver 路径。 */
export const ACCEPTANCE_IDS = REQUIRED_IDS.filter((id) => id.startsWith("PRD-ACC-"));

/** ACC-009 要求 PERF-001..007 全部 pass（`docs/TEST-PLAN.md:320`）。 */
export const ACCEPTANCE_PERF_IDS = REQUIRED_IDS.filter((id) => id.startsWith("PERF-"));

const STATUSES = ["pass", "fail", "accepted", "skipped", "missing"];
const requiredSet = new Set(REQUIRED_IDS);
const nonGateSet = new Set(NON_GATE_IDS);
const acceptanceSet = new Set(ACCEPTANCE_IDS);

const USAGE = `用法：
  gate.mjs --check <release-gate.json> [--report <out.json>] [--source-sha <sha>]
  gate.mjs --init --out <release-gate.json> [--source-sha <sha>]

--check 校验 schema 与 GATE-001 语义，失败非零退出；--init 生成全部为 missing 的骨架，
骨架本身校验不过，只用于把缺口显性化。`;

let compiledSchema;

function schemaValidator() {
  if (compiledSchema) return compiledSchema;
  const ajv = new Ajv2020({ allErrors: true, strict: true, strictTypes: false });
  addFormats(ajv);
  const schema = JSON.parse(readFileSync(schemaPath, "utf8"));
  compiledSchema = ajv.compile(schema);
  return compiledSchema;
}

function schemaErrors(document) {
  try {
    const validate = schemaValidator();
    if (validate(document)) return [];
    return (validate.errors ?? []).map(
      (error) => `schema${error.instancePath}: ${error.message ?? "violation"}`,
    );
  } catch (error) {
    return [`schema: ${error.message}`];
  }
}

/**
 * ACC-009 与 PERF 的跨条目依赖。返回错误消息；其他 ACC 不要求 PERF。
 * `statuses` 为 ID → status 的 Map。
 */
export function acc009PerfErrors(statuses) {
  if (statuses.get("PRD-ACC-009") !== "pass") return [];
  const failing = ACCEPTANCE_PERF_IDS.filter((id) => statuses.get(id) !== "pass");
  if (failing.length === 0) return [];
  return [`PRD-ACC-009: requires PERF-001..007 to pass; not passing: ${failing.join(", ")}`];
}

function statusCounts(statuses) {
  const counts = { absent: 0, invalid: 0 };
  for (const status of STATUSES) counts[status] = 0;
  for (const id of REQUIRED_IDS) {
    const status = statuses.get(id);
    if (status === undefined) counts.absent += 1;
    else if (STATUSES.includes(status)) counts[status] += 1;
    else counts.invalid += 1;
  }
  return counts;
}

/**
 * 校验报告。返回 `ok`、逐条错误、必需 ID 状态、阻断项与 waiver 使用记录。
 * `sourceSha` 是目标提交；无法确定时校验直接失败（fail-closed）。
 */
export function validateGate(document, { now = new Date(), sourceSha } = {}) {
  const errors = schemaErrors(document);
  const doc = document && typeof document === "object" && !Array.isArray(document) ? document : {};
  const entries = Array.isArray(doc.entries) ? doc.entries : [];
  const waivers = Array.isArray(doc.waivers) ? doc.waivers : [];
  const statuses = new Map();

  for (const [index, entry] of entries.entries()) {
    const id = entry && typeof entry === "object" && typeof entry.id === "string" ? entry.id : "";
    if (!requiredSet.has(id)) {
      errors.push(
        nonGateSet.has(id)
          ? `${id}: not a GATE-001 ID; Tier B proxy matrix and PERF-008 results are recorded outside the gate`
          : `${id || `entries[${index}]`}: unknown ID; not in the fixed GATE-001 required set`,
      );
      continue;
    }
    if (statuses.has(id)) {
      errors.push(`${id}: duplicate entry`);
      continue;
    }
    statuses.set(id, entry.status);
    const { status } = entry;
    if (status === "pass") {
      if (!Array.isArray(entry.evidence) || entry.evidence.length === 0)
        errors.push(`${id}: status=pass requires at least one evidence item`);
      if (entry.ran_at == null) errors.push(`${id}: status=pass requires ran_at`);
      if (entry.command == null) errors.push(`${id}: status=pass requires command`);
    } else if (status === "accepted") {
      if (acceptanceSet.has(id))
        errors.push(`${id}: PRD-ACC-001..010 must be pass; accepted is never allowed for them`);
    } else if (status === "fail" || status === "skipped" || status === "missing") {
      errors.push(`${id}: status=${status} blocks release`);
    }
  }

  const missingIds = REQUIRED_IDS.filter((id) => !statuses.has(id));
  if (missingIds.length > 0) errors.push(`missing required IDs: ${missingIds.join(", ")}`);

  errors.push(...acc009PerfErrors(statuses));

  const waiverById = new Map();
  for (const [index, waiver] of waivers.entries()) {
    const id =
      waiver && typeof waiver === "object" && typeof waiver.id === "string" ? waiver.id : "";
    if (!requiredSet.has(id)) {
      errors.push(
        nonGateSet.has(id)
          ? `${id}: not a GATE-001 ID; waivers cannot cover Tier B or PERF-008 results`
          : `${id || `waivers[${index}]`}: unknown ID; waivers must name a GATE-001 ID`,
      );
      continue;
    }
    if (acceptanceSet.has(id))
      errors.push(`${id}: PRD-ACC-001..010 have no waiver path; they must be pass`);
    if (waiverById.has(id)) {
      errors.push(`${id}: duplicate waiver`);
      continue;
    }
    waiverById.set(id, waiver);
    if (waiver.severity !== "P1")
      errors.push(
        `${id}: waiver severity must be P1 (got ${waiver.severity}); P0 cannot be accepted`,
      );
    if (waiver.blocking_class !== false)
      errors.push(`${id}: waiver blocking_class must be false (got ${waiver.blocking_class})`);
    const expiry = Date.parse(waiver.expires_at);
    if (Number.isNaN(expiry)) errors.push(`${id}: waiver expires_at must be an RFC 3339 timestamp`);
    else if (expiry <= now.getTime()) errors.push(`${id}: waiver expired at ${waiver.expires_at}`);
    for (const field of ["accepted_by", "scope", "fix_plan"])
      if (typeof waiver[field] !== "string" || waiver[field].trim() === "")
        errors.push(`${id}: waiver ${field} must be a non-empty string`);
  }

  for (const [id, status] of statuses) {
    if (status !== "accepted") continue;
    if (!waiverById.has(id)) errors.push(`${id}: status=accepted requires a waiver record`);
  }

  // 未配套 accepted 的 waiver 会让过期条目长期潜伏，
  // 因此同样拒绝：waiver 必须对应一个真实的 accepted 条目。
  for (const id of waiverById.keys())
    if (statuses.get(id) !== "accepted")
      errors.push(
        `${id}: waiver has no accepted entry (entry status is ${statuses.get(id) ?? "absent"}); remove it or mark the entry accepted`,
      );

  if (typeof sourceSha === "string" && sourceSha.length > 0) {
    if (doc.source_sha !== sourceSha)
      errors.push(
        `source_sha ${doc.source_sha ?? "(missing)"} does not match the target commit ${sourceSha}`,
      );
  } else {
    errors.push("cannot determine the target commit; pass --source-sha");
  }

  const blocking = REQUIRED_IDS.filter((id) => statuses.get(id) !== "pass").map((id) => ({
    id,
    status: statuses.get(id) ?? "absent",
  }));

  return {
    ok: errors.length === 0,
    errors,
    requiredCount: REQUIRED_IDS.length,
    missingIds,
    statuses,
    blocking,
    waiverById,
    counts: statusCounts(statuses),
  };
}

/** 报告不可读时的结论：仍然是失败，且不假定任何条目状态。 */
function unreadableResult(message) {
  const empty = new Map();
  return {
    ok: false,
    errors: [message],
    requiredCount: REQUIRED_IDS.length,
    missingIds: [],
    statuses: empty,
    blocking: [],
    waiverById: empty,
    counts: statusCounts(empty),
  };
}

/** 全部为 missing 的骨架；它本身必须校验失败，用来显式列出缺口而不是宣告通过。 */
export function buildSkeleton({ sourceSha, generatedAt = new Date() } = {}) {
  return {
    schema_version: 1,
    source_sha: sourceSha,
    generated_at: generatedAt.toISOString(),
    entries: REQUIRED_IDS.map((id) => ({
      id,
      status: "missing",
      evidence: [],
      ran_at: null,
      command: null,
    })),
    waivers: [],
  };
}

/** 机器可读结论；不含网络与远端状态，只反映本地报告与 HEAD 比对结果。 */
export function buildReport({ document, result, checkedAt = new Date(), sourceSha }) {
  const doc = document && typeof document === "object" ? document : {};
  const waiversUsed = [...result.waiverById].map(([id, waiver]) => ({
    id,
    accepted_by: waiver.accepted_by,
    scope: waiver.scope,
    expires_at: waiver.expires_at,
    fix_plan: waiver.fix_plan,
  }));
  return {
    schema_version: 1,
    gate: "GATE-001",
    ok: result.ok,
    source_sha: doc.source_sha ?? null,
    expected_source_sha: sourceSha ?? null,
    generated_at: doc.generated_at ?? null,
    checked_at: checkedAt.toISOString(),
    required_count: result.requiredCount,
    status_counts: result.counts,
    missing_ids: result.missingIds,
    blocking: result.blocking,
    waivers_used: waiversUsed,
    errors: result.errors,
  };
}

export function resolveHead(root = moduleRoot) {
  try {
    return execFileSync("git", ["rev-parse", "HEAD"], { cwd: root, encoding: "utf8" }).trim();
  } catch {
    return null;
  }
}

function writeJson(filename, value) {
  mkdirSync(path.dirname(path.resolve(filename)), { recursive: true });
  writeFileSync(filename, `${JSON.stringify(value, null, 2)}\n`);
}

export function parseArgs(argv) {
  const options = {
    check: null,
    init: false,
    out: null,
    report: null,
    sourceSha: null,
    help: false,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    const next = () => {
      const value = argv[index + 1];
      if (value === undefined) throw new Error(`${arg} requires a value`);
      index += 1;
      return value;
    };
    if (arg === "--check") options.check = next();
    else if (arg === "--init") options.init = true;
    else if (arg === "--out") options.out = next();
    else if (arg === "--report") options.report = next();
    else if (arg === "--source-sha") options.sourceSha = next();
    else if (arg === "--help" || arg === "-h") options.help = true;
    else throw new Error(`unknown argument ${arg}`);
  }
  if (options.help) return options;
  if (options.check && options.init) throw new Error("--check and --init are mutually exclusive");
  if (!options.check && !options.init) throw new Error("one of --check or --init is required");
  if (options.init && !options.out) throw new Error("--init requires --out");
  return options;
}

export function main(argv = process.argv.slice(2)) {
  let options;
  try {
    options = parseArgs(argv);
  } catch (error) {
    console.error(`release-gate: ${error.message}\n\n${USAGE}`);
    return 2;
  }
  if (options.help) {
    console.log(USAGE);
    return 0;
  }
  const sourceSha = options.sourceSha ?? resolveHead();
  if (options.init) {
    const skeleton = buildSkeleton({ sourceSha: sourceSha ?? null });
    writeJson(options.out, skeleton);
    console.log(
      `release-gate: wrote ${options.out} with ${skeleton.entries.length} entries, all missing`,
    );
    console.log("release-gate: the skeleton does not pass GATE-001; fill evidence before checking");
    return 0;
  }

  let document;
  try {
    if (!existsSync(options.check))
      throw new Error(
        `${options.check}: report file not found; a missing GATE-001 report is a failure, not a skip`,
      );
    document = JSON.parse(readFileSync(options.check, "utf8"));
  } catch (error) {
    const message =
      error instanceof SyntaxError
        ? `${options.check}: not valid JSON (${error.message})`
        : (error.message ?? String(error));
    console.error(`release-gate: FAIL\n  - ${message}`);
    // 缺报告也要留下机器可读结论，否则调用方只能看到一个无正文的失败退出码。
    if (options.report)
      writeJson(
        options.report,
        buildReport({ document: {}, result: unreadableResult(message), sourceSha }),
      );
    return 1;
  }

  const result = validateGate(document, { sourceSha: sourceSha ?? undefined });
  if (options.report) writeJson(options.report, buildReport({ document, result, sourceSha }));
  if (result.ok) {
    console.log(
      `release-gate: PASS (${result.requiredCount} required IDs, source_sha ${document.source_sha})`,
    );
    return 0;
  }
  console.error(`release-gate: FAIL (${result.errors.length} problem(s))`);
  for (const error of result.errors) console.error(`  - ${error}`);
  return 1;
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === path.resolve(fileURLToPath(import.meta.url))
) {
  process.exitCode = main();
}

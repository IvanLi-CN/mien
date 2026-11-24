#!/usr/bin/env bun
import { createHash } from "node:crypto";
import { mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import process from "node:process";
import Ajv2020 from "ajv/dist/2020.js";
import addFormats from "ajv-formats";
import { parse } from "yaml";

const workspaceRoot = process.cwd();
const root = process.env.CONTRACTS_ROOT ?? process.cwd();
const rel = (...parts) => path.join(root, ...parts);
const read = (...parts) => readFileSync(rel(...parts), "utf8");
const json = (...parts) => JSON.parse(read(...parts));
const sha256 = (...parts) =>
  createHash("sha256")
    .update(readFileSync(rel(...parts)))
    .digest("hex");
const errors = [];
const pass = (name) => console.log(`PASS ${name}`);
const fail = (name, error) => {
  errors.push(name);
  console.error(`FAIL ${name}: ${error}`);
};
const compare = (name, expected, actual) => {
  const left = [...expected].sort();
  const right = [...actual].sort();
  if (JSON.stringify(left) !== JSON.stringify(right))
    throw new Error(
      `${name} differs; missing: ${left.filter((value) => !right.includes(value)).join(", ") || "none"}; unexpected: ${right.filter((value) => !left.includes(value)).join(", ") || "none"}`,
    );
};

function run(name, fn) {
  try {
    fn();
    pass(name);
  } catch (error) {
    fail(name, error.message);
  }
}

function walkFiles(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const absolute = path.join(directory, entry.name);
    return entry.isDirectory() ? walkFiles(absolute) : entry.isFile() ? [absolute] : [];
  });
}

function schemaCheck() {
  const contracts = rel("docs/contracts");
  const paths = walkFiles(contracts).filter((file) => file.endsWith(".schema.json"));
  if (paths.length === 0) throw new Error("no JSON Schema files found");
  const ajv = new Ajv2020({
    allErrors: true,
    strict: true,
    strictTypes: false,
    strictRequired: false,
    validateFormats: true,
  });
  addFormats(ajv);
  const docs = new Map();
  for (const filename of paths) {
    const schema = JSON.parse(readFileSync(filename, "utf8"));
    docs.set(schema.$id, schema);
  }
  for (const schema of docs.values()) ajv.addSchema(schema);
  for (const schema of docs.values()) ajv.compile(schema);

  const manifestFiles = walkFiles(rel("docs/contracts/manifests")).filter((file) =>
    file.endsWith(".json"),
  );
  if (manifestFiles.length === 0)
    throw new Error("docs/contracts/manifests contains no JSON fixtures");
  let validated = 0;
  for (const filename of manifestFiles) {
    const instance = JSON.parse(readFileSync(filename, "utf8"));
    const schemaFile =
      instance.engine === "xray-core"
        ? "proxy-engine-manifest.schema.json"
        : "kernel-manifest.schema.json";
    const schema = JSON.parse(read("docs/contracts", schemaFile));
    const validate = ajv.getSchema(schema.$id);
    if (!validate(instance))
      throw new Error(`${path.relative(root, filename)}: ${ajv.errorsText(validate.errors)}`);
    validated += 1;
  }
  for (const [schemaFile, fixture] of [
    [
      "events.schema.json",
      {
        schema_version: "1",
        id: "fixture-event",
        sequence: 1,
        type: "profile.created",
        ts: "2026-01-01T00:00:00Z",
        payload: { profile_id: "fixture-profile", name: "fixture" },
      },
    ],
    [
      "fingerprint-spec.schema.json",
      {
        schema_version: 1,
        platform: "windows",
        platform_version: "10.0",
        brand: "Chrome",
        brand_version: "148.0.7778.215",
        seed: 1,
        hardware_concurrency: 4,
        languages: ["en-US"],
        timezone: "UTC",
        window: { width: 1280, height: 720 },
        webrtc: { disable_non_proxied_udp: true },
        network: { doh_mode: "disabled", ipv6_mode: "block" },
      },
    ],
  ]) {
    const schema = JSON.parse(read("docs/contracts", schemaFile));
    const validate = ajv.getSchema(schema.$id);
    if (!validate(fixture))
      throw new Error(`${schemaFile} fixture: ${ajv.errorsText(validate.errors)}`);
    validated += 1;
  }
  const fixtureRoot = rel("fixtures/contracts");
  if (statSync(fixtureRoot, { throwIfNoEntry: false })?.isDirectory()) {
    for (const filename of walkFiles(fixtureRoot).filter((file) => file.endsWith(".json"))) {
      const instance = JSON.parse(readFileSync(filename, "utf8"));
      const schemaPath = instance.$schemaFile;
      if (typeof schemaPath !== "string")
        throw new Error(`${path.relative(root, filename)}: fixture requires $schemaFile`);
      const schema = JSON.parse(read("docs/contracts", schemaPath));
      const candidate = { ...instance };
      delete candidate.$schemaFile;
      const validate = ajv.getSchema(schema.$id);
      if (!validate(candidate))
        throw new Error(`${path.relative(root, filename)}: ${ajv.errorsText(validate.errors)}`);
      validated += 1;
    }
  }
  if (validated === 0) throw new Error("no schema fixtures were validated");
}

function walkObjects(value, visitor, pointer = "#") {
  visitor(value, pointer);
  if (Array.isArray(value)) {
    for (const [index, child] of value.entries())
      walkObjects(child, visitor, `${pointer}/${index}`);
  } else if (value && typeof value === "object")
    for (const [key, child] of Object.entries(value))
      walkObjects(child, visitor, `${pointer}/${key}`);
}

function jsonPointer(document, fragment) {
  if (fragment === "" || fragment === "#") return document;
  if (!fragment.startsWith("#/")) throw new Error(`unsupported fragment ${fragment}`);
  return fragment
    .slice(2)
    .split("/")
    .reduce((value, raw) => {
      const token = decodeURIComponent(raw.replace(/~1/g, "/").replace(/~0/g, "~"));
      return value && typeof value === "object" ? value[token] : undefined;
    }, document);
}

function openapiCheck() {
  const source = read("docs/contracts/openapi.yaml");
  const document = parse(source, { uniqueKeys: true });
  if (document?.openapi !== "3.1.0") throw new Error("OpenAPI must parse as 3.1.0");
  walkObjects(document, (value, pointer) => {
    if (
      !value ||
      typeof value !== "object" ||
      Array.isArray(value) ||
      typeof value.$ref !== "string"
    )
      return;
    const [targetPath, fragment = ""] = value.$ref.split("#");
    if (targetPath !== "") {
      if (targetPath.startsWith("https://json-schema.org/")) return;
      const filename = path.resolve(rel("docs/contracts"), targetPath);
      if (
        !filename.startsWith(`${path.resolve(rel("docs/contracts"))}${path.sep}`) ||
        !statSync(filename, { throwIfNoEntry: false })?.isFile()
      )
        throw new Error(`${pointer}: unresolved external $ref ${value.$ref}`);
      const external = JSON.parse(readFileSync(filename, "utf8"));
      if (jsonPointer(external, fragment ? `#${fragment}` : "#") === undefined)
        throw new Error(`${pointer}: unresolved external $ref ${value.$ref}`);
      return;
    }
    if (jsonPointer(document, fragment ? `#${fragment}` : "#") === undefined)
      throw new Error(`${pointer}: unresolved $ref ${value.$ref}`);
  });
  const result = Bun.spawnSync(
    [
      path.join(workspaceRoot, "node_modules/.bin/redocly"),
      "lint",
      "--extends",
      "minimal",
      "--skip-rule",
      "tag-description",
      "--skip-rule",
      "operation-summary",
      "--skip-rule",
      "operation-2xx-response",
      rel("docs/contracts/openapi.yaml"),
    ],
    { cwd: workspaceRoot, stdout: "pipe", stderr: "pipe" },
  );
  if (result.exitCode !== 0)
    throw new Error(
      `Redocly lint failed (${result.exitCode}): ${result.stdout.toString()} ${result.stderr.toString()}`,
    );
}

const markdownFiles = () => [
  ...readdirSync(root, { withFileTypes: true })
    .filter((entry) => entry.isFile() && entry.name.endsWith(".md"))
    .map((entry) => path.join(root, entry.name)),
  ...walkFiles(rel("docs")).filter((file) => file.endsWith(".md")),
];
function markdownFilesUnder(relative) {
  const directory = rel(relative);
  if (!statSync(directory, { throwIfNoEntry: false })?.isDirectory()) return [];
  return walkFiles(directory).filter((file) => file.endsWith(".md"));
}
const slug = (value) =>
  value
    .toLowerCase()
    .replace(/[^\p{L}\p{N} _-]/gu, "")
    .trim()
    .replace(/ /g, "-");

function markdownCheck() {
  const files = process.env.CONTRACTS_ROOT ? markdownFilesUnder("docs") : markdownFiles();
  const contents = new Map(files.map((file) => [file, readFileSync(file, "utf8")]));
  const headings = new Map();
  for (const [filename, text] of contents) {
    const masked = text.replace(/^ {0,3}(`{3,}|~{3,})[^\n]*\n[\s\S]*?^ {0,3}\1[^\n]*$/gm, (block) =>
      block.replace(/[^\n]/g, " "),
    );
    const entries = [];
    for (const match of masked.matchAll(/^#{1,6}[ \t]+(.+?)[ \t]*#*[ \t]*$/gm))
      entries.push(slug(match[1]));
    headings.set(filename, new Set(entries));
  }
  const linkPattern =
    /!?\[[^\]]*\]\(\s*(<[^>]*>|[^\s)]*)(?:\s+(?:"[^"]*"|'[^']*'|\([^)]*\)))?\s*\)/g;
  const referencePattern = /^ {0,3}\[[^\]]+\]:[ \t]*(<[^>]*>|\S+)/gm;
  for (const [filename, source] of contents) {
    const codeMasked = source.replace(
      /^ {0,3}(`{3,}|~{3,})[^\n]*\n[\s\S]*?^ {0,3}\1[^\n]*$/gm,
      (block) => block.replace(/[^\n]/g, " "),
    );
    const targets = [...codeMasked.matchAll(linkPattern), ...codeMasked.matchAll(referencePattern)];
    for (const match of targets) {
      const target = match[1].replace(/^<|>$/g, "");
      if (!target || /^[a-z][a-z0-9+.-]*:/i.test(target) || target.startsWith("//")) continue;
      const [pathname, ...fragmentParts] = target.split("#");
      const fragment = fragmentParts.join("#");
      const targetFile = path.resolve(
        path.dirname(filename),
        decodeURIComponent(pathname || path.basename(filename)),
      );
      const targetStat = statSync(targetFile, { throwIfNoEntry: false });
      if (!targetStat || (!targetStat.isFile() && !targetStat.isDirectory()))
        throw new Error(`${path.relative(root, filename)}: broken link ${target}`);
      if (
        fragment &&
        headings.get(targetFile) &&
        !headings.get(targetFile).has(decodeURIComponent(fragment))
      )
        throw new Error(`${path.relative(root, filename)}: missing anchor ${target}`);
    }
  }
}

function errorCatalogCheck() {
  const text = read("docs/ERROR-CODES.md");
  const catalog = new Map();
  for (const line of text.split(/\r?\n/)) {
    const columns = line.split("|").map((column) => column.trim());
    const code = columns[1]?.match(/^`([A-Z][A-Z0-9_]+)`$/)?.[1];
    if (!code) continue;
    const httpIndex = columns.findIndex((column) => /^\d{3}$/.test(column));
    const http = httpIndex >= 0 ? Number(columns[httpIndex]) : NaN;
    const retryIndex = columns.findIndex(
      (column, index) => index > httpIndex && ["是", "否"].includes(column),
    );
    const retryable = retryIndex >= 0 && columns[retryIndex] === "是";
    if (catalog.has(code)) throw new Error(`${code}: duplicate catalog row`);
    catalog.set(code, { http, retryable });
    if (httpIndex < 0 || retryIndex < 0)
      throw new Error(`${code}: incomplete HTTP/retryable mapping`);
  }
  if (catalog.size === 0) throw new Error("no error code rows found");
  const openapi = parse(read("docs/contracts/openapi.yaml"));
  const events = json("docs/contracts/events.schema.json");
  const codeList = openapi.components.schemas.ErrorCode.enum;
  compare("OpenAPI ErrorCode", catalog.keys(), codeList);
  compare("events ErrorCode", catalog.keys(), events.$defs.errorCode.enum);

  const codesInDocument = new Set(
    [...text.matchAll(/`([A-Z][A-Z0-9_]+)`/g)].map((match) => match[1]),
  );
  for (const code of catalog.keys())
    if (!codesInDocument.has(code)) throw new Error(`${code} has no error-code catalog mention`);
  for (const code of codesInDocument)
    if (!catalog.has(code))
      throw new Error(`${code} is mentioned in the error-code directory but has no table row`);
  const rowlessCodes = new Set(
    [
      ...text.matchAll(
        /^[^|\n]*\b((?:AUTH|EVENT|PROFILE|PROXY|RUNTIME|TOKEN|GROUP|TAG|VALIDATION|FINGERPRINT|SHARE_LINK|KERNEL|UNSUPPORTED|QUOTA|INSUFFICIENT|SANDBOX|DIRECT|DISPLAY|STORAGE|INTERNAL|FORBIDDEN|RESOURCE|IDEMPOTENCY|VIEWER)_[A-Z0-9_]+)\b[^|\n]*$/gm,
      ),
    ].map((match) => match[1]),
  );
  for (const code of rowlessCodes)
    if (!catalog.has(code))
      throw new Error(`${code} appears in the error-code directory but has no table row`);
  const eventCodes = new Set(events.$defs.proxyCheckErrorCode.enum);
  const proxyCatalog = new Set(
    [...catalog].filter(([, mapping]) => mapping.http === 200).map(([code]) => code),
  );
  compare("ProxyCheckErrorCode catalog and events", proxyCatalog, eventCodes);
  const validateEvent = new Ajv2020({ allErrors: true, strict: false });
  addFormats(validateEvent);
  validateEvent.addSchema(events);
  const validateEnvelope = validateEvent.getSchema(events.$id);
  const eventFixtureRoot = rel("fixtures/events");
  if (statSync(eventFixtureRoot, { throwIfNoEntry: false })?.isDirectory()) {
    const fixtures = walkFiles(eventFixtureRoot).filter((file) => file.endsWith(".json"));
    if (fixtures.length === 0) throw new Error("fixtures/events contains no event fixtures");
    for (const filename of fixtures) {
      const instance = JSON.parse(readFileSync(filename, "utf8"));
      if (!validateEnvelope(instance))
        throw new Error(
          `${path.relative(root, filename)}: ${validateEvent.errorsText(validateEnvelope.errors)}`,
        );
      const errorCode = instance.payload?.error_code;
      if (errorCode !== undefined && errorCode !== null && !catalog.has(errorCode))
        throw new Error(`${path.relative(root, filename)}: unknown ErrorCode ${errorCode}`);
    }
  }
  const errorFixtures = rel("fixtures/api/errors");
  if (statSync(errorFixtures, { throwIfNoEntry: false })?.isDirectory()) {
    for (const filename of walkFiles(errorFixtures).filter((file) => file.endsWith(".json"))) {
      const instance = JSON.parse(readFileSync(filename, "utf8"));
      const code = instance.code ?? instance.error?.code;
      if (typeof code === "string" && !catalog.has(code))
        throw new Error(`${path.relative(root, filename)}: unknown ErrorCode ${code}`);
    }
  }
  for (const [code, mapping] of catalog) {
    if (
      !Number.isInteger(mapping.http) ||
      ![200, 401, 403, 404, 409, 422, 429, 500, 503].includes(mapping.http)
    )
      throw new Error(`${code}: invalid HTTP status`);
    if (mapping.http === 200 && !eventCodes.has(code))
      throw new Error(`${code}: unknown HTTP 200 ProxyCheckError`);
  }

  const responseSchema = openapi.components.schemas.Error;
  if (
    !responseSchema.required?.includes("retryable") ||
    responseSchema.properties?.retryable?.type !== "boolean"
  )
    throw new Error("OpenAPI Error.retryable must be a required boolean");
  const proxyErrors = new Set(events.$defs.proxyCheckErrorCode.enum);
  for (const code of proxyErrors)
    if (catalog.get(code)?.http !== 200)
      throw new Error(`${code}: ProxyCheckErrorCode must map to HTTP 200`);
  const operations = Object.entries(openapi.paths).flatMap(([route, methods]) =>
    Object.entries(methods)
      .filter(([method]) => ["get", "post", "put", "patch", "delete"].includes(method))
      .map(([method, operation]) => ({ route, method, operation })),
  );
  const responseCodes = new Map();
  for (const [status, response] of Object.entries(openapi.components.responses ?? {})) {
    const summary = `${response.description ?? ""} ${JSON.stringify(response)}`;
    const schemaRef = response.content?.["application/json"]?.schema?.$ref;
    if (schemaRef !== "#/components/schemas/Error" && !summary.includes("ErrorCode")) continue;
    const statusCode = Number(status);
    for (const [code, mapping] of catalog)
      if (
        statusCode === mapping.http ||
        (status === "Unauthorized" && mapping.http === 401) ||
        (status === "Forbidden" && mapping.http === 403) ||
        (status === "NotFound" && mapping.http === 404) ||
        (status === "Conflict" && mapping.http === 409) ||
        (status === "ValidationError" && mapping.http === 422) ||
        (status === "TooManyRequests" && mapping.http === 429) ||
        (status === "InternalError" && mapping.http === 500) ||
        (status === "Unavailable" && mapping.http === 503)
      )
        responseCodes.set(code, mapping.http);
  }
  for (const { route, method, operation } of operations) {
    const operationText = JSON.stringify(operation);
    for (const [code, mapping] of catalog) {
      if (!operationText.includes(code)) continue;
      responseCodes.set(code, mapping.http);
      if (!Object.hasOwn(operation.responses ?? {}, String(mapping.http)))
        throw new Error(
          `${method.toUpperCase()} ${route}: ${code} maps to HTTP ${mapping.http} but that response is undeclared`,
        );
    }
  }
  for (const [code, mapping] of catalog)
    if (mapping.http !== 200 && !responseCodes.has(code))
      throw new Error(`${code}: HTTP ${mapping.http} mapping missing from OpenAPI`);
  const allOpenapi = JSON.stringify(openapi);
  for (const code of allOpenapi.matchAll(/\b[A-Z][A-Z0-9_]{3,}\b/g)) {
    const value = code[0];
    if (
      /^(AUTH|EVENT|PROFILE|PROXY|RUNTIME|TOKEN|GROUP|TAG|VALIDATION|FINGERPRINT|SHARE_LINK|KERNEL|UNSUPPORTED|QUOTA|INSUFFICIENT|SANDBOX|DIRECT|DISPLAY|STORAGE|INTERNAL|FORBIDDEN|RESOURCE|IDEMPOTENCY|VIEWER)_/.test(
        value,
      ) &&
      !catalog.has(value)
    )
      throw new Error(`${value} appears in OpenAPI but not the ErrorCode catalog`);
  }
}

function databaseCheck() {
  const text = read("docs/DATABASE-SCHEMA.md");
  const blocks = [...text.matchAll(/```sql\r?\n([\s\S]*?)```/g)];
  if (blocks.length === 0) throw new Error("database DDL SQL block is missing");
  const ddl = blocks
    .map((match) => match[1])
    .find((block) => block.includes("CREATE TABLE schema_migrations"));
  if (!ddl) throw new Error("initial database DDL is missing");
  const temporary = mkdtempSync(path.join(tmpdir(), "mien-contracts-sqlite-"));
  try {
    const ddlPath = path.join(temporary, "schema.sql");
    writeFileSync(ddlPath, `${ddl}\nPRAGMA foreign_key_check;\nPRAGMA integrity_check;\n`);
    const db = Bun.spawnSync(["sqlite3", "-batch", "-noheader", ":memory:"], {
      stdin: Bun.file(ddlPath),
      stdout: "pipe",
      stderr: "pipe",
    });
    const output = db.stdout.toString().trim().split(/\r?\n/).filter(Boolean);
    if (
      db.exitCode !== 0 ||
      output.at(-1) !== "ok" ||
      output.some((line) => line !== "memory" && line !== "ok")
    )
      throw new Error(
        `SQLite DDL check failed (${db.exitCode}): ${db.stderr.toString()} ${output.join("; ")}`,
      );
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
}

function specCheck() {
  const specs = walkFiles(rel("docs/specs")).filter((file) => file.endsWith("SPEC.md"));
  if (specs.length === 0) throw new Error("docs/specs has no SPEC.md");
  const requirementOwners = new Map();
  for (const filename of specs) {
    const text = readFileSync(filename, "utf8");
    for (const match of text.matchAll(/^### (REQ-[A-Z]+-\d+)$/gm))
      requirementOwners.set(match[1], filename);
  }
  for (const filename of specs) {
    const text = readFileSync(filename, "utf8");
    if (
      !/^# .+/m.test(text) ||
      !/^## Requirements$/m.test(text) ||
      !/^## Verification$/m.test(text)
    )
      throw new Error(`${path.relative(root, filename)}: required spec sections missing`);
    const requirements = new Set(
      [...text.matchAll(/^### (REQ-[A-Z]+-\d+)$/gm)].map((match) => match[1]),
    );
    const verifications = [...text.matchAll(/^### (VER-[A-Z]+-\d+)$/gm)];
    if (requirements.size === 0 || verifications.length === 0)
      throw new Error(
        `${path.relative(root, filename)}: requirements or verifications are missing`,
      );
    const covered = new Set(
      [...text.matchAll(/^- covers: ([^\n]+)$/gm)].flatMap(
        (match) => match[1].match(/REQ-[A-Z]+-\d+/g) ?? [],
      ),
    );
    const uncovered = [...requirements].filter((requirement) => !covered.has(requirement));
    const unknown = [...covered].filter((requirement) => !requirementOwners.has(requirement));
    if (uncovered.length > 0 || unknown.length > 0)
      throw new Error(
        `${path.relative(root, filename)}: requirement coverage mismatch; uncovered=${uncovered.join(",") || "none"}; unknown=${unknown.join(",") || "none"}`,
      );
  }
}

function seccompCheck() {
  const provenance = json("deploy/docker/seccomp/PROVENANCE.json");
  const profile = json("deploy/docker/seccomp/chromium.json");
  const original = json("deploy/docker/seccomp/artifacts/moby-profiles-a0fa7080-default.json");
  if (
    !Array.isArray(profile.archMap) ||
    !Array.isArray(profile.syscalls) ||
    typeof profile.defaultAction !== "string"
  )
    throw new Error("chromium.json has invalid seccomp profile structure");
  if (provenance.commit !== "a0fa70805f742f7da15b367770d802ea2a86c68b")
    throw new Error("PROVENANCE.json commit is unexpected");
  if (sha256("deploy/docker/seccomp/chromium.json") !== provenance.modifiedSha256)
    throw new Error("chromium.json SHA-256 does not match PROVENANCE.json modifiedSha256");
  if (
    sha256("deploy/docker/seccomp/artifacts/moby-profiles-a0fa7080-default.json") !==
    provenance.upstreamSha256
  )
    throw new Error(
      "upstream seccomp artifact SHA-256 does not match PROVENANCE.json upstreamSha256",
    );
  if (!Array.isArray(original.syscalls) || original.defaultAction !== profile.defaultAction)
    throw new Error("seccomp baseline structure mismatch");
}

function runCheck(name, fn) {
  run(name, fn);
}
runCheck("JSON Schema compilation and fixtures", schemaCheck);
runCheck("OpenAPI parse, local refs, and lint", openapiCheck);
runCheck("Markdown relative links and anchors", markdownCheck);
runCheck("ErrorCode catalog ↔ OpenAPI ↔ events", errorCatalogCheck);
runCheck("SQLite DDL / foreign_key_check / integrity_check", databaseCheck);
runCheck("spec_contract_check", specCheck);
runCheck("seccomp structure and provenance SHA-256", seccompCheck);
console.log(
  `contracts: ${errors.length === 0 ? "all checks passed" : `${errors.length} check(s) failed`}`,
);
process.exitCode = errors.length === 0 ? 0 : 1;

#!/usr/bin/env bun
import { existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";

const root = path.resolve(process.env.MIEN_REPO_ROOT ?? process.cwd());
const checkOnly = process.argv.includes("--check");
const outputNotice = path.join(root, "NOTICE");
const outputLicenses = path.join(root, "LICENSES");
const outputReport = path.join(root, "third-party-report.json");

const linkedAllowed = new Set([
  "Apache-2.0",
  "Apache-2.0 WITH LLVM-exception",
  "MIT",
  "BSD-2-Clause",
  "BSD-3-Clause",
  "ISC",
  "Unicode-3.0",
  "Zlib",
]);
const frontendAllowed = new Set([
  ...linkedAllowed,
  "0BSD",
  "CC0-1.0",
  "Unlicense",
  "BlueOak-1.0.0",
]);
const runtimeAllowed = new Set([
  ...frontendAllowed,
  "MPL-2.0",
  "GPL-2.0-or-later",
  "GPL-3.0-or-later",
  "LGPL-2.1-or-later",
  "LGPL-3.0",
  "OFL-1.1",
  "CC-BY-SA-3.0",
  "BSL-1.0",
]);
const licenseUrls = {
  "0BSD": "https://spdx.org/licenses/0BSD.html",
  "Apache-2.0": "https://www.apache.org/licenses/LICENSE-2.0.txt",
  "Apache-2.0 WITH LLVM-exception": "https://spdx.org/licenses/Apache-2.0.html",
  "BSD-2-Clause": "https://spdx.org/licenses/BSD-2-Clause.html",
  "BSD-3-Clause": "https://spdx.org/licenses/BSD-3-Clause.html",
  "BlueOak-1.0.0": "https://blueoakcouncil.org/license/1.0.0",
  "BSL-1.0": "https://spdx.org/licenses/BSL-1.0.html",
  "CC-BY-SA-3.0": "https://creativecommons.org/licenses/by-sa/3.0/legalcode",
  "CC0-1.0": "https://creativecommons.org/publicdomain/zero/1.0/legalcode",
  "GPL-2.0-or-later": "https://www.gnu.org/licenses/old-licenses/gpl-2.0.txt",
  "GPL-3.0-or-later": "https://www.gnu.org/licenses/gpl-3.0.txt",
  "ISC": "https://spdx.org/licenses/ISC.html",
  "LGPL-2.1-or-later": "https://www.gnu.org/licenses/old-licenses/lgpl-2.1.txt",
  "LGPL-3.0": "https://www.gnu.org/licenses/lgpl-3.0.txt",
  "MIT": "https://spdx.org/licenses/MIT.html",
  "MPL-2.0": "https://www.mozilla.org/en-US/MPL/2.0/",
  "OFL-1.1": "https://scripts.sil.org/OFL",
  "Unicode-3.0": "https://spdx.org/licenses/Unicode-3.0.html",
  "Unlicense": "https://unlicense.org/",
  "Zlib": "https://spdx.org/licenses/Zlib.html",
};
const securityExceptions = [
  {
    id: "RUSTSEC-2026-0459",
    package: "libsqlite3-sys 0.30.1",
    class: "vulnerability",
    evidencePath: "deny.toml",
    disposition: "security exception; not a license exception",
    releaseImpact: "blocking until the documented reachability decision is accepted for the release",
  },
  {
    id: "RUSTSEC-2026-0009",
    package: "time 0.3.45",
    class: "vulnerability",
    evidencePath: "src-tauri/deny.toml",
    disposition: "security exception; not a license exception",
    releaseImpact: "blocking until upgraded or the documented reachability decision is accepted for the release",
  },
  {
    id: "RUSTSEC-2026-0194",
    package: "quick-xml 0.38.4 via plist 1.8.0",
    class: "vulnerability",
    evidencePath: "src-tauri/deny.toml",
    disposition: "security exception; not a license exception",
    releaseImpact: "blocking until upgraded or the documented reachability decision is accepted for the release",
  },
  {
    id: "RUSTSEC-2026-0195",
    package: "quick-xml 0.38.4 via plist 1.8.0",
    class: "vulnerability",
    evidencePath: "src-tauri/deny.toml",
    disposition: "security exception; not a license exception",
    releaseImpact: "blocking until upgraded or the documented reachability decision is accepted for the release",
  },
  ...[
    ["RUSTSEC-2024-0370", "proc-macro-error 1.0.4"],
    ["RUSTSEC-2025-0075", "unic-char-range 0.9.0"],
    ["RUSTSEC-2025-0080", "unic-common 0.9.0"],
    ["RUSTSEC-2025-0081", "unic-char-property 0.9.0"],
    ["RUSTSEC-2025-0098", "unic-ucd-version 0.9.0"],
    ["RUSTSEC-2025-0100", "unic-ucd-ident 0.9.0"],
  ].map(([id, pkg]) => ({
    id,
    package: pkg,
    class: "unmaintained",
    evidencePath: "src-tauri/deny.toml",
    disposition: "security exception; not a license exception",
    releaseImpact: "blocking until removed or the documented maintenance decision is accepted for the release",
  })),
];

function readText(relativePath) {
  return readFileSync(path.join(root, relativePath), "utf8");
}

function readJson(relativePath) {
  return JSON.parse(readText(relativePath));
}

function relative(filePath) {
  return path.relative(root, filePath).split(path.sep).join("/");
}

function parseJsonWithTrailingCommas(text, label) {
  try {
    return JSON.parse(text);
  } catch (firstError) {
    try {
      return JSON.parse(text.replace(/,\s*([}\]])/g, "$1"));
    } catch {
      throw new Error(`${label}: invalid JSON (${firstError.message})`);
    }
  }
}

function parseCargoLock() {
  const lock = readText("Cargo.lock");
  return [...lock.matchAll(/\[\[package\]\]([\s\S]*?)(?=\n\[\[package\]\]|\s*$)/g)].map((match) => {
    const block = match[1];
    const field = (name) => block.match(new RegExp(`^${name} = "([^"]+)"$`, "m"))?.[1] ?? null;
    return { name: field("name"), version: field("version"), source: field("source") };
  }).filter((pkg) => pkg.name && pkg.version);
}

function cargoMetadata() {
  const result = Bun.spawnSync(["cargo", "metadata", "--format-version", "1", "--locked", "--offline"], {
    cwd: root,
    stdout: "pipe",
    stderr: "pipe",
  });
  if (result.exitCode !== 0) {
    throw new Error(`cargo metadata --offline failed: ${result.stderr.toString().trim()}`);
  }
  return JSON.parse(result.stdout.toString());
}

function metadataPackageMap(metadata) {
  const map = new Map();
  for (const pkg of metadata.packages) {
    const key = `${pkg.name}\0${pkg.version}\0${pkg.source ?? ""}`;
    map.set(key, pkg);
  }
  return map;
}

function packageKey(name, version, source) {
  return `${name}\0${version}\0${source ?? ""}`;
}

function parseLicenseTokens(expression) {
  return (expression.replaceAll("/", " OR ").match(/[A-Za-z0-9][A-Za-z0-9.+-]*/g) ?? [])
    .filter((token) => !["AND", "OR", "WITH", "LLVM-exception"].includes(token));
}

function expressionAllowed(expression, allowed) {
  if (!expression || /UNKNOWN|UNLICENSED|SEE LICENSE/i.test(expression)) return false;
  const tokens = expression.replaceAll("/", " OR ").match(/\(|\)|AND|OR|WITH|[A-Za-z0-9][A-Za-z0-9.+-]*/g) ?? [];
  let cursor = 0;
  const parsePrimary = () => {
    const token = tokens[cursor++];
    if (token === "(") {
      const value = parseOr();
      if (tokens[cursor] !== ")") throw new Error(`unbalanced license expression: ${expression}`);
      cursor += 1;
      return value;
    }
    if (!token || ["AND", "OR", "WITH", ")"].includes(token)) throw new Error(`invalid license expression: ${expression}`);
    if (tokens[cursor] === "WITH") {
      cursor += 2;
    }
    return allowed.has(token);
  };
  const parseAnd = () => {
    let value = parsePrimary();
    while (tokens[cursor] === "AND") {
      cursor += 1;
      value = parsePrimary() && value;
    }
    return value;
  };
  const parseOr = () => {
    let value = parseAnd();
    while (tokens[cursor] === "OR") {
      cursor += 1;
      value = parseAnd() || value;
    }
    return value;
  };
  const result = parseOr();
  if (cursor !== tokens.length) throw new Error(`invalid license expression: ${expression}`);
  return result;
}

function licenseIds(license) {
  const expressions = Array.isArray(license) ? license : [license];
  return [...new Set(expressions.filter(Boolean).flatMap((item) => parseLicenseTokens(item)))].sort();
}

function validateRecord(record, allowed, failures) {
  const expressions = Array.isArray(record.license) ? record.license : [record.license];
  for (const expression of expressions) {
    if (!expression || expression === "SBOM-PENDING") continue;
    try {
      if (!expressionAllowed(expression, allowed)) {
        failures.push(`${record.id}@${record.version}: disallowed or unknown license ${expression}`);
      }
    } catch (error) {
      failures.push(`${record.id}@${record.version}: ${error.message}`);
    }
  }
}

function manifestFiles() {
  const dir = path.join(root, "docs/contracts/manifests");
  return readdirSync(dir).filter((name) => name.endsWith(".json")).sort().map((name) => path.join(dir, name));
}

function runtimeComponents() {
  const manifests = manifestFiles().map((file) => ({ file, data: readJson(relative(file)) }));
  const components = [];
  const fingerprint = manifests.filter(({ data }) => "kernel_version" in data);
  if (fingerprint.length === 0) throw new Error("no fingerprint-chromium manifest found");
  components.push({
    id: "fingerprint-chromium",
    version: fingerprint[0].data.kernel_version,
    license: "BSD-3-Clause",
    distribution: "independent-static-directory",
    obligations: "Retain copyright and license notices, provide the exact source URL, and do not imply upstream endorsement.",
    sourceUrl: `https://github.com/adryfish/fingerprint-chromium/tree/${fingerprint[0].data.kernel_version}`,
    evidencePaths: fingerprint.map(({ file }) => relative(file)),
    policy: "runtime",
  });
  const xray = manifests.filter(({ data }) => data.engine === "xray-core");
  if (xray.length === 0) throw new Error("no Xray-core manifest found");
  components.push({
    id: "xray-core",
    version: xray[0].data.version,
    license: xray[0].data.license,
    distribution: "independent-process",
    obligations: "Retain MPL-2.0 notices and source URL; provide Corresponding Source, and publish modified MPL-covered files if the distributed binary is modified.",
    sourceUrl: xray[0].data.source_url,
    evidencePaths: xray.map(({ file }) => relative(file)),
    policy: "runtime",
  });

  const provenancePath = path.join(root, "deploy/docker/seccomp/PROVENANCE.json");
  const provenance = readJson(relative(provenancePath));
  components.push({
    id: "moby-seccomp-profile",
    version: provenance.commit,
    license: provenance.license,
    distribution: "independent-host-configuration",
    obligations: "Retain Apache notice and provenance; distribute the modified profile with its upstream source, commit, and modification record.",
    sourceUrl: provenance.sourceUrl,
    evidencePaths: [relative(provenancePath), "deploy/docker/seccomp/chromium.json", "deploy/docker/seccomp/artifacts/moby-profiles-a0fa7080-LICENSE.txt"],
    policy: "runtime",
  });

  const container = [
    ["debian-bookworm-slim", "bookworm-slim (image digest required at build)", null, "https://www.debian.org/releases/bookworm/", "container-base-image", "Generate an image SBOM and retain each installed package license and source URL; GPL/LGPL packages require Corresponding Source or a written offer."],
    ["tini", "container-locked version", "Apache-2.0", "https://github.com/krallin/tini", "independent-process", "Retain Apache-2.0 notice and source URL."],
    ["xorg-server-xvfb", "container-locked version", "MIT", "https://gitlab.freedesktop.org/xorg/xserver", "independent-process", "Retain copyright and license text and the exact package source record."],
    ["openbox", "container-locked version", "GPL-2.0-or-later", "http://openbox.org/", "independent-process", "Retain GPL-2.0-or-later text, exact package source record, and Corresponding Source or a written offer."],
    ["x11vnc", "container-locked version", "GPL-2.0-or-later", "https://github.com/LibVNC/x11vnc", "independent-process", "Retain GPL-2.0-or-later text, exact package source record, and Corresponding Source or a written offer."],
    ["websockify", "container-locked version", "LGPL-3.0", "https://github.com/novnc/websockify", "independent-process", "Retain LGPL-3.0 text, provide Corresponding Source and a replacement path, and publish modified files."],
    ["noVNC", "container-locked version", ["MPL-2.0", "BSD-3-Clause", "BSD-2-Clause", "OFL-1.1", "CC-BY-SA-3.0", "MIT"], "https://github.com/novnc/noVNC", "independent-static-directory", "Retain MPL file headers and all bundled resource/font attributions; publish modified MPL files and preserve the BSD-style core/des.js notice."],
  ];
  for (const [id, version, license, sourceUrl, distribution, obligations] of container) {
    components.push({ id, version, license, distribution, obligations, sourceUrl, evidencePaths: ["docs/THIRD-PARTY.md", "docs/TECH-STACK.md", "deploy/docker/"], policy: "runtime" });
  }
  return components;
}

function cargoComponents() {
  const lockPackages = parseCargoLock();
  const map = metadataPackageMap(cargoMetadata());
  return lockPackages.map((lockPkg) => {
    const pkg = map.get(packageKey(lockPkg.name, lockPkg.version, lockPkg.source)) ??
      [...map.values()].find((candidate) => candidate.name === lockPkg.name && candidate.version === lockPkg.version);
    if (!pkg?.license) throw new Error(`Cargo.lock package has no license metadata: ${lockPkg.name} ${lockPkg.version}`);
    return {
      id: `cargo:${lockPkg.name}@${lockPkg.version}`,
      version: lockPkg.version,
      license: pkg.license,
      distribution: "linked-dependency",
      obligations: "Retain the selected license notice in the release license bundle; GPL/AGPL/SSPL and unknown expressions are not permitted in the linked dependency graph.",
      sourceUrl: pkg.repository ?? `https://crates.io/crates/${lockPkg.name}/${lockPkg.version}`,
      evidencePaths: ["Cargo.lock", "Cargo.toml", "deny.toml"],
      policy: "linked",
    };
  });
}

function parseBunLock() {
  return parseJsonWithTrailingCommas(readText("bun.lock"), "bun.lock");
}

function packageJson(packageName) {
  const packagePath = path.join(root, "node_modules", ...packageName.split("/"), "package.json");
  if (!existsSync(packagePath)) return null;
  return parseJsonWithTrailingCommas(readFileSync(packagePath, "utf8"), packagePath);
}

function cachedPackageJson(packageName, version) {
  const cacheRoot = path.join(process.env.HOME ?? "", ".bun/install/cache");
  if (!existsSync(cacheRoot)) return null;
  const packageRoot = packageName.startsWith("@") ? path.join(cacheRoot, packageName.split("/")[0]) : cacheRoot;
  if (!existsSync(packageRoot)) return null;
  const prefix = packageName.startsWith("@") ? `${packageName.split("/")[1]}@${version}@@@` : `${packageName}@${version}@@@`;
  const candidates = readdirSync(packageRoot).filter((entry) => entry.startsWith(prefix));
  for (const candidate of candidates) {
    const packagePath = path.join(packageRoot, candidate, "package.json");
    if (existsSync(packagePath)) return parseJsonWithTrailingCommas(readFileSync(packagePath, "utf8"), packagePath);
  }
  return null;
}

function packageVersionFromEntry(entry) {
  const reference = entry?.[0];
  if (typeof reference !== "string") return null;
  const at = reference.lastIndexOf("@");
  return at > 0 ? reference.slice(at + 1) : null;
}

function bunComponents() {
  const lock = parseBunLock();
  const workspace = lock.workspaces?.[""];
  if (!workspace) throw new Error("bun.lock has no root workspace");
  const sections = ["dependencies", "devDependencies", "optionalDependencies", "peerDependencies"];
  const names = [...new Set(sections.flatMap((section) => Object.keys(workspace[section] ?? {})))].sort();
  return names.map((name) => {
    const entry = lock.packages?.[name];
    const version = packageVersionFromEntry(entry);
    if (!version) throw new Error(`bun.lock has no resolved entry for ${name}`);
    const pkg = packageJson(name) ?? cachedPackageJson(name, version);
    if (!pkg?.license && !Array.isArray(pkg?.licenses)) {
      throw new Error(`${name}@${version}: package metadata not installed; run bun install --frozen-lockfile before license generation`);
    }
    const license = pkg.license ?? pkg.licenses.map((item) => item.type).filter(Boolean).join(" OR ");
    return {
      id: `bun:${name}@${version}`,
      version,
      license,
      distribution: "bundled-static-assets",
      obligations: "Retain the package license notice and source URL in the release license bundle; unknown expressions fail the PR gate.",
      sourceUrl: pkg.repository?.url ?? pkg.repository ?? `https://www.npmjs.com/package/${name}/v/${version}`,
      evidencePaths: ["bun.lock", "package.json", `node_modules/${name}/package.json`],
      policy: "frontend",
    };
  });
}

function standardLicenseText(id) {
  if (id === "Apache-2.0") return readText("LICENSE");
  const url = licenseUrls[id] ?? `https://spdx.org/licenses/${id}.html`;
  return `SPDX-License-Identifier: ${id}\n\nCanonical license text: ${url}\n\nThis file is generated offline from the SPDX identifier. The release staging job must replace this provenance stub with the exact upstream license text and attribution collected from the pinned package, image, or source archive; a missing exact text blocks publication.\n`;
}

function notice(components) {
  const lines = [
    "Mien third-party notices",
    "",
    "This file is generated by scripts/third-party/generate.mjs. Do not edit it by hand.",
    "The project license is Apache-2.0; see LICENSE.",
    "",
  ];
  for (const component of [...components].sort((a, b) => a.id.localeCompare(b.id))) {
    const licenses = Array.isArray(component.license) ? component.license.join("; ") : component.license;
    lines.push(`Component: ${component.id}`);
    lines.push(`Version: ${component.version}`);
    lines.push(`License: ${licenses}`);
    lines.push(`Distribution: ${component.distribution}`);
    lines.push(`Source: ${component.sourceUrl}`);
    lines.push(`Obligations: ${component.obligations}`);
    lines.push(`Evidence: ${component.evidencePaths.join(", ")}`);
    lines.push("");
  }
  return `${lines.join("\n")}\n`;
}

function build() {
  const failures = [];
  let components;
  try {
    components = [...runtimeComponents(), ...cargoComponents(), ...bunComponents()];
  } catch (error) {
    failures.push(error.message);
    components = [];
  }
  for (const component of components) {
    const allowed = component.policy === "linked" ? linkedAllowed : component.policy === "frontend" ? frontendAllowed : runtimeAllowed;
    validateRecord(component, allowed, failures);
  }
  const sortedComponents = [...components].sort((a, b) => a.id.localeCompare(b.id));
  const ids = [...new Set(sortedComponents.flatMap((component) => licenseIds(component.license)))].sort();
  const report = {
    schemaVersion: 1,
    generatedBy: "scripts/third-party/generate.mjs",
    inputs: ["docs/contracts/manifests/*.json", "Cargo.lock", "bun.lock", "deploy/docker/*"],
    outputs: ["NOTICE", "LICENSES/", "third-party-report.json"],
    licenseStatus: failures.length === 0 ? "pass" : "fail",
    releaseStatus: "blocked",
    unresolvedLicenseItems: [
      "Container image digest and package-level license evidence are supplied by the release docker-amd64 job.",
      "Exact license texts for generated SPDX identifiers are supplied from pinned package/source archives by release staging; this offline generator never downloads them.",
    ],
    releaseBlockers: [
      "Do not publish until docker-amd64 has supplied the image digest, package SBOM, and exact GPL/LGPL/MPL source/license evidence.",
      "Do not publish until release staging replaces any SPDX provenance stub in LICENSES/ with exact license text and attribution.",
      ...securityExceptions.map((exception) => ({ ...exception })),
    ],
    securityExceptions,
    allowedLicenses: {
      linked: [...linkedAllowed].sort(),
      frontend: [...frontendAllowed].sort(),
      runtime: [...runtimeAllowed].sort(),
    },
    components: sortedComponents,
    licenseIds: ids,
  };
  return { failures, components: sortedComponents, ids, report };
}

function expectedFiles(ids) {
  return new Map(ids.map((id) => [path.join(outputLicenses, `${id}.txt`), standardLicenseText(id)]));
}

function compareOrWrite(file, content) {
  if (checkOnly) {
    if (!existsSync(file) || readFileSync(file, "utf8") !== content) {
      throw new Error(`${relative(file)} is stale; run cargo run -q -p xtask -- licenses`);
    }
  } else {
    mkdirSync(path.dirname(file), { recursive: true });
    writeFileSync(file, content);
  }
}

const result = build();
if (result.failures.length > 0) {
  console.error("Third-party license gate failed:");
  for (const failure of result.failures) console.error(`  ${failure}`);
  process.exit(1);
}
try {
  compareOrWrite(outputNotice, notice(result.components));
  for (const [file, content] of expectedFiles(result.ids)) compareOrWrite(file, content);
  if (existsSync(outputLicenses)) {
    const expected = new Set([...expectedFiles(result.ids).keys()].map((file) => path.basename(file)));
    for (const name of readdirSync(outputLicenses)) {
      if (name.endsWith(".txt") && !expected.has(name)) throw new Error(`LICENSES/${name} is not in the generated license set`);
    }
  }
  compareOrWrite(outputReport, `${JSON.stringify(result.report, null, 2)}\n`);
} catch (error) {
  console.error(error.message);
  process.exit(1);
}
console.log(`Third-party licenses: ${result.components.length} components, ${result.ids.length} license texts, ${result.report.releaseStatus} release status`);
if (result.report.releaseBlockers.length > 0) console.error(`Release blockers are recorded separately: ${result.report.releaseBlockers.length}`);

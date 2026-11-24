#!/usr/bin/env bun
import { existsSync, readFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";

const root = process.cwd();
const manifest = JSON.parse(readFileSync(path.join(root, "package.json"), "utf8"));
const allowed = new Set([
  "MIT",
  "Apache-2.0",
  "BSD-2-Clause",
  "BSD-3-Clause",
  "ISC",
  "0BSD",
  "CC0-1.0",
  "Unlicense",
  "Unicode-3.0",
  "Zlib",
  "BlueOak-1.0.0",
]);
const sections = ["dependencies", "devDependencies", "optionalDependencies", "peerDependencies"];
const names = new Set(sections.flatMap((section) => Object.keys(manifest[section] ?? {})));
const failures = [];

function packageManifest(name) {
  const parts = name.startsWith("@") ? name.split("/") : [name];
  const candidate = path.join(root, "node_modules", ...parts, "package.json");
  return existsSync(candidate) ? JSON.parse(readFileSync(candidate, "utf8")) : null;
}

function licenseOf(pkg) {
  if (typeof pkg.license === "string") return pkg.license;
  if (Array.isArray(pkg.licenses))
    return pkg.licenses
      .map((entry) => entry?.type)
      .filter(Boolean)
      .join(" OR ");
  return null;
}

function isAllowed(expression) {
  if (!expression || /SEE LICENSE|UNLICENSED|UNKNOWN/i.test(expression)) return false;
  const identifiers = expression.match(/[A-Za-z0-9][A-Za-z0-9.+-]*/g) ?? [];
  const licenses = identifiers.filter((token) => !["OR", "AND", "WITH"].includes(token));
  return licenses.length > 0 && licenses.every((license) => allowed.has(license));
}

for (const name of [...names].sort()) {
  let pkg;
  try {
    pkg = packageManifest(name);
  } catch (error) {
    failures.push(`${name}: ${error.message}`);
    continue;
  }
  if (!pkg) {
    failures.push(`${name}: package metadata not installed`);
    continue;
  }
  const license = licenseOf(pkg);
  if (!isAllowed(license)) failures.push(`${name}: ${license ?? "missing license metadata"}`);
}

if (failures.length > 0) {
  console.error("Disallowed or unknown frontend dependency licenses:");
  for (const failure of failures) console.error(`  ${failure}`);
  process.exitCode = 1;
} else {
  console.log(`Frontend licenses: ${names.size} package(s) allowed`);
}

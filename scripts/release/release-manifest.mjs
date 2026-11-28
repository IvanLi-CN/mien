#!/usr/bin/env node
// Builds the artifact manifest `release-policy.mjs check-manifest` judges.
//
// The manifest is derived from the bytes on disk, not from a claim in the draft:
// every file's SHA-256 is recomputed, `SHA256SUMS` is parsed and compared to the
// recomputed digests, and the container digest plus the kernel/Xray hashes are
// read from the build records the packaging jobs wrote. Nothing is inferred — a
// missing record stays missing and the policy rejects it.
//
// This is the step that makes "artifacts include SHA-256SUMS, SBOM, licenses,
// host-side seccomp, release-identity.json" checkable rather than aspirational.
import { createHash } from "node:crypto";
import { existsSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

/**
 * Parse `sha256sum` output. `*` marks binary mode; a leading `./` or an absolute
 * path is stripped so the manifest names match the release asset names.
 */
export function parseSums(text) {
  const sums = {};
  for (const rawLine of String(text).split("\n")) {
    const line = rawLine.trim();
    if (line === "" || line.startsWith("#")) continue;
    const match = /^([0-9a-fA-F]{64})\s+\*?(.+)$/.exec(line);
    if (!match) continue;
    const name = match[2].trim().replace(/^\.\//, "");
    sums[name] = match[1].toLowerCase();
  }
  return sums;
}

export function sha256File(file) {
  return createHash("sha256").update(readFileSync(file)).digest("hex");
}

/** Every regular file under `root`, as release-asset relative paths. */
export function listAssets(root) {
  const assets = [];
  const walk = (dir, prefix) => {
    for (const entry of readdirSync(dir, { withFileTypes: true }).sort((a, b) =>
      a.name.localeCompare(b.name),
    )) {
      const absolute = path.join(dir, entry.name);
      const relative = prefix === "" ? entry.name : `${prefix}/${entry.name}`;
      if (entry.isDirectory()) walk(absolute, relative);
      else if (entry.isFile()) assets.push({ name: relative, absolute });
    }
  };
  if (!existsSync(root)) return assets;
  walk(root, "");
  return assets;
}

/**
 * Assemble the manifest. `dockerRecord` and `binaryRecords` are the JSON files the
 * packaging jobs write; absent files leave the fields unset so the policy reports
 * `missing-docker-digest` / `missing-binary-digest` instead of a silent pass.
 */
export function buildManifest({
  assetsDir,
  canonicalVersion,
  dockerRecord = null,
  kernelRecord = null,
  xrayRecord = null,
}) {
  const assets = listAssets(assetsDir);
  const files = assets
    .filter((asset) => asset.name !== "manifest.json")
    .map((asset) => ({ name: asset.name, sha256: sha256File(asset.absolute) }));

  const sumsAsset = assets.find((asset) => asset.name === "SHA256SUMS");
  const sums = sumsAsset ? parseSums(readFileSync(sumsAsset.absolute, "utf8")) : {};

  return {
    canonical_version: canonicalVersion,
    files,
    sums,
    docker: dockerRecord ?? {},
    binaries: {
      ...(kernelRecord ? { kernel: kernelRecord } : {}),
      ...(xrayRecord ? { xray: xrayRecord } : {}),
    },
    generated_from: path.basename(path.resolve(assetsDir)),
    asset_count: files.length,
  };
}

const USAGE = `usage:
  release-manifest.mjs --assets <dir> --canonical <X.Y.Z> [--docker <record.json>]
                       [--kernel <record.json>] [--xray <record.json>] [--out <manifest.json>]

Recomputes every asset digest and writes the manifest release-policy.mjs checks.
Exit 0 always writes a manifest; the policy decides whether it is complete.`;

function parseArgs(argv) {
  const options = {};
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (!arg.startsWith("--")) continue;
    const key = arg.slice(2);
    const next = argv[index + 1];
    if (next === undefined || next.startsWith("--")) {
      options[key] = true;
      continue;
    }
    options[key] = next;
    index += 1;
  }
  return options;
}

export function main(argv = process.argv.slice(2)) {
  const options = parseArgs(argv);
  if (typeof options.assets !== "string" || typeof options.canonical !== "string") {
    console.error(USAGE);
    return 2;
  }
  if (!existsSync(options.assets) || !statSync(options.assets).isDirectory()) {
    console.error(`release-manifest: ${options.assets} is not a directory`);
    return 2;
  }
  const readMaybe = (file) =>
    typeof file === "string" && existsSync(file) ? JSON.parse(readFileSync(file, "utf8")) : null;
  const manifest = buildManifest({
    assetsDir: options.assets,
    canonicalVersion: options.canonical,
    dockerRecord: readMaybe(options.docker),
    kernelRecord: readMaybe(options.kernel),
    xrayRecord: readMaybe(options.xray),
  });
  const text = `${JSON.stringify(manifest, null, 2)}\n`;
  console.log(text);
  if (typeof options.out === "string") writeFileSync(path.resolve(options.out), text);
  return 0;
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === path.resolve(fileURLToPath(import.meta.url))
) {
  process.exitCode = main();
}

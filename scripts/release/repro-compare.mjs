#!/usr/bin/env node
// Compares the two independent build records produced by
// `.github/workflows/reproducibility.yml` and decides whether the OCI digest that
// would be published is identical.
//
// The workflow builds the same commit twice on separate runners, each with its own
// freshly created builder, and never imports or exports build cache. Each build
// writes a record with the digest that buildx reports (`containerimage.digest`, the
// root manifest that would be pushed), the digest re-derived from the exported OCI
// archive, the config digest, and the ordered layer descriptors. This script checks
// that every fixed parameter agrees and that the two digests match, and when they
// do not it names the first differing layer instead of printing two opaque hashes.
//
// It reads only local JSON and makes no network calls, so it is unit-testable
// without Docker. Exit code 0 means equal; 1 means different or malformed; 2 means
// bad usage.
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const USAGE = `用法：
  repro-compare.mjs --a <build-a.json> --b <build-b.json> [--report <report.json>]
                    [--summary <summary.md>]
`;

/** Parameters that must be identical before digest equality means anything. */
const PARAMETERS = [
  ["source_sha", "source commit"],
  ["source_date_epoch", "SOURCE_DATE_EPOCH"],
  ["platform", "export platform"],
  ["frontend", "Dockerfile frontend reference"],
  ["buildkit_image", "pinned BuildKit image"],
  ["buildx_version", "buildx version"],
  ["compression", "layer compression"],
];

function parseArgs(argv) {
  const options = { a: null, b: null, report: null, summary: null };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    const next = () => {
      const value = argv[index + 1];
      if (value === undefined) throw new Error(`${arg} needs a value`);
      index += 1;
      return value;
    };
    switch (arg) {
      case "--a":
        options.a = next();
        break;
      case "--b":
        options.b = next();
        break;
      case "--report":
        options.report = next();
        break;
      case "--summary":
        options.summary = next();
        break;
      case "-h":
      case "--help":
        options.help = true;
        break;
      default:
        throw new Error(`unknown argument: ${arg}`);
    }
  }
  return options;
}

function readRecord(file) {
  if (!existsSync(file)) throw new Error(`${file}: build record not found`);
  try {
    return JSON.parse(readFileSync(file, "utf8"));
  } catch (error) {
    throw new Error(`${file}: not valid JSON (${error.message})`);
  }
}

function requireShape(record, side) {
  const problems = [];
  for (const [key] of PARAMETERS) {
    if (record[key] === undefined || record[key] === null || record[key] === "")
      problems.push(`${side}.${key} missing`);
  }
  for (const key of ["image_digest", "oci_manifest_digest", "config_digest"]) {
    if (typeof record[key] !== "string" || !record[key].startsWith("sha256:"))
      problems.push(`${side}.${key} is not a sha256 digest`);
  }
  for (const key of ["layers", "diff_ids"]) {
    if (!Array.isArray(record[key])) problems.push(`${side}.${key} is not an array`);
  }
  if (!Number.isInteger(record.source_date_epoch))
    problems.push(`${side}.source_date_epoch is not an integer`);
  if (problems.length > 0) throw new Error(problems.join("; "));
}

function equalLayers(a, b) {
  if (a.length !== b.length) return false;
  return a.every((layer, index) => layer.digest === b[index].digest);
}

/**
 * Decides whether two build records describe the same publishable image.
 *
 * Returns every check rather than stopping at the first failure: a report that
 * lists all diverging dimensions is what makes a non-reproducible build
 * diagnosable.
 */
export function compare(a, b) {
  requireShape(a, "a");
  requireShape(b, "b");
  const checks = [];

  for (const [key, label] of PARAMETERS) {
    const left = JSON.stringify(a[key]);
    const right = JSON.stringify(b[key]);
    checks.push({
      name: key,
      label,
      ok: left === right,
      a: left,
      b: right,
    });
  }

  checks.push({
    name: "image_digest",
    label: "publishable manifest digest (buildx metadata)",
    ok: a.image_digest === b.image_digest,
    a: a.image_digest,
    b: b.image_digest,
  });
  checks.push({
    name: "oci_manifest_digest",
    label: "manifest digest re-derived from the exported OCI archive",
    ok: a.oci_manifest_digest === b.oci_manifest_digest,
    a: a.oci_manifest_digest,
    b: b.oci_manifest_digest,
  });
  checks.push({
    name: "config_digest",
    label: "image config digest",
    ok: a.config_digest === b.config_digest,
    a: a.config_digest,
    b: b.config_digest,
  });
  checks.push({
    name: "layers",
    label: "ordered layer digests",
    ok: equalLayers(a.layers, b.layers),
    a: `${a.layers.length} layer(s)`,
    b: `${b.layers.length} layer(s)`,
  });
  checks.push({
    name: "diff_ids",
    label: "ordered uncompressed diff IDs",
    ok: JSON.stringify(a.diff_ids) === JSON.stringify(b.diff_ids),
    a: `${a.diff_ids.length} diff ID(s)`,
    b: `${b.diff_ids.length} diff ID(s)`,
  });

  // The exported archive digest must agree with what buildx says it would push.
  // Two builders agreeing on a value that does not describe the exported artifact
  // would be a meaningless comparison.
  for (const [side, record] of [
    ["a", a],
    ["b", b],
  ]) {
    checks.push({
      name: `archive_matches_metadata_${side}`,
      label: `${side}: exported archive manifest equals buildx metadata digest`,
      ok: record.image_digest === record.oci_manifest_digest,
      a: record.image_digest,
      b: record.oci_manifest_digest,
    });
  }

  let firstDifferentLayer = null;
  const shared = Math.min(a.layers.length, b.layers.length);
  for (let index = 0; index < shared; index += 1) {
    if (a.layers[index].digest !== b.layers[index].digest) {
      firstDifferentLayer = {
        index,
        a: a.layers[index],
        b: b.layers[index],
      };
      break;
    }
  }

  // `digestsAgree` is the publishable claim: the same root manifest digest from two
  // builders whose exported archives also match. Layer and config equality are
  // required too, because a layer difference with a matching manifest digest would
  // mean the archive was not assembled the way buildx reported it.
  const artifactChecks = checks.filter((check) =>
    ["image_digest", "oci_manifest_digest", "config_digest", "layers", "diff_ids"].includes(
      check.name,
    ),
  );
  const archiveChecks = checks.filter((check) =>
    check.name.startsWith("archive_matches_metadata_"),
  );
  const parameterChecks = checks.filter((check) => PARAMETERS.some(([key]) => key === check.name));
  // Every check is required: a matching digest computed with different fixed inputs
  // is not evidence of anything, and an archive that does not match its own metadata
  // means the digest extraction is wrong.
  const parametersAgree = parameterChecks.every((check) => check.ok);
  const digestsAgree = artifactChecks.every((check) => check.ok);
  const archivesAgree = archiveChecks.every((check) => check.ok);
  return {
    verdict: checks.every((check) => check.ok) ? "equal" : "different",
    digestsAgree,
    parametersAgree,
    archivesAgree,
    image_digest_agrees: a.image_digest === b.image_digest,
    source_sha: a.source_sha,
    source_date_epoch: a.source_date_epoch,
    image_digest: { a: a.image_digest, b: b.image_digest },
    first_different_layer: firstDifferentLayer,
    checks,
  };
}

function markdown(result) {
  const lines = [
    "## Docker reproducibility probe",
    "",
    `- source_sha: \`${result.source_sha}\``,
    `- SOURCE_DATE_EPOCH: \`${result.source_date_epoch}\``,
    `- builder A digest: \`${result.image_digest.a}\``,
    `- builder B digest: \`${result.image_digest.b}\``,
    `- verdict: **${result.verdict}**`,
    "",
    "| Check | A | B | Equal |",
    "| --- | --- | --- | --- |",
  ];
  for (const check of result.checks)
    lines.push(
      `| ${check.label} | \`${check.a}\` | \`${check.b}\` | ${check.ok ? "yes" : "**no**"} |`,
    );
  if (result.first_different_layer) {
    const { index, a, b } = result.first_different_layer;
    lines.push(
      "",
      `First differing layer: index ${index}`,
      "",
      `- A: \`${a.digest}\` ${a.mediaType} ${a.size} bytes`,
      `- B: \`${b.digest}\` ${b.mediaType} ${b.size} bytes`,
    );
  }
  return `${lines.join("\n")}\n`;
}

function main() {
  let options;
  try {
    options = parseArgs(process.argv.slice(2));
  } catch (error) {
    console.error(`repro-compare: ${error.message}\n\n${USAGE}`);
    return 2;
  }
  if (options.help) {
    console.log(USAGE);
    return 0;
  }
  if (!options.a || !options.b) {
    console.error(`repro-compare: --a and --b are required\n\n${USAGE}`);
    return 2;
  }

  let result;
  try {
    result = compare(readRecord(options.a), readRecord(options.b));
  } catch (error) {
    console.error(`repro-compare: FAIL\n  - ${error.message}`);
    return 1;
  }

  const summary = markdown(result);
  if (options.summary) writeFileSync(options.summary, summary);
  if (options.report) writeFileSync(options.report, `${JSON.stringify(result, null, 2)}\n`);
  process.stdout.write(summary);

  if (result.verdict === "equal") {
    console.log(`repro-compare: PASS (${result.image_digest.a})`);
    return 0;
  }
  console.error("repro-compare: FAIL (the two independent builds disagree)");
  for (const check of result.checks.filter((entry) => !entry.ok))
    console.error(`  - ${check.label}: A=${check.a} B=${check.b}`);

  // A differing layer whose position differs from the first build's own base
  // boundary means the divergence is in this build, not in the pinned base image;
  // without that boundary the index alone cannot say which, so state both.
  if (result.first_different_layer) {
    const { index, a, b } = result.first_different_layer;
    console.error(`  - first differing layer: index ${index}`);
    console.error(`      A ${a.digest} ${a.mediaType} ${a.size} bytes`);
    console.error(`      B ${b.digest} ${b.mediaType} ${b.size} bytes`);
  } else {
    console.error(
      "  - every shared layer matches; compare the manifest/config/history bytes above",
    );
  }
  console.error(
    "  - rebuild locally with the same flags, or diff `docker buildx du` and the layer list against the pinned base image",
  );
  return 1;
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === path.resolve(fileURLToPath(import.meta.url))
) {
  process.exitCode = main();
}

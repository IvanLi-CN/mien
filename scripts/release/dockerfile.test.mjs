// 容器构建契约测试：`node --test scripts/release/dockerfile.test.mjs`。
//
// 用例都对着真实仓库的 Dockerfile 与冻结清单跑；合成用例只改一个字段，避免一份
// “恰好合法”的 fixture 掩盖其他规则。
import assert from "node:assert/strict";
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import {
  aptPackages,
  assetDigests,
  checkContract,
  copySources,
  parseDockerfile,
  resolveArgs,
} from "./dockerfile.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");

/** Dockerfile-style ARG reference; `\u0024` keeps the marker out of a raw template. */
const argRef = (name) => `\u0024{${name}}`;
const realDockerfile = readFileSync(path.join(root, "deploy/docker/Dockerfile.server"), "utf8");

/** 复制 Dockerfile 与冻结清单的可写仓库根；只用于构造单点破坏。 */
function fixtureRoot(mutate) {
  const dir = mkdtempSync(path.join(tmpdir(), "mien-dockerfile-"));
  mkdirSync(path.join(dir, "deploy/docker"), { recursive: true });
  mkdirSync(path.join(dir, "docs/contracts/manifests"), { recursive: true });
  for (const file of [
    "fingerprint-chromium-148.0.7778.215-linux-x86_64.json",
    "xray-core-v26.11.9-linux-x86_64.json",
  ])
    copyFileSync(
      path.join(root, "docs/contracts/manifests", file),
      path.join(dir, "docs/contracts/manifests", file),
    );
  for (const file of ["entrypoint.sh", "healthcheck.sh", "verify-server.sh"])
    copyFileSync(path.join(root, "deploy/docker", file), path.join(dir, "deploy/docker", file));
  mkdirSync(path.join(dir, "deploy/docker/seccomp"), { recursive: true });
  copyFileSync(
    path.join(root, "deploy/docker/seccomp/chromium.json"),
    path.join(dir, "deploy/docker/seccomp/chromium.json"),
  );
  const dockerfile = mutate ? mutate(realDockerfile) : realDockerfile;
  writeFileSync(path.join(dir, "deploy/docker/Dockerfile.server"), dockerfile);
  return dir;
}

function checkFixture(mutate, options = {}) {
  const dir = fixtureRoot(mutate);
  try {
    return checkContract({ root: dir, serverBinary: "target/mien-server", dist: "dist", ...options });
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

test("parses the real Dockerfile without inventing stages", () => {
  const parsed = parseDockerfile(realDockerfile);
  assert.deepEqual(
    parsed.stages.map((stage) => stage.alias),
    ["runtime-base", "server-binary", "server"],
  );
  assert.equal(
    parsed.args.get("BASE_IMAGE"),
    "debian@sha256:e67eeead736ff7bbe258ef088bff683635c561b1fcfefad0806c1df4ab49ce0e",
  );
  assert.equal(parsed.args.get("SERVER_BINARY"), "mien-server");
});

test("runtime-base precedes the stages that need the kernel archives", () => {
  const parsed = parseDockerfile(realDockerfile);
  const aliases = parsed.stages.map((stage) => stage.alias);
  // A `--target runtime-base` build must not evaluate the server binary or the
  // 141 MiB Chromium COPY; that only holds while runtime-base comes first.
  assert.ok(aliases.indexOf("runtime-base") < aliases.indexOf("server-binary"));
  assert.ok(aliases.indexOf("runtime-base") < aliases.indexOf("server"));
});

test("reads every apt package the image installs", () => {
  const packages = aptPackages(parseDockerfile(realDockerfile));
  assert.equal(packages.length, 38);
  for (const expected of ["libgtk-3-0", "xvfb", "tini", "libnss3", "sqlite3"])
    assert.ok(packages.includes(expected), `${expected} must be part of the apt layer`);
  assert.ok(!packages.includes("install"), "the verb must not be read as a package");
  assert.ok(!packages.includes("--no-install-recommends"), "flags must not be read as packages");
});

test("resolves ARG references and rejects unresolved ones", () => {
  const args = new Map([["SERVER_BINARY", "mien-server"]]);
  assert.equal(resolveArgs(argRef("SERVER_BINARY"), args), "mien-server");
  assert.equal(resolveArgs("$SERVER_BINARY", args), "mien-server");
  assert.throws(() => resolveArgs(argRef("MISSING"), args), /unresolved build argument/);
});

test("collects only context COPY sources", () => {
  const sources = copySources(parseDockerfile(realDockerfile));
  assert.deepEqual(sources, [
    "chromium.tar.xz",
    "dist",
    "entrypoint.sh",
    "healthcheck.sh",
    "mien-server",
    "seccomp/chromium.json",
    "verify-server.sh",
    "xray.zip",
  ]);
  assert.ok(!sources.includes("/mien-server"), "COPY --from sources are not build-context files");
});

test("the checked-in Dockerfile satisfies the contract", () => {
  const result = checkContract({ root });
  assert.deepEqual(result.errors, []);
  assert.equal(result.info.stages, 3);
  assert.equal(result.info.aptPackages.length, 38);
});

test("rejects a floating base image tag", () => {
  const result = checkFixture((text) =>
    text.replace(
      `FROM ${argRef("BASE_IMAGE")} AS runtime-base`,
      "FROM debian:bookworm-slim AS runtime-base",
    ),
  );
  assert.ok(result.errors.some((error) => error.includes("pinned digest")));
});

test("rejects an asset digest that drifted from the frozen manifest", () => {
  const result = checkFixture((text) => text.replace(/ARG CHROMIUM_SHA256=[0-9a-f]{64}/, `ARG CHROMIUM_SHA256=${"0".repeat(64)}`));
  assert.ok(result.errors.some((error) => error.includes("CHROMIUM_SHA256")));
  assert.equal(assetDigests(root).chromium.sha256.length, 64);
});

test("rejects a COPY source that escapes the build context", () => {
  const result = checkFixture((text) => text.replace("COPY entrypoint.sh", "COPY ../entrypoint.sh"));
  assert.ok(result.errors.some((error) => error.includes("escapes the build context")));
});

test("rejects an unknown instruction instead of swallowing it", () => {
  const result = checkFixture((text) => text.replace("USER 10001:10001", "USRE 10001:10001"));
  assert.ok(result.errors.some((error) => error.includes("unknown instruction USRE")));
});

test("requires a nonzero USER in the final stage", () => {
  const result = checkFixture((text) => text.replace("USER 10001:10001", "USER 0:0"));
  assert.ok(result.errors.some((error) => error.includes("nonzero numeric uid")));
});

test("requires a HEALTHCHECK", () => {
  const result = checkFixture((text) =>
    text.replace(/HEALTHCHECK[^\n]*\\\n[ ]{2}CMD \[[^\n]*\]\n/, ""),
  );
  assert.ok(result.errors.some((error) => error.includes("HEALTHCHECK")));
});

test("require-artifacts rejects a missing binary and dist with actionable errors", () => {
  const result = checkContract({
    root,
    requireArtifacts: true,
    serverBinary: "target/does-not-exist/mien-server",
    dist: "target/does-not-exist/dist",
  });
  assert.ok(result.errors.some((error) => error.includes("server binary missing")));
  assert.ok(result.errors.some((error) => error.includes("static UI missing")));
});

function artifactRoot(machine) {
  const dir = fixtureRoot(null);
  const binary = path.join(dir, "target/x86_64-unknown-linux-gnu/release/mien-server");
  const elf = Buffer.alloc(64);
  elf.write("\x7fELF", 0, "latin1");
  elf[4] = 2; // ELFCLASS64
  elf.writeUInt16LE(machine, 18);
  mkdirSync(path.dirname(binary), { recursive: true });
  writeFileSync(binary, elf);
  chmodSync(binary, 0o755);
  mkdirSync(path.join(dir, "dist"), { recursive: true });
  writeFileSync(path.join(dir, "dist/index.html"), "<!doctype html>");
  return dir;
}

test("require-artifacts accepts a Linux x86-64 ELF and a built dist", () => {
  const dir = artifactRoot(0x3e);
  try {
    const result = checkContract({ root: dir, requireArtifacts: true });
    assert.deepEqual(result.errors, []);
    assert.equal(result.info.serverBinary.elf.machine, 0x3e);
    assert.equal(result.info.dist.indexBytes, "<!doctype html>".length);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("require-artifacts rejects a host-architecture binary", () => {
  const dir = artifactRoot(0xb7); // EM_AARCH64: the image would not exec it
  try {
    const result = checkContract({ root: dir, requireArtifacts: true });
    assert.ok(result.errors.some((error) => error.includes("not a Linux x86-64 ELF")));
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

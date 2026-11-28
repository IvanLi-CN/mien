// build.sh 上下文组装测试：`node --test scripts/release/build-context.test.mjs`。
//
// 这里真正执行 `deploy/docker/build.sh --target runtime-base --prepare-only`，断言它
// 在没有 Docker 的机器上仍能把上下文装齐、且打印的 docker 命令指向 runtime-base。
// 这不是模拟：脚本删掉暂存目录、重新创建、复制文件，测试检查的就是磁盘结果。
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const buildScript = path.join(root, "deploy/docker/build.sh");

function run(args) {
  try {
    return {
      status: 0,
      stdout: execFileSync(buildScript, args, {
        cwd: root,
        encoding: "utf8",
        stdio: ["ignore", "pipe", "pipe"],
      }),
    };
  } catch (error) {
    return { status: error.status ?? 1, stdout: `${error.stdout ?? ""}${error.stderr ?? ""}` };
  }
}

test("the OS-layer context stages exactly the files Dockerfile.server copies", () => {
  const context = mkdtempSync(path.join(tmpdir(), "mien-context-"));
  try {
    const result = run(["--target", "runtime-base", "--context-dir", context, "--prepare-only"]);
    assert.equal(result.status, 0, result.stdout);
    const staged = readdirSync(context).sort();
    assert.deepEqual(staged, [
      "Dockerfile.server",
      "entrypoint.sh",
      "healthcheck.sh",
      "seccomp",
      "verify-server.sh",
    ]);
    assert.ok(!staged.includes("mien-server"), "the OS layer must not need the server binary");
    assert.ok(!staged.includes("chromium.tar.xz"), "the OS layer must not need the kernel asset");
    assert.ok(existsSync(path.join(context, "seccomp/chromium.json")));
    assert.ok((statSync(path.join(context, "Dockerfile.server")).mode & 0o400) !== 0);
    assert.ok(result.stdout.includes("--target runtime-base"));
    assert.ok(result.stdout.includes("--platform linux/amd64"));
    assert.ok(result.stdout.includes(`-f ${context}/Dockerfile.server`));
  } finally {
    rmSync(context, { recursive: true, force: true });
  }
});

test("a re-run replaces stale context files instead of merging them", () => {
  const context = mkdtempSync(path.join(tmpdir(), "mien-context-"));
  try {
    writeFileSync(path.join(context, "stale.txt"), "left over from an older checkout");
    const result = run(["--target", "runtime-base", "--context-dir", context, "--prepare-only"]);
    assert.equal(result.status, 0, result.stdout);
    assert.ok(
      !existsSync(path.join(context, "stale.txt")),
      "stale files would be copied into the image",
    );
  } finally {
    rmSync(context, { recursive: true, force: true });
  }
});

test("the full image refuses to build without a Linux binary and dist", () => {
  const context = mkdtempSync(path.join(tmpdir(), "mien-context-"));
  try {
    const result = run([
      "--context-dir",
      context,
      "--server-binary",
      path.join(context, "absent-mien-server"),
      "--dist",
      path.join(context, "absent-dist"),
      "--prepare-only",
    ]);
    assert.equal(result.status, 1);
    assert.match(result.stdout, /server binary missing/);
    assert.match(result.stdout, /static UI missing/);
  } finally {
    rmSync(context, { recursive: true, force: true });
  }
});

test("the full image stages assets only after their digests match", () => {
  const work = mkdtempSync(path.join(tmpdir(), "mien-assets-"));
  try {
    const context = path.join(work, "context");
    const assets = path.join(work, "assets");
    const dist = path.join(work, "dist");
    mkdirSync(assets, { recursive: true });
    mkdirSync(dist, { recursive: true });
    writeFileSync(path.join(dist, "index.html"), "<!doctype html>");
    writeFileSync(path.join(assets, "chromium.tar.xz"), "not the real archive");
    writeFileSync(path.join(assets, "xray.zip"), "not the real archive");

    const binary = path.join(work, "mien-server");
    const elf = Buffer.alloc(64);
    elf.write("\x7fELF", 0, "latin1");
    elf[4] = 2;
    elf.writeUInt16LE(0x3e, 18);
    writeFileSync(binary, elf);
    chmodSync(binary, 0o755);

    const result = run([
      "--context-dir",
      context,
      "--server-binary",
      binary,
      "--dist",
      dist,
      "--assets-dir",
      assets,
      "--prepare-only",
    ]);
    assert.equal(result.status, 1, "a fake asset must never reach the build");
    assert.match(result.stdout, /sha256 mismatch/);
    assert.ok(!existsSync(path.join(context, "chromium.tar.xz")));
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
});

test("an unknown target is rejected instead of silently building the full image", () => {
  const result = run(["--target", "runtime", "--prepare-only"]);
  assert.equal(result.status, 1);
  assert.match(result.stdout, /--target must be 'server' or 'runtime-base'/);
});

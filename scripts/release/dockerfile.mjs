#!/usr/bin/env node
// 容器构建契约：解析 `deploy/docker/Dockerfile.server`，校验它与仓库输入一致。
//
// 这个模块是 `deploy/docker/build.sh`、`deploy/docker/scan-image.sh` 与 CI 的共同事实
// 来源：基础镜像 digest、apt 安装清单、COPY 源、内核/Xray 资产 digest 都从 Dockerfile
// 读取，脚本里不再抄第二份。任何一项对不上都 fail closed，不做猜测性修补。
import {
  closeSync,
  existsSync,
  openSync,
  readdirSync,
  readFileSync,
  readSync,
  statSync,
} from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const MODULE_DIR = path.dirname(fileURLToPath(import.meta.url));
const DEFAULT_ROOT = path.resolve(MODULE_DIR, "../..");
const DEFAULT_DOCKERFILE = "deploy/docker/Dockerfile.server";
const DEFAULT_SERVER_BINARY = "target/x86_64-unknown-linux-gnu/release/mien-server";
const DEFAULT_DIST = "dist";

/** Dockerfile 允许的指令；未知大写词直接报错，而不是被当成续行吞掉。 */
const INSTRUCTIONS = new Set([
  "ADD",
  "ARG",
  "CMD",
  "COPY",
  "ENTRYPOINT",
  "ENV",
  "EXPOSE",
  "FROM",
  "HEALTHCHECK",
  "LABEL",
  "MAINTAINER",
  "ONBUILD",
  "RUN",
  "SHELL",
  "STOPSIGNAL",
  "USER",
  "VOLUME",
  "WORKDIR",
]);

const SHELL_SEPARATORS = new Set(["&&", ";", "|", "||"]);
const APT_PACKAGE_PATTERN = /^[a-z0-9][a-z0-9+.-]*(=[A-Za-z0-9.+~:-]+)?$/;
const DIGEST_REFERENCE = /^[^@\s]+@sha256:[0-9a-f]{64}$/;

const USAGE = `用法：
  dockerfile.mjs check [--root DIR] [--dockerfile PATH] [--server-binary PATH]
                       [--dist PATH] [--require-artifacts]
  dockerfile.mjs base-image [--root DIR] [--dockerfile PATH]
  dockerfile.mjs apt-packages [--root DIR] [--dockerfile PATH]
  dockerfile.mjs copy-sources [--root DIR] [--dockerfile PATH]
  dockerfile.mjs assets [--root DIR] [--dockerfile PATH]

check 校验基础镜像 digest、资产摘要、apt 清单、COPY 源与 Dockerfile 结构；
--require-artifacts 额外要求服务端二进制（Linux x86-64 ELF）与 dist/ 已就绪。
其余子命令只把契约里的值打印成行，供构建与扫描脚本复用。`;

function stripQuotes(value) {
  const trimmed = value.trim();
  if (trimmed.length >= 2) {
    const first = trimmed[0];
    const last = trimmed[trimmed.length - 1];
    if ((first === '"' && last === '"') || (first === "'" && last === "'"))
      return trimmed.slice(1, -1);
  }
  return trimmed;
}

/** 把以反斜杠续行的 Dockerfile 文本折成逻辑行，并保留起始行号。 */
export function logicalLines(text) {
  const result = [];
  let buffer = "";
  let startLine = 0;
  const lines = text.split("\n");
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index].replace(/\r$/, "");
    if (buffer === "") startLine = index + 1;
    const continued = /\\\s*$/.test(line);
    buffer += continued ? `${line.replace(/\\\s*$/, "")} ` : line;
    if (!continued) {
      result.push({ line: startLine, text: buffer });
      buffer = "";
    }
  }
  if (buffer !== "") result.push({ line: startLine, text: buffer });
  return result;
}

/** 解析成 { args, stages, instructions }；args 只保留带默认值的 ARG。 */
export function parseDockerfile(text) {
  const args = new Map();
  const stages = [];
  const instructions = [];
  let current = null;
  for (const entry of logicalLines(text)) {
    const trimmed = entry.text.trim();
    if (trimmed === "" || trimmed.startsWith("#")) continue;
    const match = /^([A-Z]+)(?:\s+(.*))?$/.exec(trimmed);
    if (!match) throw new Error(`line ${entry.line}: cannot parse instruction: ${trimmed}`);
    const name = match[1];
    const rest = (match[2] ?? "").trim();
    if (!INSTRUCTIONS.has(name))
      throw new Error(`line ${entry.line}: unknown instruction ${name} (uppercase only)`);
    const instruction = { line: entry.line, name, args: rest };
    instructions.push(instruction);
    if (name === "FROM") {
      const tokens = rest.split(/\s+/);
      const asIndex = tokens.indexOf("AS");
      current = {
        line: entry.line,
        base: tokens[0],
        alias: asIndex === -1 ? null : tokens[asIndex + 1],
        instructions: [],
      };
      stages.push(current);
      continue;
    }
    if (name === "ARG") {
      const arg = /^([A-Za-z_][A-Za-z0-9_]*)(?:=(.*))?$/.exec(rest);
      if (!arg) throw new Error(`line ${entry.line}: cannot parse ARG ${rest}`);
      if (arg[2] !== undefined) args.set(arg[1], stripQuotes(arg[2]));
    }
    if (current) current.instructions.push(instruction);
  }
  if (stages.length === 0) throw new Error("no FROM instruction");
  return { args, stages, instructions };
}

/** 用 Dockerfile 里的 ARG 默认值展开 `${NAME}` / `$NAME`；未定义即报错。 */
export function resolveArgs(value, args) {
  return value.replace(
    /\$\{([A-Za-z_][A-Za-z0-9_]*)\}|\$([A-Za-z_][A-Za-z0-9_]*)/g,
    (_match, braced, bare) => {
      const name = braced ?? bare;
      if (!args.has(name)) throw new Error(`unresolved build argument \${${name}}`);
      return args.get(name);
    },
  );
}

/** 无 `--from` 的 COPY 源（上下文相对路径，已展开 ARG，已去尾斜杠）。 */
export function copySources(parsed) {
  const sources = new Set();
  for (const instruction of parsed.instructions) {
    if (instruction.name !== "COPY") continue;
    const tokens = instruction.args.split(/\s+/).filter(Boolean);
    if (tokens[0].startsWith("["))
      throw new Error(`line ${instruction.line}: JSON-form COPY is not supported by build.sh`);
    const positional = [];
    let fromStage = false;
    for (let index = 0; index < tokens.length; index += 1) {
      const token = tokens[index];
      if (token === "--from") {
        fromStage = true;
        index += 1;
        continue;
      }
      if (token.startsWith("--from=")) {
        fromStage = true;
        continue;
      }
      if (token.startsWith("--")) continue;
      positional.push(token);
    }
    if (fromStage) continue;
    if (positional.length < 2)
      throw new Error(`line ${instruction.line}: COPY needs at least one source and a destination`);
    for (const source of positional.slice(0, -1)) {
      const resolved = path.posix.normalize(resolveArgs(source, parsed.args)).replace(/\/$/, "");
      if (resolved.startsWith("../") || resolved === "..")
        throw new Error(`line ${instruction.line}: COPY source escapes the build context: ${source}`);
      sources.add(resolved);
    }
  }
  return [...sources].sort();
}

/** `apt-get install` 的包清单；版本后缀保留，调用方按需去掉。 */
export function aptPackages(parsed) {
  const packages = new Set();
  for (const instruction of parsed.instructions) {
    if (instruction.name !== "RUN") continue;
    let segmentHasAptGet = false;
    let collecting = false;
    for (const token of instruction.args.split(/\s+/).filter(Boolean)) {
      if (SHELL_SEPARATORS.has(token)) {
        segmentHasAptGet = false;
        collecting = false;
        continue;
      }
      if (token === "apt-get") {
        segmentHasAptGet = true;
        continue;
      }
      if (collecting) {
        if (token.startsWith("-")) continue;
        if (!APT_PACKAGE_PATTERN.test(token))
          throw new Error(`line ${instruction.line}: unexpected apt package token ${token}`);
        packages.add(token);
        continue;
      }
      if (token === "install" && segmentHasAptGet) collecting = true;
    }
  }
  if (packages.size === 0) throw new Error("no apt-get install packages found");
  return [...packages];
}

/** 冻结的 linux-x86_64 内核与 Xray 清单；每个都必须恰好命中一份。 */
export function assetDigests(root = DEFAULT_ROOT) {
  const directory = path.join(root, "docs/contracts/manifests");
  const files = readdirSync(directory).filter((name) => name.endsWith(".json"));
  const pick = (predicate, label) => {
    const matches = files.filter(predicate);
    if (matches.length !== 1)
      throw new Error(
        `expected exactly one ${label} manifest in docs/contracts/manifests, found ${matches.length}`,
      );
    const document = JSON.parse(readFileSync(path.join(directory, matches[0]), "utf8"));
    if (typeof document.sha256 !== "string" || !/^[0-9a-f]{64}$/.test(document.sha256))
      throw new Error(`${matches[0]}: missing 64-hex sha256`);
    return { file: matches[0], sha256: document.sha256 };
  };
  return {
    chromium: pick(
      (name) => name.startsWith("fingerprint-chromium-") && name.includes("-linux-x86_64"),
      "Chromium linux-x86_64",
    ),
    xray: pick((name) => name.startsWith("xray-core-") && name.includes("-linux-x86_64"), "Xray linux-x86_64"),
  };
}

function inspectElf(filename) {
  const fd = openSync(filename, "r");
  try {
    const header = Buffer.alloc(20);
    readSync(fd, header, 0, 20, 0);
    return {
      magic: header.subarray(0, 4).toString("hex") === "7f454c46",
      class64: header[4] === 2,
      machine: header[18] + (header[19] << 8),
    };
  } finally {
    closeSync(fd);
  }
}

function validateServerBinary(filename, errors) {
  if (!existsSync(filename) || !statSync(filename).isFile()) {
    errors.push(
      `server binary missing: ${filename} (build it with: cargo zigbuild --release -p mien-server --target x86_64-unknown-linux-gnu)`,
    );
    return null;
  }
  const stats = statSync(filename);
  if ((stats.mode & 0o111) === 0) errors.push(`server binary is not executable: ${filename}`);
  const elf = inspectElf(filename);
  if (!elf.magic || !elf.class64 || elf.machine !== 0x3e)
    errors.push(
      `server binary is not a Linux x86-64 ELF (${filename}); the image runs linux/amd64 and a host build would not start`,
    );
  return { path: filename, bytes: stats.size, elf };
}

function validateDist(directory, errors) {
  const index = path.join(directory, "index.html");
  if (!existsSync(index) || statSync(index).size === 0) {
    errors.push(`static UI missing: ${index} (build it with: bun run build)`);
    return null;
  }
  return { path: directory, entries: readdirSync(directory).length, indexBytes: statSync(index).size };
}

/**
 * 校验 Dockerfile 契约与（可选的）构建产物。
 * 返回 { errors, info }；只有 errors 为空才算通过。
 */
export function checkContract(options = {}) {
  const root = options.root ?? DEFAULT_ROOT;
  const dockerfileRelative = options.dockerfile ?? DEFAULT_DOCKERFILE;
  const dockerfile = path.resolve(root, dockerfileRelative);
  const serverBinary = path.resolve(root, options.serverBinary ?? DEFAULT_SERVER_BINARY);
  const dist = path.resolve(root, options.dist ?? DEFAULT_DIST);
  const errors = [];
  const info = { dockerfile: dockerfileRelative, stages: 0, aptPackages: [], copySources: [], assets: [] };

  if (!existsSync(dockerfile)) {
    errors.push(`Dockerfile missing: ${dockerfileRelative}; the container build has no entry point`);
    return { errors, info };
  }

  let parsed;
  try {
    parsed = parseDockerfile(readFileSync(dockerfile, "utf8"));
  } catch (error) {
    errors.push(`${dockerfileRelative}: ${error.message}`);
    return { errors, info };
  }
  info.stages = parsed.stages.length;
  info.baseImage = parsed.args.get("BASE_IMAGE") ?? null;

  const baseImage = parsed.args.get("BASE_IMAGE");
  if (!baseImage) errors.push("BASE_IMAGE default is missing; the base image must be pinned by digest");
  else if (!DIGEST_REFERENCE.test(baseImage))
    errors.push(`BASE_IMAGE must be pinned by sha256 digest, got ${baseImage}`);
  for (const stage of parsed.stages) {
    const resolved = resolveArgs(stage.base, parsed.args);
    const known = parsed.stages.some((candidate) => candidate.alias === resolved);
    if (!known && !DIGEST_REFERENCE.test(resolved))
      errors.push(`line ${stage.line}: stage base ${resolved} is neither a pinned digest nor an earlier stage`);
  }

  for (const [name, expected] of [
    ["SERVER_BINARY", "mien-server"],
    ["CHROMIUM_ARCHIVE", "chromium.tar.xz"],
    ["XRAY_ARCHIVE", "xray.zip"],
  ]) {
    const actual = parsed.args.get(name);
    if (actual !== expected)
      errors.push(`ARG ${name} must default to ${expected} (got ${actual ?? "nothing"}); build.sh stages that name`);
  }

  try {
    const digests = assetDigests(root);
    info.assets = [
      { name: "chromium.tar.xz", sha256: digests.chromium.sha256, manifest: digests.chromium.file },
      { name: "xray.zip", sha256: digests.xray.sha256, manifest: digests.xray.file },
    ];
    for (const [arg, digest, label] of [
      ["CHROMIUM_SHA256", digests.chromium.sha256, "chromium"],
      ["XRAY_SHA256", digests.xray.sha256, "xray"],
    ]) {
      const actual = parsed.args.get(arg);
      if (actual !== digest)
        errors.push(`ARG ${arg} (${actual ?? "unset"}) does not match the frozen ${label} manifest ${digest}`);
    }
  } catch (error) {
    errors.push(error.message);
  }

  try {
    info.aptPackages = aptPackages(parsed);
  } catch (error) {
    errors.push(error.message);
  }

  try {
    info.copySources = copySources(parsed);
  } catch (error) {
    errors.push(error.message);
  }
  const repoOwned = new Set(info.copySources);
  for (const source of repoOwned) {
    const staged = path.join(root, "deploy/docker", source);
    const artifact = source === "mien-server" || source === "dist" || source === "chromium.tar.xz" || source === "xray.zip";
    if (!artifact && !existsSync(staged))
      errors.push(`COPY source ${source} is not staged by build.sh and does not exist at deploy/docker/${source}`);
  }

  const finalStage = parsed.stages[parsed.stages.length - 1];
  const userInstructions = finalStage.instructions.filter((instruction) => instruction.name === "USER");
  if (userInstructions.length === 0) errors.push("final stage has no USER; the image must not run as root");
  else {
    const user = userInstructions[userInstructions.length - 1].args.split(":")[0];
    if (!/^[0-9]+$/.test(user) || Number(user) === 0)
      errors.push(`final stage USER must be a nonzero numeric uid, got ${userInstructions.at(-1).args}`);
  }
  const countOf = (name) => finalStage.instructions.filter((instruction) => instruction.name === name).length;
  if (countOf("ENTRYPOINT") !== 1) errors.push("final stage must declare exactly one ENTRYPOINT");
  if (countOf("HEALTHCHECK") !== 1) errors.push("final stage must declare exactly one HEALTHCHECK");

  if (options.requireArtifacts) {
    info.serverBinary = validateServerBinary(serverBinary, errors);
    info.dist = validateDist(dist, errors);
  }
  return { errors, info };
}

function formatBytes(bytes) {
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  return `${bytes} B`;
}

function printCheck(result) {
  if (result.errors.length > 0) {
    console.error("container-contract: FAIL");
    for (const error of result.errors) console.error(`  - ${error}`);
    return;
  }
  const { info } = result;
  console.log("container-contract: PASS");
  console.log(`  dockerfile: ${info.dockerfile} (${info.stages} stage(s))`);
  console.log(`  base-image: ${info.baseImage}`);
  console.log(`  apt-packages: ${info.aptPackages.length}`);
  console.log(`  copy-sources: ${info.copySources.join(", ")}`);
  for (const asset of info.assets) console.log(`  asset: ${asset.name} sha256=${asset.sha256}`);
  if (info.serverBinary)
    console.log(
      `  server-binary: ${info.serverBinary.path} (ELF x86-64, ${formatBytes(info.serverBinary.bytes)})`,
    );
  if (info.dist) console.log(`  web-dist: ${info.dist.path} (${info.dist.entries} entries)`);
}

export function parseArgs(argv) {
  const options = {
    command: "check",
    root: DEFAULT_ROOT,
    dockerfile: DEFAULT_DOCKERFILE,
    serverBinary: DEFAULT_SERVER_BINARY,
    dist: DEFAULT_DIST,
    requireArtifacts: false,
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
    if (arg === "--help" || arg === "-h") options.help = true;
    else if (arg === "--require-artifacts") options.requireArtifacts = true;
    else if (arg === "--root") options.root = path.resolve(next());
    else if (arg === "--dockerfile") options.dockerfile = next();
    else if (arg === "--server-binary") options.serverBinary = next();
    else if (arg === "--dist") options.dist = next();
    else if (arg.startsWith("-")) throw new Error(`unknown argument ${arg}`);
    else options.command = arg;
  }
  if (!["check", "base-image", "apt-packages", "copy-sources", "assets"].includes(options.command))
    throw new Error(`unknown command ${options.command}`);
  return options;
}

function loadParsed(options) {
  const dockerfile = path.resolve(options.root, options.dockerfile);
  if (!existsSync(dockerfile)) throw new Error(`${options.dockerfile}: Dockerfile not found`);
  return parseDockerfile(readFileSync(dockerfile, "utf8"));
}

export function main(argv = process.argv.slice(2)) {
  let options;
  try {
    options = parseArgs(argv);
  } catch (error) {
    console.error(`container-contract: ${error.message}\n\n${USAGE}`);
    return 2;
  }
  if (options.help) {
    console.log(USAGE);
    return 0;
  }
  if (options.command === "check") {
    const result = checkContract(options);
    printCheck(result);
    return result.errors.length === 0 ? 0 : 1;
  }
  try {
    const parsed = loadParsed(options);
    if (options.command === "base-image") {
      const baseImage = parsed.args.get("BASE_IMAGE");
      if (!baseImage) throw new Error("BASE_IMAGE default is missing");
      console.log(baseImage);
    } else if (options.command === "apt-packages") {
      for (const pkg of aptPackages(parsed)) console.log(pkg);
    } else if (options.command === "copy-sources") {
      for (const source of copySources(parsed)) console.log(source);
    } else {
      for (const [name, digest] of [
        ["chromium.tar.xz", parsed.args.get("CHROMIUM_SHA256")],
        ["xray.zip", parsed.args.get("XRAY_SHA256")],
      ]) {
        if (!digest) throw new Error(`ARG for ${name} is missing`);
        console.log(`${name} ${digest}`);
      }
    }
  } catch (error) {
    console.error(`container-contract: ${error.message}`);
    return 1;
  }
  return 0;
}

if (process.argv[1] && path.resolve(process.argv[1]) === path.resolve(fileURLToPath(import.meta.url)))
  process.exitCode = main();

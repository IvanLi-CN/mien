#!/usr/bin/env node
// PKG-003: 更新清单必须签名有效，且拒绝未签名或降级包。
//
// Tauri 的 updater 把 `latest.json` 里的 `signature` 与更新包本体对照：
// 签名是 base64(minisign 文本)，minisign 文本第二行是 2 字节算法标记加
// keyid 加 64 字节 Ed25519 签名；Tauri 用 `ED`（BLAKE2b-512 预哈希）而不是
// minisign 默认的 `Ed`（原文直签），实测两者不可混用，所以这里两种都按标记
// 分支处理，标记与算法不符直接拒绝。
//
// 这个检查器只依赖 Node 内置 crypto，不要求 minisign 或 tauri CLI 在场，
// 因为它必须能在发布流水线与离线复核里跑。
import { createHash, createPublicKey, verify as verifySignature } from "node:crypto";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const MINISIGN_PUBLIC_PREFIX = "untrusted comment:";
const ED25519_SIGNATURE_BYTES = 64;

/** 解析 minisign 公钥：base64(minisign 文本) 或裸 minisign 文本都接受。 */
export function parsePublicKey(input) {
  const text = decodeMaybeBase64(input);
  const lines = text.trim().split("\n");
  if (!lines[0]?.startsWith(MINISIGN_PUBLIC_PREFIX)) {
    throw new Error("public key is not a minisign key: missing untrusted comment");
  }
  const raw = Buffer.from(lines[1] ?? "", "base64");
  if (raw.length !== 42) {
    throw new Error(`public key must be 42 bytes (algorithm, key id, key); got ${raw.length}`);
  }
  const algorithm = raw.subarray(0, 2).toString("latin1");
  if (algorithm !== "Ed") {
    throw new Error(`public key algorithm must be Ed; got ${JSON.stringify(algorithm)}`);
  }
  return { keyId: raw.subarray(2, 10), publicKey: raw.subarray(10, 42) };
}

/** 解析签名块：base64(minisign 文本) 或裸 minisign 文本都接受。 */
export function parseSignature(input) {
  const text = decodeMaybeBase64(input);
  const lines = text.trim().split("\n");
  if (!lines[0]?.startsWith("untrusted comment:")) {
    throw new Error("signature is not a minisign signature: missing untrusted comment");
  }
  const raw = Buffer.from(lines[1] ?? "", "base64");
  // 2 字节算法 + 8 字节 key id + 64 字节签名，之后是可选的全局签名。
  if (raw.length < 74) {
    throw new Error(`signature block must be at least 74 bytes; got ${raw.length}`);
  }
  const algorithm = raw.subarray(0, 2).toString("latin1");
  if (algorithm !== "Ed" && algorithm !== "ED") {
    throw new Error(
      `unsupported signature algorithm ${JSON.stringify(algorithm)}; expected Ed or ED`,
    );
  }
  return {
    algorithm,
    keyId: raw.subarray(2, 10),
    signature: raw.subarray(10, 10 + ED25519_SIGNATURE_BYTES),
  };
}

/** base64 包了一层就直接解开；已经是文本则原样返回。 */
function decodeMaybeBase64(input) {
  const trimmed = input.trim();
  if (trimmed.startsWith(MINISIGN_PUBLIC_PREFIX) || trimmed.startsWith("untrusted comment:")) {
    return trimmed;
  }
  const decoded = Buffer.from(trimmed, "base64").toString("utf8");
  if (decoded.startsWith("untrusted comment:")) return decoded;
  return trimmed;
}

/**
 * 按签名算法校验 `payload`。
 * `ED` 验证 BLAKE2b-512 预哈希，`Ed` 验证原文；混用会静默失败，所以这里严格分支。
 */
export function verifyPayload({ publicKey, signature, payload }) {
  const key = createPublicKey({
    key: { kty: "OKP", crv: "Ed25519", x: publicKey.publicKey.toString("base64url") },
    format: "jwk",
  });
  const message =
    signature.algorithm === "ED" ? createHash("blake2b512").update(payload).digest() : payload;
  return verifySignature(null, message, key, signature.signature);
}

/**
 * 比较语义版本；返回负数表示 `left < right`。
 * 只支持 `X.Y.Z` 与可选的预发布后缀，足以覆盖 `release_target` 产出的版本。
 */
export function compareVersions(left, right) {
  const parse = (value) => {
    const match = /^(\d+)\.(\d+)\.(\d+)(?:-(.+))?$/.exec(value.trim());
    if (!match) throw new Error(`not a semantic version: ${value}`);
    return {
      core: [Number(match[1]), Number(match[2]), Number(match[3])],
      pre: match[4] ? match[4].split(".") : [],
    };
  };
  const a = parse(left);
  const b = parse(right);
  for (let index = 0; index < 3; index += 1) {
    if (a.core[index] !== b.core[index]) return a.core[index] - b.core[index];
  }
  // 有预发布后缀的版本低于同号正式版；两者都有时按标识符逐段比较。
  if (a.pre.length === 0 && b.pre.length === 0) return 0;
  if (a.pre.length === 0) return 1;
  if (b.pre.length === 0) return -1;
  for (let index = 0; index < Math.max(a.pre.length, b.pre.length); index += 1) {
    const leftPart = a.pre[index];
    const rightPart = b.pre[index];
    if (leftPart === undefined) return -1;
    if (rightPart === undefined) return 1;
    const leftNumeric = /^\d+$/.test(leftPart);
    const rightNumeric = /^\d+$/.test(rightPart);
    if (leftNumeric && rightNumeric) {
      const diff = Number(leftPart) - Number(rightPart);
      if (diff !== 0) return diff;
      continue;
    }
    if (leftNumeric !== rightNumeric) return leftNumeric ? -1 : 1;
    if (leftPart !== rightPart) return leftPart < rightPart ? -1 : 1;
  }
  return 0;
}

/**
 * 校验一份 `latest.json`。
 * `currentVersion` 给出时，清单版本必须严格更高，否则判定为降级包。
 */
export function checkUpdateManifest({ manifest, publicKey, payload, currentVersion, platform }) {
  const errors = [];
  const platforms = manifest?.platforms ?? {};
  const target = platform ?? Object.keys(platforms)[0];
  if (!target || !platforms[target]) {
    errors.push(`manifest has no entry for platform ${target ?? "(none)"}`);
    return { ok: false, errors, version: manifest?.version };
  }
  const entry = platforms[target];
  const version = manifest.version;

  if (typeof version !== "string" || version.trim() === "") {
    errors.push("manifest version is missing");
  } else if (currentVersion !== undefined) {
    let order;
    try {
      order = compareVersions(version, currentVersion);
    } catch (error) {
      errors.push(`cannot compare versions: ${error.message}`);
      order = 0;
    }
    if (order <= 0) {
      errors.push(
        `refusing downgrade: manifest ${version} is not newer than installed ${currentVersion}`,
      );
    }
  }

  if (typeof entry.signature !== "string" || entry.signature.trim() === "") {
    errors.push("update entry has no signature; unsigned packages are not installable");
    return { ok: false, errors, version };
  }

  let parsedSignature;
  try {
    parsedSignature = parseSignature(entry.signature);
  } catch (error) {
    errors.push(`signature is malformed: ${error.message}`);
    return { ok: false, errors, version };
  }

  if (!parsedSignature.keyId.equals(publicKey.keyId)) {
    errors.push(
      `signature key id ${parsedSignature.keyId.toString("hex")} does not match the shipped public key ${publicKey.keyId.toString("hex")}`,
    );
    return { ok: false, errors, version };
  }

  if (!verifyPayload({ publicKey, signature: parsedSignature, payload })) {
    errors.push("signature does not verify against the update payload");
    return { ok: false, errors, version };
  }

  return { ok: errors.length === 0, errors, version };
}

function main(argv) {
  const flag = (name) => {
    const index = argv.indexOf(name);
    return index >= 0 ? argv[index + 1] : undefined;
  };
  if (argv.includes("--help") || argv.includes("-h")) {
    console.log(
      "usage: updater-signature.mjs --manifest PATH --public-key PATH --payload PATH [--current VERSION] [--platform NAME]",
    );
    return 0;
  }

  const manifestPath = flag("--manifest");
  const keyPath = flag("--public-key");
  const payloadPath = flag("--payload");
  if (!manifestPath || !keyPath || !payloadPath) {
    console.error("updater-signature: --manifest, --public-key and --payload are required");
    return 2;
  }

  const result = checkUpdateManifest({
    manifest: JSON.parse(readFileSync(manifestPath, "utf8")),
    publicKey: parsePublicKey(readFileSync(keyPath, "utf8")),
    payload: readFileSync(payloadPath),
    currentVersion: flag("--current"),
    platform: flag("--platform"),
  });

  for (const error of result.errors) console.error(`updater-signature: ${error}`);
  if (result.ok) console.log(`updater-signature: PASS (version ${result.version})`);
  return result.ok ? 0 : 1;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  process.exit(main(process.argv.slice(2)));
}

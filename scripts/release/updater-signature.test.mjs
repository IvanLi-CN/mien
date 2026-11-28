// PKG-003: 更新清单签名必须有效，未签名与降级包必须被拒绝。
//
// 这些用例里的密钥与签名是测试内即时生成的，不依赖仓库外的私钥；测试只做
// 签名的验签路径，不测试 Tauri CLI 本身。

import assert from "node:assert/strict";
import { createHash, generateKeyPairSync, sign as signBytes } from "node:crypto";
import { test } from "node:test";

import {
  checkUpdateManifest,
  compareVersions,
  parsePublicKey,
  parseSignature,
  verifyPayload,
} from "./updater-signature.mjs";

const KEY_ID = Buffer.from("0102030405060708", "hex");

/** 生成一对 Ed25519 密钥，并按 minisign 结构编码成公钥/签名文本。 */
function makeKeypair() {
  const { publicKey, privateKey } = generateKeyPairSync("ed25519");
  const rawPublic = Buffer.from(publicKey.export({ format: "jwk" }).x, "base64url");
  const publicText = [
    "untrusted comment: minisign public key: 0102030405060708",
    Buffer.concat([Buffer.from("Ed", "latin1"), KEY_ID, rawPublic]).toString("base64"),
  ].join("\n");
  return { privateKey, publicText };
}

/**
 * 按 Tauri 的方式签名：`ED` 算法对 BLAKE2b-512 预哈希签名。
 * 返回值是 minisign 文本，调用方再 base64 一层，与 Tauri 的 `.sig` 文件一致。
 */
function signPayload(privateKey, payload, { keyId = KEY_ID, algorithm = "ED" } = {}) {
  const message = algorithm === "ED" ? createHash("blake2b512").update(payload).digest() : payload;
  const signature = signBytes(null, message, privateKey);
  const block = Buffer.concat([
    Buffer.from(algorithm, "latin1"),
    keyId,
    signature,
    Buffer.alloc(64),
  ]);
  return [
    "untrusted comment: signature from test key",
    block.toString("base64"),
    "trusted comment: timestamp:1700000000\tfile:payload",
    Buffer.alloc(64).toString("base64"),
  ].join("\n");
}

/** Tauri 把整个签名文本再 base64 一次写进 `latest.json`。 */
function encodeSignature(publicKeyText) {
  return Buffer.from(publicKeyText, "utf8").toString("base64");
}

function manifestFor(signature, version = "2.0.0") {
  return {
    version,
    platforms: {
      "darwin-aarch64": { signature, url: "https://example.invalid/app.tar.gz" },
    },
  };
}

test("真实格式的签名通过验证", () => {
  const { privateKey, publicText } = makeKeypair();
  const payload = Buffer.from("update payload bytes");
  const signature = encodeSignature(signPayload(privateKey, payload));
  const result = checkUpdateManifest({
    manifest: manifestFor(signature),
    publicKey: parsePublicKey(publicText),
    payload,
    currentVersion: "1.9.0",
  });
  assert.deepEqual(result.errors, []);
  assert.equal(result.ok, true);
});

test("裸 minisign 文本（未再包一层 base64）也能解析", () => {
  const { privateKey, publicText } = makeKeypair();
  const payload = Buffer.from("payload");
  const raw = signPayload(privateKey, payload);
  const parsed = parseSignature(raw);
  const key = parsePublicKey(publicText);
  assert.equal(verifyPayload({ publicKey: key, signature: parsed, payload }), true);
});

test("ED 与 Ed 两种算法不能混用", () => {
  const { privateKey, publicText } = makeKeypair();
  const payload = Buffer.from("payload");
  const edSignature = parseSignature(signPayload(privateKey, payload, { algorithm: "Ed" }));
  assert.equal(
    verifyPayload({ publicKey: parsePublicKey(publicText), signature: edSignature, payload }),
    true,
    "Ed 应在原文上验证",
  );
  // 把它标成 ED 后必须在预哈希上验证，对原文验证会失败；这正是不能混用的原因。
  const mislabelled = { ...edSignature, algorithm: "ED" };
  assert.equal(
    verifyPayload({ publicKey: parsePublicKey(publicText), signature: mislabelled, payload }),
    false,
  );
});

test("没有签名直接拒绝", () => {
  const { publicText } = makeKeypair();
  const result = checkUpdateManifest({
    manifest: manifestFor(""),
    publicKey: parsePublicKey(publicText),
    payload: Buffer.from("x"),
  });
  assert.equal(result.ok, false);
  assert.match(result.errors.join(";"), /no signature/);
});

test("key id 不匹配时拒绝", () => {
  const { privateKey, publicText } = makeKeypair();
  const payload = Buffer.from("payload");
  const otherId = Buffer.from("ffffffffffffffff", "hex");
  const signature = encodeSignature(signPayload(privateKey, payload, { keyId: otherId }));
  const result = checkUpdateManifest({
    manifest: manifestFor(signature),
    publicKey: parsePublicKey(publicText),
    payload,
  });
  assert.equal(result.ok, false);
  assert.match(result.errors.join(";"), /key id/);
});

test("负载被改动后拒绝", () => {
  const { privateKey, publicText } = makeKeypair();
  const signature = encodeSignature(signPayload(privateKey, Buffer.from("original")));
  const result = checkUpdateManifest({
    manifest: manifestFor(signature),
    publicKey: parsePublicKey(publicText),
    payload: Buffer.from("tampered"),
  });
  assert.equal(result.ok, false);
  assert.match(result.errors.join(";"), /does not verify/);
});

test("降级与同版本都拒绝", () => {
  const { privateKey, publicText } = makeKeypair();
  const payload = Buffer.from("payload");
  const signature = encodeSignature(signPayload(privateKey, payload));
  const key = parsePublicKey(publicText);

  const downgrade = checkUpdateManifest({
    manifest: manifestFor(signature, "2.0.0"),
    publicKey: key,
    payload,
    currentVersion: "2.1.0",
  });
  assert.equal(downgrade.ok, false);
  assert.match(downgrade.errors.join(";"), /downgrade/);

  const same = checkUpdateManifest({
    manifest: manifestFor(signature, "2.0.0"),
    publicKey: key,
    payload,
    currentVersion: "2.0.0",
  });
  assert.equal(same.ok, false);
});

test("平台条目缺失时报错而不是静默通过", () => {
  const { publicText } = makeKeypair();
  const result = checkUpdateManifest({
    manifest: { version: "2.0.0", platforms: {} },
    publicKey: parsePublicKey(publicText),
    payload: Buffer.from("x"),
  });
  assert.equal(result.ok, false);
  assert.match(result.errors.join(";"), /no entry for platform/);
});

test("畸形签名报错而不是抛出", () => {
  const { publicText } = makeKeypair();
  const result = checkUpdateManifest({
    manifest: manifestFor("not base64 minisign at all"),
    publicKey: parsePublicKey(publicText),
    payload: Buffer.from("x"),
  });
  assert.equal(result.ok, false);
  assert.match(result.errors.join(";"), /malformed/);
});

test("公钥长度与算法被严格校验", () => {
  assert.throws(() => parsePublicKey("untrusted comment: x\nAAAA"), /42 bytes/);
  const wrongAlg = Buffer.concat([Buffer.from("XX", "latin1"), KEY_ID, Buffer.alloc(32)]);
  assert.throws(
    () => parsePublicKey(`untrusted comment: x\n${wrongAlg.toString("base64")}`),
    /algorithm must be Ed/,
  );
});

test("版本比较覆盖预发布与段数差异", () => {
  assert.ok(compareVersions("1.0.0", "1.0.1") < 0);
  assert.ok(compareVersions("2.0.0", "1.99.99") > 0);
  assert.equal(compareVersions("1.2.3", "1.2.3"), 0);
  assert.ok(compareVersions("1.0.0-alpha", "1.0.0") < 0);
  assert.ok(compareVersions("1.0.0-alpha.1", "1.0.0-alpha.2") < 0);
  assert.throws(() => compareVersions("v1.0.0", "1.0.0"), /semantic version/);
});

// PKG-009: the workflow must actually call the tested policy, and must not pretend
// to publish.
//
// The policy rules are tested above; this file tests the wiring. A workflow that
// stopped calling `resolve`, `check-manifest`, `check-attestation` or
// `check-secrets` would leave every rule green in tests and unenforced in
// production, which is the failure mode PKG-009 exists to prevent. It also asserts
// the negative: no job builds, tags, pushes or publishes, and the one job that
// would publish fails closed.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { parse } from "yaml";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(HERE, "..", "..");
const WORKFLOW = path.join(ROOT, ".github", "workflows", "release.yml");

const workflow = () => parse(readFileSync(WORKFLOW, "utf8"));

/** Every `run:` body in the workflow, joined, for command assertions. */
function allRunScripts(document) {
  const scripts = [];
  for (const job of Object.values(document.jobs ?? {})) {
    for (const step of job.steps ?? []) {
      if (typeof step.run === "string") scripts.push(step.run);
    }
  }
  return scripts.join("\n");
}

test("the only trigger is workflow_dispatch with a required release_target", () => {
  const document = workflow();
  assert.equal(document.on.workflow_dispatch.inputs.release_target.required, true);
  assert.equal(document.on.workflow_dispatch.inputs.release_target.default, undefined);
  // A second input would allow a `mode`/`impact` alias the policy forbids.
  assert.deepEqual(Object.keys(document.on.workflow_dispatch.inputs), ["release_target"]);
  assert.deepEqual(Object.keys(document.on), ["workflow_dispatch"]);
});

test("release runs are serialized and never cancelled", () => {
  const document = workflow();
  assert.equal(document.concurrency.group, "release");
  assert.equal(document.concurrency["cancel-in-progress"], false);
});

test("every policy rule is invoked by the workflow", () => {
  const scripts = allRunScripts(workflow());
  for (const command of [
    "release-policy.mjs resolve",
    "release-policy.mjs bind-reservation",
    "release-policy.mjs check-manifest",
    "release-policy.mjs check-attestation",
    "release-policy.mjs check-secrets",
    "test:release-policy",
    "test:release-policy-plumbing",
    "rehearsal.mjs",
  ]) {
    assert.ok(scripts.includes(command), `the workflow never runs \`${command}\``);
  }
});

test("the resolution is pinned and reused, not recomputed from a moving main", () => {
  const document = workflow();
  assert.equal(document.jobs["resolve-target"].outputs.source_sha.includes("resolve"), true);
  const reserveCheckout = document.jobs.reserve.steps.find((step) =>
    step.uses?.startsWith("actions/checkout"),
  );
  assert.equal(reserveCheckout.with.ref.includes("needs.resolve-target.outputs.source_sha"), true);
  const stageCheckout = document.jobs["stage-policy"].steps.find((step) =>
    step.uses?.startsWith("actions/checkout"),
  );
  assert.equal(stageCheckout.with.ref.includes("needs.resolve-target.outputs.source_sha"), true);
});

test("the release environment holds no signing credential beyond reading a declared list", () => {
  // Only the resolution job knows about secrets, and only `secrets.GITHUB_TOKEN`,
  // which is the API token rather than a release credential.
  const declared = JSON.parse(
    readFileSync(path.join(ROOT, ".github", "release-environment-secrets.json"), "utf8"),
  );
  assert.deepEqual(declared, ["TAURI_SIGNING_PRIVATE_KEY", "TAURI_SIGNING_PRIVATE_KEY_PASSWORD"]);
  const text = readFileSync(WORKFLOW, "utf8");
  for (const forbidden of [
    "APPLE_CERTIFICATE",
    "APPLE_ID",
    "APPLE_TEAM_ID",
    "APPLE_PASSWORD",
    "NOTARY",
  ]) {
    assert.ok(!text.includes(forbidden), `the workflow references ${forbidden}`);
  }
});

test("no job claims write permission", () => {
  const document = workflow();
  assert.equal(document.permissions.contents, "read");
  for (const [name, job] of Object.entries(document.jobs)) {
    for (const [scope, level] of Object.entries(job.permissions ?? {})) {
      assert.equal(level, "read", `${name} requests ${scope}: ${level}`);
    }
  }
});

test("the workflow does not build, tag, push or publish anything", () => {
  const scripts = allRunScripts(workflow());
  for (const forbidden of [
    "git tag",
    "git push",
    "gh release create",
    "gh release edit",
    "docker push",
    "cargo build",
    "tauri build",
    "bunx tauri",
    "cosign",
    "gh attestation",
    "notarytool",
    "stapler",
  ]) {
    assert.ok(!scripts.includes(forbidden), `the workflow runs \`${forbidden}\``);
  }
});

test("the publish job exists only to fail closed", () => {
  const document = workflow();
  const publish = document.jobs.publish;
  assert.ok(publish, "the publish boundary must exist so it cannot be assumed to have run");
  const body = publish.steps.map((step) => step.run ?? "").join("\n");
  assert.match(body, /exit 1/, "publish must exit non-zero");
  assert.match(body, /not implemented/i);
});

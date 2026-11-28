// Contract sources. Every check reads the documents through here, so a negative case can point
// the same check at a copy without touching the repository.
import path from "node:path";

export const CONTRACTS_DIR = "docs/contracts";
const root = process.env.CONTRACTS_ROOT ?? ".";

export const OPENAPI = path.join(root, CONTRACTS_DIR, "openapi.yaml");
export const EVENTS_SCHEMA = path.join(root, CONTRACTS_DIR, "events.schema.json");
export const ERROR_CODES_DOC = path.join(root, "docs/ERROR-CODES.md");
export const DATABASE_SCHEMA_DOC = path.join(root, "docs/DATABASE-SCHEMA.md");
export const SPECS_DIR = path.join(root, "docs/specs");
export const SECCOMP_DIR = path.join(root, "deploy/docker/seccomp");
export const SCHEMA_FIXTURES_DIR = path.join(root, "fixtures/contracts");

export const MANIFEST_PATTERNS = [
  {
    dir: path.join(root, CONTRACTS_DIR, "manifests"),
    schema: path.join(root, CONTRACTS_DIR, "kernel-manifest.schema.json"),
  },
  {
    dir: path.join(root, CONTRACTS_DIR, "manifests"),
    schema: path.join(root, CONTRACTS_DIR, "proxy-engine-manifest.schema.json"),
  },
];

export function rootOf(relative) {
  return path.posix.dirname(relative);
}

/** Files this check set reads from `root`, so a negative case can be staged from copies. */
export function ownedPaths(root) {
  return root;
}

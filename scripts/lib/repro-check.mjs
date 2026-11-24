// Registry for the `repro:<id>` scripts of package.json.
//
// Every contract check lives in its own module and is registered by id, so a check can be run
// alone while developing and all of them run together in CI. A check is a function that receives
// the repository root and throws on failure; the registry turns the outcome into an exit code and
// a one-line summary.
import process from "node:process";

const checks = new Map();

export function check(id, fn) {
  if (checks.has(id)) throw new Error(`duplicate repro check id: ${id}`);
  checks.set(id, fn);
}

function explain(error) {
  const lines = String(error?.message ?? error)
    .split("\n")
    .filter(Boolean);
  if (lines.length <= 12) return lines.join("\n");
  return `${lines.slice(0, 12).join("\n")}\n… ${lines.length - 12} more line(s)`;
}

export function run(argv) {
  const requested = argv.filter((arg) => !arg.startsWith("-"));
  const root = process.cwd();
  const ids = requested.length > 0 ? requested : [...checks.keys()];
  const failures = [];

  for (const id of ids) {
    const fn = checks.get(id);
    if (!fn) {
      failures.push({ id, summary: "not registered" });
      continue;
    }
    const started = Date.now();
    try {
      fn(root);
      console.log(`PASS ${id} (${Date.now() - started} ms)`);
    } catch (error) {
      const summary = explain(error);
      failures.push({ id, summary });
      console.log(`FAIL ${id} (${Date.now() - started} ms)`);
      for (const line of summary.split("\n")) console.log(`  ${line}`);
    }
  }

  const total = ids.length;
  const failed = failures.length;
  const message = `contracts: ${total - failed}/${total} check(s) passed`;
  if (failed === 0) {
    console.log(message);
    return 0;
  }
  console.log(message);
  console.log(
    failed.length === 1
      ? `failed: ${failures[0].id}`
      : `failed: ${failures.map((f) => f.id).join(", ")}`,
  );
  return 1;
}

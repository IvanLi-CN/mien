#!/usr/bin/env node
// Turns GitHub API JSON into the facts `release-policy.mjs` judges.
//
// The policy makes every decision; this module only selects which API record is
// the one being described. Selecting the wrong record is itself a release bug, so
// the selectors are pure functions with tests rather than `jq` buried in YAML.
//
// Nothing here decides whether a release may proceed. A missing run becomes
// `null`, which the policy rejects; this module never fills a gap with a guess.
import { execFileSync } from "node:child_process";
import { writeFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

/** The `main` record the policy expects. */
export function buildMainRecord({
  headSha,
  pulls,
  ciMainRun,
  ciMainJobs,
  ciPrRun,
  ciPrJobs,
  dispatchRef,
}) {
  const pr = selectMergedPr(pulls ?? []);
  const required = pr === null ? null : selectJob(ciPrJobs ?? [], "required");
  const verify =
    ciMainRun === null || ciMainRun === undefined ? null : selectJob(ciMainJobs ?? [], "verify");
  return {
    dispatch_ref: dispatchRef,
    head_sha: headSha,
    merged_pr:
      pr === null
        ? null
        : {
            number: pr.number,
            merge_commit_sha: pr.merge_commit_sha,
            pr_required_conclusion: required?.conclusion ?? null,
            ci_pr_run_id: ciPrRun?.id ?? null,
          },
    ci_main:
      ciMainRun === null || ciMainRun === undefined
        ? null
        : {
            head_sha: ciMainRun.head_sha,
            conclusion: ciMainRun.conclusion,
            run_id: ciMainRun.id,
            job: verify?.name ?? null,
            job_conclusion: verify?.conclusion ?? null,
          },
  };
}

/**
 * The release source must come from a PR merged into `main`. A PR whose
 * `merge_commit_sha` is not the target SHA is not evidence for that SHA.
 */
export function selectMergedPr(pulls) {
  const merged = pulls.filter((pull) => pull?.merged_at && pull?.merge_commit_sha);
  return merged[0] ?? null;
}

const isSuccess = (run) => run?.conclusion === "success";

/**
 * The green `CI Main` run for one SHA: same head commit, `push` event on `main`,
 * successful. A workflow_dispatch or PR run does not qualify even on the same SHA.
 */
export function selectCiMainRun(runs, sha) {
  return (
    runs.find(
      (run) =>
        run?.head_sha === sha &&
        run?.event === "push" &&
        run?.head_branch === "main" &&
        isSuccess(run),
    ) ?? null
  );
}

/** The `CI PR` run for a pull request head SHA: `pull_request` event, successful. */
export function selectCiPrRun(runs, sha) {
  return (
    runs.find((run) => run?.head_sha === sha && run?.event === "pull_request" && isSuccess(run)) ??
    runs.find((run) => run?.head_sha === sha && run?.event === "merge_group" && isSuccess(run)) ??
    null
  );
}

/** A named job from `GET /repos/{owner}/{repo}/actions/runs/{id}/jobs`. */
export function selectJob(jobs, name) {
  return jobs.find((job) => job?.name === name) ?? null;
}

// ---------------------------------------------------------------------------
// CLI: only the `gh api` plumbing, no decisions
// ---------------------------------------------------------------------------

const USAGE = `usage:
  github-facts.mjs main --repo <owner/repo> --sha <sha> [--out <main.json>]

Reads the merged PR, the CI PR run and the CI Main run for one commit and writes
the facts document release-policy.mjs expects. Exit 0 with a document even when a
run is absent (the policy then rejects); 2 on usage or API failure.`;

function parseArgs(argv) {
  const options = {};
  const positional = [];
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (!arg.startsWith("--")) {
      positional.push(arg);
      continue;
    }
    const key = arg.slice(2);
    const next = argv[index + 1];
    if (next === undefined || next.startsWith("--")) {
      options[key] = true;
      continue;
    }
    options[key] = next;
    index += 1;
  }
  return { command: positional[0] ?? null, options };
}

/** `gh api` that returns parsed JSON; throws with the endpoint on failure. */
function ghApi(endpoint, paginate = false) {
  const args = ["api", "--method", "GET"];
  if (paginate) args.push("--paginate");
  args.push(endpoint);
  const stdout = execFileSync("gh", args, { encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
  // `--paginate` concatenates pages as separate JSON documents.
  const documents = [];
  let depth = 0;
  let start = 0;
  for (let index = 0; index < stdout.length; index += 1) {
    const char = stdout[index];
    if (char === "{") {
      if (depth === 0) start = index;
      depth += 1;
    } else if (char === "}") {
      depth -= 1;
      if (depth === 0) documents.push(JSON.parse(stdout.slice(start, index + 1)));
    }
  }
  return documents.length === 1 ? documents[0] : documents;
}

const workflowRuns = (repo, workflow, sha) =>
  ghApi(`repos/${repo}/actions/workflows/${workflow}/runs?head_sha=${sha}&per_page=100`)
    .workflow_runs ?? [];

const runJobs = (repo, runId) =>
  ghApi(`repos/${repo}/actions/runs/${runId}/jobs?per_page=100`).jobs ?? [];

function collectMain(repo, sha) {
  const pull = ghApi(`repos/${repo}/commits/${sha}/pulls`);
  const pulls = Array.isArray(pull) ? pull : (pull.items ?? []);
  const merged = selectMergedPr(pulls);
  const ciPrRun = merged
    ? selectCiPrRun(workflowRuns(repo, "ci-pr.yml", merged.head.sha ?? sha), merged.head.sha ?? sha)
    : null;
  const ciPrJobs = ciPrRun ? runJobs(repo, ciPrRun.id) : [];
  const ciMainRun = selectCiMainRun(workflowRuns(repo, "ci-main.yml", sha), sha);
  const ciMainJobs = ciMainRun ? runJobs(repo, ciMainRun.id) : [];
  return buildMainRecord({
    headSha: sha,
    pulls,
    ciMainRun,
    ciMainJobs,
    ciPrRun,
    ciPrJobs,
    dispatchRef: "refs/heads/main",
  });
}

export function main(argv = process.argv.slice(2)) {
  const { command, options } = parseArgs(argv);
  if (command !== "main" || typeof options.repo !== "string" || typeof options.sha !== "string") {
    console.error(USAGE);
    return 2;
  }
  try {
    const document = collectMain(options.repo, options.sha);
    const text = `${JSON.stringify(document, null, 2)}\n`;
    if (typeof options.out === "string") writeFileSync(path.resolve(options.out), text);
    console.log(text);
    return 0;
  } catch (error) {
    // An unreadable API is not "no runs": the workflow must stop, not proceed.
    console.error(`github-facts: ${error.message}`);
    return 2;
  }
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === path.resolve(fileURLToPath(import.meta.url))
) {
  process.exitCode = main();
}

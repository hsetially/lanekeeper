import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { parse } from "yaml";

// `.github/workflows/web-e2e.yml` has never run on GitHub when these tests are written, and a workflow that is wrong still
// "passes" locally. These tests read it as YAML and hold it to what the plan and the round-1 review say it must do:
//   - the P10 timing budgets are ENFORCED on every pull request (a perf job), not only measured on a developer's machine;
//   - the baselines of the visual tests can be made on the pull request itself (a label), with the least privilege that allows
//     and without weakening the rule that a missing baseline fails the run;
//   - no action is added that nobody has checked (S19), and nothing here reads a secret;
//   - the end-to-end run is split across a matrix of shards that each end inside their own timeout (the first run showed that one
//     job cannot run the whole suite in 40 minutes), and ONE job named `e2e` says if all of them passed, so that the check a
//     branch protection requires does not change with the number of shards;
//   - every step that can stall has a bound of its own (the first run's Lighthouse step hung for 38 minutes with nothing logged).
// What a text can not prove (that the jobs run, and how noisy the runner is) is listed in plans/08-frontend.blockers.md.
const root = new URL("../../../", import.meta.url);
const text = readFileSync(new URL(".github/workflows/web-e2e.yml", root), "utf8");
const ci = readFileSync(new URL(".github/workflows/ci.yml", root), "utf8");
const justfile = readFileSync(new URL("Justfile", root), "utf8");
const workflow = parse(text);
const jobs = workflow.jobs;

/** The `run` text of a job's steps, in order. */
const commands = (job) => job.steps.flatMap((step) => (typeof step.run === "string" ? [step.run] : []));
/** The step of a job whose name or first command contains `needle`. */
const stepWith = (job, needle) => job.steps.find((step) => `${step.name ?? ""}\n${step.run ?? ""}\n${step.uses ?? ""}`.includes(needle));
const indexOfStep = (job, needle) => job.steps.indexOf(stepWith(job, needle));

// The actions this workflow already used before the perf and label jobs were added, each pinned by commit SHA. A new action would
// have to be checked by a person (the sandbox that wrote the jobs cannot reach GitHub), so a new one fails here until this list
// is edited on purpose.
const KNOWN_ACTIONS = new Set([
  "actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683",
  "Swatinem/rust-cache@82a92a6e8fbeee089604da2575dc567ae9ddeaab",
  "taiki-e/install-action@9983c65e42da123ff25d1f78505eb6de315aa172",
  "pnpm/action-setup@a7487c7e89a18df4991f7f222e4898a00d66ddda",
  "actions/setup-node@39370e3970a6d050c480ffad4ff0ed4d3fdee5af",
  "actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02",
]);

describe("the workflow as a whole", () => {
  it("has the jobs: e2e-shard (the matrix), e2e (its verdict), perf and visual-baselines", () => {
    expect(Object.keys(jobs)).toEqual(["e2e-shard", "e2e", "perf", "visual-baselines"]);
  });

  it("uploads an artifact from the baseline job only (the other jobs keep their numbers in the log: a wrong pin there would fail a required check before any step ran)", () => {
    for (const [name, job] of Object.entries(jobs)) {
      const uploads = job.steps?.filter((step) => step.uses?.startsWith("actions/upload-artifact@")) ?? [];
      // The baseline job has two: the baselines, when the passes went well, and the results of the tests, when they did not.
      expect(uploads.length, name).toBe(name === "visual-baselines" ? 2 : 0);
    }
  });

  it("gives every job a timeout, and every step that runs a long command a timeout of its own that is shorter than the job's", () => {
    for (const [name, job] of Object.entries(jobs)) {
      expect(job["timeout-minutes"], `${name} has a timeout`).toBeGreaterThan(0);
      for (const step of job.steps ?? []) if (step["timeout-minutes"] !== undefined) expect(step["timeout-minutes"], `${name}: ${step.name ?? step.run}`).toBeLessThan(job["timeout-minutes"]);
    }
  });

  it("uses only actions that were already pinned here or in ci.yml, each by a full commit SHA (S19)", () => {
    const used = Object.values(jobs).flatMap((job) => job.steps.flatMap((step) => (step.uses ? [step.uses.split(" ")[0]] : [])));
    expect(used.length).toBeGreaterThan(10);
    for (const action of used) {
      expect(action, action).toMatch(/@[0-9a-f]{40}$/);
      expect(KNOWN_ACTIONS.has(action), `${action} is not one of the actions that were pinned before`).toBe(true);
    }
    // Where ci.yml pins the same action, it is the same commit.
    for (const action of KNOWN_ACTIONS) {
      const name = action.split("@")[0];
      const inCi = new RegExp(`${name}@([0-9a-f]{40})`).exec(ci);
      if (inCi) expect(action.endsWith(inCi[1]), `${name} differs from ci.yml`).toBe(true);
    }
  });

  it("reads no secret and never runs on pull_request_target (a fork's code with a token)", () => {
    expect(text).not.toMatch(/secrets\./);
    expect(text).not.toMatch(/pull_request_target/);
  });

  it("is read-only by default: the workflow asks for contents: read and nothing else", () => {
    expect(workflow.permissions).toEqual({ contents: "read" });
  });

  it("runs on pull requests (also when they get a label), on main, and by hand", () => {
    expect(Object.keys(workflow.on).sort()).toEqual(["pull_request", "push", "workflow_dispatch"]);
    expect(workflow.on.pull_request.types).toEqual(["opened", "synchronize", "reopened", "labeled"]);
    expect(workflow.on.pull_request.paths).toEqual(expect.arrayContaining(["web/**", "crates/hub/**", "api/openapi.yaml", "Justfile", ".github/workflows/web-e2e.yml"]));
  });

  it("gives a run that a label started a group of its own, which never cancels a run of the tests and is never cancelled by another label", () => {
    const { group, "cancel-in-progress": cancel } = workflow.concurrency;
    expect(group).toContain("github.ref");
    expect(group).toMatch(/github\.event\.action == 'labeled'/);
    expect(group).toContain("github.event.label.name");
    // A run in the middle of a push is not cancelled by a label, and a bootstrap that is committing is not cancelled at all.
    expect(String(cancel)).toMatch(/github\.event\.action != 'labeled'/);
  });

  it("states in its header what the perf job enforces, that shared runners are noisier, and no longer says that the gates come later", () => {
    const header = text.slice(0, text.indexOf("\nname:"));
    expect(header).not.toMatch(/Later tasks add jobs/);
    expect(header).toMatch(/P10/);
    expect(header).toMatch(/noisier|noisy/);
    expect(header).toMatch(/bootstrap-baselines/);
  });
});

describe("the perf job (P10 is enforced in CI)", () => {
  const perf = jobs.perf;

  it("runs on every pull request and every push to main, and not on a label or by hand", () => {
    expect(perf.if).toBe(jobs["e2e-shard"].if);
    expect(perf.if).toContain("github.event_name != 'workflow_dispatch'");
    expect(perf.if).toContain("github.event.action != 'labeled'");
  });

  it("has a timeout and a runner of its own (timings must not share a machine with the other tests)", () => {
    expect(perf["runs-on"]).toBe("ubuntu-latest");
    expect(perf["timeout-minutes"]).toBeGreaterThanOrEqual(20);
    expect(perf["timeout-minutes"]).toBeLessThanOrEqual(45);
    expect(perf.needs).toBeUndefined();
    expect(jobs["e2e-shard"].needs).toBeUndefined();
  });

  it("bounds the measuring step and the judging step on their own, so that a stall ends in minutes with the name of the step and not at the end of the job", () => {
    // The first run: the specs passed at minute 7, then nothing was logged until the job was cancelled at minute 45.
    const measure = stepWith(perf, "just web-perf");
    const judge = stepWith(perf, "just web-bench-check");
    expect(measure["timeout-minutes"]).toBeGreaterThanOrEqual(25);
    expect(measure["timeout-minutes"]).toBeLessThanOrEqual(35);
    expect(judge["timeout-minutes"]).toBeGreaterThanOrEqual(5);
    expect(judge["timeout-minutes"]).toBeLessThanOrEqual(15);
    expect(measure["timeout-minutes"] + judge["timeout-minutes"]).toBeLessThan(perf["timeout-minutes"]);
  });

  it("measures with `just web-perf` and then judges the five P10 budgets with `just web-bench-check`, in that order, as steps of their own", () => {
    const measure = indexOfStep(perf, "just web-perf");
    const judge = indexOfStep(perf, "just web-bench-check");
    expect(measure).toBeGreaterThan(0);
    expect(judge).toBeGreaterThan(measure);
    expect(perf.steps[measure].run.trim()).toBe("just web-perf");
    expect(perf.steps[judge].run.trim()).toBe("just web-bench-check");
  });

  it("calls only recipes that the Justfile has (their lists of budgets and specs are tested in scripts/verify08.test.mjs)", () => {
    for (const recipe of ["web-install", "web-perf", "web-bench-check"]) expect(justfile, recipe).toMatch(new RegExp(`^${recipe}[ :]`, "m"));
    expect(justfile).toMatch(/playwright test --project=perf --workers=1/);
  });

  it("installs what the recipes need: just, pnpm, node, the web dependencies and Playwright's Chromium", () => {
    expect(stepWith(perf, "taiki-e/install-action").with.tool).toBe("just");
    expect(stepWith(perf, "pnpm/action-setup")).toBeDefined();
    expect(stepWith(perf, "actions/setup-node")).toBeDefined();
    expect(indexOfStep(perf, "just web-install")).toBeGreaterThan(0);
    expect(commands(perf).join("\n")).toMatch(/playwright install --with-deps chromium/);
    expect(indexOfStep(perf, "playwright install")).toBeLessThan(indexOfStep(perf, "just web-perf"));
    expect(indexOfStep(perf, "just web-install")).toBeLessThan(indexOfStep(perf, "just web-perf"));
  });

  it("hides no failure: no continue-on-error, no `|| true`, no retry wrapper, no step-level `if` on the two gates", () => {
    const serialised = JSON.stringify(perf);
    expect(serialised).not.toMatch(/continue-on-error/);
    expect(serialised).not.toMatch(/\|\| ?true/);
    expect(serialised).not.toMatch(/retry/i);
    // A timeout is not a retry: it ends the step, and the job fails.
    expect(serialised).not.toMatch(/\bcontinue-on-error\b/);
    expect(stepWith(perf, "just web-perf").if).toBeUndefined();
    expect(stepWith(perf, "just web-bench-check").if).toBeUndefined();
  });

  it("asks for no permission of its own", () => {
    expect(perf.permissions).toBeUndefined();
  });
});

describe("the e2e shards (the whole suite does not fit in one job)", () => {
  const shard = jobs["e2e-shard"];
  const total = shard.strategy.matrix.shard.length;

  it("run on the same events as perf, not for a label or by hand, and ask for no permission of their own", () => {
    expect(shard.if).toContain("github.event_name != 'workflow_dispatch'");
    expect(shard.if).toContain("github.event.action != 'labeled'");
    expect(shard.permissions).toBeUndefined();
    expect(shard["runs-on"]).toBe("ubuntu-latest");
    expect(shard.needs).toBeUndefined();
  });

  it("are a matrix of Playwright shards 1/N to N/N of one N, and a shard that fails does not cancel the others (the failures of all of them are wanted in one run)", () => {
    expect(total).toBeGreaterThanOrEqual(3);
    expect(total).toBeLessThanOrEqual(6);
    expect(shard.strategy.matrix.shard).toEqual(Array.from({ length: total }, (_, i) => `${i + 1}/${total}`));
    expect(shard.strategy["fail-fast"]).toBe(false);
    expect(Object.keys(shard.strategy.matrix)).toEqual(["shard"]);
  });

  it("hand the shard to the recipe through LK_E2E_SHARD, and run the same recipe as before (`just web-e2e`)", () => {
    expect(shard.env).toEqual({ LK_E2E_SHARD: "${{ matrix.shard }}" });
    const run = stepWith(shard, "just web-e2e");
    expect(run.run.trim()).toBe("just web-e2e");
    // The recipe takes the shard from the environment; a second --shard on the command line would run the wrong tests.
    expect(commands(shard).join("\n")).not.toContain("--shard");
    expect(justfile).toMatch(/playwright test --project=chromium \$\{LK_E2E_SHARD:\+--shard=\$LK_E2E_SHARD\}/);
  });

  it("each have a timeout with room for the set-up and for the longest shard, and a bound on the tests that is shorter than the job's", () => {
    // Estimated from the cost of a test in each directory (plans/08-frontend.blockers.md, FIX round 1 follow-up: a11y 1.52 s,
    // layout 1.22 s, visual 1.54 s without baselines, the rest less) and the real cut that Playwright makes for each N
    // (`playwright test --list --shard=i/N`): the longest shard of the cut, in minutes of two workers. The first run of CI did
    // 1,706 tests in 34 minutes, which is what these costs give for the same tests. A shard of a number that is not here has not
    // been measured: add it with its numbers.
    const longest = { 3: 28, 4: 24, 5: 19.5, 6: 16 };
    const setup = 6;
    expect(longest[total], `${total} shards have not been measured`).toBeDefined();
    const needed = Math.ceil(longest[total] * 1.5) + setup;
    expect(shard["timeout-minutes"], `${total} shards, the longest ${longest[total]} minutes of tests`).toBeGreaterThanOrEqual(needed);
    expect(shard["timeout-minutes"]).toBeLessThanOrEqual(60);
    const tests = stepWith(shard, "just web-e2e");
    expect(tests["timeout-minutes"]).toBeLessThan(shard["timeout-minutes"]);
    expect(tests["timeout-minutes"]).toBeGreaterThanOrEqual(Math.ceil(longest[total] * 1.5));
  });

  it("each build the Rust fixture (rust-cache before the recipe, which builds it) and get React 18 before the tests, like the single job did", () => {
    expect(indexOfStep(shard, "Swatinem/rust-cache")).toBeGreaterThanOrEqual(0);
    expect(indexOfStep(shard, "Swatinem/rust-cache")).toBeLessThan(indexOfStep(shard, "just web-e2e"));
    expect(indexOfStep(shard, "React 18.3.1")).toBeGreaterThan(indexOfStep(shard, "playwright install"));
    expect(indexOfStep(shard, "React 18.3.1")).toBeLessThan(indexOfStep(shard, "just web-e2e"));
    expect(indexOfStep(shard, "playwright install")).toBeLessThan(indexOfStep(shard, "just web-e2e"));
    expect(indexOfStep(shard, "just web-install")).toBeLessThan(indexOfStep(shard, "just web-e2e"));
    expect(indexOfStep(shard, "just web-gen-check")).toBeLessThan(indexOfStep(shard, "just web-e2e"));
    expect(stepWith(shard, "taiki-e/install-action").with.tool).toBe("just");
    expect(commands(shard).join("\n")).toMatch(/playwright install --with-deps chromium/);
  });

  it("never make a baseline: a missing one fails the run (baseline-rule.mjs), in every shard", () => {
    expect(JSON.stringify(shard)).not.toContain("LK_UPDATE_BASELINES");
    expect(JSON.stringify(jobs.perf)).not.toContain("LK_UPDATE_BASELINES");
    expect(JSON.stringify(jobs.e2e)).not.toContain("LK_UPDATE_BASELINES");
  });

  it("hide no failure", () => {
    expect(JSON.stringify(shard)).not.toMatch(/continue-on-error|\|\| ?true|retry/i);
  });
});

describe("the e2e job (the one check that a branch protection requires)", () => {
  const verdict = jobs.e2e;

  it("is named e2e, waits for every shard, and runs when they failed (it is how a failure becomes the check) but not when the run was cancelled", () => {
    expect(verdict.name ?? "e2e").toBe("e2e");
    expect(verdict.needs).toEqual(["e2e-shard"]);
    expect(verdict.if).toContain("!cancelled()");
    expect(verdict.if).toContain("github.event_name != 'workflow_dispatch'");
    expect(verdict.if).toContain("github.event.action != 'labeled'");
    expect(verdict.permissions).toBeUndefined();
  });

  it("fails unless the result of the matrix is success: a failed, a cancelled and a skipped shard all fail it", () => {
    expect(verdict.steps).toHaveLength(1);
    const step = verdict.steps[0];
    expect(step.uses).toBeUndefined();
    // The result goes through the environment, never into the text of the script.
    expect(step.env).toEqual({ SHARDS: "${{ needs.e2e-shard.result }}" });
    expect(step.run).not.toContain("${{");
    for (const result of ["success", "failure", "cancelled", "skipped"]) {
      const code = spawnSync("sh", ["-c", step.run], { env: { ...process.env, SHARDS: result } }).status;
      expect(code, result).toBe(result === "success" ? 0 : 1);
    }
  });

  it("is small and quick (it checks out nothing and installs nothing)", () => {
    expect(verdict["timeout-minutes"]).toBeLessThanOrEqual(5);
    expect(verdict["runs-on"]).toBe("ubuntu-latest");
  });

  it("is explained in the header: which checks a branch protection requires, and why", () => {
    const header = text.slice(0, text.indexOf("\nname:"));
    expect(header).toMatch(/required check/i);
    expect(header).toMatch(/`e2e`/);
    expect(header).toMatch(/`perf`/);
    expect(header).toMatch(/e2e-shard/);
  });
});

describe("the baseline bootstrap on the pull request (label bootstrap-baselines)", () => {
  const job = jobs["visual-baselines"];
  const commit = stepWith(job, "Commit the baselines");

  it("runs by hand and when the label is added to a pull request of this repository, and for nothing else", () => {
    expect(job.if).toContain("github.event_name == 'workflow_dispatch'");
    expect(job.if).toContain("github.event_name == 'pull_request'");
    expect(job.if).toContain("github.event.action == 'labeled'");
    expect(job.if).toContain("github.event.label.name == 'bootstrap-baselines'");
    // A fork's pull request never gets it: its code must not run with a write token.
    expect(job.if).toContain("github.event.pull_request.head.repo.full_name == github.repository");
  });

  it("has a timeout that fits its two passes and the set-up with a margin (24 and 14 minutes measured, 6 of set-up), and a bound on each pass", () => {
    // The two passes are one job on purpose: the second compares the PNGs that the first made, on the same machine.
    expect(job["timeout-minutes"]).toBeGreaterThanOrEqual(Math.ceil((24 + 14 + 6) * 1.5));
    expect(job["timeout-minutes"]).toBeLessThanOrEqual(120);
    const passes = job.steps.filter((step) => typeof step.run === "string" && step.run.includes("playwright test --project=chromium e2e/visual"));
    expect(passes[0]["timeout-minutes"]).toBeGreaterThanOrEqual(Math.ceil(24 * 1.5));
    expect(passes[1]["timeout-minutes"]).toBeGreaterThanOrEqual(Math.ceil(14 * 1.5));
    for (const pass of passes) expect(pass["timeout-minutes"]).toBeLessThan(job["timeout-minutes"]);
  });

  it("holds the only write permission of the workflow, contents: write and nothing else", () => {
    expect(job.permissions).toEqual({ contents: "write" });
    for (const [name, other] of Object.entries(jobs)) if (name !== "visual-baselines") expect(other.permissions, name).toBeUndefined();
    expect(JSON.stringify(workflow.permissions)).not.toContain("write");
  });

  it("has a group per pull request that never cancels a job that is committing", () => {
    expect(job.concurrency["cancel-in-progress"]).toBe(false);
    expect(job.concurrency.group).toContain("github.event.pull_request.number");
  });

  it("checks out the head commit of the pull request without leaving the token on disk, so that the code it runs cannot read it", () => {
    const checkout = job.steps.find((step) => step.uses?.startsWith("actions/checkout@"));
    expect(checkout.with["persist-credentials"]).toBe(false);
    expect(checkout.with.ref).toContain("github.event.pull_request.head.sha");
    // The dispatch run has no pull request and checks out its own ref.
    expect(checkout.with.ref).toContain("github.sha");
  });

  it("makes the baselines as the by-hand job always did: the same two passes, the first one allowed to fail, the second one not", () => {
    const passes = job.steps.filter((step) => typeof step.run === "string" && step.run.includes("playwright test --project=chromium e2e/visual"));
    expect(passes).toHaveLength(2);
    expect(passes[0].env).toEqual({ LK_UPDATE_BASELINES: "1" });
    expect(passes[0]["continue-on-error"]).toBe(true);
    expect(passes[1].env).toBeUndefined();
    expect(passes[1]["continue-on-error"]).toBeUndefined();
  });

  it("commits only after the second pass has passed on its own output, and only on the pull request", () => {
    expect(commit).toBeDefined();
    expect(commit.if).toBe("github.event_name == 'pull_request'");
    expect(commit["continue-on-error"]).toBeUndefined();
    const second = job.steps.findLastIndex((step) => typeof step.run === "string" && step.run.includes("playwright test --project=chromium e2e/visual"));
    expect(job.steps.indexOf(commit)).toBeGreaterThan(second);
    expect(job.steps.indexOf(commit)).toBe(job.steps.length - 1);
  });

  it("gives the token to the step that pushes and to no other, and masks the header it builds", () => {
    const withToken = job.steps.filter((step) => JSON.stringify(step).includes("github.token") || JSON.stringify(step).includes("GITHUB_TOKEN"));
    expect(withToken).toEqual([commit]);
    expect(commit.env.GITHUB_TOKEN).toBe("${{ github.token }}");
    expect(commit.run).toContain("::add-mask::");
    expect(commit.run).not.toMatch(/set -x|xtrace/);
  });

  it("takes the branch name and the commit from the environment, never from the text of the script (script injection)", () => {
    expect(commit.run).not.toContain("${{");
    expect(commit.env.HEAD_REF).toBe("${{ github.event.pull_request.head.ref }}");
    expect(commit.env.HEAD_SHA).toBe("${{ github.event.pull_request.head.sha }}");
  });

  it("stages regular .png files of the baselines folder and nothing else, and fails if anything else is staged", () => {
    expect(commit.run).toMatch(/find "\$base" -type f -name '\*\.png'/);
    expect(commit.run).toMatch(/git add -f --/);
    expect(commit.run).toMatch(/git diff --cached --name-only \| grep -v -E '\^web\/e2e\/visual\/__baselines__\/\.\+\\\.png\$'/);
    expect(commit.run).not.toMatch(/git add (-A|--all|\.)(\s|$)/);
    expect(commit.run).not.toMatch(/commit (-a|--all)/);
  });

  it("pushes the new commit onto the pull request's branch as a fast-forward: no force, no other branch, no hooks", () => {
    expect(commit.run).toMatch(/push "https:\/\/github\.com\/\$\{GITHUB_REPOSITORY\}\.git" "HEAD:refs\/heads\/\$\{HEAD_REF\}"/);
    expect(commit.run).not.toMatch(/--force|--force-with-lease|--delete|push [^\n]*\+|push [^\n]* -f\b/);
    expect(commit.run).toContain("core.hooksPath=/dev/null");
  });

  it("commits as the Actions bot with a message that carries the attribution lines", () => {
    expect(commit.run).toContain('user.name="github-actions[bot]"');
    expect(commit.run).toContain("Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>");
    expect(commit.run).toContain("Claude-Session: https://claude.ai/code/session_01Mgp6qLACxRR9AFYLjr8okC");
  });

  it("tells the maintainer that the push starts no workflow, and what to do", () => {
    const header = text.slice(0, text.indexOf("\nname:"));
    expect(header).toMatch(/GITHUB_TOKEN/);
    expect(header).toMatch(/does not start|will not start|starts no/);
    expect(header).toMatch(/close and reopen|push (a|any) commit/);
    expect(commit.run).toContain("GITHUB_STEP_SUMMARY");
  });

  it("still uploads the baselines as an artifact, for review", () => {
    const upload = job.steps.find((step) => step.uses?.startsWith("actions/upload-artifact@"));
    expect(upload.with.name).toBe("visual-baselines");
    expect(upload.with.path).toBe("web/e2e/visual/__baselines__");
    expect(upload.with["if-no-files-found"]).toBe("error");
    expect(upload.if).toBeUndefined();
    expect(job.steps.indexOf(upload)).toBeLessThan(job.steps.indexOf(commit));
  });
});

// The first run of the bootstrap failed its second pass, and because the job stopped there nothing it had measured was kept: no
// picture of the design, none of the app, none of the difference. The failed run now leaves them (the tests attach them when a
// comparison is over its limit) in an artifact. Only this job: the shards and `perf` are the gate that a branch protection requires,
// and a step they do not need is one more thing that can fail them.
describe("the bootstrap job keeps the pictures of a failed run", () => {
  const job = jobs["visual-baselines"];
  const uploads = job.steps.filter((step) => step.uses?.startsWith("actions/upload-artifact@"));
  const baselines = uploads.find((step) => step.with.name === "visual-baselines");
  const failed = uploads.find((step) => step.with.name !== "visual-baselines");
  const secondPass = job.steps.findLastIndex((step) => typeof step.run === "string" && step.run.includes("playwright test --project=chromium e2e/visual"));
  const commit = stepWith(job, "Commit the baselines");

  it("has a step of its own for it, that runs only when an earlier step failed", () => {
    expect(failed).toBeDefined();
    expect(failed.if).toBe("failure()");
  });

  it("uses the same action, by the same commit SHA, as the upload that already ran on GitHub (a new pin would be unchecked)", () => {
    expect(failed.uses).toBe(baselines.uses);
    expect(failed.uses).toMatch(/^actions\/upload-artifact@[0-9a-f]{40}$/);
  });

  it("uploads Playwright's results folder, which holds the reference, the app and the difference of every comparison that failed", () => {
    expect(failed.with.path).toBe("web/test-results");
    // The folder is the one the config writes to, and the specs attach the three pictures when a comparison is over its limit.
    const config = readFileSync(new URL("web/playwright.config.ts", root), "utf8");
    expect(config).toMatch(/outputDir: "test-results"/);
    const attaches = { "web/e2e/visual/access/lib/matrix.ts": ["reference", "app", "diff"], "web/e2e/visual/search/kit.ts": ["reference", "app", "diff"], "web/e2e/visual/attribution/elements.ts": ["design", "app", "diff"] };
    for (const [file, names] of Object.entries(attaches)) {
      const source = readFileSync(new URL(file, root), "utf8");
      for (const name of names) expect(source, `${file} attaches ${name}`).toContain(`attach("${name}"`);
    }
  });

  it("keeps it for 7 days, says nothing when there is nothing to keep, and names the artifact after the attempt (a re-run does not collide with the first)", () => {
    expect(failed.with["retention-days"]).toBe(7);
    expect(failed.with["if-no-files-found"]).toBe("ignore");
    expect(failed.with.name).toContain("${{ github.run_attempt }}");
    expect(failed.with.name).not.toBe("visual-baselines");
  });

  it("reads no secret and uploads nothing but that folder: no token, no credentials, no .git, no wildcard that reaches outside it", () => {
    expect(JSON.stringify(failed)).not.toMatch(/secrets\.|github\.token|GITHUB_TOKEN|\.git\b|\.\./);
    expect(failed.with.path.split("\n").map((line) => line.trim()).filter(Boolean)).toEqual(["web/test-results"]);
    expect(failed.env).toBeUndefined();
  });

  it("comes after the second pass and the baselines upload and before the commit, which stays the last step, and hides no failure", () => {
    expect(job.steps.indexOf(failed)).toBeGreaterThan(secondPass);
    expect(job.steps.indexOf(failed)).toBeGreaterThan(job.steps.indexOf(baselines));
    expect(job.steps.indexOf(failed)).toBeLessThan(job.steps.indexOf(commit));
    expect(job.steps.indexOf(commit)).toBe(job.steps.length - 1);
    expect(failed["continue-on-error"]).toBeUndefined();
    // The step that uploads the baselines does not run after a failure: there is nothing to commit then.
    expect(baselines.if).toBeUndefined();
  });

  it("has a bound of its own, shorter than the job's", () => {
    expect(failed["timeout-minutes"]).toBeGreaterThan(0);
    expect(failed["timeout-minutes"]).toBeLessThan(job["timeout-minutes"]);
  });

  it("is explained in the header (what it keeps, for how long, and that only this job has it)", () => {
    const header = text.slice(0, text.indexOf("\nname:"));
    expect(header).toMatch(/test-results/);
    expect(header).toMatch(/failed run|a run that failed|fails/);
    expect(header).toMatch(/7 days/);
  });
});

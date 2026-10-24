import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { parse } from "yaml";

// `.github/workflows/web-e2e.yml` has never run on GitHub when these tests are written, and a workflow that is wrong still
// "passes" locally. These tests read it as YAML and hold it to what the plan and the round-1 review say it must do:
//   - the P10 timing budgets are ENFORCED on every pull request (a perf job), not only measured on a developer's machine;
//   - the baselines of the visual tests can be made on the pull request itself (a label), with the least privilege that allows
//     and without weakening the rule that a missing baseline fails the run;
//   - no action is added that nobody has checked (S19), and nothing here reads a secret.
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
  it("has the jobs: e2e, perf and visual-baselines", () => {
    expect(Object.keys(jobs)).toEqual(["e2e", "perf", "visual-baselines"]);
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
    expect(perf.if).toBe(jobs.e2e.if);
    expect(perf.if).toContain("github.event_name != 'workflow_dispatch'");
    expect(perf.if).toContain("github.event.action != 'labeled'");
  });

  it("has a timeout and a runner of its own (timings must not share a machine with the other tests)", () => {
    expect(perf["runs-on"]).toBe("ubuntu-latest");
    expect(perf["timeout-minutes"]).toBeGreaterThanOrEqual(20);
    expect(perf["timeout-minutes"]).toBeLessThanOrEqual(90);
    expect(perf.needs).toBeUndefined();
    expect(jobs.e2e.needs).toBeUndefined();
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
    expect(stepWith(perf, "just web-perf").if).toBeUndefined();
    expect(stepWith(perf, "just web-bench-check").if).toBeUndefined();
  });

  it("asks for no permission of its own", () => {
    expect(perf.permissions).toBeUndefined();
  });
});

describe("the e2e job", () => {
  it("does not run for a label, and asks for no permission of its own", () => {
    expect(jobs.e2e.if).toContain("github.event.action != 'labeled'");
    expect(jobs.e2e.permissions).toBeUndefined();
  });

  it("never makes a baseline: a missing one fails the run (baseline-rule.mjs)", () => {
    expect(JSON.stringify(jobs.e2e)).not.toContain("LK_UPDATE_BASELINES");
    expect(JSON.stringify(jobs.perf)).not.toContain("LK_UPDATE_BASELINES");
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
    expect(job.steps.indexOf(upload)).toBeLessThan(job.steps.indexOf(commit));
  });
});

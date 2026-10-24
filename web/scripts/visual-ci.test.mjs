import { readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { parse } from "yaml";
import { baselineIsNotRun } from "../e2e/visual/baseline-rule.mjs";

// The visual tests compare the app with the design as it is RENDERED, by the design's own runtime, which needs React 18.3.1 (the two
// UMD files it asks unpkg for). A sandbox that cannot reach unpkg reads them from a directory (LK_REACT18_DIR) and says NOT RUN when
// it has none; CI must never say that: the tier-1 comparisons run there or the job fails. These tests read the sources so that
// the rule cannot be lost: every visual spec that skips for a missing reference throws in CI instead, and the workflow supplies
// the files (from the npm registry, against pinned integrity values) and the variable.
const web = new URL("../", import.meta.url);
const root = new URL("../../", import.meta.url);

function sources(dir, out = []) {
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) {
      if (name === "__baselines__" || name === ".refs") continue;
      sources(path, out);
    } else if (/\.(ts|mjs)$/.test(name) && !/\.test\./.test(name)) out.push(path);
  }
  return out;
}

const specs = sources(new URL("e2e/visual/", web).pathname);

describe("the visual specs fail closed in CI", () => {
  it("finds the visual specs", () => {
    expect(specs.length).toBeGreaterThan(10);
  });

  it("every file that skips as NOT RUN also throws when CI is set", () => {
    const skipping = specs.filter((file) => /NOT RUN/.test(readFileSync(file, "utf8")) && /test\.skip\(|\.skip\(/.test(readFileSync(file, "utf8")));
    expect(skipping.length, "there are specs that can be NOT RUN").toBeGreaterThan(0);
    for (const file of skipping) {
      const text = readFileSync(file, "utf8");
      expect(/process\.env\["CI"\]|IN_CI/.test(text), `${file} skips as NOT RUN but has no CI branch that throws`).toBe(true);
      expect(/throw new Error\(/.test(text), `${file} has no throw`).toBe(true);
    }
  });

  it("the chrome spec (the app shell against the Shell frames) is one of them", () => {
    const chrome = readFileSync(new URL("e2e/visual/chrome.spec.ts", web), "utf8");
    expect(chrome).toMatch(/process\.env\["CI"\]/);
    expect(chrome).toMatch(/throw new Error\(`[^`]*cannot be rendered in CI/);
  });
});

describe("the web-e2e workflow supplies the references", () => {
  const workflow = readFileSync(new URL(".github/workflows/web-e2e.yml", root), "utf8");

  it("fetches React 18.3.1 and ReactDOM 18.3.1 from the npm registry and checks the tarballs against pinned integrity values", () => {
    expect(workflow).toMatch(/npm pack --silent react@18\.3\.1 react-dom@18\.3\.1/);
    expect(workflow).toContain("sha512-wS+hAgJShR0KhEvPJArfuPVN1+Hz1t0Y6n5jLrGQbkb4urgPE/0Rve+1kMB1v/oWgHgm4WIcV+i7F2pTVj+2iQ==");
    expect(workflow).toContain("sha512-5m4nQKp+rZRb09LNH59GM4BxTh9251/ylbKIbpe7TpGxfJ+9kv6BLkLBXIjjspbgbnIBNqlI23tRnTWT0snUIw==");
  });

  it("sets LK_REACT18_DIR for the steps after it, to the directory with the two files", () => {
    expect(workflow).toMatch(/LK_REACT18_DIR=\$dir\/umd" >> "\$GITHUB_ENV"/);
    expect(workflow).toContain("react.production.min.js");
    expect(workflow).toContain("react-dom.production.min.js");
  });

  it("puts that step before the end-to-end run in every shard (the tests that need the references are in every one of them)", () => {
    const shard = parse(workflow).jobs["e2e-shard"];
    const at = (needle) => shard.steps.findIndex((step) => `${step.name ?? ""}\n${step.run ?? ""}`.includes(needle));
    expect(at("LK_REACT18_DIR")).toBeGreaterThanOrEqual(0);
    expect(at("LK_REACT18_DIR")).toBeLessThan(at("just web-e2e"));
    expect(at("just web-e2e")).toBeGreaterThan(0);
  });

  it("is the same step, word for word, in the shards and in the job that makes the baselines (one copy that drifts is a comparison against the wrong file)", () => {
    const jobs = parse(workflow).jobs;
    const react = (job) => job.steps.find((step) => typeof step.run === "string" && step.run.includes("npm pack --silent react@18.3.1"));
    expect(react(jobs["e2e-shard"])).toBeDefined();
    expect(react(jobs["e2e-shard"]).run).toBe(react(jobs["visual-baselines"]).run);
  });

  it("does not add React 18 to the dependencies of the app (dev-only, never bundled, W30)", () => {
    const pkg = JSON.parse(readFileSync(new URL("package.json", web), "utf8"));
    const all = { ...pkg.dependencies, ...pkg.devDependencies };
    expect(Object.keys(all).filter((name) => /^react(18|-dom18)/.test(name))).toEqual([]);
    expect(all["react"]).toMatch(/^\^?19\./);
  });
});

describe("a baseline that is missing (the app against its own picture)", () => {
  it("is NOT RUN in a sandbox, a failure in CI, and made only when someone asks", () => {
    // Nothing asked for: Playwright creates nothing ("none"), and a sandbox says NOT RUN.
    expect(baselineIsNotRun({ exists: false, ci: false, updateSnapshots: "none" })).toBe(true);
    // CI: not skipped, so the comparison runs against nothing and fails (Playwright does not create it either).
    expect(baselineIsNotRun({ exists: false, ci: true, updateSnapshots: "none" })).toBe(false);
    // The bootstrap job, or a person refreshing the baselines: the comparison runs, and writes.
    expect(baselineIsNotRun({ exists: false, ci: false, updateSnapshots: "missing" })).toBe(false);
    expect(baselineIsNotRun({ exists: false, ci: true, updateSnapshots: "missing" })).toBe(false);
    expect(baselineIsNotRun({ exists: false, ci: false, updateSnapshots: "all" })).toBe(false);
    // A baseline that is there is always compared.
    expect(baselineIsNotRun({ exists: true, ci: false, updateSnapshots: "none" })).toBe(false);
  });

  it("is made by LK_UPDATE_BASELINES and by nothing else: the config says none unless it is set", () => {
    const config = readFileSync(new URL("playwright.config.ts", web), "utf8");
    expect(config).toMatch(/updateSnapshots: process\.env\["LK_UPDATE_BASELINES"\] \? "missing" : "none"/);
    expect(config).toContain('snapshotPathTemplate: "{testDir}/visual/__baselines__/{testFilePath}/{arg}{ext}"');
  });

  it("is the one rule of the specs that take a baseline (V1 and V2 both use it)", () => {
    for (const file of ["e2e/visual/access/lib/matrix.ts", "e2e/visual/search/kit.ts"]) expect(readFileSync(new URL(file, web), "utf8"), file).toContain("baselineIsNotRun");
  });

  it("is made in the workflow's bootstrap job, with the variable, and compared there in a second pass", () => {
    const workflow = readFileSync(new URL(".github/workflows/web-e2e.yml", root), "utf8");
    const job = workflow.slice(workflow.indexOf("  visual-baselines:"));
    expect(job).toContain('LK_UPDATE_BASELINES: "1"');
    expect(job.match(/playwright test --project=chromium e2e\/visual/g)?.length).toBe(2);
    // The e2e job, which runs on every pull request, never sets it: a missing baseline there is a failure.
    expect(workflow.slice(0, workflow.indexOf("  visual-baselines:"))).not.toContain("LK_UPDATE_BASELINES: ");
  });

  it("cannot reach a commit: the folder, Playwright's own snapshot folders and private builds are ignored", () => {
    const ignore = readFileSync(new URL(".gitignore", web), "utf8");
    for (const line of ["e2e/visual/__baselines__/", "e2e/**/*-snapshots/", "/dist-*/", "test-results/", "playwright-report/", "e2e/visual/.refs/", "dist-perf/"]) expect(ignore, line).toContain(line);
  });
});

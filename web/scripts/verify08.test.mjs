import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// `just verify-08` is the gate of prompt 08. This test reads the Justfile so the gate cannot go back to being a stub, lose a
// step, or drift from the budgets that perf/budgets.toml registers for the web app. (The recipes themselves are run by hand
// in the evidence of the task: this machine has no `just`, CI does.)
const root = new URL("../../", import.meta.url);
const justfile = readFileSync(new URL("Justfile", root), "utf8");
const budgets = readFileSync(new URL("perf/budgets.toml", root), "utf8");
const pkg = JSON.parse(readFileSync(new URL("web/package.json", root), "utf8"));

/** The header and the body lines of a recipe. */
function recipe(name) {
  const lines = justfile.split("\n");
  const at = lines.findIndex((l) => new RegExp(`^${name}(\\s|:)`).test(l) && l.includes(":") && !l.includes(":="));
  if (at < 0) return undefined;
  const body = [];
  for (let i = at + 1; i < lines.length && /^(\s+\S|\s*$)/.test(lines[i]); i++) body.push(lines[i]);
  return { header: lines[at], body: body.join("\n") };
}

const recipeNames = new Set(justfile.split("\n").flatMap((l) => (/^([a-z][a-z0-9-]*)(?:\s[^:]*)?:(?!=)/.exec(l) ? [/^([a-z][a-z0-9-]*)/.exec(l)[1]] : [])));

describe("verify-08", () => {
  const verify = recipe("verify-08");

  it("is a real gate, not the placeholder", () => {
    expect(verify).toBeDefined();
    expect(`${verify.header}\n${verify.body}`).not.toMatch(/not implemented yet|exit 1/);
  });

  it("runs every step of the plan, in order", () => {
    const text = `${verify.header}\n${verify.body}`;
    const steps = ["web-install", "web-gen-check", "pnpm lint", "pnpm typecheck", "vitest run", "pnpm build", "just web-e2e", "just web-perf", "just web-bench-check"];
    let from = 0;
    for (const step of steps) {
      const at = text.indexOf(step, from);
      expect(at, `${step} is in verify-08 after the step before it`).toBeGreaterThanOrEqual(0);
      from = at + step.length;
    }
  });

  it("only names recipes that exist", () => {
    for (const name of ["web-install", "web-gen-check", "web-fixture", "web-e2e", "web-perf", "web-bench-check", "web-dev", "web-mock"]) expect(recipeNames.has(name), name).toBe(true);
  });

  it("checks that the generated client is fresh by regenerating and looking at git", () => {
    const check = recipe("web-gen-check");
    expect(check.body).toContain("pnpm gen:api");
    expect(check.body).toContain("git status --porcelain -- src/api");
  });

  it("builds the Rust fixture and both web builds before it runs Playwright", () => {
    const e2e = recipe("web-e2e");
    expect(e2e.header).toContain("web-fixture");
    const text = e2e.body;
    expect(text.indexOf("pnpm build &&")).toBeLessThan(text.indexOf("playwright test"));
    expect(text).toContain("pnpm build:perf");
    expect(recipe("web-fixture").body).toContain("cargo build --locked -p hub --example web_fixture");
  });

  it("keeps the timing specs out of web-e2e and runs them alone in web-perf", () => {
    // A measurement that shares the machine with another test measures the other test.
    expect(recipe("web-e2e").body).toContain("playwright test --project=chromium");
    const perf = recipe("web-perf");
    expect(perf.header).toContain("web-fixture");
    expect(perf.body).toContain("playwright test --project=perf --workers=1");
  });

  it("web-perf removes the old results, builds, measures, then runs Lighthouse, in that order", () => {
    const body = recipe("web-perf").body;
    expect(body).not.toMatch(/exit 1|not added|stub/);
    const steps = ['rm -f "${CARGO_TARGET_DIR:-target}"/perf-results/web-*.json', "pnpm build\n", "pnpm build:perf", "playwright test --project=perf", "scripts/perf/lighthouse.mjs"];
    let from = 0;
    for (const step of steps) {
      const at = body.indexOf(step, from);
      expect(at, `${step} comes after the step before it`).toBeGreaterThanOrEqual(0);
      from = at + step.length;
    }
  });

  it("the two Playwright projects split the specs between them, the three timing specs in `perf`", () => {
    const config = readFileSync(new URL("web/playwright.config.ts", root), "utf8");
    expect(config).toContain('{ name: "chromium", testIgnore: TIMING }');
    expect(config).toContain('{ name: "perf", testMatch: TIMING');
    const timing = /const TIMING = \/(.*)\/;/.exec(config)?.[1] ?? "";
    for (const spec of ["route-change", "diff-6000", "grid-5000"]) expect(timing, spec).toContain(spec);
  });
});

describe("the P10 budgets of the gate", () => {
  const registered = [...budgets.matchAll(/\[\[budget\]\]([\s\S]*?)(?=\n\[\[budget\]\]|$)/g)]
    .map((m) => m[1])
    .filter((block) => /^bench = "web\//m.test(block))
    .map((block) => /^id = "([^"]+)"/m.exec(block)?.[1]);

  it("has every one of them registered, so the gate fails when a number is missing", () => {
    const blocks = [...budgets.matchAll(/\[\[budget\]\]([\s\S]*?)(?=\n\[\[budget\]\]|$)/g)].map((m) => m[1]).filter((block) => /^bench = "web\//m.test(block));
    expect(blocks).toHaveLength(5);
    for (const block of blocks) expect(block, /^id = "([^"]+)"/m.exec(block)?.[1]).toMatch(/^registered = true$/m);
  });

  it("lists exactly the web budgets of perf/budgets.toml", () => {
    const listed = /^web_budgets := "([^"]+)"/m.exec(justfile)?.[1]?.split(" ") ?? [];
    expect(listed.sort()).toEqual(registered.sort());
    expect(listed).toHaveLength(5);
  });

  it("judges them with the xtask bench-check on a cut registry, as writes-bench does", () => {
    const check = recipe("web-bench-check");
    expect(check.body).toContain("cargo xtask bench-check --budgets");
    expect(check.body).toContain("found $found of $wanted");
  });
});

describe("package scripts", () => {
  it("build is the typecheck, the bundle, then the dist scan, so CI and the image build run all three", () => {
    const build = pkg.scripts.build;
    expect(build.indexOf("tsc -b")).toBeGreaterThanOrEqual(0);
    expect(build.indexOf("vite build")).toBeGreaterThan(build.indexOf("tsc -b"));
    expect(build.indexOf("scripts/dist-check.mjs")).toBeGreaterThan(build.indexOf("vite build"));
  });

  it("typecheck builds every project (app, tests, tooling)", () => {
    expect(pkg.scripts.typecheck).toBe("tsc -b");
    const solution = JSON.parse(readFileSync(new URL("web/tsconfig.json", root), "utf8"));
    expect(solution.references.map((r) => r.path).sort()).toEqual(["./tsconfig.app.json", "./tsconfig.node.json", "./tsconfig.test.json"]);
  });

  it("the perf build is scanned too, and may carry the test routes", () => {
    expect(pkg.scripts["build:perf"]).toContain("--allow-test-pages");
  });
});

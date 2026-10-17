import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import { describe, expect, it } from "vitest";

// public/theme-init.js is a classic script that runs in the page before anything else. It is run here in a bare context
// with a fake document, which also proves it needs nothing else: no storage, no network, no eval.
const source = readFileSync(new URL("../public/theme-init.js", import.meta.url), "utf8");

function run(cookie, systemDark) {
  const classes = new Set();
  const context = {
    document: { cookie, documentElement: { classList: { add: (c) => classes.add(c) } } },
    window: { matchMedia: (q) => ({ matches: q === "(prefers-color-scheme: dark)" && systemDark }) },
  };
  runInNewContext(source, context);
  return classes;
}

describe("theme-init.js", () => {
  it("applies dark for the dark choice, whatever the system says", () => {
    expect(run("lk_theme=dark", false).has("dark")).toBe(true);
  });
  it("applies light for the light choice, whatever the system says", () => {
    expect(run("lk_theme=light", true).has("dark")).toBe(false);
  });
  it("follows the system for 'system' and for no cookie", () => {
    expect(run("lk_theme=system", true).has("dark")).toBe(true);
    expect(run("", true).has("dark")).toBe(true);
    expect(run("", false).has("dark")).toBe(false);
  });
  it("ignores a value that is not one of the three and other cookies", () => {
    expect(run("a=1; lk_theme=purple; b=2", false).has("dark")).toBe(false);
    expect(run("a=1; lk_theme=dark; b=2", false).has("dark")).toBe(true);
    expect(run("xlk_theme=dark", false).has("dark")).toBe(false);
  });
  it("uses no storage, no network and no eval", () => {
    const code = source.replace(/^\s*\/\/.*$/gm, "");
    expect(code).not.toMatch(/localStorage|sessionStorage|fetch|XMLHttpRequest|eval|Function\(|import\(|innerHTML|document\.write/);
  });
});

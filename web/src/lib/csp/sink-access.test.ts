import { describe, expect, it } from "vitest";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";

// Structural proof for S12: the one HTML sink (components/app/SanitisedDocHtml.tsx) is the only place that sets inner
// HTML, and lib/csp/trusted-types.ts is the only place that creates a Trusted Types policy. Source scans, so they run in
// the node project; the lint rules (web/lint) are the second line.
describe("who may reach the HTML sink", () => {
  const SRC = fileURLToPath(new URL("../../", import.meta.url));
  const walk = (dir: string, out: string[] = []): string[] => {
    for (const name of readdirSync(dir)) {
      const full = join(dir, name);
      if (statSync(full).isDirectory()) walk(full, out);
      else if (/\.(ts|tsx)$/.test(name)) out.push(full);
    }
    return out;
  };
  const files = walk(SRC).filter((f) => !f.endsWith(".test.ts") && !f.endsWith(".test.tsx") && !relative(SRC, f).startsWith("api/generated"));

  it("has exactly one component that sets inner HTML, and it is this one", () => {
    const users = files.filter((f) => /dangerouslySetInnerHTML/.test(readFileSync(f, "utf8"))).map((f) => relative(SRC, f));
    expect(users).toEqual(["components/app/SanitisedDocHtml.tsx"]);
  });

  it("is the only importer of the docs policy outside the editor and lib/csp", () => {
    const importers = files
      .filter((f) => /from\s+["']@\/lib\/csp\/trusted-types["']/.test(readFileSync(f, "utf8")))
      .map((f) => relative(SRC, f))
      .filter((f) => !f.startsWith("editor/") && !f.startsWith("lib/csp/"));
    expect(importers).toEqual(["components/app/SanitisedDocHtml.tsx"]);
  });

  it("is the only caller of trustedTypes.createPolicy, and it is lib/csp/trusted-types.ts", () => {
    const callers = files.filter((f) => /\.createPolicy\s*\(/.test(readFileSync(f, "utf8"))).map((f) => relative(SRC, f));
    expect(callers).toEqual(["lib/csp/trusted-types.ts"]);
  });
});

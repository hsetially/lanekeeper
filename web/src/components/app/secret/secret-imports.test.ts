import { describe, expect, it } from "vitest";
import { SECRET_DISPLAYS } from "../../../../lint/restricted-imports.mjs";
import { productSources, withoutComments } from "@/test/source-files";

// The lint rule (lint/restricted-imports.mjs) says which files may import the secret copy button and its field; this reads the
// source tree and says the same, so a file the lint does not reach (or a rule that was weakened) cannot let a copy button for a
// secret appear anywhere else, and the generic CopyButton cannot appear in a secret display.

const sources = productSources().map((f) => ({ path: f.path, code: withoutComments(f.text) }));
const IMPORTS_SECRET = /from\s+["'][^"']*\/secret(\/[^"']*)?["']/;
const IMPORTS_GENERIC_COPY = /from\s+["'][^"']*\/CopyButton["']/;
const insideSecretFolder = (path: string): boolean => path.startsWith("src/components/app/secret/");

describe("who may copy a secret", () => {
  it("has the source tree to read, and the two displays in it", () => {
    expect(sources.length).toBeGreaterThan(300);
    for (const display of SECRET_DISPLAYS) expect(sources.some((f) => f.path === display), display).toBe(true);
  });

  it("is imported only by the displays of a one-time secret: the admin join token and the personal MCP token", () => {
    const importers = sources.filter((f) => IMPORTS_SECRET.test(f.code) && !insideSecretFolder(f.path)).map((f) => f.path);
    expect(importers.sort()).toEqual([...SECRET_DISPLAYS].sort());
  });

  it("is used by each of the two displays for its copy button and its field", () => {
    for (const display of SECRET_DISPLAYS) {
      const code = sources.find((f) => f.path === display)?.code ?? "";
      expect(code, display).toMatch(/<SecretCopyButton\b/);
      expect(code, display).toMatch(/<SecretField\b/);
    }
  });

  it("keeps the generic copy button out of the two displays, so a token cannot be given to the button that refuses secrets by name only", () => {
    for (const display of SECRET_DISPLAYS) {
      expect(sources.find((f) => f.path === display)?.code ?? "", display).not.toMatch(IMPORTS_GENERIC_COPY);
    }
  });

  it("is not reached by anything that holds a token the person typed (the GitHub token page) or a revealed value", () => {
    const typed = sources.filter((f) => /^src\/features\/access\/(GithubToken|TokenSetup|TokenSaved|TokenOutcome)/.test(f.path) || /reveal/i.test(f.path));
    expect(typed.length).toBeGreaterThan(0);
    for (const f of typed) expect(f.code, f.path).not.toMatch(IMPORTS_SECRET);
  });

  it("never gives the generic button a secret by name: no `secret` prop is written on a CopyButton anywhere", () => {
    const bad = sources.filter((f) => /<CopyButton\b[^>]*\bsecret\b/.test(f.code) && f.path !== "src/components/app/CopyButton.tsx").map((f) => f.path);
    expect(bad).toEqual([]);
  });
});

import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// S21 equivalents: TypeScript stays strict in every project. A flag cannot be loosened without this test saying so.
const read = (name) => JSON.parse(readFileSync(new URL(`../${name}`, import.meta.url), "utf8"));

describe("tsconfig strictness", () => {
  for (const name of ["tsconfig.app.json", "tsconfig.test.json", "tsconfig.node.json"]) {
    it(`${name} keeps every strict flag on`, () => {
      const { compilerOptions: o } = read(name);
      expect(o.strict).toBe(true);
      expect(o.noUncheckedIndexedAccess).toBe(true);
      expect(o.exactOptionalPropertyTypes).toBe(true);
      expect(o.noImplicitOverride).toBe(true);
      expect(o.noFallthroughCasesInSwitch).toBe(true);
      expect(o.checkJs).not.toBe(true); // plain JS is linted, not type-checked
    });
  }

  it("keeps Node types out of product code: only the test and tooling projects have them", () => {
    expect(read("tsconfig.app.json").compilerOptions.types).toEqual(["vite/client"]);
    expect(read("tsconfig.test.json").compilerOptions.types).toEqual(["vite/client", "node"]);
    expect(read("tsconfig.node.json").compilerOptions.types).toEqual(["node"]);
  });

  it("the app project leaves out only tests, test helpers, mocks and the handoff data", () => {
    expect(read("tsconfig.app.json").exclude.sort()).toEqual(["src/**/*.test.ts", "src/**/*.test.tsx", "src/handoff/**", "src/mocks/**", "src/test/**"]);
  });
});

// Fails when the handoff changed and the committed generated files were not regenerated
// (run `pnpm gen:handoff`), or when a generated file was edited by hand.
import { existsSync, readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { WEB_ROOT, generateAll } from "../../scripts/handoff/generate.mjs";
import { join } from "node:path";

describe("handoff generated files", () => {
  const generated = generateAll();

  it("covers tokens, pairs, routes, keymap, states, boards and icons plus the two stylesheets", () => {
    expect([...generated.keys()].sort()).toEqual([
      "src/handoff/boards.generated.ts",
      "src/handoff/keymap.generated.ts",
      "src/handoff/pairs.generated.ts",
      "src/handoff/routes.generated.ts",
      "src/handoff/states.generated.ts",
      "src/handoff/tokens.generated.ts",
      "src/icons/icons.generated.ts",
      "src/styles/theme.generated.css",
      "src/styles/tokens.css",
    ]);
  });

  for (const [path, content] of generated) {
    it(`${path} is up to date`, () => {
      const file = join(WEB_ROOT, path);
      expect(existsSync(file), `${path} is missing: run pnpm gen:handoff`).toBe(true);
      expect(readFileSync(file, "utf8") === content, `${path} is stale: run pnpm gen:handoff`).toBe(true);
    });
  }

  it("is deterministic", () => {
    const again = generateAll();
    for (const [path, content] of generated) expect(again.get(path)).toBe(content);
  });
});

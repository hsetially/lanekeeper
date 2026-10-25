// axe-core in jsdom, for structure only: names, roles, ids, ARIA validity. jsdom has no layout, so the rules
// that need pixels (contrast, target size, region) run in Playwright instead (T4, in real layout).
import axe from "axe-core";
import { expect } from "vitest";

export async function expectNoAxeViolations(root: Element = document.body): Promise<void> {
  const results = await axe.run(root, {
    rules: {
      "color-contrast": { enabled: false },
      "target-size": { enabled: false },
      region: { enabled: false },
    },
  });
  const summary = results.violations.map((v) => `${v.id}: ${v.help} (${v.nodes.map((n) => n.target.join(" ")).join(", ")})`);
  expect(summary, summary.join("\n")).toEqual([]);
}

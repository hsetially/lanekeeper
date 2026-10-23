import { describe, expect, it } from "vitest";
import { planMasks } from "@/components/masking/plan";
import { contextFor, resolveBody } from "../scenario.ts";
import { designScenarios } from "./index.ts";

// A mask is UTF-8 byte offsets into the `text` of the object that carries it (`FileContent`, `ServedFile`, `DocText`), and the web
// believes nothing it cannot check: a line whose mask is not exactly a marker is hidden, and a mask that cannot be placed refuses the
// text. So a scenario whose masks are wrong does not show what the design shows (the first file view, which dropped such a mask and
// drew the line as text, hid one: the core file's offsets were counted in characters). Every mask any design scenario sends is
// checked here, in every answer the scenario can give, so that a wrong one is a failing test and not a "Hidden line" in a screenshot.

function* masked(value: unknown, where: string): Generator<{ where: string; text: string; masks: Parameters<typeof planMasks>[1] }> {
  if (Array.isArray(value)) {
    for (const [i, item] of value.entries()) yield* masked(item, `${where}[${i}]`);
    return;
  }
  if (typeof value !== "object" || value === null) return;
  const record = value as Record<string, unknown>;
  if (typeof record["text"] === "string" && Array.isArray(record["masks"])) yield { where, text: record["text"], masks: record["masks"] as Parameters<typeof planMasks>[1] };
  for (const [key, member] of Object.entries(record)) yield* masked(member, `${where}.${key}`);
}

describe("the masks that the design scenarios send", () => {
  const found: string[] = [];
  const wrong: string[] = [];
  for (const scenario of Object.values(designScenarios)) {
    for (const response of scenario.responses) {
      const contexts = [contextFor(response), ...(response.samples ?? []).map((sample) => ({ ...contextFor(response), ...sample }))];
      for (const context of contexts) {
        let body: unknown;
        try {
          body = resolveBody(response, context);
        } catch {
          continue;
        }
        for (const item of masked(body, `${scenario.id} ${response.method} ${response.path}`)) {
          found.push(item.where);
          const plan = planMasks(item.text, item.masks);
          if (plan.status !== "ok") wrong.push(`${item.where}: ${plan.status === "refused" ? `refused (${plan.reason})` : "none"}`);
          else for (const [line, placed] of plan.lines) if (placed.kind !== "marks") wrong.push(`${item.where}: line ${line} is hidden (${placed.reason})`);
        }
      }
    }
  }

  it("is a scan that finds masks to check", () => {
    expect(found.length).toBeGreaterThan(0);
  });

  it("are each exactly a marker, at the bytes they name", () => {
    expect(wrong).toEqual([]);
  });
});

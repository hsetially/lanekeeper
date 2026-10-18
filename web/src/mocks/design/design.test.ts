import { readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import type { Scenario } from "../scenario.ts";
import { scenarios } from "../scenarios.ts";
import { DESIGN_SLICES, collectDesignScenarios, designScenarios } from "./index.ts";

// The convention that lets eight slices add their scenarios in parallel without touching a shared file: one folder each, an
// id that starts with the slice's name, and a loader that lists every folder. If a slice folder is added and the loader does
// not list it, this fails; if two scenarios share an id, or an id does not carry its slice's name, this fails.

const dir = new URL(".", import.meta.url).pathname;

describe("the design scenario loader", () => {
  it("lists every slice folder under src/mocks/design, and only those", () => {
    const folders = readdirSync(dir).filter((name) => statSync(join(dir, name)).isDirectory());
    expect(Object.keys(DESIGN_SLICES).sort()).toEqual(folders.sort());
  });

  it("gives the shell its two scenarios and adds all of them to the scenarios of the harness", () => {
    expect(Object.keys(designScenarios)).toEqual(expect.arrayContaining(["design-shell-admin", "design-shell-viewer"]));
    for (const id of Object.keys(designScenarios)) expect(scenarios[id], id).toBe(designScenarios[id]);
  });

  it("refuses an id that does not carry its slice's name, and an id that is used twice", () => {
    const one: Scenario = { id: "design-swimlanes-six", description: "x", responses: [] };
    expect(() => collectDesignScenarios({ swimlanes: [one] })).not.toThrow();
    expect(() => collectDesignScenarios({ file: [one] })).toThrow(/must be named design-file-/);
    expect(() => collectDesignScenarios({ swimlanes: [one, one] })).toThrow(/defined twice/);
  });

  it("has a signed-in Admin whose sidebar counts 11 findings and 6 approvals, as the Shell page does", () => {
    const admin = designScenarios["design-shell-admin"];
    const lanes = admin?.responses.find((r) => r.path === "/api/v1/swimlanes")?.body as { items: Array<{ finding_count: number }> };
    expect(lanes.items.reduce((sum, l) => sum + l.finding_count, 0)).toBe(11);
    const proposals = admin?.responses.find((r) => r.path === "/api/v1/proposals")?.body as { items: unknown[] };
    expect(proposals.items).toHaveLength(6);
  });
});

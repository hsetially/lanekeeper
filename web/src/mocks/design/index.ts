// The design-parity scenarios of T4: one folder per slice under src/mocks/design/<slice>/, each with an `index.ts` that exports
// `scenarios`. THIS FILE IS THE ONLY PLACE they are collected; it lists every slice folder, and the shell slice created every
// one of them as an empty stub, so a slice adds its scenarios by replacing its own `index.ts` and never edits this file, and
// nothing has to be merged by hand. src/mocks/scenarios.ts adds the result to the scenarios of the harness and the tests, and
// src/mocks/mocks.test.ts then checks each response against the OpenAPI schema.
//
// Loaded by Node (the e2e harness) as well as by Vite: relative imports with the `.ts` extension and `import type`.
import type { Scenario } from "../scenario.ts";
import { scenarios as access } from "./access/index.ts";
import { scenarios as admin } from "./admin/index.ts";
import { scenarios as adoption } from "./adoption/index.ts";
import { scenarios as approvals } from "./approvals/index.ts";
import { scenarios as compare } from "./compare/index.ts";
import { scenarios as file } from "./file/index.ts";
import { scenarios as findings } from "./findings/index.ts";
import { scenarios as search } from "./search/index.ts";
import { scenarios as shell } from "./shell/index.ts";
import { scenarios as swimlanes } from "./swimlanes/index.ts";

/** The slice folders, in the order of the T4 plan. */
export const DESIGN_SLICES = { shell, access, swimlanes, file, compare, search, findings, adoption, approvals, admin } as const;

/** Every design scenario by id. An id is `design-<slice>-<name>`, so two slices can never collide; a collision still fails here. */
export function collectDesignScenarios(slices: Readonly<Record<string, readonly Scenario[]>> = DESIGN_SLICES): Record<string, Scenario> {
  const out: Record<string, Scenario> = {};
  for (const [slice, list] of Object.entries(slices)) {
    for (const scenario of list) {
      if (!scenario.id.startsWith(`design-${slice}-`)) throw new Error(`design scenario "${scenario.id}" must be named design-${slice}-<name>`);
      if (out[scenario.id] !== undefined) throw new Error(`design scenario "${scenario.id}" is defined twice`);
      out[scenario.id] = scenario;
    }
  }
  return out;
}

export const designScenarios: Record<string, Scenario> = collectDesignScenarios();

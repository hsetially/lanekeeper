import { describe, expect, it } from "vitest";
import { KEYMAP } from "@/handoff/keymap.generated";
import { KEYMAP_DATA, KEYMAP_NOTE } from "./keymap";

// `keymap.ts` is the reviewed copy of the handoff's keymap that product code reads (product code must not import the
// extracted data). It must be the extracted data, row for row, in both directions.

describe("the reviewed keymap against the handoff", () => {
  it("has exactly the handoff's rows, in order, with the same keys, scopes, actions and bindings", () => {
    expect(KEYMAP_DATA.map((r) => r.id)).toEqual(KEYMAP.rows.map((r) => r.id));
    for (const [i, row] of KEYMAP.rows.entries()) expect(KEYMAP_DATA[i], row.id).toEqual(row);
  });

  it("carries the handoff's note about the two shortcut sets (H-1)", () => {
    expect(KEYMAP_NOTE).toBe(KEYMAP.note);
  });
});

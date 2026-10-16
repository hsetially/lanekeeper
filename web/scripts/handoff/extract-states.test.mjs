import { describe, expect, it } from "vitest";
import { buildStates, renderStatesTs } from "./extract-states.mjs";

describe("extract-states (the States index)", () => {
  const data = buildStates();

  it("reads 11 states and 67 rows", () => {
    expect(data.sections).toHaveLength(11);
    expect(data.rows).toHaveLength(67);
    expect(data.sections.map((s) => s.code).join("")).toBe("LERPOUTCKAG");
  });

  it("numbers rows per state code and keeps the screen, file and variant", () => {
    expect(data.rows[0]).toEqual({
      id: "L01",
      code: "L",
      state: "loading",
      screen: "Swimlanes",
      file: "Lanekeeper Shell.dc.html",
      variant: "",
      wired: true,
    });
    const p01 = data.rows.find((r) => r.id === "P01");
    expect(p01).toMatchObject({ screen: "Access screen", variant: "Pending user", wired: false });
    expect(data.rows.find((r) => r.id === "U06")).toMatchObject({ state: "paused", screen: "Audit" });
  });

  it("keeps the handoff's own wired flags (the plan records that they are stale)", () => {
    expect(data.rows.filter((r) => r.wired)).toHaveLength(6 + 5 + 2 + 1 + 2 + 1);
  });

  it("renders a typed module", () => {
    expect(renderStatesTs(data)).toContain("export const STATES = ");
  });
});

import { describe, expect, it } from "vitest";
import { sourceLabel } from "./attribution";

// Who changed a version is said by the shared attribution chip (src/components/attribution); this module is only how a version
// was observed.

describe("sourceLabel", () => {
  it("says how a version was observed, in the design's words", () => {
    expect(sourceLabel("tool_write")).toBe("Tool write");
    expect(sourceLabel("scan")).toBe("Scan found a change");
    expect(sourceLabel("sync")).toBe("Sync from Git");
  });
});

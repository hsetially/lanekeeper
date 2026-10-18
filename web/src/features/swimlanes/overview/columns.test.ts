import { describe, expect, it } from "vitest";
import { COLUMN_IDS, visibleColumns } from "./columns";

describe("visibleColumns", () => {
  it("is every column when nothing was remembered", () => {
    expect(visibleColumns(undefined)).toEqual([...COLUMN_IDS]);
  });

  it("keeps the remembered ones in the design's order and always includes the Swimlane column", () => {
    expect(visibleColumns(["findings", "agent"])).toEqual(["swimlane", "agent", "findings"]);
  });

  it("ignores an id it does not know, so a stale preference cannot break the table", () => {
    expect(visibleColumns(["agent", "nope", "__proto__"])).toEqual(["swimlane", "agent"]);
  });
});

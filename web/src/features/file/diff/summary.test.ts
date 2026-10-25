import { describe, expect, it } from "vitest";
import type { SettingChange } from "@/api/generated/model";
import { summarise } from "./summary";

const value = (text: string) => ({ text, ty: "string" as const, flag: null });
const change = (path: string, kind: SettingChange["kind"], left: string | null, right: string | null, line?: number): SettingChange => ({
  path,
  kind,
  left: left === null ? null : value(left),
  right: right === null ? null : value(right),
  left_location: null,
  right_location: line === undefined ? null : { path, lines: { start: line, end: line } },
  severity: null,
});

describe("summarise", () => {
  it("counts what changed, as the design writes it: 2 settings changed, 1 added", () => {
    const s = summarise([change("a.b.x", "changed", "false", "true", 13), change("a.b.y", "added", null, "asc", 14), change("logging.level", "changed", "WARN", "INFO", 31)]);
    expect(s.headline).toBe("2 settings changed, 1 added");
    expect(s.total).toBe(3);
  });
  it("says a single kind plainly", () => {
    expect(summarise([change("a", "added", null, "1")]).headline).toBe("1 setting added");
    expect(summarise([change("a", "removed", "1", null), change("b", "removed", "2", null)]).headline).toBe("2 settings removed");
    expect(summarise([change("a", "changed", "1", "2")]).headline).toBe("1 setting changed");
  });
  it("counts a changed type as changed", () => {
    expect(summarise([change("a", "type_changed", "1", "x")]).headline).toBe("1 setting changed");
  });
  it("splits a path at its last dot, so the key can be read at a glance", () => {
    const [chip] = summarise([change("txInfinityOptions.remote-itm-teller.enableAccountSorting", "changed", "false", "true", 13)]).chips;
    expect(chip).toMatchObject({ prefix: "txInfinityOptions.remote-itm-teller.", key: "enableAccountSorting", kind: "changed", from: "false", to: "true" });
  });
  it("has no old value for an added setting and no new value for a removed one", () => {
    const s = summarise([change("a", "added", null, "asc"), change("b", "removed", "x", null)]);
    expect(s.chips[0]).toMatchObject({ from: null, to: "asc" });
    expect(s.chips[1]).toMatchObject({ from: "x", to: null });
  });
  it("lists each change with the line it is on, new side first", () => {
    const s = summarise([change("a.k", "changed", "1", "2", 13)]);
    expect(s.list).toEqual([{ key: "a.k", kind: "Changed", line: 13 }]);
  });
  it("is bounded: a long list is cut and says how many are not shown", () => {
    const many = Array.from({ length: 500 }, (_, i) => change(`k${i}`, "added", null, "1"));
    const s = summarise(many);
    expect(s.chips).toHaveLength(12);
    expect(s.list).toHaveLength(200);
    expect(s.hiddenChips).toBe(488);
    expect(s.hiddenList).toBe(300);
  });
  it("is empty for no changes", () => {
    expect(summarise([])).toMatchObject({ headline: "", total: 0, chips: [], list: [] });
  });
});

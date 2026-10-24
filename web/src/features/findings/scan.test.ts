import { describe, expect, it } from "vitest";
import { latestScan } from "./scan";

describe("latestScan", () => {
  it("is the most recent scan of any swimlane", () => {
    expect(latestScan([{ last_scan_at: 100 }, { last_scan_at: 300 }, { last_scan_at: 200 }])).toBe(300);
  });

  it("skips a swimlane that was never scanned", () => {
    expect(latestScan([{ last_scan_at: null }, { last_scan_at: 50 }])).toBe(50);
  });

  it("is null, not 0, when nothing was scanned yet, so no screen prints 1970", () => {
    expect(latestScan([])).toBeNull();
    expect(latestScan([{ last_scan_at: null }])).toBeNull();
  });
});

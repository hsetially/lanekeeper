import { describe, expect, it } from "vitest";
import { CONFIRM_PHRASE, formatBytes, formatCount, MAX_DAYS, MAX_LAST, MIN_DAYS, MIN_LAST, parseRetention, policySentence, sameValues, wouldDelete } from "./retention-model";

// What a retention policy accepts before it is sent, and how its numbers read. What a policy would delete is the hub's answer (the
// dry run): nothing here estimates it.

describe("parseRetention", () => {
  it("reads two whole numbers", () => {
    expect(parseRetention("180", "50")).toEqual({ ok: true, value: { keep_days: 180, keep_last_versions: 50 } });
    expect(parseRetention(" 90 ", " 10 ")).toEqual({ ok: true, value: { keep_days: 90, keep_last_versions: 10 } });
  });

  it("accepts the bounds and refuses a value just outside them", () => {
    expect(parseRetention(String(MIN_DAYS), String(MIN_LAST)).ok).toBe(true);
    expect(parseRetention(String(MAX_DAYS), String(MAX_LAST)).ok).toBe(true);
    expect(parseRetention(String(MIN_DAYS - 1), "50")).toMatchObject({ ok: false, errors: { days: "Minimum is 30 days" } });
    expect(parseRetention(String(MAX_DAYS + 1), "50")).toMatchObject({ ok: false, errors: { days: "Maximum is 3,650 days" } });
    expect(parseRetention("180", String(MIN_LAST - 1))).toMatchObject({ ok: false, errors: { last: "Keep at least 5 versions per file" } });
    expect(parseRetention("180", String(MAX_LAST + 1))).toMatchObject({ ok: false, errors: { last: "Maximum is 10,000 versions" } });
  });

  it("asks for a number, and for a whole one", () => {
    expect(parseRetention("", "")).toMatchObject({ ok: false, errors: { days: "Enter the number of days", last: "Enter the number of versions" } });
    expect(parseRetention("1.5", "50")).toMatchObject({ ok: false, errors: { days: "Enter a whole number of days" } });
    expect(parseRetention("180", "five")).toMatchObject({ ok: false, errors: { last: "Enter a whole number" } });
    expect(parseRetention("-30", "50")).toMatchObject({ ok: false, errors: { days: "Enter a whole number of days" } });
    expect(parseRetention("1e3", "50")).toMatchObject({ ok: false, errors: { days: "Enter a whole number of days" } });
  });

  it("reports both fields at once, and only the one that is wrong", () => {
    const both = parseRetention("", "");
    expect(both.ok ? [] : Object.keys(both.errors).sort()).toEqual(["days", "last"]);
    const one = parseRetention("180", "");
    expect(one.ok ? [] : Object.keys(one.errors)).toEqual(["last"]);
  });

  it("does not repeat the value in a message", () => {
    const bad = parseRetention("hunter2", "50");
    expect(JSON.stringify(bad)).not.toContain("hunter2");
  });
});

describe("sameValues", () => {
  it("compares the two numbers", () => {
    expect(sameValues({ keep_days: 180, keep_last_versions: 50 }, { keep_days: 180, keep_last_versions: 50 })).toBe(true);
    expect(sameValues({ keep_days: 180, keep_last_versions: 50 }, { keep_days: 90, keep_last_versions: 50 })).toBe(false);
  });
});

describe("policySentence", () => {
  it("says what a policy keeps", () => {
    expect(policySentence({ keep_days: 180, keep_last_versions: 50 })).toBe("every version for 180 days and the last 50 versions of each file");
    expect(policySentence({ keep_days: 30, keep_last_versions: 5 })).toBe("every version for 30 days and the last 5 versions of each file");
  });
});

describe("formatBytes", () => {
  it("writes binary units with one decimal", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(1023)).toBe("1,023 B");
    expect(formatBytes(1024)).toBe("1.0 KiB");
    expect(formatBytes(3.2 * 1024 ** 3)).toBe("3.2 GiB");
    expect(formatBytes(64 * 1024 ** 2)).toBe("64.0 MiB");
    expect(formatBytes(5 * 1024 ** 4)).toBe("5.0 TiB");
  });

  it("does not choke on a number that is not one", () => {
    expect(formatBytes(Number.NaN)).toBe("unknown");
    expect(formatBytes(-1)).toBe("unknown");
  });
});

describe("formatCount", () => {
  it("separates thousands", () => {
    expect(formatCount(3402)).toBe("3,402");
  });
});

describe("wouldDelete", () => {
  it("is true when the dry run found versions to delete, or bytes to free", () => {
    expect(wouldDelete({ versions_to_delete: 1, bytes_to_free: 0, versions_kept_by_reference: 0 })).toBe(true);
    expect(wouldDelete({ versions_to_delete: 0, bytes_to_free: 10, versions_kept_by_reference: 0 })).toBe(true);
    expect(wouldDelete({ versions_to_delete: 0, bytes_to_free: 0, versions_kept_by_reference: 7 })).toBe(false);
  });
});

describe("the phrase that confirms a deleting policy", () => {
  it("is words, not a number, so that it is typed on purpose", () => {
    expect(CONFIRM_PHRASE).toBe("delete versions");
  });
});

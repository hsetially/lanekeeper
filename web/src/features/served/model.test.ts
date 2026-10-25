import { describe, expect, it } from "vitest";
import {
  fieldProblem,
  isAppName,
  isTenantId,
  placeholdersShown,
  problemsOf,
  readSelection,
  sameSelection,
  selectionFrom,
  servedHref,
  type ServedSelection,
} from "./model";

// What the "As served" view takes from its address, and what it refuses. The address is untrusted input (anyone can send a link), the
// server checks again, and these checks only keep a value that cannot be valid off the wire and say why next to the field.

const FULL = { file: "tx-infinity-api/tx-infinity-core-sit7.yml", application: "tx-infinity-core", tenant: "sit7", channel: "beta" };

describe("the names", () => {
  it.each(["tx-infinity-core", "core", "A", "_x", "a.b_c-d", "x".repeat(128)])("accepts %j as an application", (name) => {
    expect(isAppName(name)).toBe(true);
  });
  it.each(["", "-x", ".x", "a b", "a/b", "a\\b", "x".repeat(129), "ünï", "a\nb", "a‮b"])("refuses %j as an application", (name) => {
    expect(isAppName(name)).toBe(false);
  });
  it.each(["sit7", "a", "0x", "sit_7", "a-b", "x".repeat(63)])("accepts %j as a tenant", (name) => {
    expect(isTenantId(name)).toBe(true);
  });
  it.each(["", "Sit7", "_x", "-x", "sit 7", "sit.7", "x".repeat(64), "sit7\n"])("refuses %j as a tenant", (name) => {
    expect(isTenantId(name)).toBe(false);
  });
});

describe("one field at a time", () => {
  it("asks for the three that are needed, and lets the channel be empty", () => {
    expect(fieldProblem("file", "")).toBe("A file is required");
    expect(fieldProblem("application", "")).toBe("An application is required");
    expect(fieldProblem("tenant", "")).toBe("A tenant is required");
    expect(fieldProblem("channel", "")).toBeNull();
  });

  it("says what a valid value looks like, and never repeats the value it was given", () => {
    const secretLooking = "hunter2/../x";
    expect(fieldProblem("file", secretLooking)).toMatch(/relative path/i);
    expect(fieldProblem("file", secretLooking)).not.toContain("hunter2");
    expect(fieldProblem("application", "no spaces here")).toMatch(/letters, digits/i);
    expect(fieldProblem("application", "no spaces here")).not.toContain("spaces");
    expect(fieldProblem("tenant", "Upper")).toMatch(/lower case/i);
    expect(fieldProblem("channel", "-x")).toMatch(/letters, digits/i);
  });

  it("refuses a path that leaves the swimlane's root, in every way the contract refuses", () => {
    for (const bad of ["/etc/passwd", "../x.yml", "a/../b.yml", "a//b.yml", "a/./b.yml", "a\\b.yml", "a\u0000b", "a\nb", "x".repeat(1025)]) {
      expect(fieldProblem("file", bad), JSON.stringify(bad)).not.toBeNull();
    }
    expect(fieldProblem("file", "core/a.yml")).toBeNull();
    expect(fieldProblem("file", "application.properties")).toBeNull();
  });

  it("ignores spaces around a value, as a person pastes it", () => {
    expect(fieldProblem("application", "  tx-infinity-core ")).toBeNull();
    expect(fieldProblem("tenant", " sit7")).toBeNull();
  });
});

describe("the form as a whole", () => {
  it("has no problem for a full selection", () => {
    expect(problemsOf(FULL)).toEqual({});
  });
  it("names every field that is wrong at once", () => {
    expect(Object.keys(problemsOf({ file: "", application: "bad name", tenant: "", channel: "-x" })).sort()).toEqual(["application", "channel", "file", "tenant"]);
  });
  it("makes a selection only from a complete, valid form, trimmed, with no channel as null", () => {
    expect(selectionFrom(FULL)).toEqual(FULL);
    expect(selectionFrom({ ...FULL, channel: "" })).toEqual({ ...FULL, channel: null });
    expect(selectionFrom({ ...FULL, channel: "   " })).toEqual({ ...FULL, channel: null });
    expect(selectionFrom({ ...FULL, application: " tx-infinity-core " })?.application).toBe("tx-infinity-core");
    expect(selectionFrom({ ...FULL, tenant: "" })).toBeNull();
    expect(selectionFrom({ ...FULL, file: "../x" })).toBeNull();
  });
});

describe("reading the address", () => {
  it("keeps the four keys and reads a complete address as a selection", () => {
    const read = readSelection(FULL);
    expect(read.selection).toEqual(FULL);
    expect(read.draft).toEqual(FULL);
    expect(read.problems).toEqual({});
  });

  it("reads an address without a channel as no channel", () => {
    expect(readSelection({ file: FULL.file, application: FULL.application, tenant: FULL.tenant }).selection).toEqual({ ...FULL, channel: null });
  });

  it("is not a selection while a required key is missing, and does not scold about what was never given", () => {
    const read = readSelection({ file: FULL.file });
    expect(read.selection).toBeNull();
    expect(read.draft).toEqual({ file: FULL.file, application: "", tenant: "", channel: "" });
    expect(read.problems).toEqual({});
  });

  it("is not a selection for a value that cannot be valid, and says which key", () => {
    const read = readSelection({ ...FULL, application: "a b", file: "../x.yml" });
    expect(read.selection).toBeNull();
    expect(Object.keys(read.problems).sort()).toEqual(["application", "file"]);
  });

  it("never lets a hostile value become a selection", () => {
    for (const hostile of ["javascript:alert(1)", "<img src=x onerror=alert(1)>", "a‮b", "x".repeat(600)]) {
      expect(readSelection({ ...FULL, application: hostile }).selection, hostile).toBeNull();
    }
  });
});

describe("comparing two selections", () => {
  it("is the same when every part is the same", () => {
    expect(sameSelection(FULL, { ...FULL })).toBe(true);
    expect(sameSelection(null, null)).toBe(true);
  });
  it("differs by any part, and by null against a selection", () => {
    for (const part of ["file", "application", "tenant"] as const) expect(sameSelection(FULL, { ...FULL, [part]: "other" })).toBe(false);
    expect(sameSelection(FULL, { ...FULL, channel: null })).toBe(false);
    expect(sameSelection(FULL, null)).toBe(false);
  });
});

describe("the link to a selection", () => {
  const selection: ServedSelection = FULL;
  it("is the address of the view, with the keys the hub's own link uses (application, tenant, file), and the channel when there is one", () => {
    expect(servedHref("sitb", selection)).toBe("/swimlanes/sitb/served?application=tx-infinity-core&channel=beta&file=tx-infinity-api%2Ftx-infinity-core-sit7.yml&tenant=sit7");
  });
  it("leaves the channel out when there is none", () => {
    expect(servedHref("sitb", { ...selection, channel: null })).toBe("/swimlanes/sitb/served?application=tx-infinity-core&file=tx-infinity-api%2Ftx-infinity-core-sit7.yml&tenant=sit7");
  });
  it("reads back to the selection it was made from", () => {
    const href = servedHref("sitb", selection);
    const params = new URL(href, "http://hub.test").searchParams;
    expect(readSelection(Object.fromEntries(params)).selection).toEqual(selection);
  });
});

describe("the placeholders that were left in the text", () => {
  it("shows them all when there are few", () => {
    expect(placeholdersShown(["a", "b"], 12)).toEqual({ shown: ["a", "b"], more: 0 });
  });
  it("shows the first ones and counts the rest, so ten thousand do not become ten thousand elements", () => {
    const many = Array.from({ length: 10_000 }, (_, i) => `k${i}`);
    const out = placeholdersShown(many, 12);
    expect(out.shown).toHaveLength(12);
    expect(out.shown[0]).toBe("k0");
    expect(out.more).toBe(9_988);
  });
  it("shows each once", () => {
    expect(placeholdersShown(["a", "a", "b"], 12)).toEqual({ shown: ["a", "b"], more: 0 });
  });
});

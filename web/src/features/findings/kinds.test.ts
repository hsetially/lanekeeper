import { describe, expect, it } from "vitest";
import { FindingKind, ValidationIssueKind } from "@/api/generated/model";
import { KINDS, KIND_INFO, codeOf, kindOfCode, nameOf } from "./kinds";

// The twelve kinds of finding of the contract (checks C1 to C12), each with its code, its name, what it means and what to do.
// This is words and nothing else: whether a finding exists, and how severe it is, is the server's.

describe("the kinds of finding", () => {
  it("are the contract's twelve, C1 to C12 in order, with nothing added and nothing missing", () => {
    expect(KINDS).toEqual(Object.values(FindingKind));
    expect(KINDS).toHaveLength(12);
    expect(KINDS.map(codeOf)).toEqual(["C1", "C2", "C3", "C4", "C5", "C6", "C7", "C8", "C9", "C10", "C11", "C12"]);
    expect(Object.keys(KIND_INFO).sort()).toEqual([...KINDS].sort());
  });

  it("do not include the file view's redaction marker, which is a validation issue and not a check", () => {
    expect(KINDS as readonly string[]).not.toContain(ValidationIssueKind.redaction_marker);
  });

  it("have a name of their own, a meaning and at least one next step each", () => {
    const names = KINDS.map(nameOf);
    expect(new Set(names).size).toBe(names.length);
    for (const kind of KINDS) {
      const info = KIND_INFO[kind];
      expect(info.name.trim(), kind).not.toBe("");
      expect(info.meaning.trim().length, kind).toBeGreaterThan(30);
      expect(info.steps.length, kind).toBeGreaterThan(0);
      for (const step of info.steps) expect(step.trim(), kind).not.toBe("");
    }
  });

  it("never use the old model's bare 'pending restart', and never point at an action this screen does not have", () => {
    for (const kind of KINDS) {
      const text = [KIND_INFO[kind].meaning, ...KIND_INFO[kind].steps].join(" ");
      expect(text, kind).not.toMatch(/pending restart/i);
      expect(text, kind).not.toMatch(/mark(ed)? (it )?intentional|restore (the )?lost version|restore version/i);
    }
  });

  it("find a kind from its code in either case, and from nothing else", () => {
    expect(kindOfCode("C8")).toBe(FindingKind.duplicate_keys);
    expect(kindOfCode("c11")).toBe(FindingKind.config_server_restart_required);
    expect(kindOfCode("C12")).toBe(FindingKind.served_mismatch);
    for (const bad of ["", "C0", "C13", "8", "C 8", "C8 ", "duplicate_keys"]) expect(kindOfCode(bad), bad).toBeUndefined();
  });

  it("offer the diff only where Git is what the file is compared with, and the history only where a version was lost", () => {
    const withDiff = KINDS.filter((k) => KIND_INFO[k].opens.includes("diff"));
    const withHistory = KINDS.filter((k) => KIND_INFO[k].opens.includes("history"));
    expect(withDiff).toEqual([FindingKind.drift]);
    expect(withHistory).toEqual([FindingKind.overwritten_nfs_change]);
  });
});

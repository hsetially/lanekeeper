import { describe, expect, it } from "vitest";
import { Severity } from "@/api/generated/model";
import { buildMatch, changeLabel, checkRuleGlob, DEFAULT_PRIORITY, findingLabel, matchesEverything, matchParts, notRatedYet, SEVERITY_CHOICES, severityLabel } from "./severity-model";

// What a severity rule accepts before it is sent, and how its conditions read in a list. The facts about severity (which rules
// match, what the result is) are the engine's and the registry's: nothing here rates anything.

describe("the choices", () => {
  it("offers the four severities of the contract, the most serious first", () => {
    expect(SEVERITY_CHOICES).toEqual([Severity.critical, Severity.high, Severity.medium, Severity.low]);
    expect(SEVERITY_CHOICES.map(severityLabel)).toEqual(["Critical", "High", "Medium", "Low"]);
  });

  it("names a finding type by its check, and a change type in words", () => {
    expect(findingLabel("drift")).toBe("C5 NFS and Git disagree");
    expect(findingLabel("served_mismatch")).toMatch(/^C12 /);
    expect(changeLabel("added")).toBe("Setting added");
    expect(changeLabel("type_changed")).toBe("Setting changed type");
  });

  it("sends a priority that the hub does not use: with a maximum, the order of the rules cannot matter (D103)", () => {
    expect(DEFAULT_PRIORITY).toBeGreaterThanOrEqual(0);
    expect(DEFAULT_PRIORITY).toBeLessThanOrEqual(10_000);
  });
});

describe("checkRuleGlob", () => {
  it.each(["security/**", "**/*.yml", "services/*/application-*.yml", "db.{url,user}", "[ab]c/*", "a b/c"])("accepts %s", (glob) => {
    expect(checkRuleGlob(glob)).toBeNull();
  });

  it("takes an empty glob as a condition that is not stated", () => {
    expect(checkRuleGlob("")).toBeNull();
    expect(checkRuleGlob("   ")).toBeNull();
  });

  it.each([
    ["a leading slash, because a path never starts with one so the rule would match nothing", "/security/**", /no leading \//],
    ["a backslash", "a\\b", /backslashes or control characters/],
    ["a control character", "a\u0007b", /backslashes or control characters/],
    ["a right-to-left override", "a‮b", /backslashes or control characters/],
    ["an unclosed class", "a[bc", /unclosed \[/],
    ["an unclosed group", "a{b,c", /unclosed \{/],
  ])("refuses %s", (_what, glob, message) => {
    expect(checkRuleGlob(glob)).toMatch(message);
  });

  it("counts bytes, as the hub does: 256 are fine and 257 are not, and an accented letter is two", () => {
    expect(checkRuleGlob("a".repeat(256))).toBeNull();
    expect(checkRuleGlob("a".repeat(257))).toBe("At most 256 bytes");
    expect(checkRuleGlob("é".repeat(129))).toBe("At most 256 bytes");
  });

  it("does not repeat the glob in its message", () => {
    expect(checkRuleGlob("/very-secret-path/**")).not.toContain("very-secret-path");
  });
});

describe("buildMatch", () => {
  it("leaves out every condition that is not stated, so that the body holds only what the rule says", () => {
    expect(buildMatch({ findingKinds: [], changeKinds: [], pathGlob: "", settingGlob: "  " })).toEqual({});
    expect(buildMatch({ findingKinds: ["drift"], changeKinds: ["added", "removed"], pathGlob: " security/** ", settingGlob: "db.*" })).toEqual({
      finding_kinds: ["drift"],
      change_kinds: ["added", "removed"],
      path_glob: "security/**",
      setting_glob: "db.*",
    });
  });
});

describe("matchParts", () => {
  it("reads each stated condition, with the checks by their code", () => {
    expect(matchParts({ finding_kinds: ["environment_host_mismatch", "drift"], path_glob: "services/**" })).toEqual([
      { label: "Findings", value: "C6, C5", glob: false },
      { label: "Path", value: "services/**", glob: true },
    ]);
    expect(matchParts({ change_kinds: ["removed"], setting_glob: "db.*" })).toEqual([
      { label: "Changes", value: "Setting removed", glob: false },
      { label: "Setting", value: "db.*", glob: true },
    ]);
  });

  it("reads a null glob and an empty list as not stated", () => {
    expect(matchParts({ path_glob: null, setting_glob: null, finding_kinds: [], change_kinds: [] })).toEqual([]);
  });
});

describe("matchesEverything", () => {
  it("is true for a rule that states no condition, as the engine says, and false once it states one", () => {
    expect(matchesEverything({})).toBe(true);
    expect(matchesEverything({ path_glob: null, finding_kinds: [] })).toBe(true);
    expect(matchesEverything({ path_glob: "a/**" })).toBe(false);
    expect(matchesEverything({ finding_kinds: ["drift"] })).toBe(false);
  });
});

describe("notRatedYet", () => {
  it("is true for a rule with a change type or a setting glob, which the registry does not look at when it rates a file or a finding (D103)", () => {
    expect(notRatedYet({ change_kinds: ["added"] })).toBe(true);
    expect(notRatedYet({ setting_glob: "db.*" })).toBe(true);
    expect(notRatedYet({ path_glob: "a/**", finding_kinds: ["drift"] })).toBe(false);
    expect(notRatedYet({ change_kinds: [], setting_glob: null })).toBe(false);
  });
});

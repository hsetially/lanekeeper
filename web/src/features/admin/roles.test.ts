import { describe, expect, it } from "vitest";
import { Role } from "@/api/generated/model";
import { IDS, people, person } from "@/mocks/design/admin/fixtures";
import { activeAdmins, ROLE_CHOICES, roleNote, roleWord, wouldLeaveNoAdmin } from "./roles";

describe("roles", () => {
  it("lists the roles in the contract's order, lowest first", () => {
    expect(ROLE_CHOICES).toEqual(["viewer", "editor", "operator", "admin"]);
  });

  it("has a sentence for every role, and none that promises more than the decisions say", () => {
    for (const role of ROLE_CHOICES) expect(roleNote(role), role).toMatch(new RegExp(`^${role[0]?.toUpperCase() ?? ""}${role.slice(1)} can `));
    expect(roleNote(Role.operator)).toMatch(/restart and notify services/);
    expect(roleNote(Role.admin)).toMatch(/decide proposals, change roles and edit every swimlane/);
  });

  it("says a missing role as such", () => {
    expect(roleWord(null)).toBe("no role");
    expect(roleWord(Role.operator)).toBe("Operator");
  });
});

describe("the last active admin", () => {
  it("counts only active admins", () => {
    expect(activeAdmins(people())).toBe(2);
    expect(activeAdmins([person(IDS.amir, "A", { role: "admin", status: "disabled" })])).toBe(0);
  });

  it("is not left alone: a change that removes every active admin is seen", () => {
    const two = people();
    expect(wouldLeaveNoAdmin(two, IDS.amir, { role: "editor" })).toBe(false);
    const one = two.map((u) => (u.id === IDS.priya ? { ...u, role: "editor" as const } : u));
    expect(wouldLeaveNoAdmin(one, IDS.amir, { role: "editor" })).toBe(true);
    expect(wouldLeaveNoAdmin(one, IDS.amir, { status: "disabled" })).toBe(true);
    expect(wouldLeaveNoAdmin(one, IDS.amir, { role: "operator" })).toBe(true);
    expect(wouldLeaveNoAdmin(one, IDS.lena, { role: "viewer" })).toBe(false);
  });

  it("lets a person become an admin when there was none", () => {
    expect(wouldLeaveNoAdmin([person(IDS.lena, "L", { role: "editor" })], IDS.lena, { role: "admin" })).toBe(false);
  });
});

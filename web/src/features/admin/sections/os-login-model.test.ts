import { describe, expect, it } from "vitest";
import { person } from "@/mocks/design/admin/fixtures";
import { checkOsLogin, indexUsers, MAX_OS_LOGIN, pathSegment, userOptionText, userStatusNote } from "./os-login-model";

// What an OS Login mapping accepts before it is sent, and how a user is named beside one.

describe("checkOsLogin", () => {
  it.each(["dana_okoye_corp_example", "ext_ada_example_com", "sa_1234567890", "a.b-c@d+e", "_svc", "A1"])("accepts %s", (name) => {
    expect(checkOsLogin(name)).toBeNull();
  });

  it("asks for a name", () => {
    expect(checkOsLogin("")).toBe("Enter the OS Login username");
  });

  it.each([
    ["a space", "dana okoye"],
    ["a slash, which would change the address it is sent to", "a/b"],
    ["dots as a path", "../users"],
    ["a question mark", "a?x=1"],
    ["a percent sign", "a%2Fb"],
    ["a hash", "a#b"],
    ["a backslash", "a\\b"],
    ["a newline", "a\nb"],
    ["a right-to-left override", "ab‮cd"],
    ["a zero-width space", "ab​cd"],
    ["a name that begins with a dot", ".hidden"],
    ["a name that begins with a dash", "-rf"],
    ["a letter outside ASCII", "dänä"],
  ])("refuses %s", (_what, name) => {
    expect(checkOsLogin(name)).toMatch(/^Use the name as the VM records it/);
  });

  it("refuses a name of more than the contract's 256 characters, and accepts 256", () => {
    expect(checkOsLogin("a".repeat(MAX_OS_LOGIN))).toBeNull();
    expect(checkOsLogin("a".repeat(MAX_OS_LOGIN + 1))).toBe("At most 256 characters");
  });

  it("does not repeat the value in its message", () => {
    expect(checkOsLogin("secret value")).not.toContain("secret value");
  });
});

describe("pathSegment", () => {
  it("encodes a name for the address, so that no name can change which mapping is addressed", () => {
    expect(pathSegment("dana_okoye")).toBe("dana_okoye");
    expect(pathSegment("a/../../users")).toBe("a%2F..%2F..%2Fusers");
    expect(pathSegment("a?b#c%d")).toBe("a%3Fb%23c%25d");
  });
});

describe("indexUsers", () => {
  it("finds a user by id", () => {
    const a = person("11111111-1111-4111-8111-111111111111:aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa", "Ada");
    const index = indexUsers([a]);
    expect(index.get(a.id)).toBe(a);
    expect(index.get("nobody")).toBeUndefined();
  });
});

describe("userStatusNote", () => {
  it("says nothing about an active user, and names the other two states", () => {
    expect(userStatusNote("active")).toBeNull();
    expect(userStatusNote("disabled")).toBe("disabled");
    expect(userStatusNote("pending")).toBe("no role yet");
  });
});

describe("userOptionText", () => {
  it("is the name, the email and the state when it is not active, as plain text with hidden characters made visible", () => {
    expect(userOptionText(person("u", "Ines Costa", { status: "disabled" }))).toBe("Ines Costa · ines.costa@corp.example (disabled)");
    expect(userOptionText(person("u", "Priya Shah"))).toBe("Priya Shah · priya.shah@corp.example");
    expect(userOptionText(person("u", "Pri​ya"))).toContain("<U+200B>");
  });
});

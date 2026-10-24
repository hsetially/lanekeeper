import { describe, expect, it } from "vitest";
import { forAll } from "@/test/prng";
import { validateNext, validateRequestId } from "./params";

describe("validateNext (where to go after sign-in)", () => {
  it.each(["/swimlanes", "/swimlanes/sit1/files?path=a%2Fb.yml&line=42", "/compare?lanes=sit,sitb&only=diff", "/", "/approvals/17", "/search?q=a%20b#x"])("accepts %s", (value) => {
    expect(validateNext(value)).toBe(value);
  });

  it.each([
    "https://evil.example/",
    "//evil.example/",
    "/\\evil.example",
    "\\\\evil.example",
    "javascript:alert(1)",
    "swimlanes",
    "",
    "/swimlanes\n/x",
    "/swimlanes\t",
    "/a b",
    "/%2F%2Fevil.example/",
    "/%2f/evil.example",
    "/%5Cevil.example",
    "/.//evil.example/",
    "/./",
    "/a/../b",
    "/a/%2e%2e/b",
    "/a//b",
    "/\u202eevil",
    "/\u00e9",
    `/${"a".repeat(600)}`,
    "/signin",
    "/signin?next=/swimlanes",
    "/auth/login",
    "/access-requested",
  ])("refuses %j", (value) => {
    expect(validateNext(value)).toBeUndefined();
  });

  it("refuses what is not a string", () => {
    for (const bad of [undefined, null, 1, {}, ["/a"], true]) expect(validateNext(bad)).toBeUndefined();
  });

  it("never lets through a value that leaves this origin, however it is built (property)", () => {
    const origin = "https://lanekeeper.example";
    const alphabet = "/\\.%2fF5cC?#&=:@a-z0-9 \n\t\u0000\u202e\u00e9";
    forAll(
      50_000,
      (rng) => rng.string(alphabet, 24),
      (value) => {
        const accepted = validateNext(value);
        if (accepted === undefined) return;
        for (const candidate of [accepted, safeDecode(accepted)]) {
          expect(new URL(candidate, origin).origin, candidate).toBe(origin);
        }
        expect(accepted.startsWith("/")).toBe(true);
        expect(accepted.length).toBeLessThanOrEqual(512);
        expect(Array.from(accepted).some((c) => c === "\\" || c.charCodeAt(0) < 0x20 || c.charCodeAt(0) === 0x7f)).toBe(false);
      },
    );
  });
});

function safeDecode(value: string): string {
  try {
    return decodeURIComponent(value);
  } catch {
    return value;
  }
}

describe("validateRequestId (the req of the error screens)", () => {
  it("accepts a short token", () => {
    expect(validateRequestId("d1c7b8cb0c941473")).toBe("d1c7b8cb0c941473");
    expect(validateRequestId("req_abc.DEF-123")).toBe("req_abc.DEF-123");
  });
  it("refuses anything else", () => {
    for (const bad of ["", "a b", "<script>", "x".repeat(129), "a/b", "a\nb", undefined, null, 5, "\u00e9"]) expect(validateRequestId(bad)).toBeUndefined();
  });
  it("only ever returns a token (property)", () => {
    forAll(
      20_000,
      (rng) => rng.string("ab-_.9 <>&\"'\\/%\n", 20),
      (value) => {
        const accepted = validateRequestId(value);
        if (accepted !== undefined) expect(accepted).toMatch(/^[A-Za-z0-9._-]{1,128}$/);
      },
    );
  });
});

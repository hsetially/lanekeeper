import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { parse } from "yaml";
import { ACCESS_DISABLED_PATH, ACCESS_PENDING_PATH, AUTH_PREFIX, API_PREFIX, EVENTS_PATH, LOGIN_PATH, LOGOUT_PATH } from "./constants";
import { CSRF_COOKIE, CSRF_HEADER } from "@/lib/csrf";

// The values the app shares with the hub are read back from the hub's source and from the contract, so a rename on either
// side fails here and not for a user. (The web image build copies only web/, so these tests run in CI and in verify-08.)
const rust = (path: string): string => readFileSync(new URL(`../../../${path}`, import.meta.url), "utf8");

function rustConst(source: string, name: string): string {
  const match = new RegExp(`pub const ${name}: &str = "([^"]*)";`).exec(source);
  if (!match?.[1]) throw new Error(`${name} not found`);
  return match[1];
}

describe("constants the hub owns", () => {
  const cookie = rust("crates/hub-identity/src/session/cookie.rs");
  const authRoutes = rust("crates/hub-identity/src/routes/auth.rs");

  it("the CSRF cookie and header names", () => {
    expect(CSRF_COOKIE).toBe(rustConst(cookie, "CSRF_COOKIE"));
    expect(CSRF_HEADER.toLowerCase()).toBe(rustConst(cookie, "CSRF_HEADER"));
  });

  it("the paths the sign-in callback redirects to", () => {
    expect(ACCESS_PENDING_PATH).toBe(rustConst(authRoutes, "ACCESS_PENDING_PATH"));
    expect(ACCESS_DISABLED_PATH).toBe(rustConst(authRoutes, "ACCESS_DISABLED_PATH"));
  });
});

describe("paths the contract owns", () => {
  const doc = parse(rust("api/openapi.yaml")) as { paths: Record<string, Record<string, { operationId?: string }>> };

  it("the events stream, sign-in and sign-out", () => {
    expect(doc.paths[EVENTS_PATH]?.["get"]?.operationId).toBe("streamEvents");
    expect(doc.paths[LOGIN_PATH]?.["get"]?.operationId).toBe("authLogin");
    expect(doc.paths[LOGOUT_PATH]?.["post"]?.operationId).toBe("authLogout");
  });

  it("every path of the contract is under a prefix apiFetch accepts (the webhook is the one that is not, and is not the app's)", () => {
    const outside = Object.keys(doc.paths).filter((p) => !p.startsWith(API_PREFIX) && !p.startsWith(AUTH_PREFIX));
    expect(outside).toEqual(["/hooks/github"]);
  });
});

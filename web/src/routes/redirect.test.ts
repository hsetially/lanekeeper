import { describe, expect, it } from "vitest";
import { redirectHref } from "./redirect";

// A route that only redirects (an alias, or a link the MCP tools send) can carry the visitor's search keys to the screen that
// answers it, renamed, and can add keys of its own. What it carries is what the redirecting route READ: a key it does not name
// never reaches the target.

describe("redirectHref", () => {
  it("fills the target's path parameters from the route's own", () => {
    expect(redirectHref({ to: "/approvals/:id" }, { id: "17" }, {})).toBe("/approvals/17");
    expect(redirectHref({ to: "/swimlanes/:id/services" }, { id: "sit 1" }, {})).toBe("/swimlanes/sit%201/services");
  });

  it("writes the keys the redirect adds, and carries the visitor's keys under the names the target reads", () => {
    const redirect = { to: "/search", with: { type: "docs" }, carry: { q: "q" } };
    expect(redirectHref(redirect, {}, { q: "server.port" })).toBe("/search?q=server.port&type=docs");
    expect(redirectHref({ to: "/compare", carry: { file: "file", swimlanes: "lanes" } }, {}, { file: "a/b.yml", swimlanes: "sitb,sitc" })).toBe("/compare?file=a%2Fb.yml&lanes=sitb%2Csitc");
  });

  it("leaves out a key the visitor did not send, or sent empty", () => {
    const redirect = { to: "/compare", carry: { file: "file", swimlanes: "lanes" } };
    expect(redirectHref(redirect, {}, { file: "a.yml" })).toBe("/compare?file=a.yml");
    expect(redirectHref(redirect, {}, { file: "", swimlanes: undefined })).toBe("/compare");
  });

  it("lets the redirect's own key win over a carried one with the same target name", () => {
    expect(redirectHref({ to: "/search", with: { type: "docs" }, carry: { type: "type" } }, {}, { type: "text" })).toBe("/search?type=docs");
  });

  it("is a relative path every time: nothing the visitor sent can turn it into another origin", () => {
    const href = redirectHref({ to: "/search", carry: { q: "q" } }, {}, { q: "//evil.example/" });
    expect(href.startsWith("/search?")).toBe(true);
    expect(href).not.toContain("//evil");
    expect(redirectHref({ to: "/approvals/:id" }, { id: "//evil.example" }, {})).toBe("/approvals/%2F%2Fevil.example");
  });
});

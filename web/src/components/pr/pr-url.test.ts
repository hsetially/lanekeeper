import { describe, expect, it } from "vitest";
import { prAddress } from "./pr-url";

// A pull request's address comes from the server (`PrLink.pr_url`, `PrJob.pr_url`) and is opened as a link: it is untrusted. It is
// a link only when it is an https address with a host and no credentials, and what is drawn, opened and copied is one string.

describe("prAddress", () => {
  it("accepts a GitHub pull request and reads its number", () => {
    expect(prAddress("https://github.com/csp/csp-tenant-data/pull/418")).toEqual({
      href: "https://github.com/csp/csp-tenant-data/pull/418",
      display: "github.com/csp/csp-tenant-data/pull/418",
      number: 418,
    });
  });

  it("accepts a host of GitHub Enterprise, with a port, and a trailing slash", () => {
    expect(prAddress("https://git.example.test:8443/csp/x/pull/7/")).toMatchObject({ href: "https://git.example.test:8443/csp/x/pull/7/", number: 7 });
  });

  it("gives no number to an address that is not a pull request", () => {
    expect(prAddress("https://github.com/csp/x/issues/3")).toMatchObject({ number: null });
    expect(prAddress("https://github.com/csp/x/pull/abc")).toMatchObject({ number: null });
    expect(prAddress("https://github.com/csp/x/pull/1234567890")).toMatchObject({ number: null });
  });

  it.each([
    ["http", "http://github.com/csp/x/pull/1"],
    ["a script", "javascript:alert(1)"],
    ["a data URL", "data:text/html,<b>x</b>"],
    ["a relative path", "/csp/x/pull/1"],
    ["a protocol-relative address", "//github.com/csp/x/pull/1"],
    ["credentials in the address", "https://user:secret@github.com/csp/x/pull/1"],
    ["a user name that looks like a host", "https://github.com@evil.example.test/csp/x/pull/1"],
    ["no host", "https:///csp/x/pull/1"],
    ["a space", "https://github.com/csp/x/pull/1 2"],
    ["a line break", "https://github.com/csp/x/pull/1\nhttps://evil.example.test"],
    ["a backslash (read as a slash by a browser)", "https://github.com\\evil.example.test/pull/1"],
    ["a tab", "https://github.com/\tcsp/x/pull/1"],
    ["more than the 512 characters the contract allows", `https://github.com/${"a".repeat(600)}`],
    ["an empty string", ""],
  ])("refuses %s", (_name, value) => {
    expect(prAddress(value)).toBeNull();
  });

  it("draws the address the browser would open: a bidi control in the path is encoded, not shown", () => {
    const found = prAddress("https://github.com/csp/x/pull/1/‮");
    expect(found).not.toBeNull();
    expect(found?.href).not.toContain("‮");
    expect(found?.display).not.toContain("‮");
  });
});

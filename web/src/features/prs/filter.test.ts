import { describe, expect, it } from "vitest";
import { FILTERS, filterOf, listParams } from "./filter";

// The state filter of the Pull requests page is one search key, `state`. What the URL says is untrusted text: only the three states
// the contract has and "all" mean anything, and anything else is the default, never a request the server refuses.

describe("filterOf", () => {
  it("reads the four filters", () => {
    for (const filter of FILTERS) expect(filterOf(filter)).toBe(filter);
  });

  it("defaults to open: the pull requests that are waiting for something", () => {
    expect(filterOf(undefined)).toBe("open");
    expect(filterOf("")).toBe("open");
  });

  it.each(["OPEN", "draft", "open,merged", "merged ", "__proto__", "constructor", "<script>", "0", "all\n"])("reads %j as the default", (value) => {
    expect(filterOf(value)).toBe("open");
  });
});

describe("listParams", () => {
  it("asks for the swimlane's links in the state, 100 at a time", () => {
    expect(listParams("sitb", "merged")).toEqual({ swimlane: "sitb", state: "merged", limit: 100 });
    expect(listParams("sitb", "open")).toEqual({ swimlane: "sitb", state: "open", limit: 100 });
    expect(listParams("sitb", "closed")).toEqual({ swimlane: "sitb", state: "closed", limit: 100 });
  });

  it("asks for no state at all for 'all'", () => {
    expect(listParams("sitb", "all")).toEqual({ swimlane: "sitb", limit: 100 });
    expect("state" in listParams("sitb", "all")).toBe(false);
  });
});

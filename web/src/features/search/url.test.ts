import { describe, expect, it } from "vitest";
import { MAX_LANES, NO_LANES, readSearch, toSearch, type SearchParams } from "./url";

// The address of the Search screen is its state: the tab, the query, the two toggles, the swimlanes and the glob. It is read
// from what the router kept (strings only) and checked here, because it is untrusted input, and written back with the keys the
// route reads and nothing else.

const base: SearchParams = { tab: "text", q: "", caseSensitive: false, regex: false, lanes: null, glob: "" };

describe("readSearch", () => {
  it("defaults to the Text tab with nothing asked", () => {
    expect(readSearch({})).toEqual(base);
  });

  it("reads the tab from `type`, and from `mode` where a link from the MCP tools names it", () => {
    expect(readSearch({ type: "settings" }).tab).toBe("settings");
    expect(readSearch({ type: "docs" }).tab).toBe("docs");
    expect(readSearch({ mode: "settings" }).tab).toBe("settings");
    expect(readSearch({ type: "docs", mode: "settings" }).tab).toBe("docs");
  });

  it("falls back to Text for a tab it does not know", () => {
    expect(readSearch({ type: "files" }).tab).toBe("text");
    expect(readSearch({ type: "__proto__" }).tab).toBe("text");
  });

  it("reads the query as it is written, spaces included", () => {
    expect(readSearch({ q: "  enable.*Sorting " }).q).toBe("  enable.*Sorting ");
  });

  it("reads the toggles from 1", () => {
    expect(readSearch({ cs: "1", rx: "1" })).toMatchObject({ caseSensitive: true, regex: true });
    expect(readSearch({ cs: "0", rx: "yes" })).toMatchObject({ caseSensitive: false, regex: false });
  });

  it("reads the swimlanes as a list of ids, dropping what is not an id and what repeats", () => {
    expect(readSearch({ lanes: "sit,sitb, presita,sit,../x,UPPER,,a b" }).lanes).toEqual(["sit", "sitb", "presita"]);
  });

  it("reads `-` as no swimlane at all, and nothing usable as all of them", () => {
    expect(readSearch({ lanes: NO_LANES }).lanes).toEqual([]);
    expect(readSearch({ lanes: "" }).lanes).toBeNull();
    expect(readSearch({ lanes: "../x,UPPER" }).lanes).toBeNull();
  });

  it("keeps at most as many swimlanes as the API takes", () => {
    const many = Array.from({ length: 100 }, (_, i) => `lane${i}`).join(",");
    expect(readSearch({ lanes: many }).lanes).toHaveLength(MAX_LANES);
  });

  it("reads the glob", () => {
    expect(readSearch({ glob: "**/*-sit7.yml" }).glob).toBe("**/*-sit7.yml");
  });
});

describe("toSearch", () => {
  it("writes nothing for a default, but always the tab", () => {
    expect(toSearch(base)).toEqual({ type: "text" });
    expect(toSearch({ ...base, tab: "docs" })).toEqual({ type: "docs" });
  });

  it("writes what is set, and only that", () => {
    expect(toSearch({ tab: "text", q: "timeout", caseSensitive: true, regex: true, lanes: ["sit", "sitb"], glob: "**/*.yml" })).toEqual({
      type: "text",
      q: "timeout",
      cs: "1",
      rx: "1",
      lanes: "sit,sitb",
      glob: "**/*.yml",
    });
  });

  it("writes `-` for no swimlane", () => {
    expect(toSearch({ ...base, lanes: [] }).lanes).toBe(NO_LANES);
  });

  it("reads back what it wrote", () => {
    const params: SearchParams = { tab: "settings", q: "enableAccountSorting", caseSensitive: true, regex: false, lanes: ["sitb"], glob: "" };
    expect(readSearch(toSearch(params))).toEqual(params);
    expect(readSearch(toSearch({ ...params, lanes: [] }))).toEqual({ ...params, lanes: [] });
  });
});

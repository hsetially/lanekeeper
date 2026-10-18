import { describe, expect, it } from "vitest";
import type { TextHit } from "@/api/generated/model";
import { docHref, fileParts, focusWindow, groupTextHits, hitSegments, termSegments, snippetRuns, snippetSegments, pluralise } from "./results";

// What the screen makes of the server's answers: the lines cut at the ranges the server gave (UTF-8 byte offsets, F10), the
// hits grouped in the order the server sent them, and the few words of copy that depend on a count. Untrusted text is only ever
// split into pieces here, never interpreted.

const hit = (over: Partial<TextHit> & Pick<TextHit, "swimlane" | "path" | "line">): TextHit => ({ text: "x", ranges: [], ...over });

describe("hitSegments", () => {
  it("cuts the line at the ranges of the server, which are bytes", () => {
    expect(hitSegments("enableAccountSorting: true", [[0, 6]])).toEqual([
      { text: "enable", mark: true },
      { text: "AccountSorting: true", mark: false },
    ]);
  });

  it("counts bytes, not characters: a character before the match is more than one byte", () => {
    // "é" is two bytes, so the match "ab" starts at byte 3.
    expect(hitSegments("é ab", [[3, 5]])).toEqual([
      { text: "é ", mark: false },
      { text: "ab", mark: true },
    ]);
  });

  it("handles a match after an emoji, which is four bytes and two UTF-16 units", () => {
    expect(hitSegments("😀 hit", [[5, 8]])).toEqual([
      { text: "😀 ", mark: false },
      { text: "hit", mark: true },
    ]);
  });

  it("drops a range that is not on a character boundary, and shows the line whole", () => {
    expect(hitSegments("é ab", [[1, 3]])).toEqual([{ text: "é ab", mark: false }]);
  });

  it("shows a line with no ranges as it is", () => {
    expect(hitSegments("plain", [])).toEqual([{ text: "plain", mark: false }]);
    expect(hitSegments("", [])).toEqual([]);
  });

  it("marks two matches and merges ranges that touch", () => {
    expect(hitSegments("abcabc", [[0, 2], [2, 3], [3, 5]])).toEqual([
      { text: "abcab", mark: true },
      { text: "c", mark: false },
    ]);
  });
});

describe("focusWindow", () => {
  it("leaves a short line alone", () => {
    expect(focusWindow("short line", [[0, 5]], 60)).toEqual({ text: "short line", ranges: [[0, 5]], cutBefore: false, cutAfter: false });
  });

  it("starts a long line a little before the first match, so the match is on screen", () => {
    const line = `${"a".repeat(200)}needle${"b".repeat(200)}`;
    const out = focusWindow(line, [[200, 206]], 40);
    expect(out.cutBefore).toBe(true);
    expect(out.cutAfter).toBe(true);
    expect(out.text.slice(out.ranges[0]?.[0], out.ranges[0]?.[1])).toBe("needle");
    expect(out.text.indexOf("needle")).toBeLessThanOrEqual(40);
    expect(out.text.length).toBeLessThanOrEqual(240);
  });

  it("does not cut a surrogate pair in two", () => {
    const line = `${"😀".repeat(100)}needle`;
    const at = "😀".repeat(100).length;
    const out = focusWindow(line, [[at, at + 6]], 5);
    expect(out.text.startsWith("\udc00")).toBe(false);
    expect(Array.from(out.text).join("")).toBe(out.text);
  });
});

describe("termSegments", () => {
  it("marks every occurrence of the query, ignoring case", () => {
    expect(termSegments("remote-itm-teller.EnableAccountSorting", ["accountsorting"])).toEqual([
      { text: "remote-itm-teller.Enable", mark: false },
      { text: "AccountSorting", mark: true },
    ]);
  });

  it("marks each word of a query", () => {
    expect(termSegments("Account sorting orders accounts", ["account", "sorting"]).filter((s) => s.mark).map((s) => s.text)).toEqual(["Account", "sorting", "account"]);
  });

  it("treats the query as plain text: a pattern character does not match anything but itself", () => {
    expect(termSegments("a.b and axb", ["a.b"]).filter((s) => s.mark).map((s) => s.text)).toEqual(["a.b"]);
    expect(termSegments("(x", ["("]).filter((s) => s.mark).map((s) => s.text)).toEqual(["("]);
  });

  it("returns the text whole when there is nothing to mark", () => {
    expect(termSegments("abc", [])).toEqual([{ text: "abc", mark: false }]);
    expect(termSegments("abc", [""])).toEqual([{ text: "abc", mark: false }]);
    expect(termSegments("abc", ["zzz"])).toEqual([{ text: "abc", mark: false }]);
  });
});

describe("snippetSegments", () => {
  it("shows the text between backticks as code, and marks the words in both", () => {
    const parts = snippetSegments("`enableAccountSorting` turns on sorting", ["sorting"]);
    expect(parts[0]).toMatchObject({ code: true });
    expect(parts.map((p) => p.text).join("")).toBe("enableAccountSorting turns on sorting");
    expect(parts.filter((p) => p.mark).map((p) => p.text)).toEqual(["Sorting", "sorting"]);
  });

  it("leaves an unpaired backtick as it is", () => {
    expect(snippetSegments("a ` b", []).map((p) => p.text).join("")).toBe("a ` b");
  });
});

describe("snippetRuns", () => {
  it("keeps a stretch of code whole even when a word is marked inside it", () => {
    const runs = snippetRuns("`enableAccountSorting` turns on sorting", ["account", "sorting"]);
    expect(runs[0]?.code).toBe(true);
    expect(runs[0]?.segments.map((s) => s.text).join("")).toBe("enableAccountSorting");
    expect(runs[0]?.segments.filter((s) => s.mark).map((s) => s.text)).toEqual(["Account", "Sorting"]);
    expect(runs.slice(1).map((r) => r.segments.map((s) => s.text).join("")).join("")).toBe(" turns on sorting");
  });

  it("makes two runs of code when plain text sits between them", () => {
    expect(snippetRuns("`a` and `b`", []).map((r) => [r.code, r.segments.map((s) => s.text).join("")])).toEqual([
      [true, "a"],
      [false, " and "],
      [true, "b"],
    ]);
  });
});

describe("groupTextHits", () => {
  it("groups by swimlane and then by file, in the order the server sent them", () => {
    const groups = groupTextHits([
      hit({ swimlane: "sitb", path: "a/core-sit7.yml", line: 13, tenant: "sit7" }),
      hit({ swimlane: "sitb", path: "a/core-sit7.yml", line: 20, tenant: "sit7" }),
      hit({ swimlane: "sitb", path: "a/core.yml", line: 13 }),
      hit({ swimlane: "sitc", path: "a/core.yml", line: 5 }),
    ]);
    expect(groups.map((g) => [g.swimlane, g.hits, g.files.map((f) => [f.path, f.kind, f.hits.length])])).toEqual([
      ["sitb", 3, [["a/core-sit7.yml", "tenant", 2], ["a/core.yml", "base", 1]]],
      ["sitc", 1, [["a/core.yml", "base", 1]]],
    ]);
  });

  it("gives each hit an id that is its place, so two hits of one line stay two rows", () => {
    const groups = groupTextHits([hit({ swimlane: "s", path: "p", line: 1 }), hit({ swimlane: "s", path: "p", line: 1 })]);
    const ids = groups.flatMap((g) => g.files.flatMap((f) => f.hits.map((h) => h.id)));
    expect(new Set(ids).size).toBe(2);
  });

  it("is empty for no hits", () => {
    expect(groupTextHits([])).toEqual([]);
  });
});

describe("fileParts", () => {
  it("splits a path at its last slash", () => {
    expect(fileParts("tx-infinity-api/tx-infinity-core.yml")).toEqual({ dir: "tx-infinity-api", name: "tx-infinity-core.yml" });
    expect(fileParts("core.yml")).toEqual({ dir: "", name: "core.yml" });
    expect(fileParts("a/b/c.yml")).toEqual({ dir: "a/b", name: "c.yml" });
  });
});

describe("docHref", () => {
  it("opens the reader at a path, with the heading the server named", () => {
    expect(docHref("/uploads/instant-payments.md", "doc-feature-flags")).toBe("/docs/view?path=%2Fuploads%2Finstant-payments.md#doc-feature-flags");
  });

  it("opens the doc from the top when the server named no heading, or one that is not shaped like a heading id", () => {
    expect(docHref("/uploads/a.md", null)).toBe("/docs/view?path=%2Fuploads%2Fa.md");
    expect(docHref("/uploads/a.md", undefined)).toBe("/docs/view?path=%2Fuploads%2Fa.md");
    expect(docHref("/uploads/a.md", "javascript:alert(1)")).toBe("/docs/view?path=%2Fuploads%2Fa.md");
    expect(docHref("/uploads/a.md", "doc-ok#x")).toBe("/docs/view?path=%2Fuploads%2Fa.md");
  });

  it("gives no link for a path that is not a doc path", () => {
    expect(docHref("/etc/passwd", null)).toBeUndefined();
  });
});

describe("pluralise", () => {
  it("writes one and many", () => {
    expect(pluralise(1, "match", "matches")).toBe("1 match");
    expect(pluralise(0, "match", "matches")).toBe("0 matches");
    expect(pluralise(1234, "match", "matches")).toBe("1,234 matches");
  });
});

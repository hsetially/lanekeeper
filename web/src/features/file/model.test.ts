import { describe, expect, it } from "vitest";
import type { FileContent } from "@/api/generated/model";
import { effectiveStem, encodingLabel, eolLabel, fileFacts, formatSize, imageKind, isPlausibleNfsPath, languageOf, lineCount, parseLine, parseTab, parsePanel, splitPath, unknownFileCount } from "./model";

// The file view shows what the server said about a file. These helpers do the plain work of that: where a path splits,
// which language the editor colours, how a size and a line ending are spelled, and which search values are taken at all.

const content = (over: Partial<FileContent> = {}): FileContent => ({
  path: "tx-infinity-api/tx-infinity-core-sit7.yml",
  hash: "a".repeat(64),
  class: "structured",
  format: { eol: "crlf", encoding: "utf8", bom: false },
  size: 1200,
  content_withheld: false,
  text: "a: 1\r\nb: 2\r\n",
  ...over,
});

describe("splitPath", () => {
  it("splits the folder from the name", () => {
    expect(splitPath("tx-infinity-api/tx-infinity-core-sit7.yml")).toEqual({ dir: "tx-infinity-api", name: "tx-infinity-core-sit7.yml" });
    expect(splitPath("a/b/c.yml")).toEqual({ dir: "a/b", name: "c.yml" });
  });
  it("has no folder for a file at the root", () => {
    expect(splitPath("root.yml")).toEqual({ dir: "", name: "root.yml" });
  });
});

describe("isPlausibleNfsPath", () => {
  it("accepts a relative path", () => {
    expect(isPlausibleNfsPath("tx-infinity-api/tx-infinity-core-sit7.yml")).toBe(true);
    expect(isPlausibleNfsPath("a.yml")).toBe(true);
  });
  it.each([
    ["", "empty"],
    ["/etc/passwd", "a leading slash"],
    ["a/../b", "a parent segment"],
    ["..", "only a parent segment"],
    ["a\\b", "a backslash"],
    ["a\u0000b", "a control character"],
    ["a//b", "an empty segment"],
    ["a/./b", "a current-folder segment"],
    ["x".repeat(1025), "more than 1024 characters"],
  ])("refuses %j (%s) before it is sent", (path) => {
    expect(isPlausibleNfsPath(path)).toBe(false);
  });
  it("is not fooled by a segment that merely contains dots", () => {
    expect(isPlausibleNfsPath("a/..b/c..yml")).toBe(true);
  });
});

describe("languageOf", () => {
  it.each([
    ["a.yml", "yaml"],
    ["a.YAML", "yaml"],
    ["a.xml", "xml"],
    ["a.properties", "ini"],
    ["a.json", "json"],
    ["a.txt", "plaintext"],
    ["noext", "plaintext"],
  ])("%s is %s", (name, language) => {
    expect(languageOf(name)).toBe(language);
  });
});

describe("labels", () => {
  it("spells the line ending", () => {
    expect(eolLabel("lf")).toBe("LF");
    expect(eolLabel("crlf")).toBe("CRLF");
    expect(eolLabel("mixed")).toBe("Mixed endings");
  });
  it("spells the encoding, with the BOM", () => {
    expect(encodingLabel("utf8", false)).toBe("UTF-8");
    expect(encodingLabel("utf8", true)).toBe("UTF-8 with BOM");
    expect(encodingLabel("utf16le", false)).toBe("UTF-16 LE");
    expect(encodingLabel("utf16be", true)).toBe("UTF-16 BE with BOM");
    expect(encodingLabel("latin1", false)).toBe("Latin-1");
  });
  it("spells a size", () => {
    expect(formatSize(0)).toBe("0 B");
    expect(formatSize(512)).toBe("512 B");
    expect(formatSize(3277)).toBe("3.2 KB");
    expect(formatSize(18 * 1024)).toBe("18 KB");
    expect(formatSize(5 * 1024 * 1024)).toBe("5 MB");
  });
});

describe("lineCount", () => {
  it("counts lines, not line endings", () => {
    expect(lineCount("")).toBe(0);
    expect(lineCount("a")).toBe(1);
    expect(lineCount("a\nb\n")).toBe(2);
    expect(lineCount("a\r\nb\r\nc")).toBe(3);
  });
});

describe("fileFacts", () => {
  it("names the line ending, the encoding and the lines of a text file", () => {
    expect(fileFacts(content(), "tx-infinity-core-sit7.yml").map((f) => f.text)).toEqual(["CRLF", "UTF-8", "2 lines"]);
  });
  it("counts one line in the singular", () => {
    expect(fileFacts(content({ text: "a" }), "a.yml").map((f) => f.text)).toContain("1 line");
  });
  it("groups thousands, as the design writes 6,140 lines", () => {
    const text = Array.from({ length: 6140 }, (_, i) => `k${i}: 1`).join("\n");
    expect(fileFacts(content({ text, format: { eol: "lf", encoding: "utf8", bom: false } }), "a.yml").map((f) => f.text)).toContain("6,140 lines");
  });
  it("names a binary file by its kind and size, with no line count", () => {
    const facts = fileFacts(content({ class: "binary", text: null, size: 18 * 1024, path: "r/receipt-logo.jpeg" }), "receipt-logo.jpeg").map((f) => f.text);
    expect(facts).toEqual(["Binary", "JPEG", "18 KB"]);
  });
  it("names a withheld file by its size only", () => {
    const facts = fileFacts(content({ class: "denied", text: null, content_withheld: true, size: 3277 }), "keystore.jks").map((f) => f.text);
    expect(facts).toEqual(["Binary", "3.2 KB"]);
  });
  it("marks a mixed line ending, so it is noticed", () => {
    const facts = fileFacts(content({ format: { eol: "mixed", encoding: "utf8", bom: false } }), "a.yml");
    expect(facts[0]).toEqual({ text: "Mixed endings", tone: "info" });
  });
});

describe("imageKind", () => {
  it("knows the images a browser shows from a blob", () => {
    expect(imageKind("a.jpeg")).toBe("JPEG");
    expect(imageKind("a.JPG")).toBe("JPEG");
    expect(imageKind("a.png")).toBe("PNG");
  });
  it("does not treat an SVG as an image to draw (it can carry script)", () => {
    expect(imageKind("a.svg")).toBeNull();
    expect(imageKind("a.jks")).toBeNull();
  });
});

describe("effectiveStem", () => {
  it("is the logical file's name without its extension", () => {
    expect(effectiveStem("tx-infinity-api/tx-infinity-core.yml")).toBe("tx-infinity-core");
    expect(effectiveStem("noext")).toBe("noext");
  });
});

describe("search values", () => {
  it("takes a tab it knows and nothing else", () => {
    expect(parseTab("diff")).toBe("diff");
    expect(parseTab("git")).toBe("git");
    expect(parseTab("baseline")).toBe("baseline");
    expect(parseTab("nfs")).toBe("nfs");
    expect(parseTab("<script>")).toBe("nfs");
    expect(parseTab(undefined)).toBe("nfs");
  });
  it("takes a panel it knows and nothing else", () => {
    expect(parsePanel("history")).toBe("history");
    expect(parsePanel("docs")).toBe("docs");
    expect(parsePanel("x")).toBeNull();
    expect(parsePanel(undefined)).toBeNull();
  });
  it("takes a positive whole line number", () => {
    expect(parseLine("42")).toBe(42);
    expect(parseLine("0")).toBeNull();
    expect(parseLine("-3")).toBeNull();
    expect(parseLine("4.5")).toBeNull();
    expect(parseLine("abc")).toBeNull();
    expect(parseLine("99999999999")).toBeNull();
    expect(parseLine(undefined)).toBeNull();
  });
});

describe("unknownFileCount", () => {
  it("reads the unknown count out of the drift counts", () => {
    expect(unknownFileCount([["in_sync", 80], ["unknown", 412]])).toBe(412);
  });
  it("is null when there is none, or when the number cannot be one", () => {
    expect(unknownFileCount([["in_sync", 80]])).toBeNull();
    expect(unknownFileCount(undefined)).toBeNull();
    expect(unknownFileCount([["unknown", -1]])).toBeNull();
    expect(unknownFileCount([["unknown", 1.5]])).toBeNull();
  });
});

import { describe, expect, it } from "vitest";
import { docFileName, docPathFromSlug, docSource, isDocPath, slugFromDocPath } from "./doc-path";

// A doc path is untrusted input: it comes from the address bar. The reader checks its shape before it asks the server, and
// refuses a path that is not one, so a wrong link is a "not a doc" screen and not a request.

describe("isDocPath", () => {
  it.each(["/git/main/docs/account-sorting.md", "/uploads/instant-payments.md", "/git/feature/x/docs/a.md", "/uploads/Ünï.md"])("accepts %s", (path) => {
    expect(isDocPath(path)).toBe(true);
  });

  it.each([
    ["empty", ""],
    ["not under git or uploads", "/etc/passwd"],
    ["relative", "git/main/a.md"],
    ["a dot-dot segment", "/git/main/../../x.md"],
    ["a dot segment", "/uploads/./a.md"],
    ["an empty segment", "/uploads//a.md"],
    ["a backslash", "/uploads/a\\b.md"],
    ["a control character", "/uploads/a\u0007.md"],
    ["a line break", "/uploads/a\n.md"],
    ["a NUL", "/uploads/a\u0000.md"],
    ["a bidi override", "/uploads/a‮b.md"],
    ["only the root", "/uploads/"],
    ["over 1024 characters", `/uploads/${"a".repeat(1020)}`],
  ])("refuses %s", (_name, path) => {
    expect(isDocPath(path)).toBe(false);
  });

  it("accepts exactly 1024 characters", () => {
    const path = `/uploads/${"a".repeat(1024 - "/uploads/".length)}`;
    expect(path).toHaveLength(1024);
    expect(isDocPath(path)).toBe(true);
  });
});

describe("the reader's slug", () => {
  it("is the base64url of the path, without padding, and round-trips", () => {
    for (const path of ["/uploads/a.md", "/git/main/docs/account-sorting.md", "/uploads/Ünï çødé.md"]) {
      const slug = slugFromDocPath(path);
      expect(slug).toMatch(/^[A-Za-z0-9_-]+$/);
      expect(docPathFromSlug(slug)).toBe(path);
    }
  });

  it("refuses a slug that is not the canonical encoding of a doc path", () => {
    // Plain words, as the handoff's example `account-sorting` writes them, are not an encoded path.
    expect(docPathFromSlug("account-sorting")).toBeUndefined();
    expect(docPathFromSlug("")).toBeUndefined();
    // Valid base64url of something that is not a doc path.
    expect(docPathFromSlug(slugFromDocPathUnchecked("/etc/passwd"))).toBeUndefined();
    // Valid base64url of bytes that are not UTF-8.
    expect(docPathFromSlug("_-8")).toBeUndefined();
    // A slug with padding or a character outside the alphabet.
    expect(docPathFromSlug(`${slugFromDocPath("/uploads/a.md")}=`)).toBeUndefined();
    expect(docPathFromSlug("L3VwbG9hZHMvYS5tZA+")).toBeUndefined();
  });

  it("refuses a slug whose last character carries bits the encoding does not use", () => {
    // `/uploads/a.md` is 13 bytes: the last base64 character holds two bits of data and four of padding, which must be zero.
    const good = slugFromDocPath("/uploads/a.md");
    const last = good.at(-1) ?? "";
    const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    const other = alphabet[(alphabet.indexOf(last) + 1) % 64] ?? "A";
    expect(docPathFromSlug(good.slice(0, -1) + other)).toBeUndefined();
  });

  it("does not decode percent escapes in a path (a doc link is handed on as it came)", () => {
    expect(isDocPath("/uploads/%2e%2e/x.md")).toBe(true);
    expect(docPathFromSlug(slugFromDocPath("/uploads/%2e%2e/x.md"))).toBe("/uploads/%2e%2e/x.md");
  });
});

function slugFromDocPathUnchecked(text: string): string {
  const bytes = new TextEncoder().encode(text);
  return btoa(String.fromCharCode(...bytes)).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
}

describe("docSource and docFileName", () => {
  it("reads the source from the documented prefix", () => {
    expect(docSource("/git/main/docs/a.md")).toBe("git");
    expect(docSource("/uploads/a.md")).toBe("upload");
    expect(docSource("/other/a.md")).toBeUndefined();
  });

  it("names the file by the last segment", () => {
    expect(docFileName("/git/main/docs/a.md")).toBe("a.md");
    expect(docFileName("/uploads/instant-payments.md")).toBe("instant-payments.md");
  });
});

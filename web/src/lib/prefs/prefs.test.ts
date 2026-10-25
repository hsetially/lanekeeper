import { beforeEach, describe, expect, it } from "vitest";
import { forAll } from "@/test/prng";
import { createPrefs, type PrefsStorage } from "./index";
import { THEME_COOKIE, parseThemeCookie, themeCookieValue } from "./theme";

function memory(): PrefsStorage & { data: Map<string, string> } {
  const data = new Map<string, string>();
  return {
    data,
    getItem: (k) => data.get(k) ?? null,
    setItem: (k, v) => void data.set(k, v),
    removeItem: (k) => void data.delete(k),
    key: (i) => [...data.keys()][i] ?? null,
    get length() {
      return data.size;
    },
  };
}

describe("prefs", () => {
  let storage: ReturnType<typeof memory>;
  let prefs: ReturnType<typeof createPrefs>;
  beforeEach(() => {
    storage = memory();
    prefs = createPrefs(() => storage);
  });

  it("returns the default until something is set", () => {
    expect(prefs.get("sidebarCollapsed")).toBe(false);
    expect(prefs.get("tableColumns", "findings")).toBeUndefined();
    expect(prefs.get("splitWidth", "file-tree")).toBeUndefined();
  });

  it("round-trips the typed values", () => {
    prefs.set("sidebarCollapsed", true);
    prefs.set("tableColumns", "findings", ["severity", "kind", "path"]);
    prefs.set("splitWidth", "file-tree", 320);
    expect(prefs.get("sidebarCollapsed")).toBe(true);
    expect(prefs.get("tableColumns", "findings")).toEqual(["severity", "kind", "path"]);
    expect(prefs.get("splitWidth", "file-tree")).toBe(320);
  });

  it("stores only keys with the app's prefix, so it never touches another script's data", () => {
    prefs.set("sidebarCollapsed", true);
    expect([...storage.data.keys()].every((k) => k.startsWith("lk.pref."))).toBe(true);
  });

  it("refuses a value of the wrong shape, and a value that would be content", () => {
    // @ts-expect-error the type says boolean
    prefs.set("sidebarCollapsed", "yes");
    prefs.set("tableColumns", "findings", ["a path/with/slashes.yml"]);
    // @ts-expect-error a width is a number
    prefs.set("splitWidth", "file-tree", "wide");
    expect(storage.data.size).toBe(0);
  });

  it("refuses a table or pane id that is not an identifier", () => {
    prefs.set("tableColumns", "../../secret", ["a"]);
    prefs.set("splitWidth", "x".repeat(100), 100);
    expect(storage.data.size).toBe(0);
  });

  it("ignores a stored value that has since become invalid", () => {
    storage.setItem("lk.pref.sidebarCollapsed", '"banana"');
    storage.setItem("lk.pref.tableColumns.findings", "not json");
    storage.setItem("lk.pref.splitWidth.file-tree", "1e9");
    expect(prefs.get("sidebarCollapsed")).toBe(false);
    expect(prefs.get("tableColumns", "findings")).toBeUndefined();
    expect(prefs.get("splitWidth", "file-tree")).toBeUndefined();
  });

  it("bounds a column list", () => {
    prefs.set("tableColumns", "findings", Array.from({ length: 100 }, (_, i) => `c${i}`));
    expect(prefs.get("tableColumns", "findings")).toBeUndefined();
  });

  it("keeps working in memory when storage is unavailable or throws", () => {
    const broken = createPrefs(() => {
      throw new Error("blocked");
    });
    broken.set("sidebarCollapsed", true);
    expect(broken.get("sidebarCollapsed")).toBe(true);
    const full = createPrefs(() => ({ ...memory(), setItem: () => { throw new DOMException("full", "QuotaExceededError"); } }));
    full.set("sidebarCollapsed", true);
    expect(full.get("sidebarCollapsed")).toBe(true);
  });

  it("clears only its own keys", () => {
    storage.setItem("other", "keep");
    prefs.set("sidebarCollapsed", true);
    prefs.clear();
    expect(storage.data.get("other")).toBe("keep");
    expect([...storage.data.keys()]).toEqual(["other"]);
  });

  it("never stores a string that is not one of the typed shapes (property)", () => {
    forAll(
      10_000,
      (rng) => ({ id: rng.string("abcdefghij-_/.%\u0000 ", 40), value: rng.string("abc/.\\ <>\"'", 30) }),
      ({ id, value }) => {
        const s = memory();
        const p = createPrefs(() => s);
        p.set("tableColumns", id, [value]);
        for (const [k, v] of s.data) {
          expect(k).toMatch(/^lk\.pref\.[A-Za-z]+(\.[a-z][a-z0-9-]{0,31})?$/);
          expect(v.length).toBeLessThanOrEqual(2048);
        }
      },
    );
  });
});

describe("theme cookie", () => {
  it("is named as theme-init.js expects", () => expect(THEME_COOKIE).toBe("lk_theme"));
  it("reads light, dark and system and nothing else", () => {
    expect(parseThemeCookie("a=1; lk_theme=dark")).toBe("dark");
    expect(parseThemeCookie("lk_theme=light")).toBe("light");
    expect(parseThemeCookie("lk_theme=system")).toBe("system");
    expect(parseThemeCookie("lk_theme=purple")).toBe("system");
    expect(parseThemeCookie("")).toBe("system");
  });
  it("writes a cookie that is not a session cookie and is readable by theme-init.js", () => {
    expect(themeCookieValue("dark")).toBe("lk_theme=dark; Path=/; Max-Age=31536000; SameSite=Lax; Secure");
  });
});

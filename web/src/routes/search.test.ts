import { describe, expect, it } from "vitest";
import { forAll } from "@/test/prng";
import { KNOWN_SEARCH_KEYS, parseAppSearch, parseSearch, pickSearch, stringifySearch, MAX_SEARCH_PARAMS, MAX_SEARCH_VALUE } from "./search";

describe("parseSearch", () => {
  it("reads plain strings, with or without the question mark", () => {
    expect(parseSearch("?a=1&b=two")).toEqual({ a: "1", b: "two" });
    expect(parseSearch("a=1")).toEqual({ a: "1" });
    expect(parseSearch("")).toEqual({});
    expect(parseSearch("?")).toEqual({});
  });

  it("keeps every value a string: ?q=123 is the text 123, never a number", () => {
    expect(parseSearch("?q=123&flag=true&x=null&y={\"a\":1}")).toEqual({ q: "123", flag: "true", x: "null", y: '{"a":1}' });
  });

  it("decodes percent escapes once and a plus as a space", () => {
    expect(parseSearch("?path=a%2Fb.yml&q=a+b")).toEqual({ path: "a/b.yml", q: "a b" });
    expect(parseSearch("?q=%252F")).toEqual({ q: "%2F" });
  });

  it("keeps the last of a repeated key", () => {
    expect(parseSearch("?a=1&a=2")).toEqual({ a: "2" });
  });

  it("never lets a key reach Object.prototype", () => {
    const parsed = parseSearch("?__proto__=x&constructor=y&prototype=z&ok=1");
    expect(parsed).toEqual({ ok: "1" });
    expect(({} as Record<string, unknown>)["x"]).toBeUndefined();
    expect(Object.getPrototypeOf(parsed)).toBe(Object.prototype);
  });

  it("survives a malformed escape", () => {
    expect(parseSearch("?a=%E0%A4%A&b=1")).toEqual({ a: "%E0%A4%A", b: "1" });
  });

  it("is bounded: a flood of parameters and a very long value are cut", () => {
    const many = Array.from({ length: 500 }, (_, i) => `k${i}=v`).join("&");
    expect(Object.keys(parseSearch(`?${many}`)).length).toBe(MAX_SEARCH_PARAMS);
    expect(parseSearch(`?q=${"a".repeat(5000)}`)["q"]?.length).toBe(MAX_SEARCH_VALUE);
    expect(parseSearch(`?${"k".repeat(500)}=1`)).toEqual({});
  });
});

describe("stringifySearch", () => {
  it("writes strings, sorted, with a leading question mark, or nothing when empty", () => {
    expect(stringifySearch({ b: "2", a: "1" })).toBe("?a=1&b=2");
    expect(stringifySearch({})).toBe("");
  });
  it("drops undefined, empty and non-string values", () => {
    expect(stringifySearch({ a: undefined, b: "", c: "x", d: 5, e: { x: 1 } })).toBe("?c=x");
  });
  it("encodes what must be encoded", () => {
    expect(stringifySearch({ path: "a/b c&d=e#f" })).toBe("?path=a%2Fb+c%26d%3De%23f");
  });
});

describe("round trip (property)", () => {
  it("parse(stringify(x)) equals x for any strings, and stringify is stable", () => {
    const alphabet = "abc XYZ019&=?#%+/\\.~:;,'\"<>\u00e9\u202e";
    forAll(
      10_000,
      (rng) => {
        const entries: Record<string, string> = {};
        const n = rng.int(6);
        for (let i = 0; i < n; i++) {
          const key = `k${rng.int(20)}`;
          const value = rng.string(alphabet, 40);
          if (value !== "") entries[key] = value;
        }
        return entries;
      },
      (entries) => {
        const text = stringifySearch(entries);
        expect(parseSearch(text)).toEqual(entries);
        expect(stringifySearch(parseSearch(text))).toBe(text);
      },
    );
  });
});

describe("pickSearch", () => {
  it("keeps only the listed keys", () => {
    expect(pickSearch(["a", "b"])({ a: "1", c: "2", b: "3" })).toEqual({ a: "1", b: "3" });
  });
  it("drops what is not a string, and trims nothing", () => {
    expect(pickSearch(["a", "b"])({ a: 1, b: " x " })).toEqual({ b: " x " });
  });
  it("validates `next` as a same-origin path and `req` as a request id", () => {
    const pick = pickSearch(["next", "req"]);
    expect(pick({ next: "/swimlanes", req: "abc123" })).toEqual({ next: "/swimlanes", req: "abc123" });
    expect(pick({ next: "https://evil.example/", req: "<script>" })).toEqual({});
    expect(pick({ next: "//evil.example/" })).toEqual({});
  });
});

describe("parseAppSearch (what the router reads)", () => {
  it("keeps the keys some route reads and drops every other", () => {
    expect(parseAppSearch("?q=x&path=a.yml&evil=1&__proto__=2")).toEqual({ q: "x", path: "a.yml" });
  });
  it("names the keys the table lists, and next and req", () => {
    for (const key of ["q", "path", "line", "diff", "state", "tier", "next", "req", "lanes", "only"]) expect(KNOWN_SEARCH_KEYS.has(key), key).toBe(true);
    expect(KNOWN_SEARCH_KEYS.has("evil")).toBe(false);
  });
  it("drops a next that is not a same-origin path and a req that is not a token, so no screen sees them", () => {
    expect(parseAppSearch("?next=https%3A%2F%2Fevil.example%2F&req=%3Cscript%3E")).toEqual({});
    expect(parseAppSearch("?next=%2Fswimlanes&req=abc123")).toEqual({ next: "/swimlanes", req: "abc123" });
  });
});

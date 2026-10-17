import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { installNonce, readNonce } from "./nonce";
import { brandRenderedDoc, createMonacoPolicy, docsHtml, PolicyError, resetPoliciesForTests } from "./trusted-types";

// jsdom has no Trusted Types: a minimal factory with the real rules (a name may be created once; a value is an object).
function fakeFactory(allowed: string[]) {
  const names: string[] = [];
  const factory = {
    createPolicy: vi.fn((name: string, options?: TrustedTypePolicyOptions) => {
      if (!allowed.includes(name)) throw new TypeError(`Policy "${name}" disallowed.`);
      if (names.includes(name)) throw new TypeError(`Policy "${name}" already exists.`);
      names.push(name);
      return {
        name,
        createHTML: (input: string) => ({ toString: () => options?.createHTML?.(input) ?? input, kind: "html" }) as unknown as TrustedHTML,
        createScriptURL: (input: string) => ({ toString: () => options?.createScriptURL?.(input) ?? input, kind: "url" }) as unknown as TrustedScriptURL,
      };
    }),
  };
  return { factory, names };
}

describe("trusted types", () => {
  let trustedTypes: ReturnType<typeof fakeFactory>;
  beforeEach(() => {
    resetPoliciesForTests();
    trustedTypes = fakeFactory(["lanekeeper-docs", "lanekeeper-monaco"]);
    vi.stubGlobal("window", { trustedTypes: trustedTypes.factory });
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    resetPoliciesForTests();
  });

  const doc = (html: string) => brandRenderedDoc({ html });

  it("creates lanekeeper-docs once, however often the docs sink is used", () => {
    docsHtml(doc("<p>a</p>"));
    docsHtml(doc("<p>b</p>"));
    expect(trustedTypes.factory.createPolicy).toHaveBeenCalledTimes(1);
    expect(trustedTypes.names).toEqual(["lanekeeper-docs"]);
  });

  it("makes the same HTML it was given, as a Trusted Type value", () => {
    const value = docsHtml(doc("<p>a</p>"));
    expect(typeof value).toBe("object");
    expect(String(value)).toBe("<p>a</p>");
  });

  it("creates lanekeeper-monaco once, with the rules of the editor", () => {
    const rules = { createHTML: vi.fn((s: string) => `[${s}]`), createScriptURL: vi.fn((s: string) => s) };
    const a = createMonacoPolicy(rules);
    const b = createMonacoPolicy(rules);
    expect(a).toBe(b);
    expect(trustedTypes.names).toEqual(["lanekeeper-monaco"]);
    expect(String(a.createHTML("x"))).toBe("[x]");
  });

  it("creates the two policies and no others", () => {
    docsHtml(doc("x"));
    createMonacoPolicy({ createHTML: (s) => s, createScriptURL: (s) => s });
    expect(trustedTypes.names.sort()).toEqual(["lanekeeper-docs", "lanekeeper-monaco"]);
  });

  it("fails loudly when the hub's trusted-types list does not name the policy, instead of falling back to a string", () => {
    vi.stubGlobal("window", { trustedTypes: fakeFactory(["something-else"]).factory });
    expect(() => docsHtml(doc("x"))).toThrow(PolicyError);
  });

  it("does not make script URLs from the docs policy", () => {
    docsHtml(doc("x"));
    // The docs policy's createScriptURL refuses; the editor policy is the only one that makes script URLs.
    const calls = trustedTypes.factory.createPolicy.mock.calls[0];
    expect(() => calls?.[1]?.createScriptURL?.("/assets/x.js")).toThrow(PolicyError);
  });
});

describe("without Trusted Types (other browsers, tests)", () => {
  beforeEach(() => {
    resetPoliciesForTests();
    vi.stubGlobal("window", {});
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    resetPoliciesForTests();
  });

  it("passes the HTML through as a string and does not crash", () => {
    expect(docsHtml(brandRenderedDoc({ html: "<p>a</p>" }))).toBe("<p>a</p>");
  });

  it("still applies the editor's rules", () => {
    const policy = createMonacoPolicy({ createHTML: (s) => s.toUpperCase(), createScriptURL: (s) => s });
    expect(policy.createHTML("a")).toBe("A");
    expect(policy.createScriptURL("/assets/a.js")).toBe("/assets/a.js");
  });
});

describe("the nonce", () => {
  const meta = (attrs: Record<string, string>, nonceProperty?: string): Document => {
    const element = { getAttribute: (name: string) => attrs[name] ?? null, nonce: nonceProperty } as unknown as HTMLMetaElement;
    return { querySelector: (selector: string) => (selector === 'meta[property="csp-nonce"]' ? element : null) } as unknown as Document;
  };

  it("reads the property first, because the browser hides the attribute", () => {
    expect(readNonce(meta({ nonce: "" }, "abcdef0123456789"))).toBe("abcdef0123456789");
  });

  it("falls back to the attribute where the property is missing", () => {
    expect(readNonce(meta({ nonce: "abcdef0123456789" }))).toBe("abcdef0123456789");
  });

  it("ignores the placeholder and anything that is not a nonce", () => {
    expect(readNonce(meta({ nonce: "__CSP_NONCE__" }, "__CSP_NONCE__"))).toBeUndefined(); // nothing replaced the placeholder: a dev server
    expect(readNonce(meta({}, "short"))).toBeUndefined();
    expect(readNonce(meta({}, 'a"b\nc'.repeat(4)))).toBeUndefined();
    expect(readNonce({ querySelector: () => null } as unknown as Document)).toBeUndefined();
  });

  it("sets window.__webpack_nonce__ for libraries that make a <style>", () => {
    const win = {} as Window;
    expect(installNonce(win, meta({}, "abcdef0123456789"))).toBe("abcdef0123456789");
    expect(win.__webpack_nonce__).toBe("abcdef0123456789");
    const empty = {} as Window;
    installNonce(empty, { querySelector: () => null } as unknown as Document);
    expect(empty.__webpack_nonce__).toBeUndefined();
  });
});

import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { forAll } from "@/test/prng";
import { installGlobalPatches, type Uninstall } from "./global-patches";
import { checkWorkerUrl, rewriteStyleAttrsInTags } from "./policy";
import { applyStyleData, createStyleAttrName } from "./style-apply";

// Monaco under the hub's CSP (plan W3, option (e), G0 variant 3d). The rewrite turns the `style="..."` attributes in the
// HTML strings Monaco builds into a data attribute the HTML parser does not treat as style (so style-src-attr never
// fires); a MutationObserver then copies the value into el.style, which is CSSOM and allowed. These are the unit tests;
// e2e/editor/csp.spec.ts is the proof in a real browser.

const A = "data-lk-s-test";

describe("rewriteStyleAttrsInTags", () => {
  const r = (html: string) => rewriteStyleAttrsInTags(html, A);

  it("renames a double-quoted, a single-quoted and an unquoted style attribute", () => {
    expect(r('<div style="top:0px;height:19px">x</div>')).toBe(`<div ${A}="top:0px;height:19px">x</div>`);
    expect(r("<div style='top:0px'>x</div>")).toBe(`<div ${A}="top:0px">x</div>`);
    expect(r("<div style=top:0px>x</div>")).toBe(`<div ${A}="top:0px">x</div>`);
  });

  it("keeps the other attributes and their order", () => {
    expect(r('<span class="a" style="left:1px" role="x">')).toBe(`<span class="a" ${A}="left:1px" role="x">`);
  });

  it("rewrites every tag of a fragment", () => {
    const html = '<div style="top:0"><span style="left:1px">a</span><span>b</span></div>';
    expect(r(html)).toBe(`<div ${A}="top:0"><span ${A}="left:1px">a</span><span>b</span></div>`);
  });

  it("never touches text between tags, even text that looks like an attribute", () => {
    expect(r('<div>a style="color:red" b</div>')).toBe('<div>a style="color:red" b</div>');
    expect(r('<div class="k">x: <b>style=\'top:5px\'</b></div>')).toBe('<div class="k">x: <b>style=\'top:5px\'</b></div>');
  });

  it("does not touch an attribute that only ends in style, or a style that is a value", () => {
    expect(r('<div data-style="x" xstyle="y" class="style=z">')).toBe('<div data-style="x" xstyle="y" class="style=z">');
  });

  it("honours a > inside a quoted attribute value", () => {
    expect(r('<div title="a > b" style="top:0">x</div>')).toBe(`<div title="a > b" ${A}="top:0">x</div>`);
  });

  it("copes with a lone < in text and with an unclosed tag", () => {
    expect(r("a < b and c > d")).toBe("a < b and c > d");
    expect(r('<div style="top:0"')).toBe(`<div ${A}="top:0"`);
  });

  it("escapes a double quote that was inside a single-quoted value", () => {
    expect(r(`<div style='font-family:"Menlo"'>`)).toBe(`<div ${A}="font-family:&quot;Menlo&quot;">`);
  });

  it("is a no-op on HTML with no style attribute", () => {
    const html = '<div class="a">text &amp; more</div>';
    expect(r(html)).toBe(html);
  });

  it("never leaves a style attribute inside a tag, and never changes text outside tags (property)", () => {
    const pieces = ['<div style="a:b">', "<span style='c:d'>", "</div>", "</span>", "text", " style=\"x\" ", "a < b", "&amp;", '<i class="q">', "<br/>", "<p style=z>"];
    forAll(
      10_000,
      (rng) => Array.from({ length: rng.int(8) }, () => rng.pick(pieces)).join(""),
      (html) => {
        const out = r(html);
        // Every tag of the output has no ` style=` attribute.
        for (const tag of out.matchAll(/<[A-Za-z][^>]*>/g)) expect(tag[0], tag[0]).not.toMatch(/\sstyle\s*=/i);
        // The text outside tags is the same.
        const text = (s: string) => s.replace(/<[A-Za-z/][^>]*>/g, "");
        expect(text(out)).toBe(text(html));
      },
    );
  });
});

describe("checkWorkerUrl", () => {
  const origin = "https://hub.example";
  it("accepts a same-origin script under /assets", () => {
    expect(checkWorkerUrl("/assets/editor.worker-ABC123.js", origin)).toBe("https://hub.example/assets/editor.worker-ABC123.js");
  });
  it.each(["https://evil.example/assets/x.js", "//evil.example/assets/x.js", "/assets/../x.js", "/other/x.js", "/assets/x.mjs", "/assets/x.js?a=b", "blob:https://hub.example/uuid", "data:text/javascript,1", "javascript:1", "/assets/a/b.js", "/assets/x.js#h"])("refuses %s", (url) => {
    expect(() => checkWorkerUrl(url, origin)).toThrow(TypeError);
  });
});

describe("style data", () => {
  it("copies the value to the element's style through CSSOM and removes the attribute", () => {
    const root = document.createElement("div");
    root.innerHTML = `<div ${A}="top:10px;height:19px"><span ${A}="left:3px"></span></div>`;
    applyStyleData(root, A);
    const outer = root.firstElementChild as HTMLElement;
    expect(outer.style.top).toBe("10px");
    expect(outer.style.height).toBe("19px");
    expect(outer.hasAttribute(A)).toBe(false);
    const inner = outer.firstElementChild as HTMLElement;
    expect(inner.style.left).toBe("3px");
    expect(inner.hasAttribute(A)).toBe(false);
  });

  it("applies the root itself", () => {
    const el = document.createElement("div");
    el.setAttribute(A, "width:5px");
    applyStyleData(el, A);
    expect(el.style.width).toBe("5px");
  });

  it("creates an attribute name that foreign HTML cannot carry, different on every page load", () => {
    const a = createStyleAttrName();
    const b = createStyleAttrName();
    expect(a).toMatch(/^data-lk-s-[0-9a-f]{16}$/);
    expect(a).not.toBe(b);
  });
});

describe("global patches", () => {
  let uninstall: Uninstall;
  beforeEach(() => {
    uninstall = installGlobalPatches({ nonce: "abcdef0123456789", styleAttr: A });
  });
  afterEach(() => uninstall());

  it("puts the page nonce on every <style> element made with createElement", () => {
    const style = document.createElement("style") as HTMLStyleElement & { nonce?: string };
    expect(style.nonce).toBe("abcdef0123456789");
    const div = document.createElement("div") as HTMLElement & { nonce?: string };
    expect(div.nonce).toBeFalsy();
    expect((document.createElement("STYLE") as HTMLStyleElement).nonce).toBe("abcdef0123456789");
  });

  it("routes setAttribute('style') to CSSOM, in any case, and leaves other attributes alone", () => {
    const el = document.createElement("div");
    el.setAttribute("style", "top:7px;width:2px");
    expect(el.style.top).toBe("7px");
    expect(el.style.width).toBe("2px");
    el.setAttribute("STYLE", "left:1px");
    expect(el.style.left).toBe("1px");
    el.setAttribute("title", "t");
    expect(el.getAttribute("title")).toBe("t");
  });

  it("applies style data on elements added to the document, from any insertion", async () => {
    const host = document.createElement("div");
    document.body.append(host);
    const frag = document.createElement("template");
    frag.innerHTML = `<div id="added" ${A}="top:9px"></div>`;
    host.append(frag.content);
    await Promise.resolve();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect((document.getElementById("added") as HTMLElement).style.top).toBe("9px");
    expect(document.getElementById("added")?.hasAttribute(A)).toBe(false);
    host.remove();
  });

  it("is installed once, however many times it is asked for", () => {
    const again = installGlobalPatches({ nonce: "abcdef0123456789", styleAttr: A });
    const el = document.createElement("div");
    el.setAttribute("style", "top:1px");
    expect(el.style.top).toBe("1px");
    again();
    // The first install is still in force until it is uninstalled.
    const el2 = document.createElement("div");
    el2.setAttribute("style", "top:2px");
    expect(el2.style.top).toBe("2px");
  });

  it("restores the built-ins when it is uninstalled", () => {
    uninstall();
    const style = document.createElement("style") as HTMLStyleElement & { nonce?: string };
    expect(style.nonce).toBeFalsy();
    const el = document.createElement("div");
    el.setAttribute("style", "top:3px");
    expect(el.getAttribute("style")).toBe("top:3px");
    uninstall = installGlobalPatches({ nonce: "abcdef0123456789", styleAttr: A });
  });
});

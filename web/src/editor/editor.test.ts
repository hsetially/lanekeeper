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

  // D133: the rewrite renames the style attribute of a TAG and nothing else. These are the shapes that a pattern over the whole tag gets wrong:
  // an attribute VALUE that holds ` style='x' ` is not an attribute, and renaming it in place closes the value early, so the rest of the value
  // becomes attributes of the tag (Monaco puts text of the file into some titles and labels).

  it("never touches text, whatever the text says (a style attribute written as text, an escaped tag, a lone angle bracket)", () => {
    for (const html of [
      '<div>a style="x" b</div>',
      "<span class=\"k\">style='top:5px'</span>",
      '<div>&lt;div style="top:0"&gt;</div>',
      '<div>if (a < b) style="x"</div>',
      'style="x"',
      '<div>x</div> style = "y" <div>z</div>',
    ]) {
      expect(r(html), html).toBe(html);
    }
  });

  it("never touches a style that is only part of another attribute's value, in either kind of quote", () => {
    for (const html of [
      `<div title="a style='x' b">`,
      `<div title='a style="x" b'>`,
      '<div class="a style=z b">',
      `<div aria-label="x style = 'y' z" data-q='style="w"'>`,
      `<div title="a style='x' onmouseover=alert(1) b">`,
      '<div title="a\nstyle=\'x\'">',
      '<div data-a="style=x" data-b=\'style="y"\'>',
    ]) {
      expect(r(html), html).toBe(html);
    }
  });

  it("renames the real style attribute of a tag whose other attributes hold style-looking values, and only that one", () => {
    expect(r(`<div title="a style='x' b" style="top:0">`)).toBe(`<div title="a style='x' b" ${A}="top:0">`);
    expect(r(`<div style='left:1px' title="a style='x' b">`)).toBe(`<div ${A}="left:1px" title="a style='x' b">`);
    expect(r('<div class="a style=z" style="top:0" data-x=\'style="w"\'>')).toBe(`<div class="a style=z" ${A}="top:0" data-x='style="w"'>`);
  });

  it("finds the attribute whatever its spelling: any case, any space or line break around the equals sign, a slash before it", () => {
    expect(r('<div STYLE="top:0">')).toBe(`<div ${A}="top:0">`);
    expect(r("<div StYlE='top:0'>")).toBe(`<div ${A}="top:0">`);
    expect(r('<div style = "top:0">')).toBe(`<div ${A}="top:0">`);
    expect(r('<div style\n=\n"top:0">')).toBe(`<div ${A}="top:0">`);
    expect(r("<div\tstyle\t=\ttop:0\tclass=a>")).toBe(`<div\t${A}="top:0"\tclass=a>`);
    expect(r('<br/style="top:0">')).toBe(`<br/${A}="top:0">`);
    expect(r('<div class=a  style="top:0"  >')).toBe(`<div class=a  ${A}="top:0"  >`);
  });

  it("leaves a style without a value, and a name that only contains the word, as they are", () => {
    for (const html of ["<div style>", "<div style class=a>", "<div style=>", '<div style-x="1" data-style="2" xstyle="3" styled="4">']) expect(r(html), html).toBe(html);
  });

  it("keeps a > or a quote of the other kind inside the value it renames", () => {
    expect(r('<div style="a>b">x</div>')).toBe(`<div ${A}="a>b">x</div>`);
    expect(r("<div style='a>b'>x</div>")).toBe(`<div ${A}="a>b">x</div>`);
    expect(r('<div style=a"b c=1>')).toBe(`<div ${A}="a&quot;b" c=1>`);
  });

  it("never reads a comment, a doctype, an end tag or a processing instruction as a tag", () => {
    for (const html of [
      '<!-- <div style="x"> -->',
      '<!-- style="x" --><div style="y">',
      '<!-->style="x"',
      '<!--->style="x"',
      '<!-- a --!> <div style="x">',
      '<!DOCTYPE html style="x">',
      '</div style="x">',
      '</div title=">" style="x">',
      '<?php style="x" ?>',
      '<![CDATA[ <div style="x"> ]]>',
    ]) {
      const out = r(html);
      // What follows a comment is a tag again, so only a style outside the comment may change.
      expect(out.replaceAll(A, "style"), html).toBe(html);
    }
    expect(r('<!-- <div style="x"> --><div style="y">')).toBe(`<!-- <div style="x"> --><div ${A}="y">`);
    expect(r('<!-- a --!><div style="y">')).toBe(`<!-- a --!><div ${A}="y">`);
    expect(r('<!--><div style="y">')).toBe(`<!--><div ${A}="y">`);
    expect(r('<!---><div style="y">')).toBe(`<!---><div ${A}="y">`);
    expect(r('</div title=">"><div style="y">')).toBe(`</div title=">"><div ${A}="y">`);
  });

  it("agrees with a model of the HTML tag syntax on every tag and text it is given (property)", () => {
    // The model builds the page and the page that must come out of it together, from attributes it knows the meaning of.
    const names = ["style", "STYLE", "Style", "class", "title", "data-style", "xstyle", "aria-label", "style-x"];
    const values = ["top:0", "a b", "a style='x' b", 'say "hi"', "a>b", "style=z", "x style = \"y\" z", "", "&amp; style='q'", "left:1px;width:2px", "p\nstyle='z'"];
    const equals = ["=", " = ", "\n=\n", "=  ", "\t=\t"];
    const texts = ["text", ' style="x" ', "a < b", "&lt;div style=&quot;x&quot;&gt;", " > ", "style='y'"];
    forAll(
      4_000,
      (rng) => {
        let input = "";
        let expected = "";
        for (let t = 0, tags = 1 + rng.int(4); t < tags; t++) {
          const text = rng.pick(texts);
          input += text;
          expected += text;
          const tag = rng.pick(["div", "span", "DIV", "i"]);
          input += `<${tag}`;
          expected += `<${tag}`;
          for (let k = 0, attrs = rng.int(4); k < attrs; k++) {
            const name = rng.pick(names);
            const value = rng.pick(values);
            const sep = rng.pick([" ", "  ", "\n", "\t", " / "]);
            const eq = rng.pick(equals);
            // A double quote in the value needs single quotes and the other way round; an unquoted value has neither and no space.
            const quotes = [value.includes('"') ? null : '"', value.includes("'") ? null : "'", /^[^\s"'>=<`]+$/.test(value) ? "" : null].filter((q): q is string => q !== null);
            const quote = rng.pick(quotes);
            const written = `${name}${eq}${quote}${value}${quote}`;
            input += sep + written;
            expected += sep + (name.toLowerCase() === "style" ? `${A}="${value.replaceAll('"', "&quot;")}"` : written);
          }
          input += ">";
          expected += ">";
        }
        return { input, expected };
      },
      ({ input, expected }) => expect(r(input)).toBe(expected),
    );
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

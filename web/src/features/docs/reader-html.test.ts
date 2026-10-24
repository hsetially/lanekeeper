import { describe, expect, it } from "vitest";
import { belowThePageTitle, forTheReader, withoutHeadTags, withoutImages } from "./reader-html";

// The reader does not let the browser fetch an image that a doc names. The server's allowlist lets an `img` take any path on
// this origin, and a browser fetches an image with a GET and the session cookie, so a doc could point at any GET of the app, the
// event stream included (docs/interfaces.md, open item 4). Until a docs-asset prefix exists the image is replaced by its alt text.

describe("withoutImages", () => {
  it("replaces an image by its alt text", () => {
    expect(withoutImages('<p>See <img src="/api/v1/events" alt="the diagram"> below</p>')).toBe("<p>See the diagram below</p>");
  });

  it("drops an image that has no alt text, and handles a self-closing tag", () => {
    expect(withoutImages('<p>a<img src="/x.png">b<img src="/y.png" alt="" />c</p>')).toBe("<p>abc</p>");
  });

  it("keeps the alt text exactly as the server wrote it (its entities stay entities)", () => {
    expect(withoutImages('<img src="/x.png" alt="a &lt;b&gt; &amp; c">')).toBe("a &lt;b&gt; &amp; c");
  });

  it("finds the tag whatever the case, and an alt that comes before the src", () => {
    expect(withoutImages('<IMG ALT="shout" SRC="/x.png">')).toBe("shout");
    expect(withoutImages('<img alt="first" src="/x.png">')).toBe("first");
  });

  it("removes every image, and leaves everything else as it was", () => {
    const html = '<h2 id="doc-a">A</h2><p><a href="/docs/view?path=%2Fuploads%2Fb.md" rel="noopener noreferrer">b</a></p><pre><code>x &lt; y</code></pre>';
    expect(withoutImages(html)).toBe(html);
    expect(withoutImages(`${html}<img src="/a" alt="a"><img src="/b" alt="b">`)).toBe(`${html}ab`);
  });

  it("leaves no image behind even when the server breaks its own rule about angle brackets in a value", () => {
    // The part up to the first `>` is removed; whatever is left is not an `img`. (The server's allowlist is the control here.)
    expect(withoutImages('<img src="/x" alt="a>b<c">')).not.toMatch(/<img/i);
  });

  it("does not look inside a longer tag name", () => {
    expect(withoutImages("<imgx>")).toBe("<imgx>");
    expect(withoutImages("<p>images</p>")).toBe("<p>images</p>");
  });
});

// A rendered doc may open with a heading of level 1 of its own: the allowlist lists `h1` (docs/interfaces.md, "Web additions"), and a
// Markdown file's `# Title` is one. The page already has its one h1, the doc's title, so a second would give the screen two
// (axe `page-has-heading-one` and the plan's "one h1 per page"; a screen reader lists the headings of a page by level).
// The reader moves the doc's headings down one level when it has an h1. That is a rename of tags inside the allowlist (h2 to h6
// are on it, with the same `id` rule): it adds no element, no attribute and no text, and it changes no sanitiser contract.

describe("belowThePageTitle", () => {
  it("leaves a page that has no h1 exactly as it is", () => {
    const html = '<h2 id="doc-a">A</h2><p>text</p><h3 id="doc-b">B</h3>';
    expect(belowThePageTitle(html)).toBe(html);
  });

  it("moves every heading down one level when the page has an h1, closing tags too", () => {
    const html = '<h1 id="doc-title">Title</h1><p>intro</p><h2 id="doc-a">A</h2><h3 id="doc-b">B</h3><h6 id="doc-c">C</h6>';
    expect(belowThePageTitle(html)).toBe('<h2 id="doc-title">Title</h2><p>intro</p><h3 id="doc-a">A</h3><h4 id="doc-b">B</h4><h6 id="doc-c">C</h6>');
  });

  it("never goes past h6 (h5 becomes h6, h6 stays h6)", () => {
    expect(belowThePageTitle("<h1>a</h1><h5>b</h5><h6>c</h6>")).toBe("<h2>a</h2><h6>b</h6><h6>c</h6>");
  });

  it("finds a heading anywhere in the page, whatever the case of the tag", () => {
    expect(belowThePageTitle("<p>x</p><H1>a</H1><H2>b</H2>")).toBe("<p>x</p><h2>a</h2><h3>b</h3>");
  });

  it("changes tag names and nothing else: text, entities, attributes and other elements stay as the server wrote them", () => {
    const html = '<h1 id="doc-t">A &lt;b&gt; &amp; c</h1><p>h1 in text is not a tag, nor is &lt;h1&gt;</p><hr><a href="/x" rel="noopener noreferrer">h2</a><pre><code>&lt;h1&gt;x&lt;/h1&gt;</code></pre>';
    expect(belowThePageTitle(html)).toBe('<h2 id="doc-t">A &lt;b&gt; &amp; c</h2><p>h1 in text is not a tag, nor is &lt;h1&gt;</p><hr><a href="/x" rel="noopener noreferrer">h2</a><pre><code>&lt;h1&gt;x&lt;/h1&gt;</code></pre>');
  });

  it("does not look inside a longer tag name", () => {
    expect(belowThePageTitle("<h1>a</h1><h1x>b</h1x><hr>")).toBe("<h2>a</h2><h1x>b</h1x><hr>");
    expect(belowThePageTitle("<h1x>b</h1x>")).toBe("<h1x>b</h1x>");
  });

  it("produces only elements the server's allowlist has, and no h1", () => {
    const out = belowThePageTitle('<h1 id="doc-a">a</h1><h2 id="doc-b">b</h2><h3 id="doc-c">c</h3><h4 id="doc-d">d</h4><h5 id="doc-e">e</h5><h6 id="doc-f">f</h6><p>p</p>');
    const tags = [...out.matchAll(/<\/?([a-z0-9]+)/gi)].map((m) => m[1]?.toLowerCase());
    expect(new Set(tags)).toEqual(new Set(["h2", "h3", "h4", "h5", "h6", "p"]));
    expect(out).not.toMatch(/<\/?h1\b/i);
  });
});

// A `<meta http-equiv=refresh>` sends the whole page to another address by itself, with no click, once it is inserted. Neither the CSP
// (`navigate-to` no longer exists) nor Trusted Types governs it, so if the server's allowlist (no `meta`, `base` or `link`) ever failed,
// nothing but the reader would stand between a doc and an open redirect (or a reload loop). The reader removes the three elements that
// act on the page without being drawn: `meta` (refresh, a cookie, a policy), `base` (every relative link) and `link` (a stylesheet,
// a prefetch). e2e/csp/xss.spec.ts proves it in the browser, under the real header.

describe("withoutHeadTags", () => {
  it("removes a meta refresh to another origin, to this origin, and a plain reload", () => {
    expect(withoutHeadTags('<p>a</p><meta http-equiv="refresh" content="0;url=https://evil.example/"><p>b</p>')).toBe("<p>a</p><p>b</p>");
    expect(withoutHeadTags('<meta http-equiv="refresh" content="0;url=/docs/view?path=%2Fx.md">')).toBe("");
    expect(withoutHeadTags('<meta http-equiv="refresh" content="0">')).toBe("");
  });

  it("removes base and link, whatever they are for", () => {
    expect(withoutHeadTags('<base href="https://evil.example/"><p>x</p>')).toBe("<p>x</p>");
    expect(withoutHeadTags('<p>x</p><link rel="stylesheet" href="https://evil.example/x.css">')).toBe("<p>x</p>");
    expect(withoutHeadTags('<link rel="prefetch" href="/api/v1/events"><link rel="icon" href="/i.png"><p>y</p>')).toBe("<p>y</p>");
  });

  it("finds the tag whatever its spelling: case, spaces, a slash, a line break, unquoted attributes, no closing angle bracket", () => {
    expect(withoutHeadTags('<META HTTP-EQUIV=refresh CONTENT=0>x')).toBe("x");
    expect(withoutHeadTags('<Meta\nhttp-equiv="refresh"\ncontent="0"\n>x')).toBe("x");
    expect(withoutHeadTags('<meta/http-equiv=refresh content=0/>x')).toBe("x");
    expect(withoutHeadTags("<base\thref=//evil.example/>x<link\fhref=//evil.example/>y")).toBe("xy");
    expect(withoutHeadTags("<p>a</p><meta")).toBe("<p>a</p>");
    expect(withoutHeadTags('<p>a</p><meta http-equiv=refresh content="0;url=//evil.example/"')).toBe("<p>a</p>");
  });

  it("removes a tag whose attributes hold an angle bracket, however the server's rule about them was broken", () => {
    expect(withoutHeadTags('<meta http-equiv=refresh content="0;url=https://evil.example/" <b>text')).not.toMatch(/<meta/i);
    expect(withoutHeadTags('<meta content="a>b" http-equiv=refresh>')).not.toMatch(/<meta/i);
  });

  it("still renders the rest of the doc: its text, its headings, its links, its code, in the order the server wrote them", () => {
    const parts = [
      '<h2 id="doc-a">A</h2>',
      '<p>text with <a href="/docs/view?path=%2Fuploads%2Fb.md" rel="noopener noreferrer">a link</a> and <code>x &lt; y</code></p>',
      '<pre><code>&lt;meta http-equiv="refresh"&gt;</code></pre>',
      "<ul><li>one</li><li>two</li></ul>",
    ];
    const meta = '<meta http-equiv="refresh" content="0;url=https://evil.example/">';
    const base = '<base href="https://evil.example/">';
    const link = '<link rel="stylesheet" href="https://evil.example/x.css">';
    expect(withoutHeadTags([meta, parts[0], base, parts[1], parts[2], link, parts[3], meta].join(""))).toBe(parts.join(""));
  });

  it("leaves a page that has none of them exactly as it is, including text that talks about them", () => {
    const html = '<p>The &lt;meta&gt; tag, the &lt;base&gt; tag and the &lt;link&gt; tag.</p><pre><code>&lt;meta http-equiv="refresh" content="0"&gt;</code></pre><p>meta base link</p>';
    expect(withoutHeadTags(html)).toBe(html);
    expect(withoutHeadTags("")).toBe("");
  });

  it("does not look inside a longer tag name", () => {
    const html = "<metadata>a</metadata><linkage>b</linkage><basefont>c</basefont><base-x>d</base-x><linked>e</linked><p>f</p>";
    expect(withoutHeadTags(html)).toBe(html);
  });

  it("does not let removing a tag write a new one: a page that would be left with one shows nothing", () => {
    for (const html of [
      "<<meta>meta http-equiv=refresh content=0>",
      '<me<meta>ta http-equiv="refresh" content="0;url=//evil.example/">',
      '<ba<base>se href="//evil.example/">',
      "<li<link>nk rel=stylesheet href=//evil.example/x.css>",
      "<<<link>meta>meta http-equiv=refresh content=0>",
      "<p>kept?</p><<meta>base href=//evil.example/>",
    ]) {
      expect(withoutHeadTags(html), html).not.toMatch(/<(meta|base|link)/i);
      expect(withoutHeadTags(html), html).toBe("");
    }
  });

  it("never leaves the start of one of the three, whatever order pieces of tags come in (every string of up to six pieces)", () => {
    const pieces = ["<", ">", "meta", "link", " x=1", '"', "/"];
    const start = /<(?:meta|base|link)(?=[\s/>]|$)/i;
    let strings = 0;
    const walk = (prefix: string, depth: number): void => {
      const out = withoutHeadTags(prefix);
      strings += 1;
      if (start.test(out)) throw new Error(`a tag is left in ${JSON.stringify(prefix)}: ${JSON.stringify(out)}`);
      if (depth === 6) return;
      for (const piece of pieces) walk(prefix + piece, depth + 1);
    };
    walk("", 0);
    expect(strings).toBe(137257);
  });
});

describe("forTheReader", () => {
  it("is the page without images, without the three head elements and one level below the page's title", () => {
    const html = '<h1 id="doc-t">T</h1><meta http-equiv="refresh" content="0;url=https://evil.example/"><p>See <img src="/api/v1/events" alt="the picture"> here.</p><h2 id="doc-a">A</h2><base href="//evil.example/">';
    expect(forTheReader(html)).toBe('<h2 id="doc-t">T</h2><p>See the picture here.</p><h3 id="doc-a">A</h3>');
  });

  it("leaves a page the server wrote alone, apart from the heading levels", () => {
    const html = '<h2 id="doc-a">A</h2><p>text <a href="/x" rel="noopener noreferrer">link</a></p>';
    expect(forTheReader(html)).toBe(html);
  });

  it("does not let an image's alt text write a tag back into the page", () => {
    // An alt text is put back into the page as it stands, and the part of the page around the image can hold the angle brackets.
    for (const html of [
      '<<img src=x alt="meta http-equiv=refresh content=0">>',
      '<<img src=x alt="base href=//evil.example/">>',
      '<<img src=x alt="link rel=stylesheet href=//evil.example/x.css">>',
      '<<img src=x alt="img src=/api/v1/events onerror=x">>',
    ]) {
      expect(forTheReader(html), html).not.toMatch(/<(img|meta|base|link)/i);
    }
  });

  it("does not let removing a head element write an image back into the page", () => {
    expect(forTheReader('<<meta>img src="/api/v1/events" onerror=x>')).not.toMatch(/<(img|meta|base|link)/i);
  });

  it("does not leave an image whose tag holds an angle bracket (the part of the page the pattern for images cannot see)", () => {
    expect(forTheReader('<p>a</p><img src="/api/v1/events" <b>')).not.toMatch(/<img/i);
  });
});

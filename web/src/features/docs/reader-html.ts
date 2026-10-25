// What the reader does to a rendered doc before it reaches the one HTML sink (components/app/SanitisedDocHtml.tsx).
//
// `RenderedDoc.html` is the server's sanitised page, and the server's allowlist is the control that keeps it safe. This module
// only ever REMOVES: it adds no markup, no attribute and no text that the server did not write.
//
// Images. The allowlist lets an `img` take any path on this origin ("the floor of the allowlist": docs/interfaces.md, open item
// 4). A browser fetches an image with a GET and the user's cookie, so a doc could name any GET of the app, such as the event
// stream, which would take one of the user's five stream slots each time the doc is opened. There is no operation that serves a
// doc's own assets, so no image in a doc can be a legitimate one yet. Until the owner of the docs index (prompt 14) fixes a
// docs-asset prefix, an image is replaced by its alt text and the browser fetches nothing.
//
// The pattern relies on what the contract states about the page: attribute values are in double quotes and a `<` or `>` in a
// value is an entity, so a tag has no angle bracket inside it.

const IMG_TAG = /<img\b[^<>]*>/gi;
const ALT = /\salt="([^"]*)"/i;

/** The page with each `img` replaced by its alt text (already entity-escaped by the server, so it is text and never markup). */
export function withoutImages(html: string): string {
  if (!/<img\b/i.test(html)) return html;
  return html.replace(IMG_TAG, (tag) => ALT.exec(tag)?.[1] ?? "");
}

// Headings. The allowlist lists `h1` to `h6`, and a Markdown file's `# Title` is an `h1`. The reader's page has its one h1 already
// (the doc's title, drawn by React next to the toolbar), and a screen has exactly one (the plan's accessibility gate, axe's
// `page-has-heading-one` and a screen reader's list of headings by level). So when the doc has an h1 of its own, every heading of
// the doc moves down one level: its title becomes an h2 under the page's h1 and its sections an h3, and the order stays whole. A doc
// with no h1 is left as the server wrote it (its sections are already h2).
//
// This is a rename of tag names inside the allowlist (`h2` to `h6` carry the same `id` rule as `h1`). It adds no element, no
// attribute and no text, the heading `id`s are the server's, and the sanitiser's contract is untouched. Like `withoutImages` it
// relies on what the contract states about the page: a `<` in text or in a value is an entity, so a `<` starts a tag, and a heading
// tag is `<hN` followed by a space, `/` or `>`.

const HEADING_OPEN_H1 = /<h1(?=[\s>/])/i;
const HEADING_TAG = /<(\/?)h([1-6])(?=[\s>/])/gi;

/** The page with its headings one level lower when it has an h1 of its own (h6 stays h6); the page as it is when it has none. */
export function belowThePageTitle(html: string): string {
  if (!HEADING_OPEN_H1.test(html)) return html;
  return html.replace(HEADING_TAG, (_tag, slash: string, level: string) => `<${slash}h${Math.min(6, Number(level) + 1)}`);
}

// Head elements. `meta`, `base` and `link` are not on the server's allowlist (the checker refuses all three), because each acts on the
// page without being drawn. A `<meta http-equiv=refresh content="0;url=...">` sends the whole page to another address by itself, with no
// click, as soon as it is inserted, and the policy cannot stop it: the CSP has no directive for where a page may navigate
// (`navigate-to` was removed) and Trusted Types governs strings that reach a sink, not what the parsed element does. A `base` changes
// where every relative link goes (the CSP's `base-uri` refuses that one) and a `link` fetches a stylesheet or a prefetch (`style-src`
// and `default-src` refuse those). So the server's allowlist is the first layer, this removal the second for a `meta`, and the CSP and
// Trusted Types the layer behind both.
//
// Like the two functions above it only removes, with a pattern, and relies on what the contract states about a tag. It does not rely
// on it for its guarantee: a tag runs to the first `>` here even when the page holds an angle bracket inside it, a tag name that is
// not closed is removed to the end of the page, and since taking a tag out joins what was before it to what was after it
// (`<me<meta>ta ...>` leaves `<meta ...>`), the result is looked at again: a page that would still hold the start of one of the three
// shows nothing, because it is not a page the server wrote. A DOM parser would be the sturdier tool, and it is the one thing this
// module cannot use: it takes a string, which Trusted Types refuses everywhere but in the one sink.

const HEAD_START = /<(?:meta|base|link)(?=[\s/>]|$)/i;
const HEAD_TAG = /<(?:meta|base|link)(?=[\s/>]|$)[^>]*(?:>|$)/gi;

/** The page without its `meta`, `base` and `link` tags; nothing at all when removing them would leave the start of one. */
export function withoutHeadTags(html: string): string {
  if (!HEAD_START.test(html)) return html;
  const out = html.replace(HEAD_TAG, "");
  return HEAD_START.test(out) ? "" : out;
}

const IMG_START = /<img(?=[\s/>]|$)/i;

/**
 * The page as the sink is given it: images replaced by their alt text, `meta`, `base` and `link` removed, the headings one level lower
 * when the doc has an h1 of its own. The order is part of it: an alt text is written back into the page as it stands and can complete
 * a tag whose angle brackets are outside the image (`<<img alt="meta ...">>`), so the head tags go after the images; and taking a head
 * tag out can complete an image in turn (`<<meta>img ...>`), so the last thing done is to look for the start of an image or of a head
 * tag once more. A page that has one is not a page the server wrote and shows nothing.
 */
export function forTheReader(html: string): string {
  const out = belowThePageTitle(withoutHeadTags(withoutImages(html)));
  return IMG_START.test(out) || HEAD_START.test(out) ? "" : out;
}

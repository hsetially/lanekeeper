/**
 * The address of a page of this app, for a permalink: an app path (made by `linkTo`, never by hand) on this page's origin. A value that
 * is not an absolute path of the app (another origin, a protocol-relative `//host`, a scheme, a backslash, a space or a control
 * character) is not a link we hand out. A fragment (a heading id) is set through the URL, which encodes it, so the clipboard never holds an address the app did not build.
 */
export function absoluteLink(href: string, fragment?: string): string | null {
  if (!href.startsWith("/") || href.startsWith("//") || /[\\\s\p{Cc}]/u.test(href)) return null;
  try {
    const url = new URL(href, window.location.origin);
    if (url.origin !== window.location.origin) return null;
    if (fragment !== undefined && fragment !== "") url.hash = fragment;
    return url.href;
  } catch {
    return null;
  }
}

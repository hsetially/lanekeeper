// The address of a pull request. It comes from the server (`PrLink.pr_url`, `PrJob.pr_url`), which got it from GitHub, and it is
// opened as a link, so it is untrusted (S12): it is a link only when it is an https address with a host and no credentials. What
// is drawn, what is opened and what is copied are ONE string (`href`, the form the URL parser gives), so a button never copies
// something other than the link opens. A link to GitHub is navigation only: it carries `rel="noopener noreferrer"` where it is drawn.

import { hasControlOrBackslash } from "@/lib/text/control";

export type PrAddress = {
  /** The canonical address: what the link opens and the copy button copies. */
  href: string;
  /** The address without the scheme, for drawing (through SafeText). */
  display: string;
  /** The pull request's number, from a `/pull/<n>` path; null for any other path. */
  number: number | null;
};

/** The contract's `maxLength` of `pr_url`. */
const MAX_LENGTH = 512;
// Whitespace, controls and backslashes are refused before the URL parser sees them: it would drop or rewrite a tab, a line break
// and a backslash (read as a slash by a browser), and so make a different address from the one the server sent.
const SPACE = /\s/u;
function hasForbidden(value: string): boolean {
  return SPACE.test(value) || hasControlOrBackslash(value);
}
const PULL_NUMBER = /\/pull\/(\d{1,9})\/?$/;

export function prAddress(value: string): PrAddress | null {
  if (value.length === 0 || value.length > MAX_LENGTH || !value.startsWith("https://") || hasForbidden(value)) return null;
  // The parser reads `https:///x` as `https://x` (it ignores extra slashes after a special scheme), which is another host from the
  // one the server named: an address with no host of its own is refused.
  if (/^[/?#]/.test(value.slice("https://".length))) return null;
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return null;
  }
  if (url.protocol !== "https:" || url.hostname === "" || url.username !== "" || url.password !== "") return null;
  const found = PULL_NUMBER.exec(url.pathname);
  return {
    href: url.href,
    display: url.href.slice("https://".length),
    number: found?.[1] === undefined ? null : Number(found[1]),
  };
}

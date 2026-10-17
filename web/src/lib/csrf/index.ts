// The double-submit CSRF token (S3). The hub sets the `__Host-lk_csrf` cookie, readable by script by design, and wants the
// same value back in `X-CSRF-Token` on every request that is not a GET. src/api/constants.drift.test.ts reads both names
// from the Rust source, so a rename there fails here.

export const CSRF_COOKIE = "__Host-lk_csrf";
export const CSRF_HEADER = "X-CSRF-Token";

// The hub's token is 43 characters of unpadded base64url. The check is looser on the length (a longer token is not a
// reason to stop working) and strict on the alphabet: only these characters can never end a header line or a cookie.
const TOKEN_TEXT = /^[A-Za-z0-9_-]{16,128}$/;

function currentCookies(): string {
  return typeof document === "undefined" ? "" : document.cookie;
}

/**
 * The token in `cookies` (default: `document.cookie`), or undefined. A cookie that is present twice is believed by
 * nobody, because the hub does the same and a request built from the other copy would be refused.
 */
export function readCsrfToken(cookies: string = currentCookies()): string | undefined {
  let found: string | undefined;
  for (const pair of cookies.split(";")) {
    const eq = pair.indexOf("=");
    if (eq < 0) continue;
    if (pair.slice(0, eq).trim() !== CSRF_COOKIE) continue;
    if (found !== undefined) return undefined;
    found = pair.slice(eq + 1).trim();
  }
  return found !== undefined && TOKEN_TEXT.test(found) ? found : undefined;
}

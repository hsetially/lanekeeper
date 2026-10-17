// Validators for the two search parameters a person (or an attacker's link) can put in a URL that the app then uses:
//
//  - `next` on /signin: where to go after sign-in. A same-origin path and nothing else (an open redirect after sign-in is
//    the classic way to turn a trusted login into a phishing page). It mirrors the hub's `valid_return_to`
//    (oidc/login.rs) and is stricter: plain URL characters only, no `//` anywhere in the path, no dot segments and no
//    encoded slash or backslash, because the app must never decode or normalise a path itself
//    (docs/interfaces.md, open item 6), and a value the browser would resolve to another origin once decoded has to fail here.
//  - `req` on /404, /error and /403: the request id the screen shows and offers to copy. A short token.
//
// Both return the value unchanged or undefined; neither throws.

const MAX_NEXT = 512;
const PATH_CHARS = /^[A-Za-z0-9._~!$&'()*+,;=:@%/?#-]+$/;
const ENCODED_SLASH = /%2f|%5c|%00/i;
const REQUEST_ID = /^[A-Za-z0-9._-]{1,128}$/;
// Never a place to return to: the sign-in and access pages themselves and the hub's own auth endpoints (a loop, or a GET
// to /auth/logout-like paths).
const NOT_A_DESTINATION = /^\/(signin|auth|access|access-requested|access-disabled)(\/|\?|#|$)/;

export function validateNext(value: unknown): string | undefined {
  if (typeof value !== "string") return undefined;
  if (value.length === 0 || value.length > MAX_NEXT) return undefined;
  if (!value.startsWith("/") || value.startsWith("//") || value.startsWith("/\\")) return undefined;
  if (!PATH_CHARS.test(value)) return undefined;
  const queryAt = value.search(/[?#]/);
  const path = queryAt < 0 ? value : value.slice(0, queryAt);
  if (ENCODED_SLASH.test(path)) return undefined;
  for (const segment of path.slice(1).split("/").slice(0, -1)) {
    if (segment === "") return undefined;
  }
  for (const segment of path.slice(1).split("/")) {
    const plain = segment.replace(/%2e/gi, ".");
    if (plain === "." || plain === "..") return undefined;
  }
  if (path.slice(1).includes("//")) return undefined;
  if (NOT_A_DESTINATION.test(value)) return undefined;
  return value;
}

export function validateRequestId(value: unknown): string | undefined {
  return typeof value === "string" && REQUEST_ID.test(value) ? value : undefined;
}

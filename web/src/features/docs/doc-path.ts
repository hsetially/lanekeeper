// Doc paths (`DocPath` in api/openapi.yaml): `/git/<branch>/<path>` or `/uploads/<name>`, at most 1,024 characters.
//
// A path reaches the reader from the address bar (`/docs/view?path=` or the slug of `/docs/:slug`), so it is untrusted. The
// server checks it again; checking its shape here means a wrong link is a "that is not a doc" screen and not a request. Nothing
// here decodes percent escapes: a path is handed to the server as it came (the doc-link rule of docs/interfaces.md, open item 6).

import { hasHiddenOrBackslash } from "@/lib/text/control";

const MAX_PATH = 1024;

export type DocSource = "git" | "upload";

/** Whether `value` has the shape of a doc path. It does not say the doc exists. */
export function isDocPath(value: string): boolean {
  if (value.length === 0 || value.length > MAX_PATH) return false;
  const source = value.startsWith("/git/") ? "/git/" : value.startsWith("/uploads/") ? "/uploads/" : undefined;
  // Control characters (including NUL and line breaks), bidi and invisible format characters and backslashes have no place in a path.
  if (source === undefined || hasHiddenOrBackslash(value)) return false;
  const segments = value.slice(source.length).split("/");
  // `/uploads/` alone, `//`, `.` and `..` are not names.
  return segments.length > 0 && segments.every((s) => s !== "" && s !== "." && s !== "..");
}

/** Where a doc comes from, by the documented prefix of its path. */
export function docSource(path: string): DocSource | undefined {
  if (path.startsWith("/git/")) return "git";
  if (path.startsWith("/uploads/")) return "upload";
  return undefined;
}

/** The last segment of the path: the name of the file. */
export function docFileName(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}

// `/docs/:slug` carries the doc in one path segment: the base64url encoding (RFC 4648 section 5, no padding) of the doc path in
// UTF-8. The router's check for the segment is `[A-Za-z0-9_-]{1,2048}` (src/routes/table.ts), which is exactly that alphabet.

/** The slug of a doc path, for `/docs/:slug`. */
export function slugFromDocPath(path: string): string {
  const bytes = new TextEncoder().encode(path);
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
}

/**
 * The doc path a slug stands for, or undefined. Strict: the slug must be the canonical encoding (what `slugFromDocPath`
 * writes), the bytes must be UTF-8, and the result must be a doc path. A slug in plain words is not an encoded path.
 */
export function docPathFromSlug(slug: string): string | undefined {
  if (!/^[A-Za-z0-9_-]+$/.test(slug)) return undefined;
  let path: string;
  try {
    const padded = slug.replaceAll("-", "+").replaceAll("_", "/").padEnd(Math.ceil(slug.length / 4) * 4, "=");
    const binary = atob(padded);
    const bytes = Uint8Array.from(binary, (c) => c.charCodeAt(0));
    path = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    return undefined;
  }
  // Canonical: a slug with spare bits set, or one that a different path also decodes from, is refused.
  if (slugFromDocPath(path) !== slug) return undefined;
  return isDocPath(path) ? path : undefined;
}

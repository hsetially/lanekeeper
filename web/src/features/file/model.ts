import type { DriftState, EolStyle, FileContent, TextEncoding } from "@/api/generated/model";
import { hasControlChar } from "@/lib/text/control";

// The plain work of the file view: where a path splits, which language the editor colours, how a line ending, an encoding and a
// size are spelled, and which values of the page's search are taken at all (the URL is untrusted input).

/** A file with more lines than this opens read-only first, so the page stays fast (the design's "Large file" banner). */
export const LARGE_FILE_LINES = 2000;

export function splitPath(path: string): { dir: string; name: string } {
  const at = path.lastIndexOf("/");
  return at < 0 ? { dir: "", name: path } : { dir: path.slice(0, at), name: path.slice(at + 1) };
}

const MAX_PATH = 1024;

/**
 * Whether a `path` search value could be an NfsPath (api/openapi.yaml): relative, at most 1024 characters, no `..` or `.` or empty
 * segment, no backslash or control character. The server parses it again; this only keeps a value that cannot be one off the wire.
 */
export function isPlausibleNfsPath(path: string): boolean {
  if (path === "" || path.length > MAX_PATH || path.startsWith("/") || path.includes("\\") || hasControlChar(path)) return false;
  return path.split("/").every((segment) => segment !== "" && segment !== "." && segment !== "..");
}

export type Language = "yaml" | "xml" | "ini" | "json" | "plaintext";

const LANGUAGES: Readonly<Record<string, Language>> = { yml: "yaml", yaml: "yaml", xml: "xml", properties: "ini", ini: "ini", json: "json" };

function extensionOf(name: string): string {
  const at = name.lastIndexOf(".");
  return at <= 0 ? "" : name.slice(at + 1).toLowerCase();
}

/** The editor's language for a file name: the four the editor ships (YAML, XML, properties, JSON) and plain text. */
export function languageOf(name: string): Language {
  return LANGUAGES[extensionOf(name)] ?? "plaintext";
}

const IMAGES: Readonly<Record<string, string>> = { jpg: "JPEG", jpeg: "JPEG", png: "PNG", gif: "GIF", webp: "WebP", bmp: "BMP", ico: "ICO" };

/** The kind of an image a browser can draw from a blob, or null. SVG is not drawn: it can carry script. */
export function imageKind(name: string): string | null {
  return IMAGES[extensionOf(name)] ?? null;
}

/** `tx-infinity-core` for `tx-infinity-api/tx-infinity-core.yml`: what the "Effective file for ..." marker names. */
export function effectiveStem(logicalFile: string): string {
  const { name } = splitPath(logicalFile);
  const at = name.lastIndexOf(".");
  return at <= 0 ? name : name.slice(0, at);
}

export function eolLabel(eol: EolStyle): string {
  return eol === "crlf" ? "CRLF" : eol === "lf" ? "LF" : "Mixed endings";
}

const ENCODINGS: Readonly<Record<TextEncoding, string>> = { utf8: "UTF-8", utf16le: "UTF-16 LE", utf16be: "UTF-16 BE", latin1: "Latin-1" };

export function encodingLabel(encoding: TextEncoding, bom: boolean): string {
  return bom ? `${ENCODINGS[encoding]} with BOM` : ENCODINGS[encoding];
}

export function formatSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 1024) return `${Math.max(0, Math.round(bytes))} B`;
  const unit = (value: number, name: string): string => `${Number.isInteger(value) || value >= 100 ? Math.round(value) : value.toFixed(1)} ${name}`;
  const kb = bytes / 1024;
  if (kb < 1024) return unit(kb, "KB");
  return unit(kb / 1024, "MB");
}

/** Lines of a text: a text that ends with a line ending has no empty last line. */
export function lineCount(text: string): number {
  if (text === "") return 0;
  const lines = text.split(/\r\n|\n|\r/);
  if (lines[lines.length - 1] === "") lines.pop();
  return lines.length;
}

export type Fact = { text: string; tone?: "info" };

/** The facts under the file's name: line ending, encoding, lines (or kind and size for a file that is not text). */
export function fileFacts(content: FileContent, name: string): Fact[] {
  if (content.class === "binary" || content.class === "denied" || content.text === null) {
    const kind = content.class === "binary" ? imageKind(name) : null;
    return [{ text: "Binary" }, ...(kind === null ? [] : [{ text: kind }]), { text: formatSize(content.size) }];
  }
  const lines = lineCount(content.text);
  return [
    content.format.eol === "mixed" ? { text: eolLabel("mixed"), tone: "info" } : { text: eolLabel(content.format.eol) },
    { text: encodingLabel(content.format.encoding, content.format.bom) },
    { text: `${lines.toLocaleString("en-US")} ${lines === 1 ? "line" : "lines"}` },
  ];
}

export type Tab = "nfs" | "git" | "baseline" | "diff";
export type Panel = "history" | "docs";

export function parseTab(value: string | undefined): Tab {
  return value === "git" || value === "baseline" || value === "diff" ? value : "nfs";
}

export function parsePanel(value: string | undefined): Panel | null {
  return value === "history" || value === "docs" ? value : null;
}

const MAX_LINE = 10_000_000;

/** The `line` search value: a positive whole number, or null. */
export function parseLine(value: string | undefined): number | null {
  if (value === undefined || !/^[1-9][0-9]{0,8}$/.test(value)) return null;
  const line = Number(value);
  return line <= MAX_LINE ? line : null;
}

/** How many files the swimlane's drift counts call unknown (what an adoption has to settle), or null when they do not say. */
export function unknownFileCount(counts: readonly (readonly [DriftState, number])[] | undefined): number | null {
  const found = counts?.find(([state]) => state === "unknown");
  return found !== undefined && Number.isInteger(found[1]) && found[1] >= 0 ? found[1] : null;
}

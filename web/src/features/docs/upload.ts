// Reading a file for the upload and replace dialogs. `createDoc` and `updateDoc` take JSON (the Markdown as a string, at most
// 2 MiB), so the browser reads the file itself. What it checks here are the three states the design draws for the drop zone,
// plus the two that make a file not text; the server checks the same again and has the last word.

/** The most a doc may be: `DocCreateRequest.markdown` has `maxLength: 2097152`, and the design says "up to 2 MB". */
export const MAX_UPLOAD_BYTES = 2 * 1024 * 1024;
const MAX_TITLE = 256;

export type UploadProblem = "wrong_type" | "too_large" | "empty" | "not_text";

export type UploadReading =
  | { ok: true; name: string; size: number; markdown: string; title: string }
  | { ok: false; name: string; size: number; problem: UploadProblem; head: string; message: string };

const PROBLEMS: Record<UploadProblem, { head: string; message: string }> = {
  wrong_type: { head: "Only Markdown (.md) files are accepted.", message: "Export the document as Markdown and choose it again." },
  too_large: { head: "File is over the 2 MB limit.", message: "Split it into smaller docs, one per feature, and upload them separately." },
  empty: { head: "The file is empty.", message: "Choose a file that has text in it." },
  not_text: { head: "The file is not text.", message: "A Markdown file is UTF-8 text. Export it again as UTF-8." },
};

/** 512 B, 18 KB, 3.4 MB: a doc's size as the design draws it. (The retention page counts in binary units, KiB to TiB, with grouping and "unknown": `formatBytes` of admin/sections/retention-model.ts. Two formats on purpose, not one function twice.) */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/** A title from a file name: no extension, separators as spaces, the first letter in capitals. */
export function titleFromFileName(name: string): string {
  const stem = name.replace(/\.md$/i, "");
  const words = stem.replace(/[-_.\s]+/g, " ").trim();
  if (words === "") return "Untitled";
  const title = words.charAt(0).toUpperCase() + words.slice(1);
  return Array.from(title).slice(0, MAX_TITLE).join("");
}

function refused(file: File, problem: UploadProblem): UploadReading {
  return { ok: false, name: file.name, size: file.size, problem, ...PROBLEMS[problem] };
}

/** Check a chosen file and read it. Never throws: whatever goes wrong is one of the four refusals. */
export async function readUpload(file: File): Promise<UploadReading> {
  if (!/\.md$/i.test(file.name)) return refused(file, "wrong_type");
  if (file.size > MAX_UPLOAD_BYTES) return refused(file, "too_large");
  if (file.size === 0) return refused(file, "empty");
  let bytes: ArrayBuffer;
  try {
    bytes = await file.arrayBuffer();
  } catch {
    return refused(file, "not_text");
  }
  // The size is checked again on what was read: a file can change between the choice and the read.
  if (bytes.byteLength > MAX_UPLOAD_BYTES) return refused(file, "too_large");
  let markdown: string;
  try {
    markdown = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    return refused(file, "not_text");
  }
  if (markdown.includes("\u0000")) return refused(file, "not_text");
  return { ok: true, name: file.name, size: bytes.byteLength, markdown, title: titleFromFileName(file.name) };
}

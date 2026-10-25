// The data of the masking scenarios (T5 slice C): the core file of the File page with its masked `clientSecret` (frames 1d, 1e and 1f of
// the File view board), a file with several kinds of mask, and the answers of `revealValue`. Synthetic; nothing here is a real tenant's
// configuration, and the "value" is the design's own made-up one. Loaded by Node (the e2e harness) as well as by Vite: relative imports
// with the `.ts` extension and `import type`.
import type { FileContent, Mask } from "../../../api/generated/model/index.ts";
import { coreFile, coreLines, joinLines, CORE_PATH, H_NFS, type FileKit } from "../file/data.ts";

/** The setting the mask of line 19 names, as the Reveal dialog shows it. */
export const SECRET_SETTING = "txInfinityOptions.remote-itm-teller.clientSecret";
/** What `revealValue` answers in the design (frame 1f). */
export const REVEALED_VALUE = "tellerClient-9f2c71";
export const REVEAL_PATH = "/api/v1/swimlanes/{id}/files/reveal";

const enc = new TextEncoder();

/** The byte offset of the first `needle` at or after `from`, as the server counts it (UTF-8, into the whole text). */
function byteOffset(text: string, needle: string, from = 0): number {
  return enc.encode(text.slice(0, text.indexOf(needle, from))).length;
}

/**
 * The core file with three kinds of mask: the design's `clientSecret` (it names its setting, so it can be revealed), a marker in a
 * line with multi-byte characters that names no setting (nothing can reveal it), and a marker the list does not name (recognised by
 * its form). CRLF, as the core file is.
 */
export function multiMaskFile(): FileKit {
  const base = coreFile({ masked: true });
  const lines = coreLines(412, { secret: "masked" }).map((line, i) => {
    if (i === 21) return '    apiKey: "é😀 [REDACTED: 40 chars, aws_access_key]"';
    if (i === 22) return "    fallbackToken: [REDACTED: 12 chars, jwt]";
    return line;
  });
  const text = joinLines(lines, "\r\n");
  const first = base.content.masks?.[0];
  const apiKey = "[REDACTED: 40 chars, aws_access_key]";
  const apiStart = byteOffset(text, apiKey);
  const masks: Mask[] = [
    ...(first === undefined ? [] : [{ ...first, start: byteOffset(text, "[REDACTED: 22 chars, generic_secret]"), end: byteOffset(text, "[REDACTED: 22 chars, generic_secret]") + enc.encode("[REDACTED: 22 chars, generic_secret]").length }]),
    { line: 22, start: apiStart, end: apiStart + enc.encode(apiKey).length },
  ];
  const content: FileContent = { ...base.content, hash: H_NFS, size: enc.encode(text).length, text, masks };
  return { ...base, path: CORE_PATH, content };
}

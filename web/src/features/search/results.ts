import type { TextHit } from "@/api/generated/model";
import { byteRangesToUtf16 } from "@/lib/text/byte-ranges";
import { isDocPath } from "@/features/docs/doc-path";

// What the Search screen makes of the server's answers. Text is untrusted (a line of a config file, a path, a snippet of a doc),
// so it is only ever split into pieces and shown through SafeText; nothing here parses it or builds a pattern from it (S11).

export type Segment = { text: string; mark: boolean };

/** A text cut into plain and marked pieces at half-open UTF-16 spans (touching spans make one marked piece). */
function cut(text: string, spans: ReadonlyArray<{ start: number; end: number }>): Segment[] {
  const out: Segment[] = [];
  let at = 0;
  for (const { start, end } of spans) {
    if (start > at) out.push({ text: text.slice(at, start), mark: false });
    out.push({ text: text.slice(start, end), mark: true });
    at = end;
  }
  if (at < text.length) out.push({ text: text.slice(at), mark: false });
  return out;
}

/** A line shortened around its first match: where it was cut, and the ranges as half-open UTF-16 spans of the text returned. */
export type Windowed = { text: string; ranges: Array<[number, number]>; cutBefore: boolean; cutAfter: boolean };

const MAX_LINE = 240;

/**
 * A long line is shown from a little before its first match, so that the match is on screen and not behind the ellipsis.
 * `ranges` are the server's UTF-8 byte offsets into `text`; the result's ranges are UTF-16 spans of the text it returns, whether
 * or not the line was cut.
 */
export function focusWindow(text: string, ranges: ReadonlyArray<readonly [number, number]>, lead: number): Windowed {
  const spans = byteRangesToUtf16(
    text,
    ranges.map(([start, end]) => ({ start, end })),
  );
  const pairs = (list: ReadonlyArray<{ start: number; end: number }>): Array<[number, number]> => list.map(({ start, end }) => [start, end]);
  if (text.length <= MAX_LINE) return { text, ranges: pairs(spans), cutBefore: false, cutAfter: false };
  const first = spans[0]?.start ?? 0;
  let from = Math.max(0, first - lead);
  // Do not start on the low half of a surrogate pair, nor end after the high half of one.
  if (from > 0 && text.charCodeAt(from) >= 0xdc00 && text.charCodeAt(from) <= 0xdfff) from += 1;
  let to = Math.min(text.length, from + MAX_LINE);
  if (to < text.length && text.charCodeAt(to - 1) >= 0xd800 && text.charCodeAt(to - 1) <= 0xdbff) to -= 1;
  const inside = spans.filter((s) => s.start >= from && s.end <= to).map((s) => ({ start: s.start - from, end: s.end - from }));
  return { text: text.slice(from, to), ranges: pairs(inside), cutBefore: from > 0, cutAfter: to < text.length };
}

/** A hit's line ready to draw: shortened around its first match, cut into plain and marked pieces. */
export function windowedHit(text: string, ranges: ReadonlyArray<readonly [number, number]>, lead = 48): { segments: Segment[]; cutBefore: boolean; cutAfter: boolean } {
  const window = focusWindow(text, ranges, lead);
  return { segments: cut(window.text, window.ranges.map(([start, end]) => ({ start, end }))), cutBefore: window.cutBefore, cutAfter: window.cutAfter };
}

/** `text` with every occurrence of any term marked, ignoring case. A term is plain text, never a pattern. */
export function termSegments(text: string, terms: readonly string[]): Segment[] {
  const needles = terms.map((t) => t.toLowerCase()).filter((t) => t !== "");
  if (text === "") return [];
  if (needles.length === 0) return [{ text, mark: false }];
  const lower = text.toLowerCase();
  // `toLowerCase` can change the length of a string (for example "İ"); marks are only trusted when it did not.
  if (lower.length !== text.length) return [{ text, mark: false }];
  const spans: Array<{ start: number; end: number }> = [];
  for (let i = 0; i < lower.length; ) {
    let hit = 0;
    for (const needle of needles) if (needle.length > hit && lower.startsWith(needle, i)) hit = needle.length;
    if (hit > 0) {
      spans.push({ start: i, end: i + hit });
      i += hit;
    } else i += 1;
  }
  return cut(text, spans);
}

export type SnippetPart = Segment & { code: boolean };

/** A doc snippet is Markdown source: the text between a pair of backticks is code, and the query's words are marked in both. */
export function snippetSegments(snippet: string, terms: readonly string[]): SnippetPart[] {
  const parts: SnippetPart[] = [];
  const pieces = snippet.split(/(`[^`\n]+`)/);
  for (const piece of pieces) {
    if (piece === "") continue;
    const code = piece.length > 2 && piece.startsWith("`") && piece.endsWith("`");
    const body = code ? piece.slice(1, -1) : piece;
    for (const segment of termSegments(body, terms)) parts.push({ ...segment, code });
  }
  return parts;
}

export type SnippetRun = { code: boolean; segments: Segment[] };

/** The snippet as runs: each stretch of code is ONE run, with the query's words marked inside it. */
export function snippetRuns(snippet: string, terms: readonly string[]): SnippetRun[] {
  const runs: SnippetRun[] = [];
  for (const part of snippetSegments(snippet, terms)) {
    const last = runs[runs.length - 1];
    if (last && last.code === part.code && part.code) last.segments.push({ text: part.text, mark: part.mark });
    else runs.push({ code: part.code, segments: [{ text: part.text, mark: part.mark }] });
  }
  return runs;
}

export type HitFileKind = "tenant" | "base";
export type HitRow = { id: string; hit: TextHit };
export type FileGroup = { id: string; path: string; kind: HitFileKind; hits: HitRow[] };
export type LaneGroup = { id: string; swimlane: string; hits: number; files: FileGroup[] };

/** The hits grouped by swimlane and then by file, in the order the server sent them (swimlane order, then path). */
export function groupTextHits(hits: readonly TextHit[]): LaneGroup[] {
  const lanes: LaneGroup[] = [];
  const laneAt = new Map<string, LaneGroup>();
  const fileAt = new Map<string, FileGroup>();
  hits.forEach((hit, index) => {
    let lane = laneAt.get(hit.swimlane);
    if (lane === undefined) {
      lane = { id: hit.swimlane, swimlane: hit.swimlane, hits: 0, files: [] };
      laneAt.set(hit.swimlane, lane);
      lanes.push(lane);
    }
    const fileKey = `${hit.swimlane}\u0000${hit.path}`;
    let file = fileAt.get(fileKey);
    if (file === undefined) {
      file = { id: `${hit.swimlane}: ${hit.path}`, path: hit.path, kind: hit.tenant === null || hit.tenant === undefined ? "base" : "tenant", hits: [] };
      fileAt.set(fileKey, file);
      lane.files.push(file);
    }
    lane.hits += 1;
    // The index makes the id the hit's place in the answer: two hits on one line (the server may send them) stay two rows.
    file.hits.push({ id: `hit:${hit.swimlane}:${hit.path}:${hit.line}:${index}`, hit });
  });
  return lanes;
}

/** `tx-infinity-api/tx-infinity-core.yml` as the folder and the name. */
export function fileParts(path: string): { dir: string; name: string } {
  const at = path.lastIndexOf("/");
  return at < 0 ? { dir: "", name: path } : { dir: path.slice(0, at), name: path.slice(at + 1) };
}

const HEADING_ID = /^doc-[a-z0-9]+(-[a-z0-9]+)*$/;

/**
 * The address of the reader for a doc, at a heading. The heading id is the server's (`DocHit.anchor`) and is used only if it has
 * the shape the contract gives a heading id; a client never builds one from a title. A path that is not a doc path gives no link.
 */
export function docHref(path: string, anchor: string | null | undefined): string | undefined {
  if (!isDocPath(path)) return undefined;
  const base = `/docs/view?path=${encodeURIComponent(path)}`;
  return anchor !== null && anchor !== undefined && HEADING_ID.test(anchor) && anchor.length <= 80 ? `${base}#${anchor}` : base;
}

/** `1 match`, `3 matches`, `1,234 matches`. */
export function pluralise(count: number, one: string, many: string): string {
  return `${count.toLocaleString("en-US")} ${count === 1 ? one : many}`;
}

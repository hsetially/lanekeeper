import type { GridCell } from "@/api/generated/model";
import { matchMarker } from "@/components/masking/marker";
import { hasControlChar } from "@/lib/text/control";
import { hrefFor } from "@/routes/links";
import { routeById } from "@/routes/table";

// What the compare screen knows without asking the server: how the address is read and written, what kind of value a cell
// holds, which cells of a row stand apart from the others, and how many pages a refresh has to read. All of it is plain text
// and numbers. Nothing here decides a fact about a file (the grid, the rows that differ and every diff come from the server).

/** The most swimlanes the grid shows at once. The contract allows 64; past 40 the values stop being readable (the design). */
export const MAX_GRID_LANES = 40;
/** The most swimlane ids read from an address. Longer is a mistake or an attack, and the screen says "too many" past 40. */
const MAX_URL_LANES = 200;

const SWIMLANE = /^[a-z0-9][a-z0-9-]{0,62}$/;
const CHANNEL = /^[A-Za-z0-9_][A-Za-z0-9._-]{0,127}$/;
// A side of a comparison (CompareRef in api/openapi.yaml). Control characters are refused on top of the contract's pattern.
const REF = /^(?:(?:nfs|baseline|effective):[a-z0-9][a-z0-9-]{0,62}|git:(?:base|tenant)@.{1,255})$/;


export function isSwimlaneId(text: string): boolean {
  return SWIMLANE.test(text);
}

/** The swimlane ids of `lanes=a,b,c`: valid ones only, once each, in order, at most 200. */
export function parseLanes(text: string | undefined): string[] {
  if (text === undefined || text === "") return [];
  const seen = new Set<string>();
  for (const part of text.split(",", MAX_URL_LANES * 2)) {
    const id = part.trim();
    if (SWIMLANE.test(id)) seen.add(id);
    if (seen.size >= MAX_URL_LANES) break;
  }
  return [...seen];
}

/** A base-relative path for the server to look up: no leading slash, no `..` or empty segment, no backslash, no control character. */
export function isLogicalFile(text: string): boolean {
  if (text.length === 0 || text.length > 1024 || text.startsWith("/") || text.includes("\\") || hasControlChar(text)) return false;
  return text.split("/").every((segment) => segment !== "" && segment !== ".." && segment !== ".");
}

export function isChannelName(text: string): boolean {
  return CHANNEL.test(text);
}

export function isCompareRef(text: string): boolean {
  return text.length <= 300 && REF.test(text) && !hasControlChar(text);
}

export type RefView = {
  kind: "swimlane" | "git";
  /** The swimlane id, or the branch, tag or commit. */
  name: string;
  source: "NFS" | "Baseline" | "Effective" | "Git";
  /** For a Git side, which repository. */
  repo: "base" | "tenant" | null;
};

/** What a side is. The text must be a valid reference (`isCompareRef`). */
export function describeRef(ref: string): RefView {
  if (ref.startsWith("git:")) {
    const at = ref.indexOf("@");
    return { kind: "git", name: ref.slice(at + 1), source: "Git", repo: ref.startsWith("git:tenant@") ? "tenant" : "base" };
  }
  const colon = ref.indexOf(":");
  const kind = ref.slice(0, colon);
  const source = kind === "nfs" ? "NFS" : kind === "baseline" ? "Baseline" : "Effective";
  return { kind: "swimlane", name: ref.slice(colon + 1), source, repo: null };
}

export type CompareMode = "grid" | "lanes";

/** Everything the screen shows depends on this. The address holds the part of it that a link should keep. */
export type CompareView = {
  mode: CompareMode;
  /** The swimlanes of the grid, in column order. */
  lanes: string[];
  /** The logical file: the grid's file, and the file chosen in the tree of a whole-swimlane comparison. */
  file: string | null;
  onlyDiff: boolean;
  channel: string | null;
  /** The sides of a whole-swimlane comparison, as references. */
  left: string | null;
  right: string | null;
};

export type SearchRecord = Readonly<Record<string, string | undefined>>;

/** The view an address describes. A value that is not valid is the same as one that is not there. */
export function parseView(search: SearchRecord): CompareView {
  const file = search["file"];
  const channel = search["channel"];
  const left = search["left"];
  const right = search["right"];
  const leftRef = left !== undefined && isCompareRef(left) ? left : null;
  const rightRef = right !== undefined && isCompareRef(right) ? right : null;
  return {
    mode: leftRef !== null || rightRef !== null ? "lanes" : "grid",
    lanes: parseLanes(search["lanes"]),
    file: file !== undefined && isLogicalFile(file) ? file : null,
    onlyDiff: search["only"] !== "all",
    channel: channel !== undefined && isChannelName(channel) ? channel : null,
    left: leftRef,
    right: rightRef,
  };
}

/**
 * The address of a view. Only the keys the route reads are written (a key it does not read would throw, and would be dropped
 * from the address anyway), and a default is left out.
 */
export function compareHref(view: CompareView): string {
  const allowed = new Set(routeById("compare").search ?? []);
  const wanted: Record<string, string | undefined> =
    view.mode === "grid"
      ? { lanes: view.lanes.length === 0 ? undefined : view.lanes.join(","), file: view.file ?? undefined, only: view.onlyDiff ? undefined : "all", channel: view.channel ?? undefined }
      : { left: view.left ?? undefined, right: view.right ?? undefined, file: view.file ?? undefined };
  const kept: Record<string, string | undefined> = {};
  for (const [key, value] of Object.entries(wanted)) if (allowed.has(key)) kept[key] = value;
  return hrefFor("compare", {}, kept);
}

// ---------------------------------------------------------------------------------------------------------------------------
// Cells

export type CellKind = "value" | "empty" | "absent" | "hidden";

/**
 * A value the secret scan flagged arrives as its marker, never as the value (S14b). The shape is the contract's own:
 * `[REDACTED: N chars, rule]`. A value that only looks like one (extra text around it) is a value.
 */
export function hiddenMarker(text: string): { chars: number; rule: string } | null {
  const marker = matchMarker(text);
  return marker === null ? null : { chars: Number(marker.digits), rule: marker.rule };
}

export function cellKind(cell: GridCell): CellKind {
  const value = cell.value;
  if (value === null) return "absent";
  if (value.text === "") return "empty";
  return hiddenMarker(value.text) === null ? "value" : "hidden";
}

const ABSENT_KEY = "\u0000absent";
const EMPTY_KEY = "\u0000empty";

export type RowFacts = {
  /** The cells (by column index) that are not the value most of the row has. */
  readonly differing: ReadonlySet<number>;
  /** The row has at least one hidden value, so the page cannot say that its cells agree. */
  readonly hidden: boolean;
};
// Keyed by the row's own array of cells and held weakly: when the rows are replaced, so are the answers. (Not typed with the
// generated type: this is a cache of what the page worked out, not of what the server sent.)
const kept = new WeakMap<object, RowFacts>();

/**
 * What stands out in a row. The cells that are not the value most of the row has are the ones the grid marks. An absent key
 * and an empty value are values of their own, `yes` is not `true`, and the type tag counts. A HIDDEN value is not looked at:
 * the page has only the marker (its length and rule), and two markers that read alike may hide two different values. Whether
 * two hidden values are equal is the server's to say (the row is in the list or not; the diff of the two cells says it), so a
 * hidden cell is neither marked nor counted, and a row that has one is never called "the same everywhere". The answer is kept
 * for the row, so a row that scrolls back into view costs nothing.
 */
export function rowFacts(cells: readonly GridCell[]): RowFacts {
  const cached = kept.get(cells);
  if (cached) return cached;
  let hidden = false;
  const keys: Array<string | null> = cells.map((cell) => {
    const kind = cellKind(cell);
    if (kind === "hidden") {
      hidden = true;
      return null;
    }
    if (kind === "absent") return ABSENT_KEY;
    if (kind === "empty") return EMPTY_KEY;
    return `${cell.value?.ty ?? ""}:${cell.value?.text ?? ""}`;
  });
  const counts = new Map<string, number>();
  for (const key of keys) if (key !== null) counts.set(key, (counts.get(key) ?? 0) + 1);
  const differing = new Set<number>();
  if (counts.size > 1) {
    let modal: string | null = null;
    let most = 0;
    for (const [key, count] of counts) {
      if (count > most) {
        most = count;
        modal = key;
      }
    }
    keys.forEach((key, index) => {
      if (key !== null && key !== modal) differing.add(index);
    });
  }
  const facts: RowFacts = { differing, hidden };
  kept.set(cells, facts);
  return facts;
}

/** `a.b.c` as the part to dim and the part to stress. A dot inside an index or a quoted key is not a split point. */
export function splitPath(path: string): { parent: string; leaf: string } {
  let depth = 0;
  let quoted = false;
  let last = -1;
  for (let i = 0; i < path.length; i++) {
    const ch = path[i];
    if (ch === '"' && depth > 0) quoted = !quoted;
    else if (quoted) continue;
    else if (ch === "[") depth += 1;
    else if (ch === "]") depth = Math.max(0, depth - 1);
    else if (ch === "." && depth === 0) last = i;
  }
  return last < 0 ? { parent: "", leaf: path } : { parent: path.slice(0, last + 1), leaf: path.slice(last + 1) };
}

// ---------------------------------------------------------------------------------------------------------------------------
// Pages

export type PageSizes = { readonly first: number; readonly next: number };

/**
 * How many pages of the grid a refresh reads: those that reach the last row the person can see, and no more. The first page
 * is small and the others are large (`sizes`), so the rows that `n` pages hold are `first + (n - 1) * next`. The rows below
 * the window are dropped and read again when the person scrolls to them. At least one page; never more than are loaded.
 */
export function pagesToKeep(lastVisibleRow: number, loadedPages: number, sizes: PageSizes): number {
  const needed = Math.max(0, lastVisibleRow) + 1;
  const pages = needed <= sizes.first ? 1 : 1 + Math.ceil((needed - sizes.first) / sizes.next);
  return Math.max(1, Math.min(pages, loadedPages));
}

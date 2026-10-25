import { memo } from "react";
import type { GridCell, GridRow } from "@/api/generated/model";
import { SafeText } from "@/components/app/SafeText";
import { visibleText } from "@/components/app/safe-text";
import { SafeCell } from "@/components/app/virtual/SafeCell";
import { Lock } from "@/icons";
import { cn } from "@/lib/utils";
import { cellKind, hiddenMarker, splitPath } from "../model";

// The cells of the grid. A cell is drawn tens of thousands of times in a scroll (P10), so these are plain elements, memoised,
// with no tooltip or popover of their own. What a cell says is in words and not in shade alone: the value, `absent` for a key
// that is not in the file, `""` for a value that is blank, and `Hidden` for a value the secret scan flagged (the page has
// only its marker, never the value). A cell that stands apart from its row has a heavier weight as well as a fill.

const BOOL_HINT = "reads as a boolean in YAML 1.1; not equal to true or false";

/**
 * The native tooltip of a cell: the whole value when the cell may cut it short (past a dozen characters or so), and always the
 * hint for a value that only looks like a boolean. A tooltip is plain text, so hidden and right-to-left characters in it are
 * written out (`visibleText`); a short value, which is the usual one, costs nothing.
 */
function titleOf(text: string, isValue: boolean, bool: boolean): string | undefined {
  if (bool) return `${visibleText(text)}: ${BOOL_HINT}`;
  return isValue && text.length > 14 ? visibleText(text) : undefined;
}

/** The setting path: the parents dimmed, the leaf stressed (the design's `Setting path` column). */
export const PathCell = memo(function PathCell({ path }: { path: string }) {
  const { parent, leaf } = splitPath(path);
  return (
    <span className="flex min-w-0 items-baseline font-mono text-xs">
      {parent === "" ? null : (
        <span className="min-w-0 truncate text-muted-foreground">
          <SafeText text={parent} />
        </span>
      )}
      <SafeText text={leaf} className="max-w-full shrink-0 truncate font-medium" />
    </span>
  );
});

type ValueCellProps = {
  row: GridRow;
  /** The column of the cell among the swimlanes (0-based). */
  lane: number;
  cell: GridCell;
  /** The cell is not the value most of the row has. */
  differs: boolean;
  /** The row has the same value everywhere (shown in the muted colour, as the design does for the rows that do not differ). */
  muted: boolean;
  picked: boolean;
  /** The swimlane's agent is offline: its values are those of the last scan. */
  offline: boolean;
  onPick: (row: GridRow, lane: number) => void;
};

export const ValueCell = memo(function ValueCell({ row, lane, cell, differs, muted, picked, offline, onPick }: ValueCellProps) {
  const kind = cellKind(cell);
  const text = cell.value?.text ?? "";
  const marker = kind === "hidden" ? hiddenMarker(text) : null;
  const bool = kind === "value" && cell.value?.flag === "yaml_boolean_like";
  return (
    <button
      type="button"
      // The keyboard reaches a cell through its row: Enter on the row opens the pick bar (the grid's own keys are the row's).
      tabIndex={-1}
      aria-pressed={picked}
      data-kind={kind}
      data-differs={differs || undefined}
      data-offline={offline || undefined}
      {...(marker === null ? {} : { "aria-label": `Hidden value, ${marker.chars} characters, rule ${marker.rule}` })}
      title={titleOf(text, kind === "value", bool)}
      onClick={() => {
        onPick(row, lane);
      }}
      className={cn(
        "inline-flex h-6 max-w-full min-w-0 cursor-pointer items-center gap-1 rounded-sm border border-transparent px-1 text-left font-mono text-xs",
        muted ? "text-muted-foreground" : "text-foreground",
        differs && "bg-diff-mod-bg font-medium text-foreground",
        kind === "absent" && "bg-diff-empty italic text-muted-foreground",
        kind === "hidden" && "text-muted-foreground",
        bool && "underline decoration-dotted underline-offset-2",
        offline && "border-dashed border-input text-muted-foreground",
        picked && "border-solid border-primary ring-1 ring-primary",
      )}
    >
      {kind === "absent" ? (
        "absent"
      ) : kind === "empty" ? (
        <>
          <span>&quot;&quot;</span>
          <span className="sr-only"> empty</span>
        </>
      ) : kind === "hidden" ? (
        <>
          <Lock aria-hidden="true" className="size-3 shrink-0" />
          <span>Hidden</span>
        </>
      ) : (
        <>
          <span className="min-w-0 truncate">
            <SafeCell text={text} />
          </span>
          {bool ? <span className="sr-only"> ({BOOL_HINT})</span> : null}
        </>
      )}
      {differs ? <span className="sr-only"> (differs)</span> : null}
    </button>
  );
});

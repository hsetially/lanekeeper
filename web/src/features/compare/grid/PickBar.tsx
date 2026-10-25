import type { GridRow } from "@/api/generated/model";
import { SafeText } from "@/components/app/SafeText";
import { Button } from "@/components/ui/button";
import { NativeSelect } from "@/components/ui/native-select";
import { ArrowLeftRight, Columns2, X } from "@/icons";
import { cellKind, hiddenMarker } from "../model";
import type { LaneColumn } from "./lanes";

// The bar under the toolbar once a cell is picked: which two swimlanes are being compared at which setting. The cells of the
// grid pick with the pointer (click one, then another in the same row); the two lists pick with the keyboard too, and are the
// way to reach a swimlane that is off screen in a grid of forty columns.

type Props = {
  row: GridRow;
  lanes: readonly LaneColumn[];
  a: number | null;
  b: number | null;
  drawerOpen: boolean;
  onA: (index: number | null) => void;
  onB: (index: number | null) => void;
  onToggleDrawer: () => void;
  onClear: () => void;
};

/** What a cell says, short: the value, absent, "" or hidden (with its length). */
function shortValue(row: GridRow, index: number): string {
  const cell = row.cells[index];
  if (cell === undefined) return "";
  switch (cellKind(cell)) {
    case "absent":
      return "absent";
    case "empty":
      return '""';
    case "hidden":
      return `hidden, ${hiddenMarker(cell.value?.text ?? "")?.chars ?? 0} chars`;
    case "value":
      return cell.value?.text ?? "";
  }
}

function Pick({ letter, label, lanes, row, value, onChange }: { letter: "A" | "B"; label: string; lanes: readonly LaneColumn[]; row: GridRow; value: number | null; onChange: (index: number | null) => void }) {
  return (
    <span className="inline-flex min-w-0 items-center gap-1.5">
      <span aria-hidden="true" className="inline-flex size-4 shrink-0 items-center justify-center rounded-sm bg-primary text-xs font-semibold text-primary-foreground">
        {letter}
      </span>
      <NativeSelect
        aria-label={label}
        value={value === null ? "" : String(value)}
        onChange={(event) => {
          onChange(event.target.value === "" ? null : Number(event.target.value));
        }}
        className="h-6 w-32 font-mono text-xs"
      >
        <option value="">Choose…</option>
        {lanes.map((lane, index) => (
          <option key={lane.id} value={index}>
            {lane.id}
          </option>
        ))}
      </NativeSelect>
      {value === null ? null : <SafeText text={shortValue(row, value)} className="max-w-48 truncate font-mono text-xs" />}
    </span>
  );
}

export function PickBar({ row, lanes, a, b, drawerOpen, onA, onB, onToggleDrawer, onClear }: Props) {
  const both = a !== null && b !== null && a !== b;
  return (
    <div role="status" className="relative z-[5] flex min-h-9 flex-none flex-wrap items-center gap-2 border-b border-info-border bg-info-bg px-3 py-1 text-info-fg">
      <span className="text-xs">
        Comparing at <SafeText text={row.path} className="font-mono font-medium" />
      </span>
      <Pick letter="A" label="Swimlane A" lanes={lanes} row={row} value={a} onChange={onA} />
      <ArrowLeftRight aria-hidden="true" className="size-3.5 shrink-0" />
      <Pick letter="B" label="Swimlane B" lanes={lanes} row={row} value={b} onChange={onB} />
      {both ? null : <span className="text-xs">{a === null && b === null ? "Click a cell, or choose two swimlanes." : "Click another cell in the same row, or choose the second swimlane."}</span>}
      <span className="ml-auto inline-flex items-center gap-2">
        {both ? (
          <Button size="sm" onClick={onToggleDrawer} aria-expanded={drawerOpen}>
            <Columns2 data-icon="inline-start" />
            {drawerOpen ? "Hide diff" : "Open diff"}
          </Button>
        ) : null}
        <Button variant="ghost" size="sm" onClick={onClear} className="text-inherit">
          <X data-icon="inline-start" />
          Clear
        </Button>
      </span>
    </div>
  );
}

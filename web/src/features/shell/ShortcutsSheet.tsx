import { Fragment } from "react";
import type { Role } from "@/api/generated/model";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import { Kbd } from "@/components/ui/kbd";
import { rowCaps, type ShortcutId } from "@/keyboard";
import type { RouteId } from "@/routes/props";
import { mayOpen } from "./nav";

// The shortcuts sheet (the Shell, frames 1e and 1f): the design's four groups in two columns. Each line names a ROW of the
// keymap and takes its keys from it (src/keyboard/rows.ts), so this sheet cannot show a key that is not bound, and a key
// the role cannot use (Audit, for a Viewer) is not listed. It is a chunk of its own, loaded when it is first opened.

type Line = {
  readonly label: string;
  readonly row: ShortcutId;
  /** Which bindings of the row to show, in order. Default the first. */
  readonly bindings?: readonly number[];
  /** The screen the key goes to: a line is hidden from a role that may not open it. */
  readonly route?: RouteId;
};

export const SHEET: readonly { readonly title: string; readonly items: readonly Line[] }[] = [
  {
    title: "Global",
    items: [
      { label: "Command palette", row: "global.command-palette" },
      { label: "Search text", row: "global.search-text-focus-query" },
      { label: "Show this sheet", row: "global.shortcuts-sheet" },
      { label: "Toggle sidebar", row: "global.toggle-sidebar" },
      { label: "Close dialog or menu", row: "global.close-dialog-menu-or-palette" },
    ],
  },
  {
    title: "Go to",
    items: [
      { label: "Swimlanes", row: "go-to.swimlanes-compare-feature-status-findings", bindings: [0] },
      { label: "Compare", row: "go-to.swimlanes-compare-feature-status-findings", bindings: [1] },
      { label: "Feature status", row: "go-to.swimlanes-compare-feature-status-findings", bindings: [2] },
      { label: "Findings", row: "go-to.swimlanes-compare-feature-status-findings", bindings: [3] },
      { label: "Audit", row: "go-to.swimlanes-compare-feature-status-findings", bindings: [4], route: "audit" },
    ],
  },
  {
    title: "Diff navigation",
    items: [
      { label: "Next change", row: "diff.next-previous-change", bindings: [0] },
      { label: "Previous change", row: "diff.next-previous-change", bindings: [1] },
      { label: "Toggle side by side / unified", row: "diff.toggle-split-and-unified" },
    ],
  },
  {
    title: "Lists and trees",
    items: [
      { label: "Move selection", row: "lists-tables-trees.move-selection", bindings: [0, 1] },
      { label: "Open", row: "lists-tables-trees.open" },
      { label: "Open in new tab", row: "lists-tables-trees.open-in-new-tab" },
    ],
  },
];

function Caps({ line }: { line: Line }) {
  const groups = (line.bindings ?? [0]).map((index) => rowCaps(line.row, index));
  return (
    <span className="inline-flex items-center gap-0.5">
      {groups.map((caps, g) => (
        <Fragment key={g}>
          {caps.map((cap, i) => (cap === "then" ? <span key={i} className="px-0.5 text-xs text-muted-foreground">then</span> : <Kbd key={i} className="text-foreground">{cap}</Kbd>))}
        </Fragment>
      ))}
    </span>
  );
}

export default function ShortcutsSheet({ open, onOpenChange, role }: { open: boolean; onOpenChange: (open: boolean) => void; role: Role | null }) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent size="xl" showCloseButton={false} className="gap-0 p-0">
        <div className="flex items-center justify-between border-b border-border px-4 py-3">
          <DialogTitle className="text-lg">Keyboard shortcuts</DialogTitle>
          <Kbd>Esc</Kbd>
        </div>
        <DialogDescription className="sr-only">The keys that work on every screen, and the ones for diffs, lists and trees.</DialogDescription>
        <div className="grid grid-cols-1 gap-x-10 gap-y-4 px-4 pt-3 pb-4 sm:grid-cols-2">
          {SHEET.map((group) => (
            <section key={group.title} className="flex flex-col gap-0.5">
              <h3 className="mb-0.5 flex h-6 items-center border-b border-border text-xs font-medium text-muted-foreground">{group.title}</h3>
              <ul className="m-0 flex list-none flex-col gap-0.5 p-0">
                {group.items
                  .filter((line) => line.route === undefined || mayOpen(role, line.route))
                  .map((line) => (
                    <li key={line.label} className="flex h-7 items-center justify-between gap-2">
                      <span>{line.label}</span>
                      <Caps line={line} />
                    </li>
                  ))}
              </ul>
            </section>
          ))}
        </div>
        <div className="flex h-8 items-center rounded-b-md border-t border-border bg-muted px-4 text-xs text-muted-foreground">
          Sequences: press the first key, release, then the second. Shortcuts pause while you type in a field.
        </div>
      </DialogContent>
    </Dialog>
  );
}

// Who executes each row of the keymap. The handoff has 34 rows and the reviewed list adds the screen-local ones (rows.ts);
// every one has an entry here, so that the keymap spec can generate one keyboard-only test per row and fail when a row has no
// owner (an unbound key is a documented shortcut that does nothing).
//
//  shell      bound by the app shell through `useShortcut` (src/features/shell): the registry decides it.
//  component  handled by a shared component on its own focus (the lists, the tree, the diff, the dialogs, the split pane,
//             the palette); the registry does not see these keys.
//  screen     bound by the screen's slice through `useShortcut`, which is the only way a slice binds a key.
//
// `by` names the thing that has to be tested for the row: a component or the slice (S1 to S8 of the T4 plan).

import type { ShortcutId } from "./rows";

export type Owner = "shell" | "component" | "screen";
export type RowOwner = { readonly owner: Owner; readonly by: string };

export const OWNERS: Record<ShortcutId, RowOwner> = {
  "global.command-palette": { owner: "shell", by: "AppShell" },
  "global.search-text-focus-query": { owner: "shell", by: "AppShell" },
  "global.shortcuts-sheet": { owner: "shell", by: "AppShell" },
  "global.toggle-sidebar": { owner: "shell", by: "AppShell" },
  "global.close-dialog-menu-or-palette": { owner: "component", by: "the Radix primitives (dialog, menu, popover) and the palette; a screen binds it only to cancel an inline edit" },
  "go-to.swimlanes-compare-feature-status-findings": { owner: "shell", by: "AppShell" },
  "lists-tables-trees.move-selection": { owner: "component", by: "VirtualList, DataTable and TreeView" },
  "lists-tables-trees.open": { owner: "component", by: "DataTable and TreeView" },
  "lists-tables-trees.open-in-new-tab": { owner: "component", by: "DataTable and TreeView" },
  "lists-tables-trees.select-row-extends-the-range": { owner: "component", by: "DataTable" },
  "lists-tables-trees.compare-the-selected-rows-row": { owner: "screen", by: "S2 swimlanes (Compare selected)" },
  "tree-only.expand-or-move-to-first": { owner: "component", by: "TreeView" },
  "tree-only.first-last-row": { owner: "component", by: "TreeView" },
  "tree-only.jump-to-the-next-row": { owner: "component", by: "TreeView (type-ahead)" },
  "swimlanes.move-and-open-a-swimlane": { owner: "screen", by: "S2 swimlanes, on DataTable" },
  "file-viewer.edit": { owner: "screen", by: "S3 file view" },
  "file-viewer.review-and-save-opens-the": { owner: "screen", by: "S3 file view" },
  "file-viewer.cancel-edit-asks-if-there": { owner: "screen", by: "S3 file view" },
  "diff.next-previous-change": { owner: "screen", by: "S3 file view (DiffPane binds n and p for the whole tab); DiffView handles them itself when it has focus (the compare drawer)" },
  "diff.toggle-split-and-unified": { owner: "screen", by: "S3 file view and S4 compare, on DiffView" },
  "diff.expand-unchanged-lines": { owner: "component", by: "DiffView" },
  "command-palette.move-and-run": { owner: "component", by: "CommandPalette" },
  "command-palette.run-the-nth-result": { owner: "component", by: "CommandPalette" },
  "command-palette.clear-the-group-filter": { owner: "component", by: "CommandPalette" },
  "search.focus-query": { owner: "screen", by: "S5 search" },
  "search.toggle-buttons-no-shortcut-assigned": { owner: "component", by: "no key assigned yet (the handoff leaves it open)" },
  "approvals.move-and-open-the-drawer": { owner: "screen", by: "S7 approvals, on DataTable" },
  "approvals.close-drawer-reject-needs-a": { owner: "screen", by: "S7 approvals" },
  "dialogs.trapped-inside-initial-focus-on": { owner: "component", by: "the Radix dialog primitives" },
  "dialogs.confirms-only-when-the-primary": { owner: "component", by: "ConsequenceDialog and DestructiveConfirm" },
  "split-pane-handle.resize-16px-64px-home-end": { owner: "component", by: "Resizable" },
  "local.filter.focus": { owner: "screen", by: "S2 swimlanes, S2 detail, S4 compare, S5 status" },
  "local.detail.filter-files": { owner: "screen", by: "S2 swimlane detail" },
};

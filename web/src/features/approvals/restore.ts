import type { RouterHistory } from "@tanstack/react-router";

// The queue and the proposal's drawer are two routes (the drawer has an address of its own), so the queue is drawn anew when the
// drawer closes, and its keyboard cursor would go back to nothing. The row that was open is kept in the HISTORY ENTRY of the
// queue, not in a variable: Escape, the Close button and the browser's Back all arrive at that entry, and a visit to the queue by
// the sidebar is another entry, which has no row. It is one number, never a proposal (nothing of the server's is kept here).

const KEY = "lkApprovalRow";

/** The id of the proposal whose drawer was open when the person left this entry, or null. */
export function rowOnReturn(history: RouterHistory): number | null {
  const value = (history.location.state as unknown as Record<string, unknown>)[KEY];
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : null;
}

/** Before the drawer opens: the queue entry the person is on remembers the row, so Back finds it. */
export function rememberRow(history: RouterHistory, id: number): void {
  history.replace(history.location.href, { [KEY]: id });
}

/** Closing the drawer: go to the queue (a new entry) with the row that was open. */
export function returnToQueue(history: RouterHistory, href: string, id: number | null): void {
  if (history.location.href === href) return;
  history.push(href, id === null ? undefined : { [KEY]: id });
}

// Helpers for the tests of the masking pieces: where a planted secret could be on a page, and the checks that say it is not. A test
// plants a fake secret in a fake hub; the page may hold it only where a test says it may (the one span of a revealed value), and these
// checks look everywhere else a value can hide: the DOM, every attribute and field, the accessible names, the query and mutation
// caches, web storage, the console, the toasts, and (in the tests that press them) the clipboard. Nothing here touches the network.
import type { QueryClient } from "@tanstack/react-query";
import { screen } from "@testing-library/react";
import { expect, vi } from "vitest";
import type { Mask } from "@/api/generated/model";
import { toasts } from "@/features/shell/toast/store";

const enc = new TextEncoder();
/** The length of `text` in UTF-8 bytes, counted with the platform encoder (not with the code under test). */
export const utf8 = (text: string): number => enc.encode(text).length;

/** The mask of the first `needle` in `text` on `line`, with byte offsets as the server counts them. */
export function maskOf(text: string, line: number, needle: string, extra: Partial<Mask> = {}): Mask {
  const start = utf8(text.slice(0, text.indexOf(needle)));
  return { line, start, end: start + utf8(needle), ...extra };
}

/** Every value reachable from `value` (to a depth), as one string. Cycles are cut. */
export function dump(value: unknown, depth = 0, seen = new WeakSet()): string {
  if (depth > 8 || typeof value !== "object" || value === null) return String(value);
  if (seen.has(value)) return "";
  seen.add(value);
  return Object.entries(value as Record<string, unknown>).map(([k, v]) => `${k}:${dump(v, depth + 1, seen)}`).join("|");
}

/** Everything the query client holds, as one string: keys, states, variables, results, errors. */
export function held(client: QueryClient): string {
  return [...client.getMutationCache().getAll().map((m) => dump(m.state) + dump(m.options.mutationKey)), ...client.getQueryCache().getAll().map((q) => dump(q.state) + dump(q.queryKey))].join("#");
}

/** Capture every console call of the test. Restore with `vi.restoreAllMocks()`. */
export function watchConsole(): unknown[][] {
  const logged: unknown[][] = [];
  for (const method of ["log", "info", "warn", "error", "debug", "trace"] as const) vi.spyOn(console, method).mockImplementation((...args: unknown[]) => void logged.push(args));
  return logged;
}

// The roles whose name comes from an attribute (a label, a title) or a label element, and the roles whose name comes from what they
// contain. A revealed value is, by design, in the names of the row and the cell that hold it.
const NAMED_BY_ATTRIBUTE = ["button", "link", "textbox", "img", "table", "dialog", "alertdialog", "group", "region", "tab", "treeitem", "tree", "listbox", "option"] as const;
const NAMED_BY_CONTENT = ["row", "cell", "gridcell", "status", "alert", "note", "term", "definition", "heading", "paragraph", "listitem"] as const;

/**
 * The page holds `secret` nowhere: not in the DOM, not in an attribute or a field, not in an accessible name, the caches, the console
 * or the toasts (a test checks web storage itself: the lint keeps the storage names out of this file). With `inRevealedSpan` it may be in exactly one place, the span that shows a revealed value, and that
 * span has no attribute but its class and its slot.
 */
export function expectNowhere(secret: string, client: QueryClient, logged: readonly unknown[][], options: { inRevealedSpan?: boolean } = {}): void {
  const html = document.documentElement.outerHTML;
  const occurrences = html.split(secret).length - 1;
  if (options.inRevealedSpan === true) {
    expect(occurrences, "the secret is in the DOM exactly once").toBe(1);
    const span = document.querySelector("[data-slot='revealed-value']");
    expect(span?.textContent).toContain(secret);
    expect(span?.getAttributeNames().sort()).toEqual(["class", "data-slot"]);
  } else {
    expect(occurrences, "the secret is not in the DOM").toBe(0);
  }
  for (const el of document.querySelectorAll("*")) {
    for (const name of el.getAttributeNames()) expect(el.getAttribute(name), `${el.tagName}[${name}]`).not.toContain(secret);
    if (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement) expect(el.value, `${el.tagName} value`).not.toContain(secret);
  }
  // Accessible names, as the browser computes them.
  const roles = options.inRevealedSpan === true ? NAMED_BY_ATTRIBUTE : [...NAMED_BY_ATTRIBUTE, ...NAMED_BY_CONTENT];
  for (const role of roles) expect(screen.queryAllByRole(role, { hidden: true, name: (name) => name.includes(secret) }), `accessible name of a ${role}`).toHaveLength(0);
  expect(held(client), "the query and mutation caches").not.toContain(secret);
  expect(JSON.stringify(logged), "the console").not.toContain(secret);
  expect(dump(toasts.getSnapshot()), "the toasts").not.toContain(secret);
}

/** A problem response, with the headers a test wants (`retry-after`). */
export const refusal = (status: number, code: string, headers: Record<string, string> = {}): Response =>
  new Response(JSON.stringify({ type: `urn:lanekeeper:problem:${code}`, title: code, status, request_id: "req-test-1" }), { status, headers: { "content-type": "application/problem+json", ...headers } });

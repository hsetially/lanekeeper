import { useEffect, type RefObject } from "react";
import { useRouter, useRouterState } from "@tanstack/react-router";
import type { Crumb } from "./crumbs";
import { crumbsFor } from "./crumbs";

// After navigation (WCAG 2.4.3, 4.1.3): focus goes to the new page's h1, so that a screen reader says where the person is and
// a keyboard user starts at the top of the new page instead of on a link that no longer exists. The page may be a chunk that
// has not rendered yet, so this watches the content area until an h1 appears (or gives up). The h1 is made focusable with
// tabindex -1: it is not in the tab order, and it shows no ring (src/styles/base.css), as it is not a control.

/** Focus the first h1 under `root` once there is one. Returns the function that stops waiting. */
export function focusHeadingWhenReady(root: HTMLElement, { timeoutMs = 3000 }: { timeoutMs?: number } = {}): () => void {
  let observer: MutationObserver | undefined;
  let timer: ReturnType<typeof setTimeout> | undefined;

  const stop = (): void => {
    observer?.disconnect();
    observer = undefined;
    if (timer !== undefined) clearTimeout(timer);
    timer = undefined;
  };

  const attempt = (): boolean => {
    const heading = root.querySelector<HTMLElement>("h1");
    if (!heading) return false;
    if (!heading.hasAttribute("tabindex")) heading.setAttribute("tabindex", "-1");
    heading.focus({ preventScroll: true });
    return true;
  };

  if (attempt()) return stop;
  observer = new MutationObserver(() => {
    if (attempt()) stop();
  });
  observer.observe(root, { childList: true, subtree: true });
  timer = setTimeout(stop, timeoutMs);
  return stop;
}

/**
 * Whether a rendered page takes focus: it does when the path changed AND there was a page before it. The first page of the document
 * has no `fromLocation`; focus there is left where the browser has it, so that the first Tab lands on the skip link (WCAG 2.4.1).
 */
export function shouldFocusHeading(event: { fromLocation?: unknown; pathChanged: boolean }): boolean {
  return event.pathChanged && event.fromLocation !== undefined;
}

/** The tab title: the page, then up to two places above it, then the product. */
export function documentTitle(crumbs: readonly Crumb[]): string {
  const parts = crumbs
    .map((c) => c.label)
    .reverse()
    .slice(0, 3);
  return [...parts, "Lanekeeper"].join(" · ");
}

/**
 * Move focus to the h1 of the new page whenever the PATH changes (a change of search keys on the same page keeps focus where it
 * is). It waits for the router's `onRendered`, which comes after the new page is on screen: at the moment the location changes
 * the old page is still there (the router keeps it until the new one, a lazy chunk, is ready), and its h1 would take the focus
 * and lose it a moment later. The first page of the document is not a navigation (`shouldFocusHeading`): the skip link is the first
 * stop of a page that has just loaded.
 */
export function useFocusHeadingOnNavigation(root: RefObject<HTMLElement | null>): void {
  const router = useRouter();
  useEffect(() => {
    let stop: (() => void) | undefined;
    const off = router.subscribe("onRendered", (event) => {
      stop?.();
      stop = undefined;
      const element = root.current;
      if (shouldFocusHeading(event) && element) stop = focusHeadingWhenReady(element);
    });
    return () => {
      off();
      stop?.();
    };
  }, [router, root]);
}

/** Keep the tab title in step with the route. */
export function useDocumentTitle(): void {
  const title = useRouterState({
    select: (s) => {
      const last = s.matches.at(-1);
      return documentTitle(crumbsFor(last?.staticData.routeId, (last?.params ?? {}) as Record<string, string>, (last?.search ?? {}) as Record<string, string>));
    },
  });
  useEffect(() => {
    document.title = title;
  }, [title]);
}

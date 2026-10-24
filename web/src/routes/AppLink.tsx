import { createElement, useCallback, type AnchorHTMLAttributes, type MouseEvent } from "react";
import { useRouter } from "@tanstack/react-router";
import type { AppRouter } from "@/router";
import { LinkError, linkTo } from "./links";
import type { ParamsOf, RouteId, SearchOf } from "./props";

// A link to a screen, built from the route table (`linkTo`: the parameters are checked and encoded, a renamed route does not
// compile). It is a real anchor, so "open in a new tab", "copy link address" and the status bar work, and a plain click is a
// route change inside the app instead of a page load. A parameter that is not valid (a path that came from the server and
// does not pass the route's check) makes no link: the text is shown, never a link that goes somewhere else.

export function isPlainClick(event: Pick<MouseEvent, "button" | "metaKey" | "ctrlKey" | "shiftKey" | "altKey" | "defaultPrevented">): boolean {
  return !event.defaultPrevented && event.button === 0 && !event.metaKey && !event.ctrlKey && !event.shiftKey && !event.altKey;
}

/**
 * Go to a path of the app as a route change, once. For code that has a href and not a link (a toast, the palette, a tab).
 * A tab, a toggle button or a menu item can ask for the same address twice in one gesture (a tab asks on the mouse press and
 * again when it takes focus, before the page has re-rendered), and each push is an entry of the history, so Back would need two
 * presses. The history's own location is current at once, so asking for where it already is is not a navigation.
 * The same function between renders, so an effect that lists it does not run again.
 */
export function useNavigateTo(): (href: string) => void {
  const router = useRouter<AppRouter>();
  return useCallback(
    (href) => {
      if (router.history.location.href !== href) router.history.push(href);
    },
    [router],
  );
}

/**
 * Change the address without adding a history entry: for a filter, a sort or a tab the person is still choosing, where Back
 * should leave the screen and not undo each keystroke or step through every tab. The href is built by the caller with `linkTo`, so
 * it is checked against the route table; this only moves to it.
 */
export function useReplaceTo(): (href: string) => void {
  const router = useRouter<AppRouter>();
  return useCallback(
    (href) => {
      router.history.replace(href);
    },
    [router],
  );
}

/**
 * Open a path of the app in a new tab (the keyboard's "Open in new tab": Control or Command and Enter on a row). The tab cannot
 * see this one (`noopener noreferrer`). Only a path of this app is opened: anything else (another origin, `//host`, a scheme) is
 * refused, so a value that came from the server can never take a person somewhere else.
 */
export function openInNewTab(href: string): void {
  if (!href.startsWith("/") || href.startsWith("//") || href.startsWith("/\\")) return;
  const link = document.createElement("a");
  link.href = href;
  link.target = "_blank";
  link.rel = "noopener noreferrer";
  link.click();
}

type Props<Id extends RouteId> = Omit<AnchorHTMLAttributes<HTMLAnchorElement>, "href"> & {
  route: Id;
  params?: ParamsOf<Id>;
  search?: SearchOf<Id>;
};

export function AppLink<Id extends RouteId>({ route, params, search, onClick, target, children, ...rest }: Props<Id>) {
  const navigate = useNavigateTo();
  let href: string;
  try {
    href = (linkTo as (id: string, p?: unknown, s?: unknown) => string)(route, params, search);
  } catch (error) {
    if (error instanceof LinkError) return <span className={rest.className}>{children}</span>;
    throw error;
  }
  // Built with createElement because the props are spread (aria-current, a class, a ref come from the caller), and the lint rule
  // for `target="_blank"` cannot see through a spread. What it protects is here in the open: every link carries
  // `rel="noopener noreferrer"`, so a link that opens another tab never lets that page see this one (the app sends no referrer
  // anyway, S12; on a link without a target `noopener` does nothing).
  return createElement(
    "a",
    {
      ...rest,
      href,
      rel: "noopener noreferrer",
      ...(target === undefined ? {} : { target }),
      onClick: (event: MouseEvent<HTMLAnchorElement>) => {
        onClick?.(event);
        if (target !== undefined && target !== "_self") return;
        if (!isPlainClick(event)) return;
        event.preventDefault();
        navigate(href);
      },
    },
    children,
  );
}

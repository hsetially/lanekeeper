import { act } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { documentTitle, focusHeadingWhenReady, shouldFocusHeading } from "./focus";

// After a route change focus goes to the new page's h1 (a screen reader says where you are, a keyboard user starts at the top
// of the new page), even when the page is a lazy chunk that has not rendered yet. The title says it too.

beforeEach(() => {
  vi.useFakeTimers();
  document.body.innerHTML = '<main id="m"><button id="old">old</button></main>';
});
afterEach(() => {
  vi.useRealTimers();
});

const main = () => document.getElementById("m") as HTMLElement;

describe("focusHeadingWhenReady", () => {
  it("focuses an h1 that is already there, and makes it focusable without putting it in the tab order", () => {
    main().insertAdjacentHTML("beforeend", "<h1>Swimlanes</h1>");
    focusHeadingWhenReady(main());
    const h1 = main().querySelector("h1") as HTMLElement;
    expect(document.activeElement).toBe(h1);
    expect(h1.getAttribute("tabindex")).toBe("-1");
  });

  it("waits for an h1 that renders later, and focuses it when it does", async () => {
    focusHeadingWhenReady(main());
    expect(document.activeElement).toBe(document.body);
    await act(async () => {
      main().insertAdjacentHTML("beforeend", "<div><h1>Files</h1></div>");
      await Promise.resolve();
    });
    expect(document.activeElement?.textContent).toBe("Files");
  });

  it("focuses only the first h1, once, and then stops watching", async () => {
    const stop = focusHeadingWhenReady(main());
    await act(async () => {
      main().insertAdjacentHTML("beforeend", "<h1>One</h1>");
      await Promise.resolve();
    });
    (main().querySelector("#old") as HTMLElement).focus();
    await act(async () => {
      main().insertAdjacentHTML("beforeend", "<h1>Two</h1>");
      await Promise.resolve();
    });
    expect(document.activeElement?.id).toBe("old");
    stop();
  });

  it("gives up after the timeout without touching focus", async () => {
    focusHeadingWhenReady(main(), { timeoutMs: 1000 });
    act(() => {
      vi.advanceTimersByTime(1500);
    });
    await act(async () => {
      main().insertAdjacentHTML("beforeend", "<h1>Late</h1>");
      await Promise.resolve();
    });
    expect(document.activeElement).toBe(document.body);
  });

  it("stops when told to (the route changed again before the page rendered)", async () => {
    const stop = focusHeadingWhenReady(main());
    stop();
    await act(async () => {
      main().insertAdjacentHTML("beforeend", "<h1>Never</h1>");
      await Promise.resolve();
    });
    expect(document.activeElement).toBe(document.body);
  });

  it("does not scroll the page to the heading", () => {
    main().insertAdjacentHTML("beforeend", "<h1>Swimlanes</h1>");
    const h1 = main().querySelector("h1") as HTMLElement;
    const focus = vi.spyOn(h1, "focus");
    focusHeadingWhenReady(main());
    expect(focus).toHaveBeenCalledWith({ preventScroll: true });
  });
});

describe("documentTitle", () => {
  it("names the page first and the product last", () => {
    expect(documentTitle([{ label: "Swimlanes" }])).toBe("Swimlanes · Lanekeeper");
    expect(documentTitle([{ label: "Swimlanes", href: "/swimlanes" }, { label: "sitb", href: "/swimlanes/sitb" }, { label: "Services" }])).toBe("Services · sitb · Swimlanes · Lanekeeper");
  });

  it("is just the product when there is nothing to say, and keeps at most three parts", () => {
    expect(documentTitle([])).toBe("Lanekeeper");
    const crumbs = ["a", "b", "c", "d", "e"].map((label) => ({ label }));
    expect(documentTitle(crumbs)).toBe("e · d · c · Lanekeeper");
  });
});

// The skip link has to be the first stop of a page that has just loaded (WCAG 2.4.1), so focus does not go to the h1 for the page the
// document opened on. It goes there for a page the person moved to inside the app. The router says which it is: the first page has
// no location it came from.
describe("shouldFocusHeading", () => {
  const at = (pathname: string) => ({ pathname, href: pathname, search: "", hash: "" });
  it("is false for the first page of the document, whatever its path", () => {
    expect(shouldFocusHeading({ fromLocation: undefined, pathChanged: true })).toBe(false);
  });
  it("is true for a page the person moved to, where the path changed", () => {
    expect(shouldFocusHeading({ fromLocation: at("/swimlanes"), pathChanged: true })).toBe(true);
  });
  it("is false for a change of search keys on the same path", () => {
    expect(shouldFocusHeading({ fromLocation: at("/compare"), pathChanged: false })).toBe(false);
  });
});

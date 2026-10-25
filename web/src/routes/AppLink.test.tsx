import { act, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { json, me, renderApp } from "@/test/app";
import { AppLink, openInNewTab, useNavigateTo, useReplaceTo } from "./AppLink";

// A link built from the route table: a real anchor (so open in a new tab, copy link and the status bar all work) whose plain
// click is a route change inside the app, never a page load. A parameter that is not valid makes no link at all.

const pages = {
  swimlanes: () =>
    Promise.resolve({
    default: () => (
      <div>
        <h1>Swimlanes page</h1>
        <AppLink route="files" params={{ id: "sit1" }} search={{ path: "a/b.yml" }}>
          open the file
        </AppLink>
        <AppLink route="swimlane" params={{ id: "../x" }}>
          bad link
        </AppLink>
        <AppLink route="compare" target="_blank">
          compare elsewhere
        </AppLink>
      </div>
    ),
  }),
};
const api = (call: { path: string }) => (call.path === "/api/v1/me" ? json(me("viewer")) : new Response("{}", { status: 404 }));

describe("AppLink", () => {
  it("is an anchor with the href the route table builds", async () => {
    const app = renderApp("/swimlanes", api, { pages });
    await screen.findByText("Swimlanes page");
    expect(screen.getByRole("link", { name: "open the file" })).toHaveAttribute("href", "/swimlanes/sit1/files?path=a%2Fb.yml");
    app.unmount();
  });

  it("changes the route on a plain click, without loading the page", async () => {
    const app = renderApp("/swimlanes", api, { pages });
    await screen.findByText("Swimlanes page");
    await userEvent.click(screen.getByRole("link", { name: "open the file" }));
    await app.settled();
    expect(app.location()).toBe("/swimlanes/sit1/files?path=a%2Fb.yml");
  });

  it("leaves a modified click and a link with a target to the browser", async () => {
    const app = renderApp("/swimlanes", api, { pages });
    await screen.findByText("Swimlanes page");
    // The app must not have cancelled either click (the browser then opens the link its own way). The listener records that and
    // cancels the click after the app, so that jsdom does not try to navigate.
    const cancelledByTheApp: boolean[] = [];
    const record = (event: MouseEvent): void => {
      cancelledByTheApp.push(event.defaultPrevented);
      event.preventDefault();
    };
    document.addEventListener("click", record);
    try {
      // One user-event instance, so that the held key is still held for the click.
      const user = userEvent.setup();
      await user.keyboard("{Control>}");
      await user.click(screen.getByRole("link", { name: "open the file" }));
      await user.keyboard("{/Control}");
      await user.click(screen.getByRole("link", { name: "compare elsewhere" }));
    } finally {
      document.removeEventListener("click", record);
    }
    expect(cancelledByTheApp).toEqual([false, false]);
    expect(app.location()).toBe("/swimlanes");
  });

  it("makes no link for a parameter that is not valid, and shows its text", async () => {
    renderApp("/swimlanes", api, { pages });
    await screen.findByText("Swimlanes page");
    expect(screen.queryByRole("link", { name: "bad link" })).toBeNull();
    expect(screen.getByText("bad link")).toBeInTheDocument();
  });
});

// Going to an address as a route change, from code that has an href and not a link.
function Mover({ onReady }: { onReady: (move: { go: (href: string) => void; replace: (href: string) => void }) => void }) {
  const go = useNavigateTo();
  const replace = useReplaceTo();
  onReady({ go, replace });
  return <h1>Mover</h1>;
}
const moverPages = (onReady: (move: { go: (href: string) => void; replace: (href: string) => void }) => void) => ({
  swimlanes: () => Promise.resolve({ default: () => <Mover onReady={onReady} /> }),
});

describe("useNavigateTo", () => {
  it("goes to an address once: asking for where the history already is adds no entry (a tab asks on the press and again on focus)", async () => {
    let move: { go: (href: string) => void; replace: (href: string) => void } | undefined;
    const app = renderApp("/swimlanes", api, { pages: moverPages((m) => (move = m)) });
    await screen.findByText("Mover");
    const before = app.router.history.length;
    act(() => move?.go("/swimlanes/sit1/files?path=a"));
    act(() => move?.go("/swimlanes/sit1/files?path=a"));
    expect(app.router.history.length).toBe(before + 1);
    act(() => move?.go("/swimlanes/sit1/files?path=b"));
    expect(app.router.history.length).toBe(before + 2);
  });

  it("is the same function between renders, so an effect that lists it does not run again", async () => {
    const seen = new Set<unknown>();
    const app = renderApp("/swimlanes", api, { pages: moverPages((m) => seen.add(m.go)) });
    await screen.findByText("Mover");
    await app.settled();
    act(() => void app.router.invalidate());
    await app.settled();
    expect(seen.size).toBe(1);
  });
});

describe("useReplaceTo", () => {
  it("changes the address without adding an entry to the history, so Back leaves the screen", async () => {
    let move: { go: (href: string) => void; replace: (href: string) => void } | undefined;
    const app = renderApp("/swimlanes", api, { pages: moverPages((m) => (move = m)) });
    await screen.findByText("Mover");
    const before = app.router.history.length;
    act(() => move?.replace("/swimlanes?q=sit"));
    act(() => move?.replace("/swimlanes?q=sitb"));
    expect(app.router.history.length).toBe(before);
    expect(app.router.history.location.href).toBe("/swimlanes?q=sitb");
  });
});

describe("openInNewTab", () => {
  const clicks: Array<{ href: string; target: string; rel: string }> = [];
  const spy = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (this: HTMLAnchorElement) {
    clicks.push({ href: this.getAttribute("href") ?? "", target: this.target, rel: this.rel });
  });
  afterEach(() => {
    clicks.length = 0;
  });

  it("opens a path of the app in a new tab that cannot see this one", () => {
    openInNewTab("/swimlanes/sit1");
    expect(clicks).toEqual([{ href: "/swimlanes/sit1", target: "_blank", rel: "noopener noreferrer" }]);
    expect(document.querySelector("a[target=_blank]")).toBeNull();
  });

  it.each(["https://evil.example/", "//evil.example/", "/\\evil.example", "javascript:alert(1)", "data:text/html,x", "swimlanes", ""])("opens nothing for %j: only a path of this app is opened", (href) => {
    openInNewTab(href);
    expect(clicks).toEqual([]);
    expect(spy).toBeDefined();
  });
});

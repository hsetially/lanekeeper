import { configure, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { prefs } from "@/lib/prefs";
import { fileEntry, folderEntry, lane } from "@/mocks/design/swimlanes/fixtures";
import { newWorld, worldApi, type World } from "@/mocks/design/swimlanes/world";
import { problem, renderApp } from "@/test/app";

// The files on the left of a swimlane's screens: folders opened one at a time, the drift of each file, the logical files, the filters,
// and a file that opens in the file view. The tests drive the real tree through the real page.

vi.mock("@/components/ui/resizable", async () => import("@/mocks/design/swimlanes/plain-resizable"));
configure({ asyncUtilTimeout: 5000 });

let world: World;
beforeEach(() => {
  prefs.clear();
  world = newWorld("viewer", [lane("sitb")]);
  world.tree["sitb"] = {
    "": [folderEntry("tx-infinity-api"), folderEntry("security"), fileEntry("application.properties", { drift: "nfs_ahead" })],
    "tx-infinity-api": [
      fileEntry("tx-infinity-api/tx-infinity-core.yml", { logical_file: "tx-infinity-api/tx-infinity-core.yml" }),
      fileEntry("tx-infinity-api/tx-infinity-core-sit7.yml", { kind: "tenant", tenant: "sit7", logical_file: "tx-infinity-api/tx-infinity-core.yml", drift: "conflict" }),
      fileEntry("tx-infinity-api/logo.png", { class: "binary" }),
    ],
    security: [fileEntry("security/roles.yml")],
  };
});
afterEach(() => {
  Reflect.deleteProperty(document, "cookie");
});

const open = (url = "/swimlanes/sitb") => renderApp(url, worldApi(world));
const tree = () => screen.findByRole("tree", { name: "Files in sitb" });
const treeCalls = (app: ReturnType<typeof open>) => app.calls.filter((c) => new URL(c.path, "http://x").pathname === "/api/v1/swimlanes/sitb/tree");

describe("the tree", () => {
  it("lists the root with the folders first, and the drift of a file beside it", async () => {
    open();
    const root = await tree();
    const names = within(root).getAllByRole("treeitem").map((r) => r.textContent ?? "");
    expect(names[0]).toContain("security");
    expect(names[1]).toContain("tx-infinity-api");
    expect(names[2]).toContain("application.properties");
    expect(names[2]).toContain("NFS ahead");
  });

  it("opens a folder by asking the server for that folder only, and shows what is in it", async () => {
    const app = open();
    await tree();
    expect(treeCalls(app)).toHaveLength(1);
    await userEvent.click(screen.getByRole("button", { name: "Open tx-infinity-api" }));
    expect(await screen.findByText("tx-infinity-core-sit7.yml")).toBeInTheDocument();
    const prefixes = treeCalls(app).map((c) => new URL(c.path, "http://x").searchParams.get("prefix"));
    expect(prefixes).toEqual([null, "tx-infinity-api"]);
    expect(screen.queryByText("roles.yml")).toBeNull();
  });

  it("closes a folder and opens it again, without asking the server for it more than twice", async () => {
    const app = open();
    await tree();
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Open security" }));
    expect(await screen.findByText("roles.yml")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Close security" }));
    expect(screen.queryByText("roles.yml")).toBeNull();
    await user.click(screen.getByRole("button", { name: "Open security" }));
    expect(await screen.findByText("roles.yml")).toBeInTheDocument();
    expect(treeCalls(app).filter((c) => new URL(c.path, "http://x").searchParams.get("prefix") === "security").length).toBeLessThanOrEqual(2);
  });

  it("opens a file in the file view when it is chosen", async () => {
    const app = open();
    await tree();
    await userEvent.click(screen.getByText("application.properties", { selector: "bdi" }));
    await app.settled();
    expect(app.location()).toBe("/swimlanes/sitb/files?path=application.properties");
  });

  // The keymap's "Open in new tab" (Control or Command and Enter) and "type-ahead", on the tree of files.
  it("opens a file in a new tab on Control+Enter, and a folder has nothing to open", async () => {
    const clicks: Array<{ href: string; target: string }> = [];
    const spy = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (this: HTMLAnchorElement) {
      clicks.push({ href: this.getAttribute("href") ?? "", target: this.target });
    });
    try {
      const app = open();
      const root = await tree();
      root.focus();
      const user = userEvent.setup();
      await user.keyboard("{ArrowDown}{Control>}{Enter}{/Control}");
      expect(clicks).toEqual([]);
      await user.keyboard("{End}{Control>}{Enter}{/Control}");
      expect(clicks).toEqual([{ href: "/swimlanes/sitb/files?path=application.properties", target: "_blank" }]);
      expect(app.location()).toBe("/swimlanes/sitb");
    } finally {
      spy.mockRestore();
    }
  });

  it("jumps to the row that starts with the letters typed", async () => {
    open();
    const root = await tree();
    root.focus();
    await userEvent.keyboard("tx");
    const active = document.getElementById(root.getAttribute("aria-activedescendant") ?? "");
    expect(active?.textContent).toContain("tx-infinity-api");
    await userEvent.keyboard("{Escape}");
  });

  // Two of the page's single-key shortcuts are letters a file name can begin with: `f` (Filter files) and `g` (the start of Go to). The
  // handoff's keymap wins: the shortcut takes the key, so a row whose name begins with f or g is not reached by typing that first letter
  // (design/DEVIATIONS.md, "Tree type-ahead"). Every other letter is still type-ahead, and a filter or the arrow keys reach the rest.
  it("leaves f and g to the page's shortcuts, and every other letter to type-ahead", async () => {
    world.tree["sitb"] = { "": [fileEntry("application.properties"), fileEntry("firewall.yml"), fileEntry("gateway.yml"), fileEntry("security.yml")] };
    open();
    const root = await tree();
    root.focus();
    const active = (): string => document.getElementById(root.getAttribute("aria-activedescendant") ?? "")?.textContent ?? "";
    const user = userEvent.setup();
    await user.keyboard("s");
    expect(active()).toContain("security.yml");
    // f: the filter takes the keyboard, and the tree's cursor does not go to firewall.yml.
    await user.keyboard("f");
    expect(await screen.findByRole("searchbox", { name: "Filter files" })).toHaveFocus();
    expect(active()).toContain("security.yml");
    await user.keyboard("{Escape}");
    root.focus();
    // g: the first key of a Go to sequence, not a jump to gateway.yml.
    await user.keyboard("g");
    expect(active()).toContain("security.yml");
    await user.keyboard("{Escape}");
    // Another letter starts type-ahead as before.
    await user.keyboard("a");
    expect(active()).toContain("application.properties");
  });

  it("says the swimlane has no files", async () => {
    world.tree["sitb"] = { "": [] };
    open();
    expect(await screen.findByText("No files")).toBeInTheDocument();
  });

  it("shows the problem and a Retry when the root cannot be read", async () => {
    const api = worldApi(world);
    let failing = true;
    renderApp("/swimlanes/sitb", (call) => (failing && new URL(call.path, "http://x").pathname === "/api/v1/swimlanes/sitb/tree" ? problem(500, "internal") : api(call)));
    expect(await screen.findByText("Couldn't load the files")).toBeInTheDocument();
    failing = false;
    await userEvent.click(screen.getAllByRole("button", { name: "Retry" })[0] as HTMLElement);
    await tree();
  });

  it("does not take a file name for markup", async () => {
    world.tree["sitb"] = { "": [fileEntry("<img src=x onerror=alert(1)>.yml")] };
    open();
    await screen.findByText(/onerror/);
    expect(document.querySelector("img[src='x']")).toBeNull();
  });

  it("adds a Load more row at the end of a folder that has more, and loads the next page when it is chosen", async () => {
    world.tree["sitb"] = { "": [fileEntry("a.yml")] };
    const api = worldApi(world);
    renderApp("/swimlanes/sitb", (call) => {
      const url = new URL(call.path, "http://x");
      if (url.pathname === "/api/v1/swimlanes/sitb/tree") {
        return url.searchParams.get("cursor") === null
          ? new Response(JSON.stringify({ items: [fileEntry("a.yml")], next: "c2" }), { status: 200, headers: { "content-type": "application/json" } })
          : new Response(JSON.stringify({ items: [fileEntry("b.yml")], next: null }), { status: 200, headers: { "content-type": "application/json" } });
      }
      return api(call);
    });
    await userEvent.click(await screen.findByText("Load more…"));
    expect(await screen.findByText("b.yml")).toBeInTheDocument();
    expect(screen.queryByText("Load more…")).toBeNull();
  });
});

describe("the filters", () => {
  it("filters by name over the folders that were opened, and says so when nothing matches", async () => {
    open();
    await tree();
    await userEvent.click(screen.getByRole("button", { name: "Open tx-infinity-api" }));
    await screen.findByText("logo.png");
    await userEvent.type(screen.getByRole("searchbox", { name: "Filter files" }), "LOGO");
    await waitFor(() => expect(screen.queryByText("application.properties")).toBeNull());
    expect(screen.getByText("logo.png")).toBeInTheDocument();
    await userEvent.clear(screen.getByRole("searchbox", { name: "Filter files" }));
    await userEvent.type(screen.getByRole("searchbox", { name: "Filter files" }), "zzz");
    expect(await screen.findByText("No file matches")).toBeInTheDocument();
    expect(screen.getByText("The filter looks at the names of the folders you have opened.")).toBeInTheDocument();
  });

  it("takes a pattern character as itself", async () => {
    open();
    await tree();
    await userEvent.type(screen.getByRole("searchbox", { name: "Filter files" }), ".*");
    expect(await screen.findByText("No file matches")).toBeInTheDocument();
  });

  it("draws its two filter chips as the design does: 24 px, 6 px of padding, 4 px between the icon and the words, 12 px text and icon", async () => {
    open();
    await tree();
    for (const name of [/^Drift state/, /^File class/]) {
      const chip = screen.getByRole("button", { name });
      expect(chip).toHaveClass("h-6", "px-1.5", "gap-1", "text-xs", "[&_svg:not([class*='size-'])]:size-3");
    }
  });

  it("focuses the filter on f", async () => {
    open();
    await tree();
    await userEvent.keyboard("f");
    expect(screen.getByRole("searchbox", { name: "Filter files" })).toHaveFocus();
  });

  it("asks the server for one drift state when that state is chosen, for every folder", async () => {
    const app = open();
    await tree();
    await userEvent.click(screen.getByRole("button", { name: /^Drift state/ }));
    await userEvent.click(await screen.findByRole("menuitemradio", { name: "NFS ahead" }));
    await waitFor(() => expect(treeCalls(app).some((c) => new URL(c.path, "http://x").searchParams.get("drift") === "nfs_ahead")).toBe(true));
    expect(screen.getByRole("button", { name: "Drift state: NFS ahead" })).toBeInTheDocument();
  });

  it("filters by file class", async () => {
    open();
    await tree();
    await userEvent.click(screen.getByRole("button", { name: "Open tx-infinity-api" }));
    await screen.findByText("logo.png");
    await userEvent.click(screen.getByRole("button", { name: /^File class/ }));
    await userEvent.click(await screen.findByRole("menuitemradio", { name: "Binary" }));
    await waitFor(() => expect(screen.queryByText("tx-infinity-core.yml")).toBeNull());
    expect(screen.getByText("logo.png")).toBeInTheDocument();
  });
});

describe("logical files", () => {
  it("merges a base file and the tenant file that replaces it into one row that says it uses the tenant file", async () => {
    open();
    await tree();
    await userEvent.click(screen.getByRole("button", { name: "Open tx-infinity-api" }));
    await screen.findByText("tx-infinity-core-sit7.yml");
    await userEvent.click(screen.getByRole("switch", { name: "Show logical files" }));
    await waitFor(() => expect(screen.queryByText("tx-infinity-core-sit7.yml")).toBeNull());
    const row = screen.getByText("tx-infinity-core.yml", { selector: "bdi" }).closest<HTMLElement>("[role='treeitem']");
    if (row === null) throw new Error("no row");
    expect(within(row).getByText("Uses tenant file")).toHaveAttribute("title", "Reads tx-infinity-api/tx-infinity-core-sit7.yml beside the base file");
    expect(within(row).getByText("Conflict")).toBeInTheDocument();
  });
});

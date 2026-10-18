import { QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Role } from "@/api/generated/model";
import { createQueryClient } from "@/lib/query/client";
import CommandPalette from "./CommandPalette";

// The palette, as a person uses it: ⌘K opens it (the shell's job), then ↑ ↓ ↵, ⌘↵, ⌘1 to ⌘9, the > prefix and Esc, with the
// swimlane list and the three searches answered by a fake server that records what was asked.

const lane = (id: string, over: Record<string, unknown> = {}) => ({ id, display_name: id, tenants: ["sit1"], agent: "online", last_scan_at: null, base_version: null, drift_counts: [], finding_count: 0, tier: "SIT", ...over });
let asked: string[];

function fakeServer(role: Role) {
  asked = [];
  const respond = (input: string): Response => {
      asked.push(input);
      const json = (body: unknown) => new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
      if (input.startsWith("/api/v1/swimlanes")) return json({ items: [lane("sit"), lane("sitb"), lane("presita", { tier: "PRESIT" })], next: null });
      if (input.startsWith("/api/v1/docs/search")) return json({ hits: [{ doc: 1, title: "Account sorting", path: "remote-itm-teller.md", heading_path: ["Teller"], snippet: "x", score: 1, related_files: [] }] });
      if (input.startsWith("/api/v1/search/settings")) return json({ items: [{ swimlane: "sitb", file: "tx-infinity-core-sit1.yml", path: "core.enableSorting", value: { text: "true" }, location: { path: "a", lines: { start: 9, end: 9 } } }], next: null });
      if (input.startsWith("/api/v1/admin/users")) {
        if (role !== Role.admin) return new Response(JSON.stringify({ type: "urn:lanekeeper:problem:forbidden", title: "No", status: 403 }), { status: 403, headers: { "content-type": "application/problem+json" } });
        return json({ items: [{ id: "u:1", email: "core.dev@example.test", display_name: "Core Dev", role: "editor", status: "active", requires_approval: false, github_login: null, last_seen_at: null }], next: null });
      }
      return new Response("{}", { status: 404 });
  };
  vi.stubGlobal("fetch", vi.fn((input: string) => Promise.resolve(respond(input))));
}

const onRun = vi.fn();
const onOpenChange = vi.fn();

function open(role: Role = Role.viewer) {
  fakeServer(role);
  const user = userEvent.setup();
  render(
    <QueryClientProvider client={createQueryClient()}>
      <CommandPalette open onOpenChange={onOpenChange} role={role} onRun={onRun} />
    </QueryClientProvider>,
  );
  return user;
}

const box = () => screen.getByRole("combobox", { name: /Search or jump to/ });

beforeEach(() => {
  onRun.mockReset();
  onOpenChange.mockReset();
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe("the palette", () => {
  it("is a dialog named Command palette with the swimlanes and the actions of a Viewer, and no Users", async () => {
    open();
    expect(screen.getByRole("dialog", { name: "Command palette" })).toBeInTheDocument();
    await screen.findByRole("option", { name: /^sitb/ });
    expect(screen.getByRole("group", { name: "Swimlanes" })).toBeInTheDocument();
    expect(screen.getByRole("group", { name: "Actions" })).toBeInTheDocument();
    expect(screen.queryByRole("group", { name: "Users" })).toBeNull();
    expect(screen.queryByRole("option", { name: /Go to Audit/ })).toBeNull();
  });

  it("narrows to what is typed, best match first, and shows the match", async () => {
    const user = open();
    await screen.findByRole("option", { name: /^sitb/ });
    await user.type(box(), "sit");
    const options = screen.getAllByRole("option").map((o) => o.textContent ?? "");
    expect(options[0]).toMatch(/^sit/);
    expect(screen.queryByRole("option", { name: /Go to Audit/ })).toBeNull();
  });

  it("runs the highlighted row on Enter, moving with the arrows", async () => {
    const user = open();
    await screen.findByRole("option", { name: /^sitb/ });
    await user.type(box(), "sit");
    await user.keyboard("{ArrowDown}{Enter}");
    expect(onRun).toHaveBeenCalledTimes(1);
    expect(onRun.mock.calls[0]?.[0]).toMatchObject({ kind: "go" });
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("runs the nth result on ⌘1 to ⌘9, counting results and not actions", async () => {
    const user = open();
    await screen.findByRole("option", { name: /^sitb/ });
    await user.keyboard("{Control>}2{/Control}");
    expect(onRun).toHaveBeenCalledTimes(1);
    // The list is presita, sit, sitb: the second is "sit".
    expect(onRun.mock.calls[0]?.[0]).toEqual({ kind: "go", href: "/swimlanes/sit", swimlane: "sit" });
  });

  it("draws ⌘1 to ⌘9 on the results and the shortcut on an action", async () => {
    open();
    const first = await screen.findByRole("option", { name: /^presita/ });
    expect(within(first).getByText("1")).toBeInTheDocument();
    const goTo = screen.getByRole("option", { name: /Go to Compare/ });
    expect(within(goTo).getAllByText(/^(g|c|then)$/).length).toBe(3);
  });

  it("opens a result in a new tab on ⌘Enter, with a link that never lets the new page see this one", async () => {
    const click = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (this: HTMLAnchorElement) {
      expect(this.target).toBe("_blank");
      expect(this.rel).toContain("noopener");
      expect(this.getAttribute("href")).toBe("/swimlanes/presita");
    });
    const user = open();
    await screen.findByRole("option", { name: /^presita/ });
    await user.keyboard("{Control>}{Enter}{/Control}");
    expect(click).toHaveBeenCalledTimes(1);
    expect(onRun).not.toHaveBeenCalled();
    click.mockRestore();
  });

  it("limits to actions after > and lifts that on Backspace in the empty box", async () => {
    const user = open();
    await screen.findByRole("option", { name: /^sitb/ });
    await user.type(box(), ">");
    expect(screen.getByText("Actions", { selector: "[data-slot='palette-chip']" })).toBeInTheDocument();
    expect(screen.queryByRole("group", { name: "Swimlanes" })).toBeNull();
    await user.type(box(), "short");
    expect(screen.getAllByRole("option")).toHaveLength(1);
    await user.clear(box());
    await user.keyboard("{Backspace}");
    expect(screen.queryByText("Actions", { selector: "[data-slot='palette-chip']" })).toBeNull();
    expect(screen.getByRole("group", { name: "Swimlanes" })).toBeInTheDocument();
  });

  it("says what it found nothing for", async () => {
    const user = open();
    await screen.findByRole("option", { name: /^sitb/ });
    await user.type(box(), "qqqqqq");
    expect(await screen.findByText("No results for “qqqqqq”.")).toBeInTheDocument();
  });

  it("keeps the no-result message out of the list: a listbox may only hold options and groups (axe `aria-required-children`)", async () => {
    const user = open();
    await screen.findByRole("option", { name: /^sitb/ });
    await user.type(box(), "qqqqqq");
    const message = await screen.findByText("No results for “qqqqqq”.");
    expect(message.closest("[role='listbox'], [cmdk-list]")).toBeNull();
    // The list itself is out of the accessibility tree while it has nothing to list, so an empty listbox is not left behind.
    expect(screen.queryByRole("listbox")).toBeNull();
    // Still one live message, and the field is still the one that is focused.
    expect(message.closest("[role='status']")).not.toBeNull();
    expect(box()).toHaveFocus();
  });

  it("closes on Escape", async () => {
    const user = open();
    await user.keyboard("{Escape}");
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });
});

describe("the searches", () => {
  it("waits for the person to stop typing, then asks docs and settings once, and shows both groups", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    fakeServer(Role.viewer);
    render(
      <QueryClientProvider client={createQueryClient()}>
        <CommandPalette open onOpenChange={onOpenChange} role={Role.viewer} onRun={onRun} />
      </QueryClientProvider>,
    );
    await user.type(box(), "core");
    expect(asked.some((u) => u.startsWith("/api/v1/docs/search"))).toBe(false);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(250);
    });
    await screen.findByRole("option", { name: /Account sorting/ }, { timeout: 3000 });
    await screen.findByRole("option", { name: /core\.enableSorting/ }, { timeout: 3000 });
    expect(asked.filter((u) => u.startsWith("/api/v1/docs/search"))).toHaveLength(1);
    expect(asked.find((u) => u.startsWith("/api/v1/docs/search"))).toContain("limit=8");
    // Setting results show where, never the value.
    expect(screen.queryByText("true")).toBeNull();
  });

  it("does not ask for users unless the person is an Admin", async () => {
    const user = open(Role.editor);
    await user.type(box(), "core");
    await waitFor(() => expect(asked.some((u) => u.startsWith("/api/v1/docs/search"))).toBe(true));
    expect(asked.some((u) => u.startsWith("/api/v1/admin/users"))).toBe(false);
    expect(screen.queryByRole("group", { name: "Users" })).toBeNull();
  });

  it("gives an Admin the Users group", async () => {
    const user = open(Role.admin);
    await user.type(box(), "core");
    expect(await screen.findByRole("option", { name: /Core\s*Dev/ }, { timeout: 3000 })).toBeInTheDocument();
    expect(screen.getByRole("group", { name: "Users" })).toBeInTheDocument();
  });

  it("asks nothing for a query shorter than two characters", async () => {
    const user = open();
    await user.type(box(), "c");
    await new Promise((r) => setTimeout(r, 300));
    expect(asked.some((u) => u.startsWith("/api/v1/docs/search"))).toBe(false);
  });

  it("says so when a search fails, and keeps the other groups", async () => {
    const user = open();
    const failing = (input: string): Response => {
        if (input.startsWith("/api/v1/docs/search")) return new Response(JSON.stringify({ type: "urn:lanekeeper:problem:internal", title: "boom", status: 500 }), { status: 500, headers: { "content-type": "application/problem+json" } });
        if (input.startsWith("/api/v1/swimlanes")) return new Response(JSON.stringify({ items: [lane("sit")], next: null }), { status: 200, headers: { "content-type": "application/json" } });
        return new Response(JSON.stringify({ items: [], next: null }), { status: 200, headers: { "content-type": "application/json" } });
    };
    vi.stubGlobal("fetch", vi.fn((input: string) => Promise.resolve(failing(input))));
    await user.type(box(), "sit");
    expect(await screen.findByText("Couldn't search docs.", {}, { timeout: 4000 })).toBeInTheDocument();
    expect(screen.getAllByRole("option", { name: /^sit/ }).length).toBeGreaterThan(0);
  });
});

describe("what a search that is running or has failed says", () => {
  it("is not in the listbox as a status (axe `aria-required-children`): the rows are for the eye, and one polite status says it", async () => {
    const user = open();
    vi.stubGlobal(
      "fetch",
      vi.fn((input: string) => {
        if (input.startsWith("/api/v1/docs/search")) return Promise.resolve(new Response(JSON.stringify({ type: "urn:lanekeeper:problem:internal", title: "boom", status: 500 }), { status: 500, headers: { "content-type": "application/problem+json" } }));
        if (input.startsWith("/api/v1/swimlanes")) return Promise.resolve(new Response(JSON.stringify({ items: [lane("sit")], next: null }), { status: 200, headers: { "content-type": "application/json" } }));
        return Promise.resolve(new Response(JSON.stringify({ items: [], next: null }), { status: 200, headers: { "content-type": "application/json" } }));
      }),
    );
    await user.type(box(), "sit");
    await screen.findByText("Couldn't search docs.", {}, { timeout: 4000 });
    const list = screen.getByRole("listbox");
    expect(within(list).queryAllByRole("status")).toHaveLength(0);
    // The row is still drawn, and the same fact is said once, by a polite status outside the list.
    const live = screen.getAllByRole("status").find((node) => /Docs search failed/.test(node.textContent ?? ""));
    expect(live).toBeDefined();
    expect(list.contains(live ?? null)).toBe(false);
  });
});

describe("keys inside the box", () => {
  it("leaves ⌘K and other chords it does not know alone", async () => {
    open();
    await screen.findByRole("option", { name: /^sitb/ });
    const event = new KeyboardEvent("keydown", { key: "k", ctrlKey: true, bubbles: true, cancelable: true });
    act(() => {
      box().dispatchEvent(event);
    });
    expect(onRun).not.toHaveBeenCalled();
    // Three swimlanes, so ⌘3 is the last result and ⌘4 is nothing.
    fireEvent.keyDown(box(), { key: "4", ctrlKey: true });
    expect(onRun).not.toHaveBeenCalled();
    fireEvent.keyDown(box(), { key: "3", ctrlKey: true });
    expect(onRun).toHaveBeenCalledTimes(1);
  });
});

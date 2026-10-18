import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AuditEvent, DomainEvent } from "@/api/generated/model";
import { AMIR, AUDIT, auditEvent, chainHash, NOW, PRIYA } from "@/mocks/design/approvals/fixtures";
import { newWorld, worldApi, type World } from "@/mocks/design/approvals/world";
import { SHELL_LANES } from "@/mocks/design/shell";
import { problem, renderApp, type Api } from "@/test/app";
import { expectNoAxeViolations } from "@/test/axe";
import { FakeHub, frame } from "@/test/fake-hub";
import { FIRST_FRAME } from "@/test/sse-golden";

// The audit log: `adminListAudit` (Admin only, W11), newest first, filtered by what the hub can filter on (user, action, swimlane
// and a range of days), paged by a keyset cursor. It shows event metadata only: who, how it arrived, what, where, how sure the
// attribution is, and the hashes of the chain. These tests drive the whole app on a memory history, with `fetch` answered by a hub
// that is a function of the test (src/mocks/design/approvals/world.ts).

let world: World;
beforeEach(() => {
  vi.useFakeTimers({ toFake: ["Date"], now: NOW });
  world = newWorld("admin");
  world.lanes = [...SHELL_LANES];
  world.audit = [...AUDIT];
});
afterEach(() => {
  vi.useRealTimers();
  Reflect.deleteProperty(document, "cookie");
});

const isAudit = (path: string): boolean => new URL(path, "http://hub.test").pathname === "/api/v1/admin/audit";
const auditCalls = (calls: ReadonlyArray<{ path: string }>) => calls.filter((c) => isAudit(c.path)).map((c) => new URL(c.path, "http://hub.test").searchParams);

async function open(url = "/audit", api: Api = worldApi(world)) {
  const app = renderApp(url, api);
  await screen.findByRole("heading", { level: 1, name: "Audit log" }, { timeout: 8000 });
  return app;
}
const grid = () => screen.findByRole("grid", { name: "Audit events" });
const rowOf = async (seq: number): Promise<HTMLElement> => {
  const cell = await within(await grid()).findByText(`#${seq.toLocaleString("en-US")}`, { exact: false });
  const row = cell.closest<HTMLElement>('[role="row"]');
  if (row === null) throw new Error(`no row for ${seq}`);
  return row;
};

describe("the log", () => {
  it("has the page's one h1 and its words, and asks the hub for the first page, newest first", async () => {
    const app = await open();
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(screen.getByText(/Every audited change: who made it, how it arrived, and what was recorded\./)).toBeInTheDocument();
    await grid();
    const [first] = auditCalls(app.calls);
    expect(first?.get("limit")).toBe("100");
    expect(first?.has("cursor")).toBe(false);
    const rows = within(await grid()).getAllByRole("row").slice(1);
    expect(within(rows[0] as HTMLElement).getByText(/#18,463/)).toBeInTheDocument();
    expect(within(rows[1] as HTMLElement).getByText(/#18,462/)).toBeInTheDocument();
  });

  it("shows when, who, how, what, where and how sure, for an event made in the web app", async () => {
    await open();
    const row = await rowOf(18462);
    expect(within(row).getByText("14:41:09")).toBeInTheDocument();
    expect(within(row).getByText(/2025-10-09/)).toBeInTheDocument();
    expect(within(row).getByText("Ana Ruiz")).toBeInTheDocument();
    expect(within(row).getByText("UI")).toBeInTheDocument();
    expect(within(row).getByText("Edit")).toBeInTheDocument();
    expect(within(row).getByText("sitb")).toBeInTheDocument();
    expect(within(row).getByText("tx-infinity-api/account-sorting-config-sit7.yml")).toBeInTheDocument();
    expect(within(row).getByText("Via Lanekeeper")).toBeInTheDocument();
    expect(within(row).getByText("Certain")).toBeInTheDocument();
  });

  it("gives the Time column the room of its second line, `2025-10-09 · #18,462` in 12px mono, and an ellipsis if it is still too long (the layout run found the day and the number cut off at every width)", async () => {
    await open();
    const grid = await screen.findByRole("grid", { name: "Audit events" });
    const template = grid.querySelector<HTMLElement>("[data-slot='virtual-list']")?.style.getPropertyValue("--lk-cols") ?? grid.style.getPropertyValue("--lk-cols");
    const tracks = template.split(/\s+(?![^(]*\))/);
    const headers = within(grid)
      .getAllByRole("columnheader")
      .map((h) => (h.textContent ?? "").trim());
    // 20 characters of 12px mono are 144px.
    expect(Number.parseInt(tracks[headers.indexOf("Time")] ?? "0", 10)).toBeGreaterThanOrEqual(148);
    const day = (await rowOf(18462)).querySelector(".text-muted-foreground");
    expect(day?.textContent).toMatch(/^2025-10-09 · #18,462$/);
    expect(day?.className).toMatch(/(^|\s)truncate(\s|$)/);
  });

  it("draws the action as a block that holds its label to the cell, which then ends in an ellipsis: an inline box is as wide as its text (the layout run, hostile text)", async () => {
    await open();
    const label = (await within(await rowOf(18462)).findByText("Edit")).closest("span.truncate");
    expect(label).not.toBeNull();
    const box = label?.parentElement;
    expect(box?.className).toMatch(/(^|\s)flex(\s|$)/);
    expect(box?.className).not.toMatch(/inline-flex/);
    expect(box?.className).toMatch(/(^|\s)min-w-0(\s|$)/);
  });

  it("marks what was made through Cursor, what was found on the NFS volume, a sync, and an unknown author", async () => {
    await open();
    expect(within(await rowOf(18460)).getByText("Cursor")).toBeInTheDocument();
    const unknown = await rowOf(18459);
    expect(within(unknown).getByText("unknown author")).toBeInTheDocument();
    expect(within(unknown).getByText("Detected")).toBeInTheDocument();
    const guess = await rowOf(18452);
    expect(within(guess).getByText("Tomás Reyes")).toBeInTheDocument();
    expect(within(guess).getByText("probable")).toBeInTheDocument();
    expect(within(guess).getByText("NFS client")).toBeInTheDocument();
    expect(within(guess).getByText("Medium")).toBeInTheDocument();
    const sync = await rowOf(18451);
    expect(within(sync).getByText("Sync")).toBeInTheDocument();
    expect(within(sync).getByText("sit7-sync")).toBeInTheDocument();
  });

  it("names an action it has no words for by its own name, and shows a hidden character in a target instead of hiding it", async () => {
    world.audit = [auditEvent({ seq: 5, at: NOW, action: "something_new_happened", actor: PRIYA, via: "ui", target: "a\u202Eb.yml" })];
    await open();
    const row = await rowOf(5);
    expect(within(row).getByText("Something new happened")).toBeInTheDocument();
    expect(within(row).getByRole("img", { name: /U\+202E/ })).toBeInTheDocument();
  });

  it("falls back to the start of the user id for someone who is not in the user list", async () => {
    world.users = [];
    await open();
    expect(within(await rowOf(18462)).getByText("user ffffffff")).toBeInTheDocument();
  });

  it("shows event metadata only: nothing of it is a request for a value", async () => {
    const app = await open();
    await grid();
    expect(app.calls.every((c) => c.method === "GET")).toBe(true);
    expect(app.calls.some((c) => c.path.includes("reveal"))).toBe(false);
  });
});

describe("who may open it", () => {
  it("shows an Editor the permission screen and asks the hub for no audit events", async () => {
    world.role = "editor";
    const app = renderApp("/audit", worldApi(world));
    expect(await screen.findByText("Audit needs the Admin role", {}, { timeout: 8000 })).toBeInTheDocument();
    expect(app.calls.some((c) => isAudit(c.path))).toBe(false);
  });
});

describe("the states", () => {
  it("draws a skeleton named for what it loads, busy, while the first page is on its way", async () => {
    const never = new Promise<Response>(() => undefined);
    const base = worldApi(world);
    await open("/audit", (call) => (isAudit(call.path) ? never : base(call)));
    expect(await screen.findByLabelText("Loading audit events")).toHaveAttribute("aria-busy", "true");
    expect(screen.getByText("Loading audit events…")).toBeInTheDocument();
  });

  it("says Couldn't load, with the title and the request ID, and offers a retry for an error that may pass", async () => {
    world.auditProblem = problem(500, "internal");
    await open();
    expect(await screen.findByText("Couldn't load audit events")).toBeInTheDocument();
    expect(screen.getByText("req-test-1")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
  });

  it("says there are no events, and why, when the log is empty", async () => {
    world.audit = [];
    await open();
    expect(await screen.findByText("No audit events yet")).toBeInTheDocument();
    expect(screen.getByText("Nothing has been audited yet.")).toBeInTheDocument();
  });

  it("says no events match, and offers to clear the filters, when the filters leave none", async () => {
    await open("/audit?swimlane=presitc");
    expect(await screen.findByText("No audit events match")).toBeInTheDocument();
    expect(screen.getByText("Widen the date range or clear the filters.")).toBeInTheDocument();
    await userEvent.click(screen.getAllByRole("button", { name: "Clear filters" })[0] as HTMLElement);
    await waitFor(() => expect(screen.getByRole("grid", { name: "Audit events" })).toBeInTheDocument());
  });

  it("pages by the hub's cursor with a Load more, and says how many are shown", async () => {
    world.pageSize = 5;
    const app = await open();
    await grid();
    expect(within(await grid()).getAllByRole("row").slice(1)).toHaveLength(5);
    expect(screen.getAllByText(/Showing 5 of 5 loaded/).length).toBeGreaterThan(0);
    await userEvent.click(screen.getByRole("button", { name: "Load more" }));
    await waitFor(() => expect(within(screen.getByRole("grid", { name: "Audit events" })).getAllByRole("row").slice(1)).toHaveLength(10));
    expect(auditCalls(app.calls).some((q) => q.get("cursor") === "18459")).toBe(true);
  });
});

describe("the filters", () => {
  it("ask the hub for a user, an action, a swimlane and a range of days, and keep them in the address", async () => {
    const app = await open();
    await grid();
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "User" }), PRIYA);
    await waitFor(() => expect(app.location()).toBe(`/audit?actor=${encodeURIComponent(PRIYA)}`));
    await waitFor(() => expect(auditCalls(app.calls).some((q) => q.get("actor") === PRIYA)).toBe(true));
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "Action" }), "proposal_approved");
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "Swimlane" }), "sitb");
    await userEvent.type(screen.getByLabelText("From"), "2025-10-09");
    await userEvent.type(screen.getByLabelText("To"), "2025-10-09");
    await waitFor(() => {
      const last = auditCalls(app.calls).at(-1);
      expect(last?.get("actor")).toBe(PRIYA);
      expect(last?.get("action")).toBe("proposal_approved");
      expect(last?.get("swimlane")).toBe("sitb");
      expect(last?.get("from")).toBe(String(Date.UTC(2025, 9, 9)));
      expect(last?.get("to")).toBe(String(Date.UTC(2025, 9, 9) + 86_400_000 - 1));
    });
    // Only the approval of the account-sorting file is left.
    const rows = within(await grid()).getAllByRole("row").slice(1);
    expect(rows).toHaveLength(1);
    expect(within(rows[0] as HTMLElement).getByText("Approve")).toBeInTheDocument();
  });

  it("open as the address says, and a value the hub would refuse is left out of the request", async () => {
    const app = await open(`/audit?actor=not-a-user&swimlane=Sit%20B!&from=yesterday&action=file_edited`);
    await grid();
    const [first] = auditCalls(app.calls);
    expect(first?.has("actor")).toBe(false);
    expect(first?.has("swimlane")).toBe(false);
    expect(first?.has("from")).toBe(false);
    expect(first?.get("action")).toBe("file_edited");
    expect(screen.getByRole("combobox", { name: "Action" })).toHaveValue("file_edited");
  });

  it("lists the users and the swimlanes it knows, and an action from the address that it has no words for", async () => {
    await open("/audit?action=odd_new_action");
    const user = await screen.findByRole("combobox", { name: "User" });
    expect(within(user).getByRole("option", { name: "Priya Shah" })).toBeInTheDocument();
    expect(within(user).getByRole("option", { name: "Any user" })).toBeInTheDocument();
    expect(within(screen.getByRole("combobox", { name: "Swimlane" })).getByRole("option", { name: "presitc" })).toBeInTheDocument();
    expect(within(screen.getByRole("combobox", { name: "Action" })).getByRole("option", { name: "odd_new_action" })).toBeInTheDocument();
  });

  it("clear, all at once", async () => {
    const app = await open("/audit?swimlane=sitb&action=file_edited");
    await grid();
    await userEvent.click(screen.getByRole("button", { name: "Clear filters" }));
    await waitFor(() => expect(app.location()).toBe("/audit"));
  });
});

describe("the quick filters", () => {
  it("count the events that are loaded and keep the Cursor ones or those of an unknown author", async () => {
    await open();
    await grid();
    const quick = within(screen.getByRole("group", { name: "Quick filter" }));
    expect(quick.getByRole("button", { name: /^All events\s*14$/ })).toHaveAttribute("aria-pressed", "true");
    expect(quick.getByRole("button", { name: /^Via Cursor\s*1$/ })).toBeInTheDocument();
    expect(quick.getByRole("button", { name: /^Unknown author\s*2$/ })).toBeInTheDocument();
    await userEvent.click(quick.getByRole("button", { name: /^Via Cursor/ }));
    expect(within(await grid()).getAllByRole("row").slice(1)).toHaveLength(1);
    expect(screen.getAllByText(/Showing 1 of 14 loaded/).length).toBeGreaterThan(0);
    await userEvent.click(quick.getByRole("button", { name: /^Unknown author/ }));
    expect(within(await grid()).getAllByRole("row").slice(1)).toHaveLength(2);
  });
});

describe("the drawer", () => {
  it("opens from a row with who, how sure, the diff the hub kept, and the links of the chain", async () => {
    await open();
    await userEvent.click(await rowOf(18453));
    const drawer = await screen.findByRole("dialog", { name: "Event detail" });
    expect(within(drawer).getByRole("heading", { level: 2, name: "Change detected tx-infinity-api/payment-limits-sit1.yml" })).toBeInTheDocument();
    expect(within(drawer).getByText("Changed outside Lanekeeper, author unknown")).toBeInTheDocument();
    expect(within(drawer).getByText("Event 18,453")).toBeInTheDocument();
    expect(within(drawer).getByText("perTxCents: 1500000", { exact: false })).toBeInTheDocument();
    expect(within(drawer).getByText("Linked to event 18,452")).toBeInTheDocument();
    expect(within(drawer).getByText(chainHash(18453))).toBeInTheDocument();
    expect(within(drawer).getByRole("button", { name: "Copy chain hash" })).toBeInTheDocument();
  });

  it("makes the diff the hub kept a scroll region the keyboard can reach (axe `scrollable-region-focusable`)", async () => {
    await open();
    await userEvent.click(await rowOf(18453));
    const drawer = await screen.findByRole("dialog", { name: "Event detail" });
    // It scrolls (max height, overflow) and holds no control, so it has to take focus itself to be scrolled with the arrow keys.
    expect(within(drawer).getByRole("group", { name: "Diff the hub kept" })).toHaveAttribute("tabindex", "0");
  });

  it("lets the request ID and the previous hash be copied from their rows, each by what it is", async () => {
    const user = userEvent.setup();
    await open();
    await user.click(await rowOf(18453));
    const drawer = await screen.findByRole("dialog", { name: "Event detail" });
    await user.click(within(drawer).getByRole("button", { name: "Copy request ID" }));
    expect(await navigator.clipboard.readText()).toBe("req_18453");
    await user.click(within(drawer).getByRole("button", { name: "Copy previous hash" }));
    expect(await navigator.clipboard.readText()).toBe(chainHash(18452));
    // There is still exactly one button for the chain hash itself: the labelled one in the footer.
    expect(within(drawer).getAllByRole("button", { name: "Copy chain hash" })).toHaveLength(1);
  });

  it("shows the evidence of an attribution: the login, the command and when it was seen", async () => {
    await open();
    await userEvent.click(await rowOf(18454));
    const drawer = await screen.findByRole("dialog", { name: "Event detail" });
    expect(within(drawer).getByText("Attribution evidence")).toBeInTheDocument();
    expect(within(drawer).getByText("dana_okoye_corp_example")).toBeInTheDocument();
    expect(within(drawer).getByText("/usr/bin/vim.basic payment-limits-sit1.yml")).toBeInTheDocument();
    expect(within(drawer).getByText("Dana Okoye")).toBeInTheDocument();
    expect(within(drawer).getByText("Edited on the NFS VM · High confidence")).toBeInTheDocument();
  });

  it("says an event made through Cursor was made for a person", async () => {
    await open();
    await userEvent.click(await rowOf(18460));
    const drawer = await screen.findByRole("dialog", { name: "Event detail" });
    expect(within(drawer).getByText("Made by a Cursor agent for Wen Zhao")).toBeInTheDocument();
  });

  it("shows a line the secret scan replaced as the hub sent it, and no more than the hub sent", async () => {
    world.audit = [auditEvent({ seq: 7, at: NOW, action: "file_edited", actor: PRIYA, via: "ui", diff: "@@ -1 +1 @@\n-key: [REDACTED: 20 chars, aws_access_key]\n+key: [REDACTED: 20 chars, aws_access_key]\n" })];
    await open();
    await userEvent.click(await rowOf(7));
    const drawer = await screen.findByRole("dialog", { name: "Event detail" });
    expect(within(drawer).getAllByText("key: [REDACTED: 20 chars, aws_access_key]", { exact: false })).toHaveLength(2);
  });

  it("is opened and closed from the keyboard", async () => {
    await open();
    const table = await grid();
    table.focus();
    await userEvent.keyboard("{ArrowDown}{ArrowDown}{Enter}");
    const drawer = await screen.findByRole("dialog", { name: "Event detail" });
    expect(within(drawer).getByText("Event 18,462")).toBeInTheDocument();
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Event detail" })).toBeNull());
  });

  it("closes with its button", async () => {
    await open();
    await userEvent.click(await rowOf(18463));
    const drawer = await screen.findByRole("dialog", { name: "Event detail" });
    await userEvent.click(within(drawer).getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("dialog", { name: "Event detail" })).toBeNull();
  });

  // Found by the keyboard journey in a real browser: the drawer's title had focus, it left with the drawer, and focus fell to the
  // page behind, so a keyboard user had to start again from the top of the screen.
  it("gives the keyboard back to the table it was opened from, on Escape and on its button", async () => {
    await open();
    const table = await grid();
    table.focus();
    await userEvent.keyboard("{ArrowDown}{Enter}");
    await screen.findByRole("dialog", { name: "Event detail" });
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Event detail" })).toBeNull());
    expect(table).toHaveFocus();
    await userEvent.click(await rowOf(18463));
    const drawer = await screen.findByRole("dialog", { name: "Event detail" });
    await userEvent.click(within(drawer).getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("dialog", { name: "Event detail" })).toBeNull();
    expect(table).toHaveFocus();
  });
});

describe("the link between events", () => {
  it("says nothing about the chain when no link is broken: an 'all clear' would read as the log having been verified, and nothing here verifies it", async () => {
    await open();
    await grid();
    expect(screen.queryByRole("region", { name: "Integrity" })).toBeNull();
    expect(screen.queryByText(/No broken links/)).toBeNull();
    expect(screen.queryByText(/verif/i)).toBeNull();
    // The chain hash of an event is still there to read and copy.
    await userEvent.click(await rowOf(18453));
    const drawer = await screen.findByRole("dialog", { name: "Event detail" });
    expect(within(drawer).getByText(chainHash(18453))).toBeInTheDocument();
    expect(within(drawer).getByRole("button", { name: "Copy chain hash" })).toBeInTheDocument();
  });

  it("marks the event that stores another hash than the one before it has, and jumps to it", async () => {
    const broken: AuditEvent[] = world.audit.map((e) => (e.seq === 18455 ? { ...e, prev_hash: chainHash(999) } : e));
    world.audit = broken;
    await open();
    await grid();
    const panel = screen.getByRole("region", { name: "Integrity" });
    expect(within(panel).getByText("Chain break at event 18,455")).toBeInTheDocument();
    // It says what it compared and that it is not the hub's own check.
    expect(within(panel).getByText(/not the hub's own verification/)).toBeInTheDocument();
    const chip = within(await rowOf(18455)).getByText("Chain break").closest("[data-slot='tone-chip']");
    expect(chip).toBeInTheDocument();
    // The clock, the day and the chip are three lines in a 48px row: 16 + 16 + 16 (a 20px chip, and a margin, ran 3px past the row, under the next one).
    expect(chip?.className).toMatch(/(^|\s)h-4(\s|$)/);
    expect(chip?.className).not.toMatch(/(^|\s)(mt-0\.5|h-5)(\s|$)/);
    await userEvent.click(within(panel).getByRole("button", { name: "Show event 18,455" }));
    const drawer = await screen.findByRole("dialog", { name: "Event detail" });
    expect(within(drawer).getByText("The chain breaks at this event")).toBeInTheDocument();
    expect(within(drawer).getByText(chainHash(999))).toBeInTheDocument();
    expect(within(drawer).getByText(chainHash(18454))).toBeInTheDocument();
  });
});

describe("how old what is on screen is", () => {
  it("says when the log was read, and reads it again on Refresh", async () => {
    const app = await open();
    await grid();
    expect(screen.getByText(/Read at \d\d:\d\d UTC/)).toBeInTheDocument();
    const before = auditCalls(app.calls).length;
    world.audit = [auditEvent({ seq: 18464, at: NOW, action: "file_edited", actor: AMIR, via: "ui" }), ...world.audit];
    await userEvent.click(screen.getByRole("button", { name: "Refresh" }));
    await waitFor(() => expect(auditCalls(app.calls).length).toBe(before + 1));
    expect(await within(await grid()).findByText(/#18,464/)).toBeInTheDocument();
  });
});

describe("the updates that may have reached the log (no polling)", () => {
  let hub: FakeHub;
  let nextId: number;
  beforeEach(() => {
    hub = new FakeHub();
    nextId = 41;
  });

  const send = async (event: DomainEvent): Promise<void> => {
    await act(async () => {
      hub.last.push(frame(nextId++, event));
      await Promise.resolve();
    });
  };

  it("count the events that changed what the log is about, and read the log again on Refresh", async () => {
    world.proposals = [];
    const app = renderApp("/audit", worldApi(world), { live: { stream: { fetch: hub.fetch, random: () => 0.5, onlineTarget: new EventTarget() } } });
    await screen.findByRole("heading", { level: 1, name: "Audit log" }, { timeout: 8000 });
    await grid();
    await waitFor(() => expect(hub.connections).toHaveLength(1), { timeout: 4000 });
    await act(async () => {
      hub.last.push(FIRST_FRAME);
      await Promise.resolve();
    });
    // The first connection reads everything on screen once; that is not news. Wait out the catch-up.
    await new Promise((resolve) => setTimeout(resolve, 1300));
    expect(screen.queryByText(/^\d+ updates?$/)).toBeNull();

    const before = auditCalls(app.calls).length;
    await send({ type: "proposal_changed", id: 2046, status: "approved" });
    await send({ type: "file_observed", swimlane: "sitb", path: "tx-infinity-api/core.yml", hash: chainHash(1), source: "tool_write" });
    expect(await screen.findByText(/^\d+ updates?$/, {}, { timeout: 3000 })).toBeInTheDocument();
    // Nothing was read again by itself.
    expect(auditCalls(app.calls).length).toBe(before);

    await userEvent.click(screen.getByRole("button", { name: /Refresh/ }));
    await waitFor(() => expect(auditCalls(app.calls).length).toBe(before + 1));
    await waitFor(() => expect(screen.queryByText(/^\d+ updates?$/)).toBeNull());
  });
});

describe("the viewer-scoped audit is not in the contract", () => {
  it("is not asked for: the screen reads `adminListAudit` and nothing else of the audit", async () => {
    const app = await open();
    await grid();
    const audit = app.calls.filter((c) => /audit|activity/.test(c.path)).map((c) => new URL(c.path, "http://hub.test").pathname);
    expect([...new Set(audit)]).toEqual(["/api/v1/admin/audit"]);
  });
});

describe("structure (axe, in jsdom: names, roles and ids; contrast and size are checked in a real browser)", () => {
  it("has none to report for the log, with an event open and a chain break shown", async () => {
    world.audit = world.audit.map((e) => (e.seq === 18455 ? { ...e, prev_hash: chainHash(999) } : e));
    await open();
    await userEvent.click(await rowOf(18454));
    await screen.findByRole("dialog", { name: "Event detail" });
    await expectNoAxeViolations();
  });

  it("has none to report for the empty log", async () => {
    world.audit = [];
    await open();
    await screen.findByText("No audit events yet");
    await expectNoAxeViolations();
  });
});

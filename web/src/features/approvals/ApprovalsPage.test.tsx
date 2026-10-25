import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Proposal } from "@/api/generated/model";
import { ANA, DANA, hash, NOW, PENDING, proposal, STALE } from "@/mocks/design/approvals/fixtures";
import { newWorld, worldApi, type World } from "@/mocks/design/approvals/world";
import { json, problem, renderApp, type Api } from "@/test/app";
import { expectNoAxeViolations } from "@/test/axe";

// The approvals queue: the page the design draws as "Approvals" (Pending, Stale, Expired and Decided), read from the contract's
// `listProposals` and kept fresh by the live events. These tests drive the whole app on a memory history, with `fetch` answered
// by a hub that is a function of the test (src/mocks/design/approvals/world.ts).

let world: World;
beforeEach(() => {
  // The proposals are dated from the clock of the visual runs; only `Date` is frozen, so the timers of the test still run.
  vi.useFakeTimers({ toFake: ["Date"], now: NOW });
  world = newWorld("admin");
});
afterEach(() => {
  vi.useRealTimers();
  Reflect.deleteProperty(document, "cookie");
});

const isList = (path: string, query?: string): boolean => {
  const url = new URL(path, "http://hub.test");
  return url.pathname === "/api/v1/proposals" && (query === undefined || url.search.includes(query));
};

/** The queue on screen, past the first load of the page's chunk (which takes longer than the default wait). */
async function open(url = "/approvals", api: Api = worldApi(world)) {
  const app = renderApp(url, api);
  await screen.findByRole("heading", { level: 1, name: "Approvals" }, { timeout: 8000 });
  return app;
}
const grid = () => screen.findByRole("grid", { name: "Proposals" });
const rowFor = async (text: string | RegExp): Promise<HTMLElement> => {
  const cell = await within(await grid()).findByText(text);
  const row = cell.closest<HTMLElement>('[role="row"]');
  if (row === null) throw new Error(`no row for ${String(text)}`);
  return row;
};

describe("the queue", () => {
  it("has the page's one h1, a subtitle with what is waiting, and the four tabs with the counts it knows", async () => {
    await open();
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(await screen.findByText("6 waiting for an Admin · 2 stale · you are signed in as Priya Shah")).toBeInTheDocument();
    const tabs = within(screen.getByRole("tablist", { name: "Approval status" }));
    expect(tabs.getByRole("tab", { name: "Pending 6" })).toHaveAttribute("aria-selected", "true");
    expect(tabs.getByRole("tab", { name: "Stale 2" })).toHaveAttribute("aria-selected", "false");
    // Expired and Decided are asked for when they are opened (each is a page of diffs), so they have no number before that.
    expect(tabs.getByRole("tab", { name: "Expired" })).toBeInTheDocument();
    expect(tabs.getByRole("tab", { name: "Decided" })).toBeInTheDocument();
  });

  it("shows one row per proposal: who, how, where, which file, how old, and how long is left", async () => {
    await open();
    const row = await rowFor("tx-infinity-core-sit7.yml");
    expect(within(row).getByText("Ana Ruiz")).toBeInTheDocument();
    expect(within(row).getByText("Edit")).toBeInTheDocument();
    expect(within(row).getByText("sitb")).toBeInTheDocument();
    expect(within(row).getByText("tx-infinity-api/")).toBeInTheDocument();
    expect(within(row).getByText("12 min")).toBeInTheDocument();
    expect(within(row).getByText("23 h")).toBeInTheDocument();
    const urgent = await rowFor("receipt-templates-sit11.yml");
    expect(within(urgent).getByText("5 h")).toHaveClass("font-semibold");
  });

  it("marks what was made through Cursor and what is your own", async () => {
    await open();
    const cursor = await rowFor("tx-infinity-payments-sit9.yml");
    expect(within(cursor).getByText("via Cursor")).toBeInTheDocument();
    expect(within(cursor).getByText("AI-made edit")).toBeInTheDocument();
    const own = await rowFor("tx-infinity-gateway-sit7.yml");
    expect(within(own).getByText("You")).toBeInTheDocument();
    expect(within(own).getByText("Pull request")).toBeInTheDocument();
  });

  it("names a restart by its services, not by a file", async () => {
    await open();
    const row = await rowFor("gateway, tx-infinity-core");
    expect(within(row).getByText("Restart")).toBeInTheDocument();
    expect(within(row).getByText("services/")).toBeInTheDocument();
  });

  it("asks for the pending proposals once: the sidebar badge and the queue are one request", async () => {
    const app = await open();
    await grid();
    expect(app.calls.filter((c) => isList(c.path, "status=pending"))).toHaveLength(1);
  });

  it("draws its own skeleton, named and busy, while the first page is on its way", async () => {
    const never = new Promise<Response>(() => undefined);
    const base = worldApi(world);
    await open("/approvals", (call) => (isList(call.path, "status=pending") ? never : base(call)));
    expect(await screen.findByLabelText("Loading proposals")).toHaveAttribute("aria-busy", "true");
    expect(screen.getByText("Loading proposals…")).toBeInTheDocument();
  });

  it("says Couldn't load, with the problem's title and its request ID, when the first page fails (and offers a retry)", async () => {
    world.listProblem = problem(500, "internal");
    await open();
    expect(await screen.findByText("Couldn't load proposals")).toBeInTheDocument();
    expect(screen.getByText("req-test-1")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
  });

  it("says there are no proposals, and why, when every list is empty", async () => {
    world.proposals = [];
    await open();
    expect(await screen.findByText("No proposals")).toBeInTheDocument();
    expect(screen.getByText("When someone proposes an edit, upload, delete, restart or pull request, it appears here as it happens.")).toBeInTheDocument();
    expect(screen.getByText("Nothing waiting")).toBeInTheDocument();
  });

  it("says Nothing in Pending when only the other tabs have something", async () => {
    world.proposals = [...STALE];
    await open();
    expect(await screen.findByText("Nothing in Pending")).toBeInTheDocument();
    expect(screen.getByText("This list updates as proposals change.")).toBeInTheDocument();
  });

  it("says the footer's facts: what is shown, the lifetime from the proposals themselves, and the rule about your own", async () => {
    await open();
    await grid();
    const foot = screen.getByText(/Proposals expire 24 hours after they are made/);
    expect(foot).toBeInTheDocument();
    expect(screen.getByText("You can't approve your own proposal")).toBeInTheDocument();
    expect(screen.getAllByText("6 proposals").length).toBeGreaterThan(0);
  });

  it("pages with the hub's own page size and a Load more, never all at once", async () => {
    world.pageSize = 2;
    const app = await open();
    await grid();
    expect(screen.getAllByRole("row").filter((r) => r.getAttribute("aria-rowindex") !== "1")).toHaveLength(2);
    await userEvent.click(screen.getByRole("button", { name: "Load more" }));
    await waitFor(() => expect(app.calls.some((c) => isList(c.path, "cursor="))).toBe(true));
    await waitFor(() => expect(screen.getAllByRole("row").filter((r) => r.getAttribute("aria-rowindex") !== "1")).toHaveLength(4));
  });
});

describe("the tabs", () => {
  it("keep the address, ask for the status they are named for, and show what is stale", async () => {
    const app = await open();
    await userEvent.click(screen.getByRole("tab", { name: /^Stale/ }));
    await waitFor(() => expect(app.location()).toBe("/approvals?state=stale"));
    await waitFor(() => expect(screen.getByRole("tab", { name: /^Stale/ })).toHaveAttribute("aria-selected", "true"));
    const row = await rowFor("payment-limits-sit1.yml");
    expect(within(row).getByText("File changed since proposed")).toBeInTheDocument();
    expect(screen.getByRole("columnheader", { name: "Expires in" })).toBeInTheDocument();
  });

  it("open on the tab the address names", async () => {
    await open("/approvals?state=expired");
    await waitFor(() => expect(screen.getByRole("tab", { name: /^Expired/ })).toHaveAttribute("aria-selected", "true"));
    const row = await rowFor("receipt-templates-sit13.yml");
    expect(within(row).getByText("6 h ago")).toBeInTheDocument();
    expect(screen.getByRole("columnheader", { name: "Expired" })).toBeInTheDocument();
  });

  it("ask for Decided with decided=true and no status, and say who decided what", async () => {
    const app = await open("/approvals?state=decided");
    const row = await rowFor("account-sorting-config-sit7.yml");
    expect(within(row).getByText(/Approved · Priya/)).toBeInTheDocument();
    const rejected = await rowFor("kafka-topics-sit7.yml");
    expect(within(rejected).getByText(/Rejected · Priya/)).toBeInTheDocument();
    const call = app.calls.find((c) => isList(c.path, "decided=true"));
    expect(call).toBeDefined();
    expect(call?.path).not.toContain("status=");
    expect(screen.getByRole("columnheader", { name: "Decision" })).toBeInTheDocument();
  });

  it("show the hub's refusal when it does not accept decided=true yet, as an error with a request ID and no retry loop", async () => {
    world.decidedProblem = problem(400, "bad_request");
    const app = await open("/approvals?state=decided");
    expect(await screen.findByText("Couldn't load proposals")).toBeInTheDocument();
    expect(screen.getByText("req-test-1")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
    expect(app.calls.filter((c) => isList(c.path, "decided=true"))).toHaveLength(1);
  });

  it("read the handoff's own words too: approved and rejected are Decided, withdrawn is nothing we know", async () => {
    await open("/approvals?state=rejected");
    expect(await screen.findByRole("tab", { name: /^Decided/ })).toHaveAttribute("aria-selected", "true");
  });
});

describe("who may open it", () => {
  it("shows an Editor the permission screen and asks the hub for no proposals", async () => {
    world.role = "editor";
    const app = renderApp("/approvals", worldApi(world));
    expect(await screen.findByText("Approvals needs the Admin role", {}, { timeout: 8000 })).toBeInTheDocument();
    expect(screen.getByText("You are signed in as Editor. Ask an Admin to change your role.")).toBeInTheDocument();
    expect(app.calls.some((c) => isList(c.path))).toBe(false);
  });
});

describe("the drawer opens from a row", () => {
  it("goes to the proposal's own address, and shows its consequence first, then the diff, who proposed it and its status", async () => {
    const app = await open();
    await userEvent.click(await rowFor("tx-infinity-core-sit7.yml"));
    await waitFor(() => expect(app.location()).toBe("/approvals/2046"));
    const drawer = await screen.findByRole("dialog", { name: "Proposal detail" });
    expect(within(drawer).getByRole("heading", { level: 2, name: "Edit tx-infinity-core-sit7.yml" })).toBeInTheDocument();
    expect(within(drawer).getByText("Pending approval")).toBeInTheDocument();
    expect(within(drawer).getByText("Writes the proposed content to tx-infinity-api/tx-infinity-core-sit7.yml on sitb.")).toBeInTheDocument();
    expect(within(drawer).getByText("Ana Ruiz")).toBeInTheDocument();
    expect(within(drawer).getByText(/Proposed 12 min ago/)).toBeInTheDocument();
    expect(within(drawer).getByText(/Expires in 23 h/)).toBeInTheDocument();
    // The diff is the server's hunks, drawn by the shared diff view; the hashes are the proposal's.
    expect(await within(drawer).findByText("maxRetries: 5")).toBeInTheDocument();
    expect(within(drawer).getByText("a91f3c0")).toBeInTheDocument();
    expect(within(drawer).getByText("7c3e55d")).toBeInTheDocument();
  });

  it("lets the person copy the proposal's number and a link to it, which is the address the drawer is at", async () => {
    const user = userEvent.setup();
    const app = await open();
    await user.click(await rowFor("tx-infinity-core-sit7.yml"));
    const drawer = await screen.findByRole("dialog", { name: "Proposal detail" });
    expect(within(drawer).getByText("AP-2046")).toBeInTheDocument();
    await user.click(within(drawer).getByRole("button", { name: "Copy proposal ID" }));
    expect(await navigator.clipboard.readText()).toBe("AP-2046");
    await user.click(within(drawer).getByRole("button", { name: "Copy link to this proposal" }));
    expect(await navigator.clipboard.readText()).toBe(`${window.location.origin}${app.location()}`);
    expect(app.location()).toBe("/approvals/2046");
  });

  it("opens straight from a link to a proposal that is not on the first page", async () => {
    world.pageSize = 2;
    await open("/approvals/2037");
    const drawer = await screen.findByRole("dialog", { name: "Proposal detail" });
    expect(within(drawer).getByText("Expired")).toBeInTheDocument();
    // The queue behind it is on the tab the proposal belongs to.
    expect(screen.getByRole("tab", { name: /^Expired/ })).toHaveAttribute("aria-selected", "true");
  });

  it("says so, with the request ID, when the proposal is not there", async () => {
    await open("/approvals/999");
    const drawer = await screen.findByRole("dialog", { name: "Proposal detail" });
    expect(await within(drawer).findByText("Couldn't load proposal")).toBeInTheDocument();
    expect(within(drawer).getByText("req-test-1")).toBeInTheDocument();
  });

  it("closes with the button and with Escape, back to the queue on its tab", async () => {
    const app = await open("/approvals/2041");
    await screen.findByRole("dialog", { name: "Proposal detail" });
    await userEvent.click(screen.getByRole("button", { name: "Close" }));
    await waitFor(() => expect(app.location()).toBe("/approvals?state=stale"));
    expect(screen.queryByRole("dialog", { name: "Proposal detail" })).toBeNull();

    await userEvent.click(await rowFor("entryGroupConfig-sit9.yml"));
    await screen.findByRole("dialog", { name: "Proposal detail" });
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(app.location()).toBe("/approvals?state=stale"));
  });

  it("is reached from the keyboard: the arrow keys move through the rows and Enter opens the proposal under the cursor", async () => {
    const app = await open();
    const table = await grid();
    table.focus();
    await userEvent.keyboard("{ArrowDown}{ArrowDown}{Enter}");
    // Newest first: 2047, then 2046.
    await waitFor(() => expect(app.location()).toBe("/approvals/2046"));
  });
});

describe("closing the drawer", () => {
  // Found by the keyboard journey in a real browser: the drawer is an address of its own, closing it is a navigation, and the shell
  // puts focus on the heading of the page after one. The drawer is a dialog and was opened from the queue: the queue is where
  // focus goes back to, so a keyboard user is not sent to the top of the page.
  it("gives the keyboard back to the queue on Escape and on the Close button", async () => {
    const app = await open();
    const table = await grid();
    table.focus();
    await userEvent.keyboard("{ArrowDown}{ArrowDown}{Enter}");
    await waitFor(() => expect(app.location()).toBe("/approvals/2046"));
    await screen.findByRole("dialog", { name: "Proposal detail" });
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(app.location()).toBe("/approvals"));
    await waitFor(() => expect(screen.getByRole("grid", { name: "Proposals" })).toHaveFocus());

    await userEvent.click(await rowFor("tx-infinity-core-sit7.yml"));
    await screen.findByRole("dialog", { name: "Proposal detail" });
    await userEvent.click(screen.getByRole("button", { name: "Close" }));
    await waitFor(() => expect(app.location()).toBe("/approvals"));
    await waitFor(() => expect(screen.getByRole("grid", { name: "Proposals" })).toHaveFocus());
  });

  // Closing the drawer is a move to another route, so the queue is drawn anew; the keyboard cursor would go back to nothing, and
  // the person who had a proposal open would have to find their place again.
  it("puts the cursor back on the row whose drawer was open, on Escape, on the Close button and on Back", async () => {
    const app = await open();
    const table = await grid();
    const cursorText = async (): Promise<string | null> => {
      const current = await grid();
      return document.getElementById(current.getAttribute("aria-activedescendant") ?? "")?.textContent ?? null;
    };
    table.focus();
    await userEvent.keyboard("{ArrowDown}{ArrowDown}{Enter}");
    await waitFor(() => expect(app.location()).toBe("/approvals/2046"));
    await screen.findByRole("dialog", { name: "Proposal detail" });
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(app.location()).toBe("/approvals"));
    await waitFor(() => expect(screen.getByRole("grid", { name: "Proposals" })).toHaveFocus());
    await waitFor(async () => expect(await cursorText()).toContain("tx-infinity-core-sit7.yml"));

    // A click on another row, and the Close button.
    await userEvent.click(await rowFor("tx-infinity-payments-sit9.yml"));
    await screen.findByRole("dialog", { name: "Proposal detail" });
    await userEvent.click(screen.getByRole("button", { name: "Close" }));
    await waitFor(() => expect(app.location()).toBe("/approvals"));
    await waitFor(async () => expect(await cursorText()).toContain("tx-infinity-payments-sit9.yml"));

    // Back from the drawer is the same place.
    await userEvent.click(await rowFor("receipt-templates-sit11.yml"));
    await screen.findByRole("dialog", { name: "Proposal detail" });
    app.router.history.back();
    await waitFor(() => expect(app.location()).toBe("/approvals"));
    await waitFor(async () => expect(await cursorText()).toContain("receipt-templates-sit11.yml"));
  });

  it("puts the cursor on the proposal of the address when the drawer was reached by it", async () => {
    const app = await open("/approvals/2041");
    await screen.findByRole("dialog", { name: "Proposal detail" });
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(app.location()).toBe("/approvals?state=stale"));
    await waitFor(() => {
      const table = screen.getByRole("grid", { name: "Proposals" });
      expect(document.getElementById(table.getAttribute("aria-activedescendant") ?? "")?.textContent ?? "").toContain("entryGroupConfig-sit9.yml");
    });
  });

  it("does the same when the drawer was reached by its address", async () => {
    const app = await open("/approvals/2041");
    await screen.findByRole("dialog", { name: "Proposal detail" });
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(app.location()).toBe("/approvals?state=stale"));
    await waitFor(() => expect(screen.getByRole("grid", { name: "Proposals" })).toHaveFocus());
  });
});

describe("the stale proposals", () => {
  it("say the file changed, offer Reject with a reason, and do not offer Approve", async () => {
    await open("/approvals/2041");
    const drawer = await screen.findByRole("dialog", { name: "Proposal detail" });
    // The banner says it, and so does the reason the disabled Approve gives (for a screen reader, and in its tooltip).
    expect(within(drawer).getAllByText("The file changed since this was proposed").length).toBeGreaterThanOrEqual(1);
    expect(within(drawer).getByText("Not approvable: the file changed since this was proposed.")).toBeInTheDocument();
    expect(within(drawer).getByRole("button", { name: "Approve" })).toHaveAttribute("aria-disabled", "true");
    expect(within(drawer).getByLabelText("Reason (required to reject)")).toBeInTheDocument();
  });
});

describe("the expired and the decided proposals", () => {
  it("are read only, with the way to the audit log", async () => {
    await open("/approvals/2037");
    const drawer = await screen.findByRole("dialog", { name: "Proposal detail" });
    expect(within(drawer).getByText("Expired proposals can't be decided.")).toBeInTheDocument();
    expect(within(drawer).queryByRole("button", { name: "Approve" })).toBeNull();
    expect(within(drawer).queryByRole("button", { name: "Reject" })).toBeNull();
    expect(within(drawer).getByRole("link", { name: "View in audit log" })).toHaveAttribute("href", "/audit?swimlane=presitc");
  });

  it("say who decided, when, and what they wrote", async () => {
    await open("/approvals/2032");
    const drawer = await screen.findByRole("dialog", { name: "Proposal detail" });
    expect(within(drawer).getByText(/Rejected by Priya Shah/)).toBeInTheDocument();
    expect(within(drawer).getByText("“Wrong swimlane. The topic belongs on sitc.”")).toBeInTheDocument();
    expect(within(drawer).queryByRole("button", { name: "Approve" })).toBeNull();
  });
});

describe("your own proposal", () => {
  it("can be neither approved nor rejected by you, and says which Admins can", async () => {
    await open("/approvals/2044");
    const drawer = await screen.findByRole("dialog", { name: "Proposal detail" });
    expect(within(drawer).getAllByText("You can't approve your own proposal").length).toBeGreaterThanOrEqual(1);
    expect(within(drawer).getByText(/Amir Haddad and Lena Brandt are the other Admins\./)).toBeInTheDocument();
    expect(within(drawer).getByRole("button", { name: "Approve" })).toHaveAttribute("aria-disabled", "true");
    expect(within(drawer).getByRole("button", { name: "Reject" })).toHaveAttribute("aria-disabled", "true");
    // The Withdraw button of the design has no operation in the contract, so it is not offered.
    expect(within(drawer).queryByRole("button", { name: /Withdraw/ })).toBeNull();
  });
});

describe("rejecting", () => {
  it("needs a reason of at least three characters, then sends it, with the CSRF token and an Idempotency-Key", async () => {
    const app = await open("/approvals/2046");
    const drawer = await screen.findByRole("dialog", { name: "Proposal detail" });
    const reject = () => within(drawer).getByRole("button", { name: "Reject" });
    expect(reject()).toHaveAttribute("aria-disabled", "true");
    await userEvent.type(within(drawer).getByLabelText("Reason (required to reject)"), "ab");
    expect(reject()).toHaveAttribute("aria-disabled", "true");
    await userEvent.type(within(drawer).getByLabelText("Reason (required to reject)"), "c  ");
    expect(reject()).not.toHaveAttribute("aria-disabled");
    await userEvent.click(reject());
    await waitFor(() => expect(world.decisions).toHaveLength(1));
    const [decision] = world.decisions;
    expect(decision?.verb).toBe("reject");
    expect(JSON.parse(decision?.body ?? "{}")).toEqual({ reason: "abc" });
    expect(decision?.headers.get("x-csrf-token")).toBeTruthy();
    expect(decision?.headers.get("idempotency-key")).toMatch(/^[0-9a-f-]{36}$/);
    // The server's answer is what the drawer says: rejected by you, with the reason.
    expect(await within(drawer).findByText(/Rejected by Priya Shah/)).toBeInTheDocument();
    expect(within(drawer).getByText("“abc”")).toBeInTheDocument();
    // And the lists were asked again.
    await waitFor(() => expect(app.calls.filter((c) => isList(c.path, "status=pending")).length).toBeGreaterThanOrEqual(2));
  });

  it("limits the reason to what the contract takes", async () => {
    await open("/approvals/2046");
    const drawer = await screen.findByRole("dialog", { name: "Proposal detail" });
    expect(within(drawer).getByLabelText("Reason (required to reject)")).toHaveAttribute("maxlength", "256");
  });
});

describe("approving", () => {
  it("sends the decision once, with no body, and shows the server's answer: approved, written, and the audit event", async () => {
    const app = await open("/approvals/2046");
    const drawer = await screen.findByRole("dialog", { name: "Proposal detail" });
    await userEvent.click(within(drawer).getByRole("button", { name: "Approve" }));
    await waitFor(() => expect(world.decisions).toHaveLength(1));
    const [decision] = world.decisions;
    expect(decision?.verb).toBe("approve");
    expect(decision?.body).toBeUndefined();
    expect(decision?.headers.get("x-csrf-token")).toBeTruthy();
    expect(decision?.headers.get("idempotency-key")).toMatch(/^[0-9a-f-]{36}$/);
    expect(await within(drawer).findByText("Approved · written to sitb")).toBeInTheDocument();
    expect(within(drawer).getByText(/audit event 18,470/i)).toBeInTheDocument();
    // The file's caches and the queue were asked again (the swimlane list is the one the sidebar holds).
    await waitFor(() => expect(app.calls.filter((c) => isList(c.path, "status=pending")).length).toBeGreaterThanOrEqual(2));
    expect(app.calls.some((c) => c.path.startsWith("/api/v1/swimlanes"))).toBe(true);
  });

  it("is never optimistic: nothing on screen says approved until the server has answered", async () => {
    let release: () => void = () => undefined;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    const base = worldApi(world);
    await open("/approvals/2046", (call) => (call.path.endsWith("/approve") ? gate.then(() => base(call)) : base(call)));
    const drawer = await screen.findByRole("dialog", { name: "Proposal detail" });
    await userEvent.click(within(drawer).getByRole("button", { name: "Approve" }));
    const working = await within(drawer).findByRole("button", { name: "Approving…" });
    expect(working).toHaveAttribute("aria-busy", "true");
    expect(within(drawer).getByRole("button", { name: "Reject" })).toHaveAttribute("aria-disabled", "true");
    expect(within(drawer).queryByText(/Approved/)).toBeNull();
    expect(within(drawer).getByText("Pending approval")).toBeInTheDocument();
    // A second click while it runs sends nothing more.
    await userEvent.click(working);
    release();
    await within(drawer).findByText("Approved · written to sitb");
    expect(world.decisions).toHaveLength(1);
  });

  it("says when the approval itself was sent on for another approval, with the new request", async () => {
    world.approve = json({ outcome: "proposal_created", id: 2051 });
    await open("/approvals/2046");
    const drawer = await screen.findByRole("dialog", { name: "Proposal detail" });
    await userEvent.click(within(drawer).getByRole("button", { name: "Approve" }));
    expect(await within(drawer).findByText("Sent for approval")).toBeInTheDocument();
    expect(within(drawer).getByRole("link", { name: "View request" })).toHaveAttribute("href", "/approvals/2051");
    expect(within(drawer).queryByText(/Approved · written/)).toBeNull();
  });

  it("says the file changed first on a 409, in a dialog, and leaves the proposal for the server to mark stale", async () => {
    world.approve = (found) => {
      world.proposals = world.proposals.map((p) => (p.id === found.id ? { ...p, status: "stale" } : p));
      return problem(409, "conflict", { current: hash("e2a8f610") });
    };
    await open("/approvals/2046");
    const drawer = await screen.findByRole("dialog", { name: "Proposal detail" });
    await userEvent.click(within(drawer).getByRole("button", { name: "Approve" }));
    const dialog = await screen.findByRole("dialog", { name: "The file changed first" });
    expect(within(dialog).getByText(/Nothing was written/)).toBeInTheDocument();
    expect(within(dialog).getByText("e2a8f61")).toBeInTheDocument();
    expect(within(dialog).getByText("req-test-1")).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("button", { name: "Close" }));
    // The proposal is asked for again and is now stale: no Approve any more.
    await waitFor(() => expect(within(drawer).getByRole("button", { name: "Approve" })).toHaveAttribute("aria-disabled", "true"));
    expect(within(drawer).getAllByText("The file changed since this was proposed").length).toBeGreaterThanOrEqual(1);
  });

  it("says the swimlane is locked on a 423, until when the hub says, and that nothing was approved", async () => {
    world.approve = problem(423, "locked", { until: 1_760_003_600_000 });
    await open("/approvals/2046");
    const drawer = await screen.findByRole("dialog", { name: "Proposal detail" });
    await userEvent.click(within(drawer).getByRole("button", { name: "Approve" }));
    const dialog = await screen.findByRole("dialog", { name: "Editing is locked" });
    expect(within(dialog).getByText(/nothing was approved/i)).toBeInTheDocument();
    expect(within(dialog).getByText(/Locked until 2025-10-09 09:53 UTC/)).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("button", { name: "Close" }));
    expect(within(drawer).getByText("Pending approval")).toBeInTheDocument();
    expect(within(drawer).getByRole("button", { name: "Approve" })).not.toHaveAttribute("aria-disabled");
  });

  it("says a 403 inside the drawer, with the request ID, and records nothing", async () => {
    world.approve = problem(403, "forbidden");
    await open("/approvals/2046");
    const drawer = await screen.findByRole("dialog", { name: "Proposal detail" });
    await userEvent.click(within(drawer).getByRole("button", { name: "Approve" }));
    expect(await within(drawer).findByText("You may not decide proposals")).toBeInTheDocument();
    expect(within(drawer).getByText("Deciding needs the Admin role. Nothing was recorded.")).toBeInTheDocument();
    expect(within(drawer).getByText("req-test-1")).toBeInTheDocument();
  });

  it("says a 422 not_allowed means someone else got there first, and asks for the proposal again", async () => {
    world.approve = (found) => {
      world.proposals = world.proposals.map((p) => (p.id === found.id ? { ...p, status: "applied", decided_by: DANA, decided_at: 1_760_000_000_000 } : p));
      return problem(422, "unprocessable", { reason: "not_allowed" });
    };
    const app = await open("/approvals/2046");
    const drawer = await screen.findByRole("dialog", { name: "Proposal detail" });
    await userEvent.click(within(drawer).getByRole("button", { name: "Approve" }));
    expect(await within(drawer).findByText("This proposal can't be decided now")).toBeInTheDocument();
    expect(within(drawer).getByText(/Your decision was not recorded/)).toBeInTheDocument();
    await waitFor(() => expect(app.calls.filter((c) => c.path === "/api/v1/proposals/2046").length).toBeGreaterThanOrEqual(2));
    expect(await within(drawer).findByText(/Approved by Dana Okoye/)).toBeInTheDocument();
  });

  it("does not repeat a decision that got no answer: it says it may or may not have been recorded", async () => {
    const base = worldApi(world);
    await open("/approvals/2046", (call) => (call.path.endsWith("/approve") ? problem(500, "internal") : base(call)));
    const drawer = await screen.findByRole("dialog", { name: "Proposal detail" });
    await userEvent.click(within(drawer).getByRole("button", { name: "Approve" }));
    expect(await within(drawer).findByText(/could not finish the decision/)).toBeInTheDocument();
    expect(world.decisions).toHaveLength(0);
  });
});

describe("the people", () => {
  it("falls back to the start of the user id when the user list has no one by that id", async () => {
    world.users = [];
    const stranger: Proposal = proposal({ id: 3000, author: ANA, swimlane: "sit", path: "a/b.yml" });
    world.proposals = [stranger, ...PENDING.slice(0, 1)];
    await open();
    expect(await screen.findByText("user ffffffff")).toBeInTheDocument();
  });
});

describe("structure (axe, in jsdom: names, roles and ids; contrast and size are checked in a real browser)", () => {
  it("has none to report for the queue", async () => {
    await open();
    await grid();
    await expectNoAxeViolations();
  });

  it("has none to report for the queue with the drawer of a stale proposal open", async () => {
    await open("/approvals/2041");
    const drawer = await screen.findByRole("dialog", { name: "Proposal detail" });
    await within(drawer).findByText("maxRetries", { exact: false }).catch(() => undefined);
    await expectNoAxeViolations();
  });

  it("has none to report for the drawer of a proposal an Admin may decide, and for the empty queue", async () => {
    await open("/approvals/2046");
    await screen.findByRole("button", { name: "Approve" });
    await expectNoAxeViolations();
  });

  it("has none to report for the empty queue", async () => {
    world.proposals = [];
    await open();
    await screen.findByText("No proposals");
    await expectNoAxeViolations();
  });
});

import { configure, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Finding, Role } from "@/api/generated/model";
import { DESIGN_FINDINGS, NOW, finding, laneCounts } from "@/mocks/design/findings/fixtures";
import { expectNoAxeViolations } from "@/test/axe";
import { json, problem, renderApp } from "@/test/app";
import { cellsOf, tableRows } from "@/mocks/design/findings/dom";
import { isFindings, newWorld, queryOf, worldApi, type World } from "@/mocks/design/findings/world";

// The Findings screen as a person meets it: the real router, session and shell, and a fake hub that answers the way the registry
// does (the filters of the contract, newest first, a keyset cursor). What each cell says, the order, the filters and the address,
// the drawer and where its links go, and every state of the States index for this screen. The live behaviour is in
// FindingsPage.live.test.tsx and every check C1 to C12 in Kinds.test.tsx.

// The first screen of a run loads the page's chunk and the shell's, which takes longer than the default wait on a busy machine.
configure({ asyncUtilTimeout: 8_000 });
vi.setConfig({ testTimeout: 30_000 });

let world: World;
beforeEach(() => {
  world = newWorld(DESIGN_FINDINGS, "admin");
});
afterEach(() => {
  Reflect.deleteProperty(document, "cookie");
});

const open = (url = "/findings", role: Role = "admin") => {
  world.role = role;
  return renderApp(url, worldApi(world));
};
/** A button of the toolbar (not a column header of the same name). */
const filterButton = (name: RegExp): HTMLElement => within(screen.getByRole("group", { name: "Filters" })).getByRole("button", { name });
const findingCalls = (app: ReturnType<typeof open>) => app.calls.filter((c) => isFindings(c.path));
const rowFor = async (text: string | RegExp): Promise<HTMLElement> => {
  await screen.findByRole("grid", { name: "Findings" });
  const row = await waitFor(() => {
    const found = tableRows().find((r) => (typeof text === "string" ? (r.textContent ?? "").includes(text) : text.test(r.textContent ?? "")));
    if (found === undefined) throw new Error(`no row with ${String(text)}`);
    return found;
  });
  return row;
};

describe("the screen", () => {
  it("shows its title at once, then the facts the swimlane list adds up to", async () => {
    open();
    expect(await screen.findByRole("heading", { level: 1, name: "Findings" })).toBeInTheDocument();
    expect(await screen.findByText("11 open across 5 swimlanes")).toBeInTheDocument();
    expect(await screen.findByText("Last scan 08:49 UTC · 12 checks across 6 swimlanes")).toBeInTheDocument();
  });

  it("lists every finding, most severe first, then by swimlane in the swimlane list's order", async () => {
    open();
    await rowFor("security-roles.yml");
    const rows = tableRows();
    expect(rows).toHaveLength(11);
    const lanes = rows.map((row) => cellsOf(row)[2]);
    expect(lanes).toEqual(["sitb", "sitc", "presita", "presitb", "presitb", "sitb", "sitb", "sitc", "presita", "sit", "sitc"]);
    const severities = rows.map((row) => within(row).getByText(/^(Critical|High|Medium|Low)$/).textContent);
    expect(severities).toEqual([...Array<string>(5).fill("High"), ...Array<string>(4).fill("Medium"), ...Array<string>(2).fill("Low")]);
  });

  it("says in each row the severity, the check, the swimlane, the file, what is wrong and the lines", async () => {
    open();
    const row = await rowFor("security-roles.yml");
    const [severity, check, lane, file, wrong, lines] = cellsOf(row);
    expect(severity).toBe("High");
    expect(check).toBe("C8Duplicate keys");
    expect(lane).toBe("presitb");
    expect(file).toContain("security-roles.yml");
    expect(file).toContain("security/");
    expect(wrong).toBe("Duplicate keys in security-roles.yml at lines 5171, 5643 and 6115");
    expect(lines).toBe("L5171 +2");
  });

  it("links the lines to the file at the first of them", async () => {
    open();
    const row = await rowFor("security-roles.yml");
    const link = within(row).getByRole("link", { name: /L5171/ });
    expect(link).toHaveAttribute("href", "/swimlanes/presitb/files?line=5171&path=security%2Fsecurity-roles.yml");
  });

  it("says there are no lines for a finding that names none, and no file for one about no file", async () => {
    world = newWorld([finding(1, { kind: "drift" }), finding(2, { kind: "config_server_restart_required", path: null })], "viewer");
    open("/findings", "viewer");
    const noLines = await rowFor("tx-infinity-core-sit7.yml");
    expect(within(noLines).queryByRole("link", { name: /L\d/ })).toBeNull();
    expect(cellsOf(noLines)[5]).toBe("None");
    const noFile = await rowFor("C11");
    expect(cellsOf(noFile)[3]).toBe("No file");
  });

  it("shows text from the server with its invisible characters made visible", async () => {
    world = newWorld([finding(1, { message: "host is evil‮com", path: "a/b​c.yml" })], "viewer");
    open("/findings", "viewer");
    const row = await rowFor("host is evil");
    expect(within(row).getAllByRole("img").map((el) => el.getAttribute("aria-label"))).toEqual(expect.arrayContaining([expect.stringContaining("U+202E"), expect.stringContaining("U+200B")]));
  });

  it("asks for nothing the browser should not decide: no severity, kind or drift is computed here", async () => {
    const app = open();
    await rowFor("security-roles.yml");
    const calls = findingCalls(app);
    expect(calls).toHaveLength(1);
    expect(queryOf(calls[0]?.path ?? "").toString()).toBe("");
  });
});

describe("the order", () => {
  it("can be changed by the swimlane column, both ways, and the header says which", async () => {
    open();
    await rowFor("security-roles.yml");
    const grid = screen.getByRole("grid", { name: "Findings" });
    await userEvent.click(within(grid).getByRole("button", { name: "Swimlane" }));
    expect(within(grid).getByRole("columnheader", { name: "Swimlane" })).toHaveAttribute("aria-sort", "ascending");
    expect(tableRows().map((r) => cellsOf(r)[2])).toEqual(["sit", "sitb", "sitb", "sitb", "sitc", "sitc", "sitc", "presita", "presita", "presitb", "presitb"]);
    await userEvent.click(within(grid).getByRole("button", { name: "Swimlane" }));
    expect(within(grid).getByRole("columnheader", { name: "Swimlane" })).toHaveAttribute("aria-sort", "descending");
    expect(tableRows().map((r) => cellsOf(r)[2])[0]).toBe("presitb");
  });
});

describe("the filters", () => {
  it("narrow by check on the server, and the address keeps them", async () => {
    const app = open();
    await rowFor("security-roles.yml");
    await userEvent.click(filterButton(/^Check/));
    await userEvent.click(await screen.findByRole("menuitemcheckbox", { name: /C8.*Duplicate keys/ }));
    await waitFor(() => expect(tableRows()).toHaveLength(1));
    expect(queryOf(findingCalls(app).at(-1)?.path ?? "").get("kinds")).toBe("duplicate_keys");
    expect(app.location()).toBe("/findings?code=C8");
    // The menu stays open while the person ticks, and Escape closes it.
    expect(screen.getByRole("menu")).toBeInTheDocument();
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
    expect(filterButton(/^Check/)).toHaveTextContent("C8");
  });

  it("narrow by severity, 'that and above', on the server", async () => {
    const app = open();
    await rowFor("security-roles.yml");
    await userEvent.click(filterButton(/^Severity/));
    await userEvent.click(await screen.findByRole("menuitemradio", { name: "High and above" }));
    await waitFor(() => expect(tableRows()).toHaveLength(5));
    expect(queryOf(findingCalls(app).at(-1)?.path ?? "").get("min_severity")).toBe("high");
    expect(app.location()).toBe("/findings?severity=high");
  });

  it("narrow to one swimlane on the server", async () => {
    const app = open();
    await rowFor("security-roles.yml");
    await userEvent.click(filterButton(/^Swimlane/));
    await userEvent.click(await screen.findByRole("menuitemcheckbox", { name: /^SIT sitb/ }));
    await waitFor(() => expect(tableRows()).toHaveLength(3));
    expect(queryOf(findingCalls(app).at(-1)?.path ?? "").get("swimlane")).toBe("sitb");
    expect(app.location()).toBe("/findings?lanes=sitb");
  });

  it("narrow to several swimlanes here, because the server filters by one, and say how many each has", async () => {
    const app = open("/findings?lanes=sitb,sitc");
    await waitFor(() => expect(tableRows()).toHaveLength(6));
    expect(new Set(tableRows().map((r) => cellsOf(r)[2]))).toEqual(new Set(["sitb", "sitc"]));
    expect(queryOf(findingCalls(app).at(-1)?.path ?? "").has("swimlane")).toBe(false);
    await userEvent.click(filterButton(/^Swimlane/));
    expect(await screen.findByRole("menuitemcheckbox", { name: "PRESIT presitb 2" })).toHaveAttribute("aria-checked", "false");
    expect(screen.getByRole("menuitemcheckbox", { name: "SIT sitb 3" })).toHaveAttribute("aria-checked", "true");
  });

  it("start from the address, and show their values on their buttons", async () => {
    const app = open("/findings?code=C6&severity=medium&lanes=presita");
    await waitFor(() => expect(tableRows()).toHaveLength(1));
    const query = queryOf(findingCalls(app).at(-1)?.path ?? "");
    expect([query.get("kinds"), query.get("min_severity"), query.get("swimlane")]).toEqual(["environment_host_mismatch", "medium", "presita"]);
    expect(filterButton(/^Swimlane/)).toHaveTextContent("presita");
    expect(filterButton(/^Check/)).toHaveTextContent("C6");
    expect(filterButton(/^Severity/)).toHaveTextContent("Medium and above");
  });

  it("ignore what the address says that is not a check, a severity or a swimlane", async () => {
    const app = open("/findings?code=C99&severity=urgent&lanes=../x");
    await waitFor(() => expect(tableRows()).toHaveLength(11));
    expect(queryOf(findingCalls(app)[0]?.path ?? "").toString()).toBe("");
  });

  it("are cleared in one click, and the address is bare again", async () => {
    const app = open("/findings?code=C8&severity=high");
    await waitFor(() => expect(tableRows()).toHaveLength(1));
    await userEvent.click(screen.getByRole("button", { name: "Clear filters" }));
    await waitFor(() => expect(tableRows()).toHaveLength(11));
    expect(app.location()).toBe("/findings");
    expect(screen.queryByRole("button", { name: "Clear filters" })).toBeNull();
  });

  it("show an empty answer as 'no match', with the way out, and not as 'nothing open'", async () => {
    open("/findings?lanes=sit&code=C8");
    expect(await screen.findByText("No findings match these filters")).toBeInTheDocument();
    expect(screen.getByText(/clear them all to see 11 findings/)).toBeInTheDocument();
    expect(screen.queryByText("No open findings")).toBeNull();
    await userEvent.click(screen.getAllByRole("button", { name: "Clear filters" })[0] as HTMLElement);
    await waitFor(() => expect(tableRows()).toHaveLength(11));
  });
});

describe("the drawer", () => {
  it("opens on a click on a row, names the check, and says what it means and what to do", async () => {
    open();
    await userEvent.click(await rowFor("security-roles.yml"));
    const drawer = await screen.findByRole("dialog", { name: "Duplicate keys" });
    expect(within(drawer).getByText("High severity")).toBeInTheDocument();
    expect(within(drawer).getByText("C8")).toBeInTheDocument();
    expect(within(drawer).getByText(/The same key appears more than once/)).toBeInTheDocument();
    expect(within(drawer).getByText("Open the file at each line the finding names.")).toBeInTheDocument();
    expect(within(drawer).getByText("Duplicate keys in security-roles.yml at lines 5171, 5643 and 6115")).toBeInTheDocument();
  });

  it("shows where the finding is: the swimlane, the tenant file, the base file and when it was found", async () => {
    open();
    await userEvent.click(await rowFor("gateway-sit7.yml"));
    const drawer = await screen.findByRole("dialog", { name: "Sync overwrote an NFS change" });
    expect(within(drawer).getByText("sitb")).toBeInTheDocument();
    expect(within(drawer).getByText("tx-infinity-api/tx-infinity-gateway-sit7.yml")).toBeInTheDocument();
    expect(within(drawer).getByText("Tenant")).toBeInTheDocument();
    expect(within(drawer).getByText("sit7")).toBeInTheDocument();
    expect(within(drawer).getByText("Base file")).toBeInTheDocument();
    expect(within(drawer).getByText("tx-infinity-api/tx-infinity-gateway.yml")).toBeInTheDocument();
    expect(within(drawer).getByText(/2025-10-09 08:32 UTC/)).toBeInTheDocument();
    expect(within(drawer).getByText("default.overwritten-nfs-change")).toBeInTheDocument();
  });

  it("opens with focus on Close, never on the copy button that only shows when it is hovered or focused", async () => {
    open();
    await userEvent.click(await rowFor("security-roles.yml"));
    const drawer = await screen.findByRole("dialog", { name: "Duplicate keys" });
    await waitFor(() => expect(within(drawer).getByRole("button", { name: "Close" })).toHaveFocus());
  });

  it("lets the path and the base file be copied, each by what it is", async () => {
    const user = userEvent.setup();
    open();
    await user.click(await rowFor("gateway-sit7.yml"));
    const drawer = await screen.findByRole("dialog", { name: "Sync overwrote an NFS change" });
    await user.click(within(drawer).getByRole("button", { name: "Copy path" }));
    expect(await navigator.clipboard.readText()).toBe("tx-infinity-api/tx-infinity-gateway-sit7.yml");
    await user.click(within(drawer).getByRole("button", { name: "Copy base file path" }));
    expect(await navigator.clipboard.readText()).toBe("tx-infinity-api/tx-infinity-gateway.yml");
  });

  it("jumps to each place in the file named by the finding", async () => {
    open();
    await userEvent.click(await rowFor("security-roles.yml"));
    const drawer = await screen.findByRole("dialog", { name: "Duplicate keys" });
    for (const line of [5171, 5643, 6115]) {
      expect(within(drawer).getByRole("link", { name: new RegExp(`Open at L${line}`) })).toHaveAttribute("href", `/swimlanes/presitb/files?line=${line}&path=security%2Fsecurity-roles.yml`);
    }
    expect(within(drawer).getAllByText("roles.teller-supervisor")).toHaveLength(3);
  });

  it("opens the file at the first line, and for a drift the diff against Git", async () => {
    world = newWorld([finding(1, { kind: "drift", path: "a/b.yml", locations: [{ path: "x", lines: { start: 7, end: 9 } }] })], "viewer");
    open("/findings", "viewer");
    await userEvent.click(await rowFor("b.yml"));
    const drawer = await screen.findByRole("dialog", { name: "NFS and Git disagree" });
    expect(within(drawer).getByRole("link", { name: "Open file" })).toHaveAttribute("href", "/swimlanes/sitb/files?line=7&path=a%2Fb.yml");
    expect(within(drawer).getByRole("link", { name: "Open diff" })).toHaveAttribute("href", "/swimlanes/sitb/files?diff=git&path=a%2Fb.yml&tab=diff");
  });

  it("opens the file's history for a change a sync overwrote, since that is where the lost version is", async () => {
    open();
    await userEvent.click(await rowFor("gateway-sit7.yml"));
    const drawer = await screen.findByRole("dialog", { name: "Sync overwrote an NFS change" });
    expect(within(drawer).getByRole("link", { name: "Open history" })).toHaveAttribute("href", "/swimlanes/sitb/files?line=22&panel=history&path=tx-infinity-api%2Ftx-infinity-gateway-sit7.yml");
  });

  it("offers no action without an operation behind it: no marking as intentional, no restoring", async () => {
    open();
    await userEvent.click(await rowFor("gateway-sit7.yml"));
    await screen.findByRole("dialog", { name: "Sync overwrote an NFS change" });
    for (const name of [/intentional/i, /restore/i, /reopen/i]) {
      expect(screen.queryByRole("button", { name })).toBeNull();
      expect(screen.queryByRole("link", { name })).toBeNull();
    }
    expect(screen.queryByRole("tab", { name: /intentional/i })).toBeNull();
    expect(screen.queryByRole("button", { name: /^Intentional/ })).toBeNull();
  });

  it("is the same for a Viewer: there is nothing to disable, so nothing is hidden and nothing is greyed", async () => {
    open("/findings", "viewer");
    await userEvent.click(await rowFor("security-roles.yml"));
    const drawer = await screen.findByRole("dialog", { name: "Duplicate keys" });
    expect(within(drawer).getByRole("link", { name: "Open file" })).toBeInTheDocument();
    // Nothing to act on: the only buttons are Close and the copy buttons, which read and write nothing of the hub's.
    expect(within(drawer).queryByRole("button", { name: /^(?!Close|Copy )/ })).toBeNull();
  });

  it("says so for a finding about no file, and has no link to a file", async () => {
    world = newWorld([finding(1, { kind: "config_server_restart_required", path: null, message: "channels.yml changed" })], "viewer");
    open("/findings", "viewer");
    await userEvent.click(await rowFor("C11"));
    const drawer = await screen.findByRole("dialog", { name: "Config-server restart needed" });
    expect(within(drawer).getByText("This finding is about the swimlane, not about one file.")).toBeInTheDocument();
    expect(within(drawer).queryByRole("link")).toBeNull();
  });

  it("shows the first places and the rest on request when a finding names many", async () => {
    const many = Array.from({ length: 45 }, (_, i) => ({ path: `k.${i}`, lines: { start: i + 1, end: i + 1 } }));
    world = newWorld([finding(1, { kind: "duplicate_keys", locations: many })], "viewer");
    open("/findings", "viewer");
    await userEvent.click(await rowFor("tx-infinity-core-sit7.yml"));
    const drawer = await screen.findByRole("dialog", { name: "Duplicate keys" });
    expect(within(drawer).getAllByRole("link", { name: /Open at L/ })).toHaveLength(20);
    await userEvent.click(within(drawer).getByRole("button", { name: "Show all 45 places" }));
    expect(within(drawer).getAllByRole("link", { name: /Open at L/ })).toHaveLength(45);
  });

  it("closes on Escape and on Close, and gives the keyboard back to the table", async () => {
    open();
    await userEvent.click(await rowFor("security-roles.yml"));
    await screen.findByRole("dialog", { name: "Duplicate keys" });
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Duplicate keys" })).toBeNull());
    await waitFor(() => expect(screen.getByRole("grid", { name: "Findings" })).toHaveFocus());
    await userEvent.click(await rowFor("security-roles.yml"));
    await userEvent.click(within(await screen.findByRole("dialog", { name: "Duplicate keys" })).getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Duplicate keys" })).toBeNull());
  });

  it("opens from the keyboard: arrows to a row, Enter to open it", async () => {
    open();
    await rowFor("security-roles.yml");
    screen.getByRole("grid", { name: "Findings" }).focus();
    await userEvent.keyboard("{ArrowDown}{Enter}");
    // The first row of the default order is the sitb gateway change (a C7).
    expect(await screen.findByRole("dialog", { name: "Sync overwrote an NFS change" })).toBeInTheDocument();
  });

  it("switches to another finding when another row is clicked, without closing in between", async () => {
    open();
    await userEvent.click(await rowFor("security-roles.yml"));
    await screen.findByRole("dialog", { name: "Duplicate keys" });
    await userEvent.click(await rowFor("kafka-topics-sit11.yml"));
    expect(await screen.findByRole("dialog", { name: "Tenant copy is behind base" })).toBeInTheDocument();
    expect(screen.getAllByRole("dialog")).toHaveLength(1);
  });

  it("marks the open finding's row as selected", async () => {
    open();
    const row = await rowFor("security-roles.yml");
    expect(row).toHaveAttribute("aria-selected", "false");
    await userEvent.click(row);
    await screen.findByRole("dialog", { name: "Duplicate keys" });
    expect(await rowFor("security-roles.yml")).toHaveAttribute("aria-selected", "true");
  });

  it("draws the open finding's row with the selection colour (the shared table does it from aria-selected)", async () => {
    open();
    await userEvent.click(await rowFor("security-roles.yml"));
    await screen.findByRole("dialog", { name: "Duplicate keys" });
    expect((await rowFor("security-roles.yml")).firstElementChild).toHaveClass("bg-selection");
    expect((await rowFor("kafka-topics-sit11.yml")).firstElementChild).not.toHaveClass("bg-selection");
  });

  it("opens the file at the finding's first line in a new tab on Control+Enter, and the drawer stays shut", async () => {
    const opened: string[] = [];
    const click = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (this: HTMLAnchorElement) {
      opened.push(`${this.getAttribute("href")} ${this.target}`);
    });
    try {
      open();
      const row = await rowFor("security-roles.yml");
      const grid = screen.getByRole("grid", { name: "Findings" });
      grid.focus();
      const index = Number(row.getAttribute("data-index"));
      for (let i = 0; i <= index; i++) await userEvent.keyboard("{ArrowDown}");
      await userEvent.keyboard("{Control>}{Enter}{/Control}");
      expect(opened).toEqual(["/swimlanes/presitb/files?line=5171&path=security%2Fsecurity-roles.yml _blank"]);
      expect(screen.queryByRole("dialog")).toBeNull();
    } finally {
      click.mockRestore();
    }
  });

  it("does not open when the link in a row is what was clicked: the click goes to the file", async () => {
    const app = open();
    const row = await rowFor("security-roles.yml");
    await userEvent.click(within(row).getByRole("link", { name: /L5171/ }));
    await waitFor(() => expect(app.location()).toBe("/swimlanes/presitb/files?line=5171&path=security%2Fsecurity-roles.yml"));
    expect(screen.queryByRole("dialog", { name: "Duplicate keys" })).toBeNull();
  });
});

describe("the states of the States index", () => {
  it("L07: shows a skeleton of the table's own columns and names what is loading, with the title already there", async () => {
    let release: () => void = () => undefined;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    world.findingsAnswer = async () => {
      await gate;
      return json({ items: [], next: null });
    };
    open();
    expect(await screen.findByRole("heading", { level: 1, name: "Findings" })).toBeInTheDocument();
    expect(await screen.findByLabelText("Loading findings")).toHaveAttribute("aria-busy", "true");
    expect(screen.getByText("Loading findings…")).toBeInTheDocument();
    release();
    expect(await screen.findByText("No open findings")).toBeInTheDocument();
  });

  it("E07: says nothing is open, when the last scan was, and that new findings appear here", async () => {
    world = newWorld([], "admin");
    open();
    expect(await screen.findByText("No open findings")).toBeInTheDocument();
    expect(screen.getByText(/The last scan at 08:49 UTC ran 12 checks on 6 swimlanes and found nothing to fix\./)).toBeInTheDocument();
    expect(screen.getByText("Nothing to fix")).toBeInTheDocument();
  });

  it("R07: says it could not load, with the problem's title and the request id, and Retry loads it", async () => {
    let fail = true;
    world.findingsAnswer = () => (fail ? problem(500, "internal", { title: "The registry is busy" }) : json({ items: [], next: null }));
    open();
    expect(await screen.findByText("Couldn't load findings")).toBeInTheDocument();
    expect(screen.getByText("The registry is busy")).toBeInTheDocument();
    expect(screen.getByText("req-test-1")).toBeInTheDocument();
    fail = false;
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByText("No open findings")).toBeInTheDocument();
  });

  it("says plainly that the role is not enough when the server refuses the list, and offers no retry", async () => {
    world.findingsAnswer = () => problem(403, "forbidden");
    open("/findings", "viewer");
    expect(await screen.findByText("Couldn't load findings")).toBeInTheDocument();
    expect(screen.getByText(/You do not have access to the findings list/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
  });

  it("O06: says which agents are offline and what the findings of their swimlanes are as of", async () => {
    world.lanes = laneCounts(DESIGN_FINDINGS, { presitb: { agent: "disconnected", last_scan_at: NOW - 14 * 60_000 } });
    open();
    expect(await screen.findByText("Agent for presitb is offline")).toBeInTheDocument();
    expect(screen.getByText(/Findings for presitb are as of 08:39 UTC/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Check agent" })).toHaveAttribute("href", "/admin/swimlanes");
    // The rows stay: what was last known is still shown.
    expect(await rowFor("security-roles.yml")).toBeInTheDocument();
  });

  it("O06: names several offline agents in one banner, and gives a Viewer no link to the admin screen", async () => {
    world.lanes = laneCounts(DESIGN_FINDINGS, { presitb: { agent: "disconnected" }, sitc: { agent: "disconnected" } });
    open("/findings", "viewer");
    expect(await screen.findByText("Agents for sitc, presitb are offline")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Check agent" })).toBeNull();
  });

  it("O06: an offline agent with no scan on record says so instead of printing a time", async () => {
    world.lanes = laneCounts(DESIGN_FINDINGS, { presitb: { agent: "disconnected", last_scan_at: null } });
    open();
    expect(await screen.findByText("Agent for presitb is offline")).toBeInTheDocument();
    expect(screen.getByText("Findings for presitb have no scan time on record.")).toBeInTheDocument();
    expect(screen.queryByText(/1970/)).toBeNull();
  });

  it("does not warn about an offline agent whose swimlane has no findings", async () => {
    world.lanes = laneCounts(DESIGN_FINDINGS, { presitc: { agent: "disconnected" } });
    open();
    await rowFor("security-roles.yml");
    expect(screen.queryByText(/is offline|are offline/)).toBeNull();
  });

  it("T03: says the list is cut off when the screen stopped loading, and how many it shows", async () => {
    const crowd: Finding[] = Array.from({ length: 1_050 }, (_, i) => finding(i + 1, { swimlane: "sit", message: `Finding ${i + 1}` }));
    world = newWorld(crowd, "admin");
    open();
    expect(await screen.findByText("Results cut off", {}, { timeout: 15_000 })).toBeInTheDocument();
    expect(screen.getByText("Showing the first 1,000 findings. Narrow the query to see the rest.")).toBeInTheDocument();
    expect(screen.getByText(/Sorted within the 1,000 loaded findings/)).toBeInTheDocument();
    // The pages the screen asked for are not live changes: no "n updates" count holds them back, and the grid has all the rows.
    expect(screen.queryByText("The list is held still while you read it.")).toBeNull();
    expect(screen.getByRole("grid", { name: "Findings" })).toHaveAttribute("aria-rowcount", expect.stringMatching(/^1001$|^-1$/));
  }, 30_000);

  it("T03: says that the swimlanes were picked out of the loaded findings when the list is cut off and several were chosen", async () => {
    const crowd: Finding[] = Array.from({ length: 1_050 }, (_, i) => finding(i + 1, { swimlane: i % 2 === 0 ? "sit" : "sitb", message: `Finding ${i + 1}` }));
    world = newWorld(crowd, "admin");
    open("/findings?lanes=sit,sitb");
    expect(await screen.findByText("Results cut off", {}, { timeout: 15_000 })).toBeInTheDocument();
    expect(screen.getByText(/Sorted and filtered by swimlane within the 1,000 loaded findings/)).toBeInTheDocument();
  }, 30_000);

  it("still lists the findings when the swimlane list cannot be read, and builds the swimlane menu from what it has", async () => {
    world.lanesProblem = problem(500, "internal");
    open();
    await rowFor("security-roles.yml");
    expect(screen.queryByText("11 open across 5 swimlanes")).toBeNull();
    await userEvent.click(filterButton(/^Swimlane/));
    for (const id of ["sit", "sitb", "sitc", "presita", "presitb"]) expect(await screen.findByRole("menuitemcheckbox", { name: new RegExp(`^${id}$`) })).toBeInTheDocument();
  });
});

describe("accessibility", () => {
  it("has no structural violation with the list shown, nor with a menu or the drawer open", async () => {
    open();
    await rowFor("security-roles.yml");
    await expectNoAxeViolations();
    await userEvent.click(filterButton(/^Check/));
    await screen.findAllByRole("menuitemcheckbox");
    await expectNoAxeViolations();
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
    await userEvent.click(await rowFor("security-roles.yml"));
    await screen.findByRole("dialog", { name: "Duplicate keys" });
    await expectNoAxeViolations();
  });

  it("names the table, its columns and its sortable headers", async () => {
    open();
    await rowFor("security-roles.yml");
    const grid = screen.getByRole("grid", { name: "Findings" });
    expect(within(grid).getAllByRole("columnheader").map((h) => h.textContent)).toEqual(["Severity", "Check", "Swimlane", "File", "What is wrong", "Lines"]);
    expect(within(grid).getByRole("columnheader", { name: "Severity" })).toHaveAttribute("aria-sort", "descending");
  });
});


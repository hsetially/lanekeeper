import { configure, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PrLink, Role } from "@/api/generated/model";
import { prefs } from "@/lib/prefs";
import { swimlaneKeys } from "@/lib/query/keys";
import { auditEvent, fileEntry, finding, lane } from "@/mocks/design/swimlanes/fixtures";
import { newWorld, worldApi, type World } from "@/mocks/design/swimlanes/world";
import { json, problem, renderApp } from "@/test/app";

// The tabs that list things: Drift, Findings and Activity, and the swimlane's Audit page. Each is a route; each has the states of
// the States index that apply to it.

vi.mock("@/components/ui/resizable", async () => import("@/mocks/design/swimlanes/plain-resizable"));
configure({ asyncUtilTimeout: 5000 });

const NOW = Date.now();
let world: World;
beforeEach(() => {
  prefs.clear();
  world = newWorld("admin", [lane("sitb", { last_scan_at: NOW - 5_000, drift_counts: [["conflict", 1], ["nfs_ahead", 2]], finding_count: 3 }), lane("presitc", { tier: "PRESIT", baseline: { state: "unconfirmed", by: null, at: null }, drift_counts: [["unknown", 412]] })]);
});
afterEach(() => {
  Reflect.deleteProperty(document, "cookie");
});

const open = (url: string, role: Role = "admin") => {
  world.role = role;
  return renderApp(url, worldApi(world));
};

describe("the Drift tab", () => {
  beforeEach(() => {
    world.drift["sitb"] = [
      fileEntry("tx-infinity-api/tx-infinity-core-sit7.yml", { drift: "conflict", kind: "tenant", tenant: "sit7" }),
      fileEntry("tx-infinity-api/application.properties", { drift: "nfs_ahead" }),
      fileEntry("security/keystore.jks", { drift: "nfs_ahead", class: "denied", content_withheld: true }),
      fileEntry("somewhere/in-sync.yml", { drift: "in_sync" }),
    ];
  });

  it("groups the files that are not in sync by state, the most urgent first, with how many are in each", async () => {
    open("/swimlanes/sitb/drift");
    const list = await screen.findByRole("list", { name: "Files not in sync" });
    const groups = within(list).getAllByRole("button", { expanded: true });
    expect(groups.map((g) => g.textContent)).toEqual(["Conflict1", "NFS ahead2"]);
    expect(screen.getByText("tx-infinity-api/tx-infinity-core-sit7.yml")).toBeInTheDocument();
    expect(screen.getByText("3 files")).toBeInTheDocument();
    expect(screen.queryByText("somewhere/in-sync.yml")).toBeNull();
  });

  it("says in a sentence what each state means, and links each file to the file view and to its diff", async () => {
    open("/swimlanes/sitb/drift");
    const core = (await screen.findByText("tx-infinity-api/tx-infinity-core-sit7.yml")).closest<HTMLElement>("li, [role='listitem']");
    if (core === null) throw new Error("no row");
    expect(within(core).getByText("NFS and Git both changed since the baseline, and they differ.")).toBeInTheDocument();
    expect(within(core).getByText("sit7")).toBeInTheDocument();
    expect(within(core).getByRole("link", { name: "Open file" })).toHaveAttribute("href", "/swimlanes/sitb/files?path=tx-infinity-api%2Ftx-infinity-core-sit7.yml");
    expect(within(core).getByRole("link", { name: "View diff" })).toHaveAttribute("href", "/swimlanes/sitb/files?diff=git&path=tx-infinity-api%2Ftx-infinity-core-sit7.yml");
  });

  describe("who changed a file, how serious it is, and its pull request", () => {
    const OPEN: PrLink = { id: 3, swimlane: "sitb", path: "tx-infinity-api/application.properties", nfs_hash: "a".repeat(64), state: "open", pr_url: "https://github.com/csp/csp-tenant-data/pull/418", pr_number: 418, author: "x:y", created_at: 1, closed_at: null };
    beforeEach(() => {
      world.drift["sitb"] = [
        fileEntry("tx-infinity-api/application.properties", { drift: "nfs_ahead", severity: "medium", attribution: { source: "sentinel_login", confidence: "high", actor: { kind: "os_login", login: "dana.okoye" }, evidence: {} } }),
        fileEntry("tx-infinity-api/z-critical.yml", { drift: "nfs_ahead", severity: "critical", attribution: { source: "unknown", confidence: "low", actor: null, evidence: {} } }),
        fileEntry("tx-infinity-api/b-unrated.yml", { drift: "nfs_ahead", severity: null }),
        fileEntry("tx-infinity-api/tx-infinity-core-sit7.yml", { drift: "conflict", severity: "high" }),
      ];
      world.prs = [OPEN];
    });
    const rowOf = async (path: string): Promise<HTMLElement> => {
      const row = (await screen.findByText(path)).closest<HTMLElement>("[role='listitem']");
      if (row === null) throw new Error(`no row for ${path}`);
      return row;
    };

    it("puts the most severe file of a group first, and a file nobody rated last", async () => {
      open("/swimlanes/sitb/drift");
      await screen.findByText("tx-infinity-api/z-critical.yml");
      const paths = screen.getAllByText(/^tx-infinity-api\/(application\.properties|z-critical\.yml|b-unrated\.yml)$/).map((n) => n.textContent);
      expect(paths).toEqual(["tx-infinity-api/z-critical.yml", "tx-infinity-api/application.properties", "tx-infinity-api/b-unrated.yml"]);
    });

    it("shows each file's severity with the one shared badge, and nothing for a file nobody rated", async () => {
      open("/swimlanes/sitb/drift");
      const critical = await rowOf("tx-infinity-api/z-critical.yml");
      expect(critical.querySelector("[data-slot='severity-badge'][data-severity='critical']")).not.toBeNull();
      expect((await rowOf("tx-infinity-api/tx-infinity-core-sit7.yml")).querySelector("[data-severity='high']")?.className).toContain("text-status-conflict-fg");
      expect((await rowOf("tx-infinity-api/b-unrated.yml")).querySelector("[data-slot='severity-badge']")).toBeNull();
    });

    it("says who changed a file with the attribution chip, which opens its evidence", async () => {
      open("/swimlanes/sitb/drift");
      const row = await rowOf("tx-infinity-api/application.properties");
      await userEvent.click(within(row).getByRole("button", { name: /dana\.okoye/ }));
      expect(await screen.findByLabelText("Attribution evidence")).toBeInTheDocument();
      expect(within(await rowOf("tx-infinity-api/z-critical.yml")).getByText("Unknown author")).toBeInTheDocument();
    });

    it("shows the open pull request on its file from ONE read of the swimlane's links, not one for each row", async () => {
      const app = open("/swimlanes/sitb/drift");
      const row = await rowOf("tx-infinity-api/application.properties");
      expect(await within(row).findByRole("link", { name: /PR #418 open/ })).toHaveAttribute("href", "https://github.com/csp/csp-tenant-data/pull/418");
      expect((await rowOf("tx-infinity-api/z-critical.yml")).querySelector("[data-slot='pr-badge']")).toBeNull();
      expect(app.calls.filter((c) => c.path.startsWith("/api/v1/prs?"))).toHaveLength(2);
    });

    // WCAG 2.5.8 (axe `target-size`): a link under 24 px high needs 24 px of clear space around it, and the chip above the links is a
    // control of 24 px that sits 2 px over them, so the links are targets of 24 px themselves (found by the axe run at 1024 wide).
    it("makes the links of a row targets of at least 24 px, as the chip above them is", async () => {
      open("/swimlanes/sitb/drift");
      const row = await rowOf("tx-infinity-api/application.properties");
      for (const name of ["Open file", "View diff"]) expect(within(row).getByRole("link", { name }).className, `the "${name}" link`).toMatch(/(^|\s)min-h-6(\s|$)/);
    });

    it("keeps the files at a level and above, and says the filter reaches only the loaded files while more are on their way", async () => {
      const api = worldApi(world);
      renderApp("/swimlanes/sitb/drift", (call) => {
        const url = new URL(call.path, "http://x");
        // A second page that never arrives: the list is not whole.
        if (url.pathname === "/api/v1/swimlanes/sitb/drift") {
          if (url.searchParams.has("cursor")) return new Promise<Response>(() => undefined);
          return json({ items: world.drift["sitb"], next: "c2" });
        }
        return api(call);
      });
      await screen.findByText("tx-infinity-api/z-critical.yml");
      expect(screen.queryByText(/loaded so far/)).toBeNull();
      await userEvent.selectOptions(screen.getByLabelText("Severity"), "high");
      expect(screen.getByText("Applies to the 4 files loaded so far. More are not loaded.")).toBeInTheDocument();
      expect(screen.getByText("tx-infinity-api/z-critical.yml")).toBeInTheDocument();
      expect(screen.getByText("tx-infinity-api/tx-infinity-core-sit7.yml")).toBeInTheDocument();
      expect(screen.queryByText("tx-infinity-api/application.properties")).toBeNull();
      expect(screen.queryByText("tx-infinity-api/b-unrated.yml")).toBeNull();
    });

    it("says no more when every page is loaded, and says when the filter leaves nothing", async () => {
      open("/swimlanes/sitb/drift");
      await screen.findByText("tx-infinity-api/z-critical.yml");
      await userEvent.selectOptions(screen.getByLabelText("Severity"), "critical");
      expect(screen.queryByText(/loaded so far/)).toBeNull();
      expect(screen.queryByText("tx-infinity-api/application.properties")).toBeNull();
      await userEvent.selectOptions(screen.getByLabelText("Severity"), "any");
      expect(await screen.findByText("tx-infinity-api/application.properties")).toBeInTheDocument();
    });
  });

  it("offers no diff of a file whose content is withheld, and says so", async () => {
    open("/swimlanes/sitb/drift");
    const row = (await screen.findByText("security/keystore.jks")).closest<HTMLElement>("[role='listitem']");
    if (row === null) throw new Error("no row");
    expect(within(row).getByText("Content withheld")).toBeInTheDocument();
    expect(within(row).queryByRole("link", { name: "View diff" })).toBeNull();
    expect(within(row).getByRole("link", { name: "Open file" })).toBeInTheDocument();
  });

  it("closes a group and opens it again", async () => {
    open("/swimlanes/sitb/drift");
    const nfs = await screen.findByRole("button", { name: /NFS ahead/ });
    await userEvent.click(nfs);
    expect(screen.queryByText("tx-infinity-api/application.properties")).toBeNull();
    expect(screen.getByRole("button", { name: /NFS ahead/ })).toHaveAttribute("aria-expanded", "false");
    await userEvent.click(screen.getByRole("button", { name: /NFS ahead/ }));
    expect(await screen.findByText("tx-infinity-api/application.properties")).toBeInTheDocument();
  });

  it("does not take a file name for markup", async () => {
    world.drift["sitb"] = [fileEntry("<img src=x onerror=alert(1)>.yml", { drift: "conflict" })];
    open("/swimlanes/sitb/drift");
    await screen.findByText(/onerror/);
    expect(document.querySelector("img[src='x']")).toBeNull();
  });

  it("says drift is unknown, and asks for nothing, while the baseline is not confirmed", async () => {
    const app = open("/swimlanes/presitc/drift");
    expect(await screen.findByText("Drift is unknown without a baseline")).toBeInTheDocument();
    expect(screen.getByText(/can't say which files differ until an admin confirms what/)).toBeInTheDocument();
    expect(app.calls.some((c) => c.path.includes("/drift"))).toBe(false);
  });

  it("says everything is in sync when nothing differs", async () => {
    world.drift["sitb"] = [];
    open("/swimlanes/sitb/drift");
    expect(await screen.findByText("Everything is in sync")).toBeInTheDocument();
  });

  it("shows the problem and the request ID when the list cannot be read", async () => {
    const api = worldApi(world);
    renderApp("/swimlanes/sitb/drift", (call) => (new URL(call.path, "http://x").pathname === "/api/v1/swimlanes/sitb/drift" ? problem(500, "internal") : api(call)));
    expect(await screen.findByText("Couldn't load drift")).toBeInTheDocument();
    expect(screen.getByText("req-test-1")).toBeInTheDocument();
  });

  it("is refetched when the swimlane's events say the drift changed", async () => {
    const app = open("/swimlanes/sitb/drift");
    await screen.findByText("tx-infinity-api/application.properties");
    world.drift["sitb"] = [fileEntry("tx-infinity-api/new.yml", { drift: "git_ahead" })];
    // What a drift_changed event does: it invalidates the swimlane's key, and the drift list sits under it.
    await app.queryClient.invalidateQueries({ queryKey: swimlaneKeys.detail("sitb") });
    expect(await screen.findByText("tx-infinity-api/new.yml")).toBeInTheDocument();
    expect(screen.queryByText("tx-infinity-api/application.properties")).toBeNull();
  });
});

describe("the Findings tab", () => {
  beforeEach(() => {
    world.findings = [
      finding(1, { kind: "redundant_tenant_file", severity: "low", path: "tx-infinity-api/a.yml", message: "Identical to the base file.", locations: [] }),
      finding(2, { kind: "duplicate_keys", severity: "critical", locations: [{ path: "a.b", lines: { start: 212, end: 212 } }, { path: "a.c", lines: { start: 488, end: 490 } }] }),
      finding(3, { kind: "drift", severity: "high", path: null, message: "Seven files are not in sync." }),
    ];
  });

  it("lists what the checks found, the most severe first, with the code of the check, its words and its severity", async () => {
    open("/swimlanes/sitb/findings");
    await screen.findByText("Duplicate keys");
    // The words of the Findings screen (features/findings/kinds.ts), so a check is called the same thing in both places.
    const titles = screen.getAllByText(/^(Duplicate keys|NFS and Git disagree|Override identical to base)$/, { selector: "strong" }).map((s) => s.textContent);
    expect(titles).toEqual(["Duplicate keys", "NFS and Git disagree", "Override identical to base"]);
    expect(screen.getByText("C8")).toBeInTheDocument();
    expect(screen.getByText("C5")).toBeInTheDocument();
    expect(screen.getByText("C2")).toBeInTheDocument();
    for (const severity of ["Critical", "High", "Low"]) expect(screen.getByText(severity)).toBeInTheDocument();
  });

  it("draws a severity in the colours of the Findings screen, with the one shared badge", async () => {
    open("/swimlanes/sitb/findings");
    await screen.findByText("Duplicate keys");
    const high = document.querySelector("[data-slot='severity-badge'][data-severity='high']");
    expect(high?.className).toContain("text-status-conflict-fg");
    const low = document.querySelector("[data-slot='severity-badge'][data-severity='low']");
    expect(low?.className).toContain("text-info-fg");
  });

  it("shows the pull request on the file of a drift finding, from one read of the links, and on no other kind of finding", async () => {
    world.findings = [
      finding(1, { kind: "drift", severity: "high", path: "tx-infinity-api/a.yml", message: "NFS ahead.", locations: [] }),
      finding(2, { kind: "duplicate_keys", severity: "high", path: "tx-infinity-api/a.yml", message: "Twice." }),
    ];
    world.prs = [{ id: 3, swimlane: "sitb", path: "tx-infinity-api/a.yml", nfs_hash: "a".repeat(64), state: "open", pr_url: "https://github.com/csp/csp-tenant-data/pull/418", pr_number: 418, author: "x:y", created_at: 1, closed_at: null }];
    const app = open("/swimlanes/sitb/findings");
    const drift = (await screen.findByText("NFS ahead.")).closest<HTMLElement>("[role='listitem']");
    expect(await within(drift as HTMLElement).findByRole("link", { name: /PR #418 open/ })).toBeInTheDocument();
    const other = screen.getByText("Twice.").closest<HTMLElement>("[role='listitem']");
    expect(within(other as HTMLElement).queryByRole("link", { name: /PR #/ })).toBeNull();
    expect(app.calls.filter((c) => c.path.startsWith("/api/v1/prs?"))).toHaveLength(2);
  });

  // WCAG 2.5.8 (axe `target-size`): the badge is a link of 20 px and the file below it a link of 16 px; stacked 2 px apart, neither has
  // the 24 px of clear space an undersized target needs (found by the axe run). On one line they are side by side, with room between.
  it("puts the pull request badge on the line of the file it is for, not over it", async () => {
    world.findings = [finding(1, { kind: "drift", severity: "high", path: "tx-infinity-api/a.yml", message: "NFS ahead.", locations: [] })];
    world.prs = [{ id: 3, swimlane: "sitb", path: "tx-infinity-api/a.yml", nfs_hash: "a".repeat(64), state: "open", pr_url: "https://github.com/csp/csp-tenant-data/pull/418", pr_number: 418, author: "x:y", created_at: 1, closed_at: null }];
    open("/swimlanes/sitb/findings");
    const row = (await screen.findByText("NFS ahead.")).closest<HTMLElement>("[role='listitem']");
    if (row === null) throw new Error("no row");
    const badge = await within(row).findByRole("link", { name: /PR #418 open/ });
    const file = within(row).getByRole("link", { name: "tx-infinity-api/a.yml" });
    expect(badge.parentElement, "the badge and the file link are in one line").toBe(file.parentElement);
  });

  it("links each line of a finding to the file at that line", async () => {
    open("/swimlanes/sitb/findings");
    const first = await screen.findByRole("link", { name: "L212" });
    expect(first).toHaveAttribute("href", "/swimlanes/sitb/files?line=212&path=tx-infinity-api%2Ftx-infinity-core-sit7.yml");
    expect(screen.getByRole("link", { name: "L488–490" })).toHaveAttribute("href", "/swimlanes/sitb/files?line=488&path=tx-infinity-api%2Ftx-infinity-core-sit7.yml");
  });

  it("shows a finding that has no file without a link to one", async () => {
    open("/swimlanes/sitb/findings");
    const row = (await screen.findByText("Seven files are not in sync.")).closest<HTMLElement>("[role='listitem']");
    if (row === null) throw new Error("no row");
    expect(within(row).queryByRole("link")).toBeNull();
  });

  it("asks the server for this swimlane's findings only", async () => {
    const app = open("/swimlanes/sitb/findings");
    await screen.findByText("Duplicate keys");
    const call = app.calls.find((c) => c.path.startsWith("/api/v1/findings"));
    expect(new URL(call?.path ?? "", "http://x").searchParams.get("swimlane")).toBe("sitb");
  });

  it("can be narrowed from the address, says so, and Show all removes the narrowing", async () => {
    const app = open("/swimlanes/sitb/findings?code=C8&severity=high");
    expect(await screen.findByText("Showing C8, duplicate keys, high severity and above.")).toBeInTheDocument();
    const call = app.calls.find((c) => c.path.startsWith("/api/v1/findings"));
    const params = new URL(call?.path ?? "", "http://x").searchParams;
    expect(params.get("kinds")).toBe("duplicate_keys");
    expect(params.get("min_severity")).toBe("high");
    await userEvent.click(screen.getByRole("link", { name: "Show all" }));
    await waitFor(() => expect(app.location()).toBe("/swimlanes/sitb/findings"));
  });

  it("ignores a filter it does not know", async () => {
    open("/swimlanes/sitb/findings?code=C99&severity=urgent");
    await screen.findByText("Duplicate keys");
    expect(screen.queryByText(/^Showing /)).toBeNull();
  });

  it("says every check passes when there is nothing", async () => {
    world.findings = [];
    open("/swimlanes/sitb/findings");
    expect(await screen.findByText("No open findings")).toBeInTheDocument();
    expect(screen.getByText(/Every check passes on/)).toBeInTheDocument();
  });

  describe("in a panel of any width (the layout run: at 1024 the panel is 360px, the Finding column was 12px wide and its title and lines ran into each other)", () => {
    const widthOf = { value: 0 };
    beforeEach(() => {
      // The test setup gives every element a box (src/test/setup.ts); the tab's is given the width the test asks for.
      const real = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "getBoundingClientRect")?.value as (this: HTMLElement) => DOMRect;
      vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
        const rect = real.call(this);
        return this.dataset["slot"] === "findings-tab" && widthOf.value > 0 ? new DOMRect(rect.x, rect.y, widthOf.value, rect.height) : rect;
      });
    });
    afterEach(() => {
      vi.restoreAllMocks();
      widthOf.value = 0;
    });
    const rowOfTitle = async (): Promise<HTMLElement> => {
      const row = (await screen.findByText("Duplicate keys")).closest<HTMLElement>("[role='listitem']");
      if (row === null) throw new Error("no row");
      return row;
    };
    const gridOf = (row: HTMLElement): HTMLElement => {
      const grid = row.firstElementChild;
      if (!(grid instanceof HTMLElement)) throw new Error("no grid");
      return grid;
    };

    it("keeps the four columns and the design's 96px rows where there is room for them", async () => {
      widthOf.value = 800;
      open("/swimlanes/sitb/findings");
      const row = await rowOfTitle();
      expect(gridOf(row).className).toContain("grid-cols-[44px_96px_minmax(0,1fr)_140px]");
      expect(row.style.height).toBe("96px");
    });

    it("moves the lines of a finding under its title, and makes the rows taller, in a panel narrower than 560px", async () => {
      widthOf.value = 360;
      open("/swimlanes/sitb/findings");
      const row = await rowOfTitle();
      const grid = gridOf(row);
      expect(grid.className).toContain("grid-cols-[44px_96px_minmax(0,1fr)]");
      expect(grid.className).not.toContain("_140px]");
      expect(row.style.height).toBe("148px");
      // The header has the same columns as the rows, minus the one that moved under the finding.
      const header = screen.getByText("Check").parentElement;
      expect(header?.className).toContain("grid-cols-[44px_96px_minmax(0,1fr)]");
      expect(within(header as HTMLElement).queryByText("Lines")).toBeNull();
      expect(within(row).getByRole("link", { name: "L212" })).toBeInTheDocument();
    });
  });

  it("does not take a message for markup", async () => {
    world.findings = [finding(9, { message: "<img src=x onerror=alert(1)>‮" })];
    open("/swimlanes/sitb/findings");
    await screen.findByText(/onerror/);
    expect(document.querySelector("img[src='x']")).toBeNull();
  });
});

describe("the Activity tab", () => {
  beforeEach(() => {
    world.audit = [auditEvent(1, { action: "file.edit" }), auditEvent(3, { action: "service.restart", target: "sitb/tx-infinity-core", via: "mcp", actor: null }), auditEvent(2, { action: "proposal.create", swimlane: "other" })];
  });

  it("lists the audited events of this swimlane, newest first, each with who, how and what", async () => {
    open("/swimlanes/sitb/activity");
    expect(await screen.findByText("service.restart")).toBeInTheDocument();
    const rows = within(screen.getByRole("list", { name: "Activity" }))
      .getAllByRole("listitem")
      .map((r) => r.textContent ?? "");
    expect(rows).toHaveLength(2);
    expect(rows[0]).toContain("service.restart");
    expect(rows[0]).toContain("MCP");
    expect(rows[0]).toContain("Lanekeeper");
    expect(rows[1]).toContain("file.edit");
    expect(rows[1]).toContain("Web");
    expect(rows[1]).toContain("user aaaaaaaa");
    expect(screen.queryByText("proposal.create")).toBeNull();
  });

  it("says who made a change with the attribution chip where the event carries one, and from the event's actor where it does not", async () => {
    world.audit = [
      auditEvent(1, { action: "file.edit", attribution: { source: "sentinel_login", confidence: "high", actor: { kind: "os_login", login: "dana.okoye" }, evidence: {} } }),
      auditEvent(2, { action: "file.scan", actor: null, attribution: { source: "unknown", confidence: "low", actor: null, evidence: {} } }),
    ];
    open("/swimlanes/sitb/activity");
    const rows = within(await screen.findByRole("list", { name: "Activity" })).getAllByRole("listitem");
    expect(within(rows[0] as HTMLElement).getByText("Unknown author")).toBeInTheDocument();
    expect(within(rows[1] as HTMLElement).getByRole("button", { name: /dana\.okoye/ })).toBeInTheDocument();
  });

  it("asks the Admin-only audit read for this swimlane", async () => {
    const app = open("/swimlanes/sitb/activity");
    await screen.findByText("service.restart");
    const call = app.calls.find((c) => c.path.startsWith("/api/v1/admin/audit"));
    expect(new URL(call?.path ?? "", "http://x").searchParams.get("swimlane")).toBe("sitb");
  });

  it("is a screen of its own for someone who is not an Admin, not a half-built tab", async () => {
    open("/swimlanes/sitb/activity", "operator");
    expect(await screen.findByText("Activity needs the Admin role")).toBeInTheDocument();
  });

  it("says nothing was recorded when nothing was", async () => {
    world.audit = [];
    open("/swimlanes/sitb/activity");
    expect(await screen.findByText("No activity yet")).toBeInTheDocument();
  });

  it("shows the problem and the request ID when the trail cannot be read", async () => {
    const api = worldApi(world);
    renderApp("/swimlanes/sitb/activity", (call) => (new URL(call.path, "http://x").pathname === "/api/v1/admin/audit" ? problem(500, "internal") : api(call)));
    expect(await screen.findByText("Couldn't load activity")).toBeInTheDocument();
    expect(screen.getByText("req-test-1")).toBeInTheDocument();
  });

  it("says when the swimlane has changed since the rows were read, and Refresh reads them again; nothing polls", async () => {
    const app = open("/swimlanes/sitb/activity");
    await screen.findByText("service.restart");
    expect(screen.queryByText("This swimlane has changed since these rows were read")).toBeNull();
    const before = app.calls.filter((c) => c.path.startsWith("/api/v1/admin/audit")).length;
    world.audit = [auditEvent(4, { action: "file.upload" }), ...world.audit];
    await new Promise((resolve) => setTimeout(resolve, 1100));
    // A live event refreshes the swimlane's own query; the audit rows stay as they were until the person asks.
    await app.queryClient.invalidateQueries({ queryKey: swimlaneKeys.detail("sitb") });
    expect(await screen.findByText("This swimlane has changed since these rows were read")).toBeInTheDocument();
    expect(app.calls.filter((c) => c.path.startsWith("/api/v1/admin/audit")).length).toBe(before);
    await userEvent.click(screen.getByRole("button", { name: "Refresh" }));
    expect(await screen.findByText("file.upload")).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByText("This swimlane has changed since these rows were read")).toBeNull());
  });
});

describe("the swimlane's Audit page", () => {
  it("has one title, the events with their sequence number and request ID, and a way back", async () => {
    world.audit = [auditEvent(5, { request_id: "req-abc-1" }), auditEvent(4)];
    open("/swimlanes/sitb/audit");
    expect(await screen.findByRole("heading", { level: 1, name: "Audit" })).toBeInTheDocument();
    expect(await screen.findByText("#5")).toBeInTheDocument();
    expect(screen.getByText("req-abc-1")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Back to the swimlane" })).toHaveAttribute("href", "/swimlanes/sitb");
  });

  it("is for an Admin only", async () => {
    open("/swimlanes/sitb/audit", "editor");
    expect(await screen.findByText("Audit needs the Admin role")).toBeInTheDocument();
  });
});

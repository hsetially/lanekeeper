import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { Version } from "@/api/generated/model";
import { HASH_A, HASH_B, HASH_C, problem, renderFile, routes } from "../test-utils";
import { HistoryPanel } from "./HistoryPanel";

const ME = "00000000-0000-4000-8000-000000000001:00000000-0000-4000-8000-000000000002";
const version = (hash: string, over: Partial<Version> = {}): Version => ({
  hash,
  observed_at: Date.UTC(2026, 9, 17, 14, 5),
  source: "tool_write",
  size: 120,
  attribution: { source: "tool_write", confidence: "certain", actor: { kind: "user", user: ME }, evidence: {} },
  severity: "low",
  commit: null,
  ...over,
});

const HISTORY = "GET /api/v1/swimlanes/sitb/files/history";
const allowed = { allowed: true, reason: null };
const props = { id: "sitb", path: "tx-infinity-api/a.yml", currentHash: HASH_A, revertGate: allowed, onRevert: vi.fn(), onClose: vi.fn() };

const list = (items: Version[], next: string | null = null) => ({ [HISTORY]: () => ({ items, next }) });

describe("HistoryPanel", () => {
  it("lists the versions newest first, with when, hash, how it was seen and who", async () => {
    renderFile(<HistoryPanel {...props} />, {
      api: routes(
        list([
          version(HASH_A, { source: "scan", observed_at: Date.UTC(2026, 9, 17, 14, 5), attribution: { source: "unknown", confidence: "low", actor: null, evidence: {} }, severity: "high" }),
          version(HASH_B, { observed_at: Date.UTC(2026, 9, 17, 13, 58) }),
        ]),
      ),
    });
    const panel = await screen.findByRole("complementary", { name: "History" });
    const rows = await within(panel).findAllByRole("listitem");
    expect(rows).toHaveLength(2);
    expect(within(rows[0] as HTMLElement).getByText("2026-10-17 14:05 UTC")).toBeInTheDocument();
    expect(within(rows[0] as HTMLElement).getByText("Scan found a change")).toBeInTheDocument();
    expect(within(rows[0] as HTMLElement).getByText("Current")).toBeInTheDocument();
    expect(within(rows[0] as HTMLElement).getByText("High")).toBeInTheDocument();
    // The same chip the rest of the app draws: nothing identified who, so no author and no score (D73), and never "Low".
    expect(within(rows[0] as HTMLElement).getByText("Unknown author")).toBeInTheDocument();
    expect(within(rows[0] as HTMLElement).queryByText("Low")).not.toBeInTheDocument();
    expect(within(rows[1] as HTMLElement).getByText("Tool write")).toBeInTheDocument();
    expect(within(rows[1] as HTMLElement).getByText("You")).toBeInTheDocument();
    expect(within(rows[1] as HTMLElement).getByText("via Lanekeeper")).toBeInTheDocument();
    expect(within(rows[1] as HTMLElement).getByText("Certain")).toBeInTheDocument();
  });

  it("offers a copy button for each version's hash, and none when the content of the file is withheld (the hash is still shown)", async () => {
    const { unmount } = renderFile(<HistoryPanel {...props} />, { api: routes(list([version(HASH_A), version(HASH_B)])) });
    const panel = await screen.findByRole("complementary", { name: "History" });
    await within(panel).findAllByRole("listitem");
    expect(within(panel).getAllByRole("button", { name: "Copy hash" })).toHaveLength(2);
    unmount();
    renderFile(<HistoryPanel {...props} copyHash={false} />, { api: routes(list([version(HASH_A), version(HASH_B)])) });
    const withheld = await screen.findByRole("complementary", { name: "History" });
    const rows = await within(withheld).findAllByRole("listitem");
    expect(rows).toHaveLength(2);
    expect(within(withheld).queryByRole("button", { name: "Copy hash" })).toBeNull();
    expect(within(rows[0] as HTMLElement).getByText(HASH_A.slice(0, 7))).toBeInTheDocument();
  });

  it("offers a revert on every version but the current one", async () => {
    renderFile(<HistoryPanel {...props} />, { api: routes(list([version(HASH_A), version(HASH_B), version(HASH_C)])) });
    const rows = await screen.findAllByRole("listitem");
    expect(within(rows[0] as HTMLElement).queryByRole("button", { name: /Revert/ })).not.toBeInTheDocument();
    expect(within(rows[1] as HTMLElement).getByRole("button", { name: /Revert/ })).toBeInTheDocument();
    expect(within(rows[2] as HTMLElement).getByRole("button", { name: /Revert/ })).toBeInTheDocument();
  });

  it("asks for the revert of the version chosen, and does nothing itself", async () => {
    const onRevert = vi.fn();
    const calls = renderFile(<HistoryPanel {...props} onRevert={onRevert} />, { api: routes(list([version(HASH_A), version(HASH_B)])) }).calls;
    const rows = await screen.findAllByRole("listitem");
    await userEvent.click(within(rows[1] as HTMLElement).getByRole("button", { name: /Revert/ }));
    expect(onRevert).toHaveBeenCalledWith(expect.objectContaining({ hash: HASH_B }));
    expect(calls.filter((c) => c.method !== "GET")).toHaveLength(0);
  });

  it("disables the revert with its reason when the person may not write", async () => {
    renderFile(<HistoryPanel {...props} revertGate={{ allowed: false, reason: "Needs the Editor role. Ask an Admin to grant it." }} />, { role: "viewer", api: routes(list([version(HASH_A), version(HASH_B)])) });
    const rows = await screen.findAllByRole("listitem");
    expect(within(rows[1] as HTMLElement).getByRole("button", { name: /Revert/ })).toHaveAttribute("aria-disabled", "true");
  });

  it("names who the contract does not name: another person is not given a made-up name", async () => {
    renderFile(<HistoryPanel {...props} />, { api: routes(list([version(HASH_A, { attribution: { source: "tool_write", confidence: "certain", actor: { kind: "user", user: "11111111-1111-4111-8111-111111111111:22222222-2222-4222-8222-222222222222" }, evidence: {} } })])) });
    expect(await screen.findByText("Lanekeeper user 22222222")).toBeInTheDocument();
  });

  it("gives an Admin the name from the user list, and an Editor the short id with no request for the list", async () => {
    const OTHER = "11111111-1111-4111-8111-111111111111:22222222-2222-4222-8222-222222222222";
    const other = version(HASH_A, { attribution: { source: "tool_write", confidence: "certain", actor: { kind: "user", user: OTHER }, evidence: {} } });
    const page = { items: [other], next: null };
    const users = { items: [{ id: OTHER, display_name: "Marco Diaz", email: "marco@example.test", github_login: "mdiaz", role: "editor", status: "active", created_at: 1, last_seen_at: null }], next: null };
    const admin = renderFile(<HistoryPanel {...props} />, { role: "admin", api: routes({ [HISTORY]: () => page, "GET /api/v1/admin/users": () => users }) });
    expect(await screen.findByText("Marco Diaz")).toBeInTheDocument();
    admin.unmount();
    const editor = renderFile(<HistoryPanel {...props} />, { role: "editor", api: routes({ [HISTORY]: () => page, "GET /api/v1/admin/users": () => users }) });
    expect(await screen.findByText("Lanekeeper user 22222222")).toBeInTheDocument();
    expect(editor.calls.some((c) => c.path.startsWith("/api/v1/admin/users"))).toBe(false);
  });

  it("opens the evidence behind the chip on a press, and hands the severity to the one shared badge", async () => {
    renderFile(<HistoryPanel {...props} />, { api: routes(list([version(HASH_A, { severity: "critical", attribution: { source: "sentinel_login", confidence: "high", actor: { kind: "os_login", login: "dana.okoye" }, evidence: { executable: "/usr/bin/vi" } } })])) });
    const row = (await screen.findAllByRole("listitem"))[0] as HTMLElement;
    expect(row.querySelector("[data-slot='severity-badge'][data-severity='critical']")).not.toBeNull();
    await userEvent.click(within(row).getByRole("button", { name: /dana\.okoye/ }));
    expect(await screen.findByText("/usr/bin/vi")).toBeInTheDocument();
  });

  it("shows a skeleton while it loads, and an error with the request id when it fails", async () => {
    renderFile(<HistoryPanel {...props} />, { api: () => problem(403, "forbidden") });
    expect(await screen.findByText("Couldn't load history")).toBeInTheDocument();
    expect(screen.getByText("req-test-1")).toBeInTheDocument();
  });

  it("says so when the file has no versions yet", async () => {
    renderFile(<HistoryPanel {...props} />, { api: routes(list([])) });
    expect(await screen.findByText("No versions recorded yet")).toBeInTheDocument();
  });

  it("reads on when asked, and says when it has read all there is", async () => {
    let page = 0;
    renderFile(<HistoryPanel {...props} />, {
      api: routes({
        [HISTORY]: () => {
          page += 1;
          return page === 1 ? { items: [version(HASH_A)], next: "c2" } : { items: [version(HASH_B)], next: null };
        },
      }),
    });
    await userEvent.click(await screen.findByRole("button", { name: "Show older versions" }));
    expect(await screen.findAllByRole("listitem")).toHaveLength(2);
    expect(screen.queryByRole("button", { name: "Show older versions" })).not.toBeInTheDocument();
  });

  it("closes", async () => {
    const onClose = vi.fn();
    renderFile(<HistoryPanel {...props} onClose={onClose} />, { api: routes(list([version(HASH_A)])) });
    await userEvent.click(await screen.findByRole("button", { name: "Close" }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});

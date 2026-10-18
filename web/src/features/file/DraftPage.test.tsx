import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { Draft } from "@/api/generated/model";
import DraftPage from "./DraftPage";
import { HASH_A, HASH_B, problem, renderFile, routes } from "./test-utils";

// The draft page (CG17): a draft a tool saved, read before it is applied. Applying is the one write; its answer is shown and
// never assumed. A draft is the author's alone, so someone else's is the same 404 as one that does not exist.

const PATH = "tx-infinity-api/tx-infinity-core-sit7.yml";
const draft = (over: Partial<Draft> = {}): Draft => ({
  id: "7",
  author: "00000000-0000-4000-8000-000000000001",
  swimlane: "sitb",
  path: PATH,
  expected: { state: "hash", hash: HASH_A },
  new_hash: HASH_B,
  hunks: [
    {
      old_start: 4,
      old_lines: 2,
      new_start: 4,
      new_lines: 3,
      lines: [
        { kind: "context", text: "txInfinityOptions:", inline_changes: [] },
        { kind: "removed", text: "  enableAccountSorting: false", inline_changes: [] },
        { kind: "added", text: "  enableAccountSorting: true", inline_changes: [] },
        { kind: "added", text: "  sortOrder: asc", inline_changes: [] },
      ],
    },
  ],
  setting_changes: [
    { path: "txInfinityOptions.enableAccountSorting", kind: "changed", left: { text: "false", ty: "bool", flag: null }, right: { text: "true", ty: "bool", flag: null }, left_location: null, right_location: null, severity: null },
    { path: "txInfinityOptions.sortOrder", kind: "added", left: null, right: { text: "asc", ty: "string", flag: null }, left_location: null, right_location: null, severity: null },
  ],
  status: "draft",
  expires_at: Date.UTC(2026, 9, 18, 10, 0),
  severity: "medium",
  effects: [[{ namespace: "csp", name: "tx-infinity-core" }, "needs_notify_or_restart"]],
  ...over,
});

const GET = "GET /api/v1/drafts/7";
const APPLY = "POST /api/v1/drafts/7/apply";
const applied = { outcome: "applied", hash: HASH_B, audit_id: 4 };
const detail = { id: "sitb", display_name: "sitb", tenants: ["sit7"], agent: "connected", last_scan_at: null, base_version: null, drift_counts: [["unknown", 412]], finding_count: 0, baseline: { state: "unconfirmed", by: null, at: null }, services: [], auto_notify: false };

const page = () => <DraftPage params={{ id: "7" }} search={{}} />;
const server = (over: Record<string, () => unknown> = {}) => routes({ [GET]: () => draft(), "GET /api/v1/swimlanes/sitb": () => detail, ...over });

async function openApply() {
  await userEvent.click(await screen.findByRole("button", { name: "Apply draft" }));
  return screen.findByRole("alertdialog", { name: "Apply this draft?" });
}

describe("DraftPage: reading", () => {
  it("has one h1 and reads the draft by the number in the address", async () => {
    const { calls } = renderFile(page(), { api: server() });
    expect(await screen.findByRole("heading", { level: 1, name: "Review draft" })).toBeInTheDocument();
    await screen.findByRole("region", { name: "Draft" });
    expect(calls.find((c) => c.path === "/api/v1/drafts/7")?.method).toBe("GET");
  });

  it("shows which swimlane and file, what it is based on, the new version, how serious and until when", async () => {
    renderFile(page(), { api: server() });
    const facts = await screen.findByRole("region", { name: "Draft" });
    expect(within(facts).getByRole("link", { name: "sitb" })).toHaveAttribute("href", "/swimlanes/sitb");
    expect(within(facts).getByText(PATH)).toBeInTheDocument();
    expect(within(facts).getByText("Draft")).toBeInTheDocument();
    expect(within(facts).getByText("Medium")).toBeInTheDocument();
    expect(within(facts).getByText("2026-10-18 10:00 UTC")).toBeInTheDocument();
  });

  it("says a draft for a new file creates it, and that a severity is not rated when the server has none", async () => {
    renderFile(page(), { api: server({ [GET]: () => draft({ expected: { state: "absent" }, severity: null }) }) });
    expect(await screen.findByText("No file: applying this creates it")).toBeInTheDocument();
    expect(screen.getByText("Not rated")).toBeInTheDocument();
  });

  it("lists the services and when each would pick the change up", async () => {
    renderFile(page(), { api: server() });
    const list = await screen.findByRole("list", { name: "Services" });
    expect(within(list).getByText("csp/tx-infinity-core")).toBeInTheDocument();
    expect(within(list).getByText("Needs notify or restart")).toBeInTheDocument();
  });

  it("shows the settings that change and the diff the engine computed", async () => {
    renderFile(page(), { api: server() });
    expect(await screen.findByText("1 setting changed, 1 added")).toBeInTheDocument();
    const grid = await screen.findByRole("grid", { name: "Changes to tx-infinity-core-sit7.yml" });
    expect(within(grid).getByText("@@ -4,2 +4,3 @@")).toBeInTheDocument();
    expect(within(grid).getByText("sortOrder: asc", { exact: false })).toBeInTheDocument();
  });

  it("flips the diff between side by side and unified", async () => {
    renderFile(page(), { api: server() });
    await screen.findByRole("grid");
    expect(screen.getByRole("radio", { name: "Side by side" })).toBeChecked();
    await userEvent.click(screen.getByRole("radio", { name: "Unified" }));
    expect(screen.getByRole("radio", { name: "Unified" })).toBeChecked();
  });

  it("links to the file", async () => {
    renderFile(page(), { api: server() });
    expect(await screen.findByRole("link", { name: "Open the file" })).toHaveAttribute("href", `/swimlanes/sitb/files?path=${encodeURIComponent(PATH)}`);
  });

  it("draws a path from the server as text", async () => {
    const { container } = renderFile(page(), { api: server({ [GET]: () => draft({ path: "a/<img src=x onerror=alert(1)>.yml" }) }) });
    expect(await screen.findByText(/<img src=x onerror=alert\(1\)>/)).toBeInTheDocument();
    expect(container.querySelector("img")).toBeNull();
  });
});

describe("DraftPage: states", () => {
  it("shows a busy region while the draft loads", async () => {
    renderFile(page(), { api: () => new Promise<Response>(() => undefined) });
    expect(await screen.findByLabelText("Loading draft")).toHaveAttribute("aria-busy", "true");
  });

  it("says the draft was not found, in the same words for someone else's draft", async () => {
    renderFile(page(), { api: routes({ [GET]: () => problem(404, "not_found") }) });
    expect(await screen.findByText("Draft not found")).toBeInTheDocument();
    expect(screen.getByText(/can be opened only by the person who made it/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Apply draft" })).not.toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Go to the swimlanes" })).toHaveAttribute("href", "/swimlanes");
  });

  it("names the problem and the request id when the draft cannot be read", async () => {
    renderFile(page(), { api: routes({ [GET]: () => problem(403, "forbidden") }) });
    expect(await screen.findByText("Couldn't load draft")).toBeInTheDocument();
    expect(screen.getByText("req-test-1")).toBeInTheDocument();
  });

  it.each([
    ["applied", "This draft was applied", "This draft was already applied."],
    ["expired", "This draft expired", "This draft can no longer be applied."],
    ["stale", "The file changed after this draft was made", "This draft can no longer be applied."],
    ["rejected", "Rejected", "This draft can no longer be applied."],
    ["pending", "Sent for approval", "This draft is waiting for approval."],
    ["approved", "Approved", "This draft can no longer be applied."],
  ] as const)("a %s draft says so and cannot be applied", async (status, title, reason) => {
    renderFile(page(), { api: server({ [GET]: () => draft({ status }) }) });
    expect(await screen.findByText(title, { selector: "[data-slot=alert-title]" })).toBeInTheDocument();
    const apply = screen.getByRole("button", { name: "Apply draft" });
    expect(apply).toHaveAttribute("aria-disabled", "true");
    expect(apply).toHaveAccessibleDescription(reason);
  });

  it("does not let a Viewer apply, and says which role does", async () => {
    renderFile(page(), { role: "viewer", api: server() });
    const apply = await screen.findByRole("button", { name: "Apply draft" });
    expect(apply).toHaveAttribute("aria-disabled", "true");
    expect(apply).toHaveAccessibleDescription("Needs the Editor role. Ask an Admin to grant it.");
  });

  it("does not let anyone apply while the live stream is down", async () => {
    renderFile(page(), { online: false, api: server() });
    expect(await screen.findByRole("button", { name: "Apply draft" })).toHaveAccessibleDescription("Unavailable offline");
  });
});

describe("DraftPage: applying", () => {
  it("asks first, with the consequence, and puts focus on Cancel", async () => {
    const { calls } = renderFile(page(), { api: server() });
    const dialog = await openApply();
    expect(within(dialog).getByText(/writes the draft's version of the file to sitb NFS now/)).toBeInTheDocument();
    expect(within(dialog).getByText(/only if the file is still as it was when the draft was made/)).toBeInTheDocument();
    await waitFor(() => expect(within(dialog).getByRole("button", { name: "Cancel" })).toHaveFocus());
    expect(calls.some((c) => c.method === "POST")).toBe(false);
  });

  it("writes nothing when the person cancels", async () => {
    const { calls } = renderFile(page(), { api: server() });
    const dialog = await openApply();
    await userEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    expect(calls.some((c) => c.method === "POST")).toBe(false);
  });

  it("applies with the CSRF token and an idempotency key, and says it is applied only after the server did", async () => {
    let status: Draft["status"] = "draft";
    const { calls } = renderFile(page(), {
      api: server({
        [GET]: () => draft({ status }),
        [APPLY]: () => {
          status = "applied";
          return applied;
        },
      }),
    });
    const dialog = await openApply();
    await userEvent.click(within(dialog).getByRole("button", { name: "Apply draft" }));
    expect(await screen.findByText("Draft applied")).toBeInTheDocument();
    const post = calls.find((c) => c.method === "POST");
    expect(post?.path).toBe("/api/v1/drafts/7/apply");
    expect(post?.headers.get("Idempotency-Key")).toMatch(/\S{8,}/);
    expect(post?.headers.get("X-CSRF-Token")).toBeTruthy();
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    // The draft is read again, and now says applied: Apply is off.
    await waitFor(() => expect(screen.getByRole("button", { name: "Apply draft" })).toHaveAttribute("aria-disabled", "true"));
  });

  it("says a draft sent for approval is not applied yet, and offers the request to an Admin", async () => {
    renderFile(page(), { role: "admin", api: server({ [APPLY]: () => ({ outcome: "proposal_created", id: 9 }) }) });
    const dialog = await openApply();
    await userEvent.click(within(dialog).getByRole("button", { name: "Apply draft" }));
    const sent = await screen.findByRole("dialog", { name: "Sent for approval" });
    expect(within(sent).getByText(/Nothing has changed on NFS yet/)).toBeInTheDocument();
    expect(within(sent).getByText("AP-9")).toBeInTheDocument();
    expect(within(sent).getByRole("link", { name: "View request" })).toHaveAttribute("href", "/approvals/9");
    expect(screen.queryByText("Draft applied")).not.toBeInTheDocument();
  });

  it("does not offer the request to someone who cannot open approvals", async () => {
    renderFile(page(), { role: "editor", api: server({ [APPLY]: () => ({ outcome: "proposal_created", id: 9 }) }) });
    const dialog = await openApply();
    await userEvent.click(within(dialog).getByRole("button", { name: "Apply draft" }));
    const sent = await screen.findByRole("dialog", { name: "Sent for approval" });
    expect(within(sent).queryByRole("link", { name: "View request" })).not.toBeInTheDocument();
  });

  it("says nothing was applied when the file changed since, and offers the file", async () => {
    renderFile(page(), { api: server({ [APPLY]: () => problem(409, "conflict", { current: HASH_B }) }) });
    const dialog = await openApply();
    await userEvent.click(within(dialog).getByRole("button", { name: "Apply draft" }));
    const conflict = await screen.findByRole("dialog", { name: "The file changed since this draft was made" });
    expect(within(conflict).getByText(/Nothing was applied/)).toBeInTheDocument();
    expect(within(conflict).getByText("req-test-1")).toBeInTheDocument();
    expect(within(conflict).getByRole("link", { name: "Open the file" })).toBeInTheDocument();
    expect(screen.queryByText("Draft applied")).not.toBeInTheDocument();
  });

  it("says the swimlane is locked, with why, when the apply is refused with 423", async () => {
    renderFile(page(), { role: "admin", api: server({ [APPLY]: () => problem(423, "locked") }) });
    const dialog = await openApply();
    await userEvent.click(within(dialog).getByRole("button", { name: "Apply draft" }));
    const locked = await screen.findByRole("dialog", { name: "sitb is locked until it is adopted" });
    expect(await within(locked).findByText("412")).toBeInTheDocument();
    expect(within(locked).getByRole("link", { name: "Start adoption" })).toBeInTheDocument();
    expect(screen.queryByText("Draft applied")).not.toBeInTheDocument();
  });

  it("keeps the confirmation open with the reason and the request id when the content is refused (422)", async () => {
    renderFile(page(), { api: server({ [APPLY]: () => problem(422, "unprocessable", { reason: "invalid_content", issues: [{ kind: "secret_match", line: 5, rule: "aws_key" }] }) }) });
    const dialog = await openApply();
    await userEvent.click(within(dialog).getByRole("button", { name: "Apply draft" }));
    const alert = await within(dialog).findByRole("alert");
    expect(alert).toHaveTextContent("Not saved: the content was refused");
    expect(alert).toHaveTextContent("Line 5 looks like a secret");
    expect(alert).toHaveTextContent("req-test-1");
    expect(screen.getByRole("alertdialog", { name: "Apply this draft?" })).toBeInTheDocument();
  });

  it("keeps the confirmation open when the draft is gone (404) and does not claim it was applied", async () => {
    renderFile(page(), { api: server({ [APPLY]: () => problem(404, "not_found") }) });
    const dialog = await openApply();
    await userEvent.click(within(dialog).getByRole("button", { name: "Apply draft" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Nothing was written");
    expect(screen.queryByText("Draft applied")).not.toBeInTheDocument();
  });

  it("is not optimistic: nothing says applied while the request is still out", async () => {
    renderFile(page(), { api: (call) => (call.method === "POST" ? new Promise<Response>(() => undefined) : server()(call)) });
    const dialog = await openApply();
    await userEvent.click(within(dialog).getByRole("button", { name: "Apply draft" }));
    expect(await within(dialog).findByRole("button", { name: "Applying…" })).toBeInTheDocument();
    expect(screen.queryByText("Draft applied")).not.toBeInTheDocument();
  });

  it("reads the draft again after a conflict, since it may be out of date now", async () => {
    let reads = 0;
    renderFile(page(), {
      api: server({
        [GET]: () => {
          reads += 1;
          return draft({ status: reads > 1 ? "stale" : "draft" });
        },
        [APPLY]: () => problem(409, "conflict"),
      }),
    });
    const dialog = await openApply();
    await userEvent.click(within(dialog).getByRole("button", { name: "Apply draft" }));
    await screen.findByRole("dialog", { name: "The file changed since this draft was made" });
    await userEvent.keyboard("{Escape}");
    expect(await screen.findByText("The file changed after this draft was made", { selector: "[data-slot=alert-title]" })).toBeInTheDocument();
  });
});

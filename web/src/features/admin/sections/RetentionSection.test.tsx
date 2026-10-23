import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { IDS, NOW, previewFor, retention } from "@/mocks/design/admin-t5/fixtures";
import { FIRST, holdUntil, installAdminClock, openSection } from "@/mocks/design/admin-t5/harness";
import { newT5World, t5Api } from "@/mocks/design/admin-t5/world";
import { json, problem, renderApp } from "@/test/app";
import { expectNoAxeViolations } from "@/test/axe";

// Retention (route /admin/retention, Admin only). The retention job DELETES stored versions that the policy does not keep (D118), and a
// deleted version cannot be restored, so this page makes that plain, shows the dry run of a policy before it can be saved, and asks
// for words to be typed before a policy that would delete is saved. The API has the two numbers, who changed them and when, and the
// dry run (`PUT ?dry_run=true`, which changes nothing). It does not report the job's own dry-run switch, the size of a run or the
// history of runs (design/DEVIATIONS.md), so the page does not pretend to know them.

installAdminClock();

const days = () => screen.findByLabelText("Days to keep versions");
const last = () => screen.findByLabelText("Versions to always keep per file");
const retentionCalls = <T extends { method: string; path: string }>(calls: readonly T[]): T[] => calls.filter((c) => c.method !== "GET" && c.path.startsWith("/api/v1/admin/retention"));

async function setPolicy(user: ReturnType<typeof userEvent.setup>, d: string, l = "50") {
  const dEl = await days();
  await user.clear(dEl);
  await user.type(dEl, d);
  const lEl = await last();
  await user.clear(lEl);
  await user.type(lEl, l);
}

describe("the policy", () => {
  it("shows what is kept now, who changed it and when, and the dry run of the saved policy", async () => {
    await openSection(newT5World(), "/admin/retention");
    expect(await screen.findByRole("heading", { level: 2, name: "Retention" })).toBeInTheDocument();
    const policy = await screen.findByRole("region", { name: "Current policy" });
    await waitFor(() => expect(policy).toHaveTextContent("Keeps every version for 180 days and the last 50 versions of each file. Last changed 2025-08-30 08:53 UTC by Priya Shah."));
    expect(await days()).toHaveValue("180");
    expect(await last()).toHaveValue("50");
    const panel = screen.getByRole("region", { name: "Dry run" });
    expect(within(panel).getByText("Dry run · nothing is deleted")).toBeInTheDocument();
    expect(within(panel).getByText("The saved policy: 180 days, last 50 versions")).toBeInTheDocument();
    expect(within(panel).getByText("41,208")).toBeInTheDocument();
    expect(within(panel).getByText("3.2 GiB")).toBeInTheDocument();
    expect(within(panel).getByText("318")).toBeInTheDocument();
  });

  // The three numbers of a dry run are in columns of at least 9rem that fit the room, not a third each: ten digits of the code font do
  // not fit a third of a panel of 356 px, and the number ran under its neighbour (found by the layout run with 12 billion versions).
  it("lays the three numbers of the dry run in as many columns as fit, so that a long number is never under its neighbour", async () => {
    await openSection(newT5World(), "/admin/retention");
    const panel = await screen.findByRole("region", { name: "Dry run" });
    const list = within(panel).getByText("Versions that would be deleted").closest("dl");
    expect(list?.className).toContain("auto-fit");
    expect(list?.className).not.toContain("grid-cols-3");
    expect(within(panel).getByText("41,208").className).toContain("overflow-wrap:anywhere");
  });

  it("says plainly that the job deletes, that a deleted version cannot come back, and what is always kept", async () => {
    await openSection(newT5World(), "/admin/retention");
    const warning = (await screen.findByText("Retention permanently deletes stored versions")).closest<HTMLElement>('[data-slot="alert"]');
    expect(warning).not.toBeNull();
    expect(warning).toHaveTextContent("The retention job deletes by default");
    expect(warning).toHaveTextContent("a deleted version cannot be restored");
    expect(warning).toHaveTextContent("A dry run only shows what a policy would delete.");
    expect(warning).toHaveTextContent("Baselines and versions referenced by audit events, proposals, drafts or PR links are always kept");
  });

  it("says that the job's own dry-run switch is not shown by the API, so the policy is to be read as deleting", async () => {
    await openSection(newT5World(), "/admin/retention");
    const note = await screen.findByText(/does not report whether this hub's retention job is deleting or only counting/);
    expect(note).toHaveTextContent("Read every policy here as one that deletes.");
  });

  it("says when the policy was never changed and the defaults apply", async () => {
    const world = newT5World();
    world.retention = retention({ updated_at: null, updated_by: null });
    await openSection(world, "/admin/retention");
    expect(await screen.findByRole("region", { name: "Current policy" })).toHaveTextContent("Never changed: the defaults apply. Keeps every version for 180 days");
  });

  it("explains when a version is deleted", async () => {
    await openSection(newT5World(), "/admin/retention");
    expect(await screen.findByText(/A version is deleted only when it is older than the day limit and is not one of the last versions of its file/)).toBeInTheDocument();
  });

  it("shows a skeleton while loading and the problem with its request id when the policy cannot be read", async () => {
    const held = newT5World();
    held.hold.add("/api/v1/admin/retention");
    await openSection(held, "/admin/retention");
    expect(await screen.findByLabelText("Loading the retention policy")).toHaveAttribute("aria-busy", "true");
  });

  it("says the policy could not be loaded, with the request id", async () => {
    const world = newT5World();
    world.overrides.push({ match: (c) => c.path.startsWith("/api/v1/admin/retention"), respond: () => problem(500, "internal"), once: true });
    await openSection(world, "/admin/retention");
    expect(await screen.findByText("Couldn't load the retention policy")).toBeInTheDocument();
    expect(screen.getByText("req-test-1")).toBeInTheDocument();
  });
});

describe("the dry run", () => {
  it("asks the hub what a policy would delete with dry_run=true, shows the answer, and changes nothing", async () => {
    const user = userEvent.setup();
    const world = newT5World();
    const app = await openSection(world, "/admin/retention");
    await setPolicy(user, "90");
    await user.click(screen.getByRole("button", { name: "Run dry run" }));
    const panel = screen.getByRole("region", { name: "Dry run" });
    const expected = previewFor(90, 50);
    expect(await within(panel).findByText("This policy: 90 days, last 50 versions")).toBeInTheDocument();
    expect(within(panel).getByText(expected.versions_to_delete.toLocaleString("en-US"))).toBeInTheDocument();
    const calls = retentionCalls(app.calls);
    expect(calls).toHaveLength(1);
    expect(calls[0]).toMatchObject({ method: "PUT", path: "/api/v1/admin/retention?dry_run=true" });
    expect(JSON.parse(calls[0]?.body ?? "{}")).toEqual({ keep_days: 90, keep_last_versions: 50 });
    expect(calls[0]?.headers.get("idempotency-key")).toMatch(/^[0-9a-f-]{36}$/);
    expect(world.retention.keep_days).toBe(180);
    // It refetches nothing: the policy was read once, when the page opened.
    expect(app.calls.filter((c) => c.method === "GET" && c.path === "/api/v1/admin/retention")).toHaveLength(1);
  });

  it("refuses values that are not a policy, says which, and asks the hub for nothing", async () => {
    const user = userEvent.setup();
    const app = await openSection(newT5World(), "/admin/retention");
    await setPolicy(user, "14", "2");
    await user.click(screen.getByRole("button", { name: "Run dry run" }));
    expect(await screen.findByText("Minimum is 30 days")).toBeInTheDocument();
    expect(screen.getByText("Keep at least 5 versions per file")).toBeInTheDocument();
    expect(await days()).toHaveAttribute("aria-invalid", "true");
    expect(retentionCalls(app.calls)).toHaveLength(0);
  });

  it("says when the hub's answer holds no preview, and does not let the policy be saved", async () => {
    const user = userEvent.setup();
    const world = newT5World();
    world.overrides.push({ match: (c) => c.method === "PUT", respond: () => json({ ...retention(), dry_run_preview: null }) });
    await openSection(world, "/admin/retention");
    await setPolicy(user, "90");
    await user.click(screen.getByRole("button", { name: "Run dry run" }));
    expect(await screen.findByText("The hub sent no preview for this policy")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save policy" })).toHaveAttribute("aria-disabled", "true");
  });

  it.each([
    [500, /could not run the dry run\. Nothing was changed/i],
    [403, /refused the dry run/i],
    [422, /did not accept these values for a dry run/i],
  ])("says what the hub's %i means, with the request id, and does not let the policy be saved", async (status, text) => {
    const user = userEvent.setup();
    const world = newT5World();
    world.overrides.push({ match: (c) => c.method === "PUT", respond: () => problem(status, "x"), once: true });
    await openSection(world, "/admin/retention");
    await setPolicy(user, "90");
    await user.click(screen.getByRole("button", { name: "Run dry run" }));
    const panel = screen.getByRole("region", { name: "Dry run" });
    expect(await within(panel).findByText(text)).toBeInTheDocument();
    expect(within(panel).getByText("req-test-1")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save policy" })).toHaveAttribute("aria-disabled", "true");
  });

  it("marks the answer as out of date when the values are changed after it, and turns saving off until it is run again", async () => {
    const user = userEvent.setup();
    await openSection(newT5World(), "/admin/retention");
    await setPolicy(user, "90");
    await user.click(screen.getByRole("button", { name: "Run dry run" }));
    await screen.findByText("This policy: 90 days, last 50 versions");
    const save = screen.getByRole("button", { name: "Save policy" });
    expect(save).not.toHaveAttribute("aria-disabled");
    await user.clear(await days());
    await user.type(await days(), "60");
    expect(await screen.findByText(/You changed the values after this dry run\. Run it again before you save/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save policy" })).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByRole("button", { name: "Save policy" })).toHaveAccessibleDescription("Run the dry run for these values first");
  });

  it("puts the values back on Revert and forgets the answer", async () => {
    const user = userEvent.setup();
    await openSection(newT5World(), "/admin/retention");
    await setPolicy(user, "90");
    await user.click(screen.getByRole("button", { name: "Run dry run" }));
    await screen.findByText("This policy: 90 days, last 50 versions");
    await user.click(screen.getByRole("button", { name: "Revert" }));
    expect(await days()).toHaveValue("180");
    expect(screen.queryByText("This policy: 90 days, last 50 versions")).toBeNull();
    expect(screen.getByText("The saved policy: 180 days, last 50 versions")).toBeInTheDocument();
  });

  it("keeps the saved policy's dry run on screen while a new one is running", async () => {
    const user = userEvent.setup();
    const world = newT5World();
    const held = holdUntil();
    world.overrides.push({ match: (c) => c.method === "PUT", respond: async () => { await held.gate; return json({ ...world.retention, dry_run_preview: previewFor(90, 50) }); }, once: true });
    await openSection(world, "/admin/retention");
    await setPolicy(user, "90");
    await user.click(screen.getByRole("button", { name: "Run dry run" }));
    expect(await screen.findByText("Running the dry run…")).toBeInTheDocument();
    expect(screen.getByText("The saved policy: 180 days, last 50 versions")).toBeInTheDocument();
    held.release();
    expect(await screen.findByText("This policy: 90 days, last 50 versions")).toBeInTheDocument();
  });
});

describe("saving", () => {
  it("is off until something changed, and says so", async () => {
    await openSection(newT5World(), "/admin/retention");
    const save = await screen.findByRole("button", { name: "Save policy" });
    expect(save).toHaveAttribute("aria-disabled", "true");
    expect(save).toHaveAccessibleDescription("Nothing has changed");
  });

  it("asks for words to be typed before a policy that deletes is saved, and says what it deletes", async () => {
    const user = userEvent.setup();
    const world = newT5World();
    const app = await openSection(world, "/admin/retention");
    await setPolicy(user, "90");
    await user.click(screen.getByRole("button", { name: "Run dry run" }));
    await screen.findByText("This policy: 90 days, last 50 versions");
    await user.click(screen.getByRole("button", { name: "Save policy" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Save a policy that deletes versions" });
    const versions = previewFor(90, 50).versions_to_delete.toLocaleString("en-US");
    expect(dialog).toHaveTextContent(`The dry run found ${versions} versions to delete`);
    expect(dialog).toHaveTextContent("A deleted version cannot be restored");
    expect(dialog).toHaveTextContent("in bounded batches");
    const confirm = within(dialog).getByRole("button", { name: "Save and delete" });
    expect(confirm).toHaveAttribute("aria-disabled", "true");
    await user.click(confirm);
    expect(retentionCalls(app.calls).filter((c) => !c.path.includes("dry_run"))).toHaveLength(0);
    const field = within(dialog).getByLabelText(/Type delete versions to confirm/);
    await user.type(field, "delete");
    await user.keyboard("{Enter}");
    expect(retentionCalls(app.calls).filter((c) => !c.path.includes("dry_run"))).toHaveLength(0);
    await user.type(field, " versions");
    await user.click(confirm);
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    const saves = retentionCalls(app.calls).filter((c) => !c.path.includes("dry_run"));
    expect(saves).toHaveLength(1);
    expect(saves[0]).toMatchObject({ method: "PUT", path: "/api/v1/admin/retention" });
    expect(JSON.parse(saves[0]?.body ?? "{}")).toEqual({ keep_days: 90, keep_last_versions: 50 });
    expect(saves[0]?.headers.get("x-csrf-token")).toBeTruthy();
    expect(world.retention).toMatchObject({ keep_days: 90, updated_at: NOW });
    expect(await screen.findByText("Retention policy saved")).toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole("region", { name: "Current policy" })).toHaveTextContent("Keeps every version for 90 days and the last 50 versions of each file."));
    expect(await days()).toHaveValue("90");
  });

  it("asks for nothing to be typed when the dry run found nothing to delete", async () => {
    const user = userEvent.setup();
    const world = newT5World();
    world.overrides.push({ match: (c) => c.method === "PUT" && c.path.includes("dry_run"), respond: () => json({ ...retention(), dry_run_preview: { versions_to_delete: 0, bytes_to_free: 0, versions_kept_by_reference: 12 } }), once: true });
    const app = await openSection(world, "/admin/retention");
    await setPolicy(user, "365", "100");
    await user.click(screen.getByRole("button", { name: "Run dry run" }));
    await screen.findByText("This policy: 365 days, last 100 versions");
    await user.click(screen.getByRole("button", { name: "Save policy" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Save the retention policy" });
    expect(dialog).toHaveTextContent("The dry run found nothing that this policy would delete now");
    expect(within(dialog).queryByLabelText(/Type delete versions to confirm/)).toBeNull();
    await user.click(within(dialog).getByRole("button", { name: "Save policy" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(retentionCalls(app.calls).filter((c) => !c.path.includes("dry_run"))).toHaveLength(1);
  });

  it.each([
    [422, /did not accept these values/i],
    [403, /refused this change/i],
    [409, /at the same time/i],
    [500, /could not complete this change/i],
  ])("keeps the dialog and says what the hub's %i means, and shows nothing as saved", async (status, text) => {
    const user = userEvent.setup();
    const world = newT5World();
    world.overrides.push({ match: (c) => c.method === "PUT" && !c.path.includes("dry_run"), respond: () => problem(status, "x"), once: true });
    await openSection(world, "/admin/retention");
    await setPolicy(user, "90");
    await user.click(screen.getByRole("button", { name: "Run dry run" }));
    await screen.findByText("This policy: 90 days, last 50 versions");
    await user.click(screen.getByRole("button", { name: "Save policy" }));
    const dialog = await screen.findByRole("alertdialog");
    await user.type(within(dialog).getByLabelText(/Type delete versions to confirm/), "delete versions");
    await user.click(within(dialog).getByRole("button", { name: "Save and delete" }));
    expect(await within(dialog).findByText(text)).toBeInTheDocument();
    expect(within(dialog).getByText("req-test-1")).toBeInTheDocument();
    expect(screen.queryByText("Retention policy saved")).toBeNull();
    expect(world.retention.keep_days).toBe(180);
  });

  it("closes the dialog on Cancel and writes nothing", async () => {
    const user = userEvent.setup();
    const app = await openSection(newT5World(), "/admin/retention");
    await setPolicy(user, "90");
    await user.click(screen.getByRole("button", { name: "Run dry run" }));
    await screen.findByText("This policy: 90 days, last 50 versions");
    await user.click(screen.getByRole("button", { name: "Save policy" }));
    await user.click(await within(await screen.findByRole("alertdialog")).findByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(retentionCalls(app.calls).filter((c) => !c.path.includes("dry_run"))).toHaveLength(0);
    expect(await days()).toHaveValue("90");
  });
});

describe("text the hub was given", () => {
  const RLO = "‮";
  const ZWSP = "​";

  it("draws the hidden characters of the name of whoever changed the policy as visible markers", async () => {
    const world = newT5World();
    world.users = world.users.map((u) => (u.id === IDS.priya ? { ...u, display_name: `Pri${RLO}ya${ZWSP}Shah` } : u));
    await openSection(world, "/admin/retention");
    await screen.findByRole("region", { name: "Current policy" });
    await waitFor(() => expect(screen.getAllByRole("img", { name: /U\+(202E|200B)/ }).length).toBeGreaterThanOrEqual(2));
    expect(document.body.textContent).not.toContain(RLO);
    expect(document.body.textContent).not.toContain(ZWSP);
  });
});

describe("a Viewer", () => {
  it("gets the permission state and the hub is asked for nothing of the Admin area", async () => {
    const app = renderApp("/admin/retention", t5Api(newT5World("viewer")));
    expect(await screen.findByRole("heading", { level: 1, name: "Admin needs the Admin role" }, FIRST)).toBeInTheDocument();
    expect(app.calls.some((c) => c.path.startsWith("/api/v1/admin"))).toBe(false);
  });
});

describe("accessibility", () => {
  it("has no structural violations, with a result shown and the confirmation open", async () => {
    const user = userEvent.setup();
    await openSection(newT5World(), "/admin/retention");
    await days();
    await expectNoAxeViolations();
    await setPolicy(user, "90");
    await user.click(screen.getByRole("button", { name: "Run dry run" }));
    await screen.findByText("This policy: 90 days, last 50 versions");
    await expectNoAxeViolations();
    await user.click(screen.getByRole("button", { name: "Save policy" }));
    await screen.findByRole("alertdialog", { name: "Save a policy that deletes versions" });
    await expectNoAxeViolations();
  });
});

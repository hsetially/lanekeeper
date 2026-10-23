import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { FIRST, installAdminClock, openSection, rowIn, writesOf } from "@/mocks/design/admin-t5/harness";
import { severityRule } from "@/mocks/design/admin-t5/fixtures";
import { newT5World, t5Api } from "@/mocks/design/admin-t5/world";
import { problem, renderApp } from "@/test/app";
import { expectNoAxeViolations } from "@/test/axe";

// Severity rules (route /admin/severity, Admin only). The rules of the admin are ADDED to the engine's built-in ones and the highest
// severity wins (D103): a rule can raise a severity and can never lower one or switch a built-in rule off, so the page says so and
// has no control that would. The contract has the rules and nothing more: no built-in rule in the list, no order (priority is
// not used), no preview against recent changes (the engine's work, no operation gives it to the browser).

installAdminClock();
const grid = () => screen.findByRole("grid", { name: "Severity rules" });

describe("the rules", () => {
  it("lists the admin's rules with what each one matches, its severity and whether it is on", async () => {
    await openSection(newT5World(), "/admin/severity");
    expect(await screen.findByRole("heading", { level: 2, name: "Severity rules" })).toBeInTheDocument();
    expect(await screen.findByText("4 rules")).toBeInTheDocument();
    const sec = await rowIn(await grid(), "Security files are High");
    const cells = within(sec).getAllByRole("gridcell");
    expect(cells[0]).toHaveTextContent("security-high");
    expect(cells[1]).toHaveTextContent("Path security/**");
    expect(cells[2]).toHaveTextContent("High");
    expect(cells[3]).toHaveTextContent("On");
    const prod = await rowIn(await grid(), "A prod host in a SIT file is Critical");
    expect(within(prod).getAllByRole("gridcell")[1]).toHaveTextContent("Findings C6 · Path services/**");
    expect(within(prod).getAllByRole("gridcell")[2]).toHaveTextContent("Critical");
    const off = await rowIn(await grid(), "Payment limits are High");
    expect(within(off).getAllByRole("gridcell")[3]).toHaveTextContent("Off");
  });

  it("says that a rule can only raise a severity, that the highest wins, and that the built-in rules are not listed", async () => {
    await openSection(newT5World(), "/admin/severity");
    await grid();
    const notice = screen.getByText("Rules can raise a severity, never lower it").closest<HTMLElement>('[data-slot="alert"]');
    expect(notice).not.toBeNull();
    expect(notice).toHaveTextContent("The built-in rules always apply and are not listed here.");
    expect(notice).toHaveTextContent("the highest severity wins");
    expect(notice).toHaveTextContent("can never lower it or switch a built-in rule off");
    expect(notice).toHaveTextContent("The order of the rules does not matter.");
  });

  it("offers no control to lower a severity that a built-in rule gives, to switch one off, or to put the rules in order", async () => {
    await openSection(newT5World(), "/admin/severity");
    await grid();
    const region = screen.getByRole("region", { name: "Admin section: Severity rules" });
    expect(within(region).queryByRole("button", { name: /lower|built-in|move up|move down|disable/i })).toBeNull();
    expect(within(region).queryByRole("switch")).toBeNull();
    // The lead above the list says the highest severity wins, not that the first rule does.
    expect(within(region).getByText(/the highest severity wins/i, { selector: "p" })).toBeInTheDocument();
    expect(within(region).queryByText(/first matching rule wins/i)).toBeNull();
  });

  it("marks a rule with a change type or a setting glob as not applied yet, because the registry has nothing to match them with", async () => {
    await openSection(newT5World(), "/admin/severity");
    const row = await rowIn(await grid(), "A removed setting is Medium");
    expect(within(row).getByText("Not applied yet")).toBeInTheDocument();
    const plain = await rowIn(await grid(), "Security files are High");
    expect(within(plain).queryByText("Not applied yet")).toBeNull();
  });

  // The cell is one line of a row of fixed height. In a language whose words are 40% longer, "Off" or "On" and the marker are wider than the
  // column, and a marker cut off by the cell loses its words (found by the layout run with the labels stretched): it gives way with an
  // ellipsis, and the explanation is in its title.
  it("lets the 'Not applied yet' marker give way with an ellipsis rather than be cut off, and keeps what it means in its title", async () => {
    await openSection(newT5World(), "/admin/severity");
    const row = await rowIn(await grid(), "A removed setting is Medium");
    const marker = within(row).getByText("Not applied yet");
    expect(marker.className).toMatch(/(^|\s)truncate(\s|$)/);
    expect(marker.className).toMatch(/(^|\s)min-w-0(\s|$)/);
    expect(marker).toHaveAttribute("title", expect.stringContaining("has nothing to match today"));
  });

  it("shows a rule that states no condition as matching everything, in a warning colour", async () => {
    const world = newT5World();
    world.severity = [severityRule("everything", { name: "Everything is High", match: {} })];
    await openSection(world, "/admin/severity");
    const row = await rowIn(await grid(), "Everything is High");
    expect(within(row).getAllByRole("gridcell")[1]).toHaveTextContent("Every change and finding");
  });

  it("explains an empty list: only the built-in rules apply", async () => {
    const world = newT5World();
    world.severity = [];
    await openSection(world, "/admin/severity");
    expect(await screen.findByText("No severity rules of your own")).toBeInTheDocument();
    expect(screen.getByText(/Only the built-in rules apply/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Add rule" })).toBeInTheDocument();
  });

  it("shows a skeleton while loading, and the problem with its request id when the list cannot be read", async () => {
    const held = newT5World();
    held.hold.add("/api/v1/admin/severity-rules");
    await openSection(held, "/admin/severity");
    expect(await screen.findByLabelText("Loading severity rules")).toHaveAttribute("aria-busy", "true");
  });

  it("says the list could not be loaded, with the request id", async () => {
    const world = newT5World();
    world.overrides.push({ match: (c) => c.path.startsWith("/api/v1/admin/severity-rules"), respond: () => problem(500, "internal"), once: true });
    await openSection(world, "/admin/severity");
    expect(await screen.findByText("Couldn't load severity rules")).toBeInTheDocument();
    expect(screen.getByText("req-test-1")).toBeInTheDocument();
  });
});

describe("adding a rule", () => {
  it("saves a rule after a dialog that says what it changes", async () => {
    const user = userEvent.setup();
    const app = await openSection(newT5World(), "/admin/severity");
    await user.click(await screen.findByRole("button", { name: "Add rule" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Add a severity rule" });
    expect(within(dialog).getByText(/Pending changes and open findings that match are rated again with this rule\. Changes already resolved keep the severity they had/)).toBeInTheDocument();
    await user.type(within(dialog).getByLabelText("Rule id"), "ssl-critical");
    await user.type(within(dialog).getByLabelText("Name"), "SSL config is Critical");
    await user.selectOptions(within(dialog).getByLabelText("Severity"), "critical");
    await user.type(within(dialog).getByLabelText("Path glob"), "security/ssl-*");
    await user.click(within(dialog).getByRole("checkbox", { name: "C5 NFS and Git disagree" }));
    await user.click(within(dialog).getByRole("button", { name: "Save rule" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    const put = writesOf(app.calls)[0];
    expect(put).toMatchObject({ method: "PUT", path: "/api/v1/admin/severity-rules/ssl-critical" });
    expect(JSON.parse(put?.body ?? "{}")).toEqual({
      name: "SSL config is Critical",
      match: { finding_kinds: ["drift"], path_glob: "security/ssl-*" },
      severity: "critical",
      priority: 100,
      enabled: true,
    });
    expect(put?.headers.get("idempotency-key")).toMatch(/^[0-9a-f-]{36}$/);
    expect(await screen.findByText("Rule ssl-critical saved")).toBeInTheDocument();
    expect(await rowIn(await grid(), "SSL config is Critical")).toBeInTheDocument();
    expect(await screen.findByText("5 rules")).toBeInTheDocument();
  });

  it("says in the dialog, as the person sets it, what the rule matches, and warns when it states nothing", async () => {
    const user = userEvent.setup();
    await openSection(newT5World(), "/admin/severity");
    await user.click(await screen.findByRole("button", { name: "Add rule" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Add a severity rule" });
    const summary = within(dialog).getByRole("status", { name: "What this rule matches" });
    expect(summary).toHaveTextContent("This rule matches every change and every finding.");
    expect(within(dialog).getByText("With no condition, this rule raises every change and every finding to its severity")).toBeInTheDocument();
    await user.type(within(dialog).getByLabelText("Path glob"), "services/**");
    expect(summary).toHaveTextContent("This rule matches: Path services/**");
    expect(within(dialog).queryByText("With no condition, this rule raises every change and every finding to its severity")).toBeNull();
  });

  it("warns that a change type or a setting glob matches nothing today, and still lets the rule be saved", async () => {
    const user = userEvent.setup();
    const app = await openSection(newT5World(), "/admin/severity");
    await user.click(await screen.findByRole("button", { name: "Add rule" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Add a severity rule" });
    await user.click(within(dialog).getByRole("checkbox", { name: "Setting removed" }));
    expect(within(dialog).getByText("Not applied yet")).toBeInTheDocument();
    expect(within(dialog).getByText(/does not look at the individual setting changes of a file when it rates it/)).toBeInTheDocument();
    await user.type(within(dialog).getByLabelText("Rule id"), "removed-high");
    await user.type(within(dialog).getByLabelText("Name"), "Removed settings are High");
    await user.click(within(dialog).getByRole("button", { name: "Save rule" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect((JSON.parse(writesOf(app.calls)[0]?.body ?? "{}") as { match?: unknown }).match).toEqual({ change_kinds: ["removed"] });
  });

  it("refuses an id that is not an id, a missing name, a glob that cannot work and an id in use, sending nothing", async () => {
    const user = userEvent.setup();
    const app = await openSection(newT5World(), "/admin/severity");
    await user.click(await screen.findByRole("button", { name: "Add rule" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Add a severity rule" });
    await user.type(within(dialog).getByLabelText("Rule id"), "Bad Id");
    await user.type(within(dialog).getByLabelText("Path glob"), "/security/**");
    await user.type(within(dialog).getByLabelText("Setting glob"), "db.[[x");
    await user.click(within(dialog).getByRole("button", { name: "Save rule" }));
    expect(await within(dialog).findByText(/lower case letters, digits/i)).toBeInTheDocument();
    expect(within(dialog).getByText("Enter a name")).toBeInTheDocument();
    expect(within(dialog).getByText("Paths start at the config root, with no leading /")).toBeInTheDocument();
    expect(within(dialog).getByText("Invalid glob: unclosed [")).toBeInTheDocument();
    expect(within(dialog).getByLabelText("Path glob")).toHaveAttribute("aria-invalid", "true");
    const id = within(dialog).getByLabelText("Rule id");
    await user.clear(id);
    await user.type(id, "security-high");
    await user.click(within(dialog).getByRole("button", { name: "Save rule" }));
    expect(await within(dialog).findByText("A rule with this id exists already. Use Edit on its row to change it.")).toBeInTheDocument();
    expect(writesOf(app.calls)).toHaveLength(0);
  });

  it.each([
    [422, /did not accept these values/i],
    [403, /refused this change/i],
    [409, /at the same time/i],
  ])("keeps the dialog and what was typed when the hub answers %i", async (status, text) => {
    const user = userEvent.setup();
    const world = newT5World();
    world.overrides.push({ match: (c) => c.method === "PUT", respond: () => problem(status, "x"), once: true });
    await openSection(world, "/admin/severity");
    await user.click(await screen.findByRole("button", { name: "Add rule" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Add a severity rule" });
    await user.type(within(dialog).getByLabelText("Rule id"), "x");
    await user.type(within(dialog).getByLabelText("Name"), "X");
    await user.click(within(dialog).getByRole("button", { name: "Save rule" }));
    expect(await within(dialog).findByText(text)).toBeInTheDocument();
    expect(within(dialog).getByText("req-test-1")).toBeInTheDocument();
    expect(within(dialog).getByLabelText("Name")).toHaveValue("X");
    expect(screen.queryByText("Rule x saved")).toBeNull();
  });
});

describe("changing a rule", () => {
  it("changes the severity of the admin's own rule, keeps its id and its priority, and cannot rename it", async () => {
    const user = userEvent.setup();
    const app = await openSection(newT5World(), "/admin/severity");
    await user.click(within(await rowIn(await grid(), "A prod host in a SIT file is Critical")).getByRole("button", { name: "Edit rule prod-host-critical" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Edit rule prod-host-critical" });
    expect(within(dialog).getByLabelText("Rule id")).toHaveAttribute("readonly");
    expect(within(dialog).getByLabelText("Severity")).toHaveValue("critical");
    expect(within(dialog).getByRole("checkbox", { name: "C6 Points at another environment" })).toBeChecked();
    expect(within(dialog).getByLabelText("Path glob")).toHaveValue("services/**");
    await user.selectOptions(within(dialog).getByLabelText("Severity"), "high");
    await user.click(within(dialog).getByRole("button", { name: "Save rule" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    const put = writesOf(app.calls)[0];
    expect(put).toMatchObject({ method: "PUT", path: "/api/v1/admin/severity-rules/prod-host-critical" });
    expect(JSON.parse(put?.body ?? "{}")).toEqual({
      name: "A prod host in a SIT file is Critical",
      match: { finding_kinds: ["environment_host_mismatch"], path_glob: "services/**" },
      severity: "high",
      priority: 200,
      enabled: true,
    });
  });

  it("switches off the admin's own rule only, and says the built-in rules stay on", async () => {
    const user = userEvent.setup();
    const app = await openSection(newT5World(), "/admin/severity");
    await user.click(within(await rowIn(await grid(), "Security files are High")).getByRole("button", { name: "Edit rule security-high" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Edit rule security-high" });
    const on = within(dialog).getByRole("switch", { name: "This rule is on" });
    expect(on).toBeChecked();
    expect(within(dialog).getByText(/This switches off this rule only: the built-in rules stay on/)).toBeInTheDocument();
    await user.click(on);
    await user.click(within(dialog).getByRole("button", { name: "Save rule" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect((JSON.parse(writesOf(app.calls)[0]?.body ?? "{}") as { enabled?: unknown }).enabled).toBe(false);
    const row = await rowIn(await grid(), "Security files are High");
    await waitFor(() => expect(within(row).getAllByRole("gridcell")[3]).toHaveTextContent("Off"));
  });
});

describe("deleting a rule", () => {
  it("says that what the rule raised goes back to what the other rules say, then removes it", async () => {
    const user = userEvent.setup();
    const app = await openSection(newT5World(), "/admin/severity");
    await user.click(within(await rowIn(await grid(), "Payment limits are High")).getByRole("button", { name: "Delete rule payments-high" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Delete rule payments-high" });
    expect(within(dialog).getByText(/Whatever this rule raised goes back to what the other rules say for pending changes and open findings\. The built-in rules are not affected/)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Delete rule" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(writesOf(app.calls)[0]).toMatchObject({ method: "DELETE", path: "/api/v1/admin/severity-rules/payments-high" });
    expect(await screen.findByText("3 rules")).toBeInTheDocument();
  });
});

describe("text the hub was given", () => {
  const RLO = "‮";
  const ZWSP = "​";

  it("draws the hidden characters of a rule's name, id and globs as visible markers", async () => {
    const world = newT5World();
    world.severity = [severityRule(`r${ZWSP}1`, { name: `Sec${RLO}urity`, match: { path_glob: `se${ZWSP}c/**`, setting_glob: `d${RLO}b.*` } })];
    await openSection(world, "/admin/severity");
    await grid();
    await waitFor(() => expect(screen.getAllByRole("img", { name: /U\+(202E|200B)/ }).length).toBeGreaterThanOrEqual(4));
    expect(document.body.textContent).not.toContain(RLO);
    expect(document.body.textContent).not.toContain(ZWSP);
  });
});

describe("a Viewer", () => {
  it("gets the permission state and the hub is asked for nothing of the Admin area", async () => {
    const app = renderApp("/admin/severity", t5Api(newT5World("viewer")));
    expect(await screen.findByRole("heading", { level: 1, name: "Admin needs the Admin role" }, FIRST)).toBeInTheDocument();
    expect(app.calls.some((c) => c.path.startsWith("/api/v1/admin"))).toBe(false);
  });
});

describe("accessibility", () => {
  it("has no structural violations, with the dialog open as well", async () => {
    const user = userEvent.setup();
    await openSection(newT5World(), "/admin/severity");
    await rowIn(await grid(), "Security files are High");
    await expectNoAxeViolations();
    await user.click(await screen.findByRole("button", { name: "Add rule" }));
    await screen.findByRole("alertdialog", { name: "Add a severity rule" });
    await expectNoAxeViolations();
  });
});

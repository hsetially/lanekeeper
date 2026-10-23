import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { IDS } from "@/mocks/design/admin-t5/fixtures";
import { FIRST, installAdminClock, openSection, rowIn, writesOf } from "@/mocks/design/admin-t5/harness";
import { newT5World, t5Api } from "@/mocks/design/admin-t5/world";
import { problem, renderApp } from "@/test/app";
import { expectNoAxeViolations } from "@/test/axe";

// OS Login users (route /admin/oslogin, Admin only): the admin's own mappings from the login name an NFS VM records for an edit to the
// Lanekeeper user it stands for. The contract has the mappings and nothing else: no list of the names seen, no automatic match, no
// last session (the design's table of accounts, its preview and its "Auto" and "Manual" column), so none of those is drawn.

installAdminClock();
const grid = () => screen.findByRole("grid", { name: "OS Login mappings" });

describe("the mappings", () => {
  it("lists each login name with the person it stands for", async () => {
    await openSection(newT5World(), "/admin/oslogin");
    expect(await screen.findByRole("heading", { level: 2, name: "OS Login user mapping" })).toBeInTheDocument();
    expect(await screen.findByText("2 mappings")).toBeInTheDocument();
    const marco = await rowIn(await grid(), "marco_diaz_contractor_example");
    expect(within(marco).getAllByRole("gridcell")[1]).toHaveTextContent("Marco Diaz");
    expect(within(marco).getAllByRole("gridcell")[1]).toHaveTextContent("marco.diaz@corp.example");
  });

  it("says in what order a login name is matched, so that an admin knows when a row is needed", async () => {
    await openSection(newT5World(), "/admin/oslogin");
    await grid();
    const note = screen.getByRole("region", { name: "How a login name is matched" });
    expect(note).toHaveTextContent("A row below is looked up first, by the exact name.");
    expect(note).toHaveTextContent("dana.okoye@corp.example becomes dana_okoye_corp_example");
    expect(note).toHaveTextContent("When two users have the same form, nothing is guessed");
    expect(note).toHaveTextContent("Otherwise the name is shown on its own");
  });

  it("marks a mapping to a user who is disabled or has no role, because edits are still attributed to them", async () => {
    const world = newT5World();
    world.osLogin = [{ os_login: "ines_costa_corp_example", user: IDS.ines }];
    await openSection(world, "/admin/oslogin");
    const row = await rowIn(await grid(), "ines_costa_corp_example");
    expect(within(row).getByText("disabled")).toBeInTheDocument();
    expect(screen.getByText(/a disabled user still made the edits recorded under their name, so they stay attributed to them/)).toBeInTheDocument();
  });

  it("shows a user the list does not have as unknown, with the id, never as a name", async () => {
    const world = newT5World();
    const gone = "11111111-1111-4111-8111-111111111111:00000000-0000-4000-8000-000000000000";
    world.osLogin = [{ os_login: "x_gone", user: gone }];
    await openSection(world, "/admin/oslogin");
    const row = await rowIn(await grid(), "x_gone");
    expect(within(row).getByText("Unknown user")).toBeInTheDocument();
    expect(within(row).getByText(gone)).toBeInTheDocument();
  });

  it("explains an empty list", async () => {
    const world = newT5World();
    world.osLogin = [];
    await openSection(world, "/admin/oslogin");
    expect(await screen.findByText("No OS Login mappings")).toBeInTheDocument();
    expect(screen.getByText(/Every login name is matched to a user by email where it can be/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Add mapping" })).toBeInTheDocument();
  });

  it("shows a skeleton while loading and the problem when the list cannot be read", async () => {
    const held = newT5World();
    held.hold.add("/api/v1/admin/os-login-users");
    await openSection(held, "/admin/oslogin");
    expect(await screen.findByLabelText("Loading OS Login mappings")).toHaveAttribute("aria-busy", "true");
  });

  it("says the list could not be loaded, with the request id", async () => {
    const world = newT5World();
    world.overrides.push({ match: (c) => c.path.startsWith("/api/v1/admin/os-login-users"), respond: () => problem(500, "internal"), once: true });
    await openSection(world, "/admin/oslogin");
    expect(await screen.findByText("Couldn't load OS Login mappings")).toBeInTheDocument();
    expect(screen.getByText("req-test-1")).toBeInTheDocument();
  });
});

describe("adding and changing a mapping", () => {
  it("maps a login name to a user after a dialog that says what it changes", async () => {
    const user = userEvent.setup();
    const app = await openSection(newT5World(), "/admin/oslogin");
    await user.click(await screen.findByRole("button", { name: "Add mapping" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Add an OS Login mapping" });
    expect(within(dialog).getByText(/Edits recorded under this login name are attributed to the user chosen/)).toBeInTheDocument();
    await user.type(within(dialog).getByLabelText("OS Login username"), "dana_okoye_work");
    await user.selectOptions(within(dialog).getByLabelText("Lanekeeper user"), IDS.dana);
    await user.click(within(dialog).getByRole("button", { name: "Save mapping" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    const put = writesOf(app.calls)[0];
    expect(put).toMatchObject({ method: "PUT", path: "/api/v1/admin/os-login-users/dana_okoye_work" });
    expect(JSON.parse(put?.body ?? "{}")).toEqual({ user: IDS.dana });
    expect(put?.headers.get("idempotency-key")).toMatch(/^[0-9a-f-]{36}$/);
    expect(put?.headers.get("x-csrf-token")).toBeTruthy();
    expect(await screen.findByText("Mapping dana_okoye_work saved")).toBeInTheDocument();
    expect(await rowIn(await grid(), "dana_okoye_work")).toBeInTheDocument();
  });

  it("offers every user with the state of those who are not active, and leaves the choice of a disabled one to the hub", async () => {
    const user = userEvent.setup();
    await openSection(newT5World(), "/admin/oslogin");
    await user.click(await screen.findByRole("button", { name: "Add mapping" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Add an OS Login mapping" });
    const select = within(dialog).getByLabelText("Lanekeeper user");
    await waitFor(() => expect(within(select).getAllByRole("option").length).toBeGreaterThan(7));
    const ines = within(select).getByRole("option", { name: "Ines Costa · ines.costa@corp.example (disabled)" });
    expect(ines).not.toBeDisabled();
    expect(within(select).getByRole("option", { name: "Choose a user" })).toHaveValue("");
  });

  it("refuses a name that is not a login name and a missing user, sending nothing", async () => {
    const user = userEvent.setup();
    const app = await openSection(newT5World(), "/admin/oslogin");
    await user.click(await screen.findByRole("button", { name: "Add mapping" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Add an OS Login mapping" });
    await user.type(within(dialog).getByLabelText("OS Login username"), "../users");
    await user.click(within(dialog).getByRole("button", { name: "Save mapping" }));
    expect(await within(dialog).findByText(/^Use the name as the VM records it/)).toBeInTheDocument();
    expect(within(dialog).getByText("Choose the user this name stands for")).toBeInTheDocument();
    expect(within(dialog).getByLabelText("OS Login username")).toHaveAttribute("aria-invalid", "true");
    expect(writesOf(app.calls)).toHaveLength(0);
  });

  it("refuses a name that is already mapped as a new one, and points to Edit", async () => {
    const user = userEvent.setup();
    const app = await openSection(newT5World(), "/admin/oslogin");
    await grid();
    await user.click(await screen.findByRole("button", { name: "Add mapping" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Add an OS Login mapping" });
    await user.type(within(dialog).getByLabelText("OS Login username"), "lena_b_corp_example");
    await user.selectOptions(within(dialog).getByLabelText("Lanekeeper user"), IDS.dana);
    await user.click(within(dialog).getByRole("button", { name: "Save mapping" }));
    expect(await within(dialog).findByText("This login name is mapped already. Use Edit on its row to change the user.")).toBeInTheDocument();
    expect(writesOf(app.calls)).toHaveLength(0);
  });

  it("changes the user of a mapping and cannot change its name", async () => {
    const user = userEvent.setup();
    const app = await openSection(newT5World(), "/admin/oslogin");
    await user.click(within(await rowIn(await grid(), "lena_b_corp_example")).getByRole("button", { name: "Edit mapping lena_b_corp_example" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Edit mapping lena_b_corp_example" });
    expect(within(dialog).getByLabelText("OS Login username")).toHaveAttribute("readonly");
    expect(within(dialog).getByLabelText("Lanekeeper user")).toHaveValue(IDS.lena);
    await user.selectOptions(within(dialog).getByLabelText("Lanekeeper user"), IDS.amir);
    await user.click(within(dialog).getByRole("button", { name: "Save mapping" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(writesOf(app.calls)[0]).toMatchObject({ method: "PUT", path: "/api/v1/admin/os-login-users/lena_b_corp_example" });
    expect(JSON.parse(writesOf(app.calls)[0]?.body ?? "{}")).toEqual({ user: IDS.amir });
    const row = await rowIn(await grid(), "lena_b_corp_example");
    await waitFor(() => expect(within(row).getAllByRole("gridcell")[1]).toHaveTextContent("Amir Haddad"));
  });

  it("keeps the dialog and the input when the hub does not accept the mapping (422), and says it", async () => {
    const user = userEvent.setup();
    const world = newT5World();
    world.overrides.push({ match: (c) => c.method === "PUT", respond: () => problem(422, "unprocessable"), once: true });
    await openSection(world, "/admin/oslogin");
    await user.click(await screen.findByRole("button", { name: "Add mapping" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Add an OS Login mapping" });
    await user.type(within(dialog).getByLabelText("OS Login username"), "ines_old");
    await user.selectOptions(within(dialog).getByLabelText("Lanekeeper user"), IDS.ines);
    await user.click(within(dialog).getByRole("button", { name: "Save mapping" }));
    expect(await within(dialog).findByText(/did not accept these values/i)).toBeInTheDocument();
    expect(within(dialog).getByText("req-test-1")).toBeInTheDocument();
    expect(within(dialog).getByLabelText("OS Login username")).toHaveValue("ines_old");
    expect(within(dialog).getByLabelText("Lanekeeper user")).toHaveValue(IDS.ines);
  });

  it.each([
    [403, /refused this change/i],
    [409, /at the same time/i],
    [500, /could not complete this change/i],
  ])("says what a %i means in the dialog, and shows nothing as done", async (status, text) => {
    const user = userEvent.setup();
    const world = newT5World();
    world.overrides.push({ match: (c) => c.method === "PUT", respond: () => problem(status, "x"), once: true });
    await openSection(world, "/admin/oslogin");
    await user.click(await screen.findByRole("button", { name: "Add mapping" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Add an OS Login mapping" });
    await user.type(within(dialog).getByLabelText("OS Login username"), "x_y");
    await user.selectOptions(within(dialog).getByLabelText("Lanekeeper user"), IDS.dana);
    await user.click(within(dialog).getByRole("button", { name: "Save mapping" }));
    expect(await within(dialog).findByText(text)).toBeInTheDocument();
    expect(screen.queryByText("Mapping x_y saved")).toBeNull();
  });
});

describe("deleting a mapping", () => {
  it("says that the login name goes back to being matched by email, then removes it", async () => {
    const user = userEvent.setup();
    const app = await openSection(newT5World(), "/admin/oslogin");
    await user.click(within(await rowIn(await grid(), "marco_diaz_contractor_example")).getByRole("button", { name: "Delete mapping marco_diaz_contractor_example" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Delete mapping marco_diaz_contractor_example" });
    expect(within(dialog).getByText(/Edits recorded under this login name are matched by email from now on, or shown by the name alone/)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Delete mapping" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(writesOf(app.calls)[0]).toMatchObject({ method: "DELETE", path: "/api/v1/admin/os-login-users/marco_diaz_contractor_example" });
    expect(await screen.findByText("1 mapping")).toBeInTheDocument();
  });

  it("addresses a name as one encoded path segment", async () => {
    const user = userEvent.setup();
    const world = newT5World();
    world.osLogin = [{ os_login: "dana@corp+ext", user: IDS.dana }];
    const app = await openSection(world, "/admin/oslogin");
    await user.click(within(await rowIn(await grid(), "dana@corp+ext")).getByRole("button", { name: "Delete mapping dana@corp+ext" }));
    await user.click(await within(await screen.findByRole("alertdialog")).findByRole("button", { name: "Delete mapping" }));
    await waitFor(() => expect(writesOf(app.calls)).toHaveLength(1));
    expect(writesOf(app.calls)[0]?.path).toBe("/api/v1/admin/os-login-users/dana%40corp%2Bext");
  });

  it("sends nothing for a name from the hub that holds a slash, which would point the request at another address, and says so", async () => {
    const user = userEvent.setup();
    const world = newT5World();
    world.osLogin = [{ os_login: "a/../../users", user: IDS.dana }];
    const app = await openSection(world, "/admin/oslogin");
    await user.click(within(await rowIn(await grid(), "a/../../users")).getByRole("button", { name: "Delete mapping a/../../users" }));
    const dialog = await screen.findByRole("alertdialog");
    await user.click(await within(dialog).findByRole("button", { name: "Delete mapping" }));
    expect(await within(dialog).findByText("The rule could not be deleted")).toBeInTheDocument();
    expect(writesOf(app.calls)).toHaveLength(0);
  });

  it("keeps the dialog when the hub says the mapping is gone (404)", async () => {
    const user = userEvent.setup();
    const world = newT5World();
    await openSection(world, "/admin/oslogin");
    const row = await rowIn(await grid(), "lena_b_corp_example");
    world.osLogin = [];
    await user.click(within(row).getByRole("button", { name: "Delete mapping lena_b_corp_example" }));
    await user.click(await within(await screen.findByRole("alertdialog")).findByRole("button", { name: "Delete mapping" }));
    expect(await screen.findByText(/no longer exists/i)).toBeInTheDocument();
  });
});

describe("text the hub was given", () => {
  const RLO = "‮";
  const ZWSP = "​";

  it("draws the hidden characters of a login name and of a person's name as visible markers", async () => {
    const world = newT5World();
    world.osLogin = [{ os_login: `dana${ZWSP}_okoye`, user: IDS.dana }];
    world.users = world.users.map((u) => (u.id === IDS.dana ? { ...u, display_name: `Dana${RLO}Okoye`, email: `d${ZWSP}ana@corp.example` } : u));
    await openSection(world, "/admin/oslogin");
    await grid();
    await waitFor(() => expect(screen.getAllByRole("img", { name: /U\+(202E|200B)/ }).length).toBeGreaterThanOrEqual(3));
    expect(document.body.textContent).not.toContain(RLO);
    expect(document.body.textContent).not.toContain(ZWSP);
  });
});

describe("a Viewer", () => {
  it("gets the permission state and the hub is asked for nothing of the Admin area", async () => {
    const app = renderApp("/admin/oslogin", t5Api(newT5World("viewer")));
    expect(await screen.findByRole("heading", { level: 1, name: "Admin needs the Admin role" }, FIRST)).toBeInTheDocument();
    expect(app.calls.some((c) => c.path.startsWith("/api/v1/admin"))).toBe(false);
  });
});

describe("accessibility", () => {
  it("has no structural violations, with the dialog open as well", async () => {
    const user = userEvent.setup();
    await openSection(newT5World(), "/admin/oslogin");
    await rowIn(await grid(), "lena_b_corp_example");
    await expectNoAxeViolations();
    await user.click(await screen.findByRole("button", { name: "Add mapping" }));
    await screen.findByRole("alertdialog", { name: "Add an OS Login mapping" });
    await expectNoAxeViolations();
  });
});

import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { FIRST, openAdmin, rowIn, installAdminClock, writesOf } from "@/mocks/design/admin/harness";
import { newWorld, worldApi } from "@/mocks/design/admin/world";
import { problem, renderApp } from "@/test/app";
import { expectNoAxeViolations } from "@/test/axe";

// Notifications (route /admin/notify, Admin only): the Microsoft Teams channels that carry events. A channel's webhook URL is a SECRET
// (S21): it is typed here, sent once, and never comes back; the list says only whether one is set. So the URL is held in the dialog's
// state and nowhere else (not the query cache, not the mutation cache, not storage, not a log), the field is emptied after every
// answer, and the request goes through useSecretMutation (gcTime 0, reset in a finally, never retried).
//
// The design's "Send test" has no operation behind it, so it is not drawn.

installAdminClock();
const grid = () => screen.findByRole("grid", { name: "Notification channels" });
const URL_SECRET = "https://corp.webhook.office.com/webhookb2/TOPSECRETPATH@tenant/IncomingWebhook/aaaa/bbbb";

const held = (app: ReturnType<typeof renderApp>): string =>
  JSON.stringify([
    app.queryClient.getQueryCache().getAll().map((q) => [q.queryKey, q.state.data, q.state.error]),
    app.queryClient.getMutationCache().getAll().map((m) => [m.options.mutationKey, m.state.variables, m.state.data, m.state.error]),
  ]);
function watchConsole(): unknown[][] {
  const logged: unknown[][] = [];
  for (const method of ["log", "info", "warn", "error", "debug"] as const) vi.spyOn(console, method).mockImplementation((...args: unknown[]) => void logged.push(args));
  return logged;
}

/** An address is pasted, as a person does, not typed one key at a time. */
async function paste(user: ReturnType<typeof userEvent.setup>, field: HTMLElement, text: string) {
  await user.click(field);
  await user.paste(text);
}

async function fillNew(user: ReturnType<typeof userEvent.setup>, dialog: HTMLElement, url = URL_SECRET) {
  await user.type(within(dialog).getByLabelText("Channel id"), "ops");
  await user.type(within(dialog).getByLabelText("Name"), "Operations");
  await user.click(within(dialog).getByRole("checkbox", { name: "Critical finding" }));
  await user.selectOptions(within(dialog).getByLabelText("Minimum severity"), "high");
  if (url !== "") await paste(user, within(dialog).getByLabelText("Webhook URL"), url);
}

describe("the channels", () => {
  it("lists them with their events, severity and whether a webhook is set, never the webhook", async () => {
    await openAdmin(newWorld(), "/admin/notify");
    expect(await screen.findByRole("heading", { level: 2, name: "Notifications" })).toBeInTheDocument();
    expect(await screen.findByText("3 channels")).toBeInTheDocument();
    const certificates = await rowIn(await grid(), "Certificate expiring");
    expect(within(certificates).getByText("2 events")).toHaveAttribute("title", "Agent disconnected, Sync window stale");
    expect(within(certificates).getByText("Medium")).toBeInTheDocument();
    expect(within(certificates).getByText("Webhook set")).toBeInTheDocument();
    expect(within(certificates).getByRole("switch", { name: "Send Certificate expiring" })).toBeChecked();
    const findings = await rowIn(await grid(), "Critical findings");
    expect(within(findings).getByText("Not set")).toBeInTheDocument();
    expect(within(findings).getByRole("switch", { name: "Send Critical findings" })).not.toBeChecked();
    expect(screen.queryByText("Send test")).toBeNull();
    expect(document.body.innerHTML).not.toContain("webhook.office.com");
  });

  it("says notifications are off when no channel has a webhook that is switched on", async () => {
    const world = newWorld();
    world.channels = [];
    await openAdmin(world, "/admin/notify");
    expect(await screen.findByText("Notifications are off")).toBeInTheDocument();
    expect(screen.getByText(/Events show in Lanekeeper only/)).toBeInTheDocument();
    expect(await screen.findByText("No notification channels")).toBeInTheDocument();
  });

  it("does not say they are off while a channel with a webhook is switched on", async () => {
    await openAdmin(newWorld(), "/admin/notify");
    await grid();
    expect(screen.queryByText("Notifications are off")).toBeNull();
  });

  it("shows a skeleton while loading and the problem when the list cannot be read", async () => {
    const heldWorld = newWorld();
    heldWorld.hold.add("/api/v1/admin/notifications");
    await openAdmin(heldWorld, "/admin/notify");
    expect(await screen.findByLabelText("Loading notification channels")).toHaveAttribute("aria-busy", "true");
  });

  it("says the list could not be loaded, with the request id", async () => {
    const world = newWorld();
    world.overrides.push({ match: (c) => c.path.startsWith("/api/v1/admin/notifications"), respond: () => problem(500, "internal"), once: true });
    await openAdmin(world, "/admin/notify");
    expect(await screen.findByText("Couldn't load notification channels")).toBeInTheDocument();
    expect(screen.getByText("req-test-1")).toBeInTheDocument();
  });
});

describe("saving a webhook", () => {
  it("sends it once, shows it nowhere afterwards, and keeps it out of every cache and log", async () => {
    const user = userEvent.setup();
    const logged = watchConsole();
    const world = newWorld();
    const app = await openAdmin(world, "/admin/notify");
    await user.click(await screen.findByRole("button", { name: "Add channel" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Add a notification channel" });
    await fillNew(user, dialog);
    await user.click(within(dialog).getByRole("button", { name: "Save channel" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    const put = writesOf(app.calls)[0];
    expect(put).toMatchObject({ method: "PUT", path: "/api/v1/admin/notifications/ops" });
    expect(JSON.parse(put?.body ?? "{}")).toEqual({ name: "Operations", min_severity: "high", kinds: ["critical_finding"], enabled: true, webhook_url: URL_SECRET });
    expect(put?.headers.get("X-CSRF-Token")).toBeTruthy();
    expect(put?.headers.get("Idempotency-Key")).toBeTruthy();
    // It reached the hub, and that is the only place it is.
    expect(world.webhooks["ops"]).toBe(URL_SECRET);
    expect(held(app)).not.toContain("TOPSECRETPATH");
    expect(document.body.innerHTML).not.toContain("TOPSECRETPATH");
    expect(screen.queryByDisplayValue(URL_SECRET)).toBeNull();
    expect(JSON.stringify(logged)).not.toContain("TOPSECRETPATH");
    expect(await screen.findByText("Channel Operations saved")).toBeInTheDocument();
    expect(screen.getByText(/masked from now on and can't be shown again/)).toBeInTheDocument();
    const row = await rowIn(await grid(), "Operations");
    expect(within(row).getByText("Webhook set")).toBeInTheDocument();
  });

  it("hides the address as it is typed, and shows it only while the person asks to", async () => {
    const user = userEvent.setup();
    await openAdmin(newWorld(), "/admin/notify");
    await user.click(await screen.findByRole("button", { name: "Add channel" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Add a notification channel" });
    const field = within(dialog).getByLabelText("Webhook URL");
    expect(field).toHaveAttribute("type", "password");
    await paste(user, field, URL_SECRET);
    await user.click(within(dialog).getByRole("button", { name: "Show webhook URL" }));
    expect(field).toHaveAttribute("type", "text");
    await user.click(within(dialog).getByRole("button", { name: "Hide webhook URL" }));
    expect(field).toHaveAttribute("type", "password");
    expect(within(dialog).queryByRole("button", { name: /copy/i })).toBeNull();
  });

  it("empties the field and says so when the hub refuses, and the address is in no cache", async () => {
    const user = userEvent.setup();
    const logged = watchConsole();
    const world = newWorld();
    world.overrides.push({ match: (c) => c.method === "PUT", respond: () => problem(422, "unprocessable"), once: true });
    const app = await openAdmin(world, "/admin/notify");
    await user.click(await screen.findByRole("button", { name: "Add channel" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Add a notification channel" });
    await fillNew(user, dialog);
    await user.click(within(dialog).getByRole("button", { name: "Save channel" }));
    expect(await within(dialog).findByText(/did not accept these values/i)).toBeInTheDocument();
    expect(within(dialog).getByText(/Enter the webhook URL again/i)).toBeInTheDocument();
    expect(within(dialog).getByLabelText("Webhook URL")).toHaveValue("");
    expect(within(dialog).getByLabelText("Name")).toHaveValue("Operations");
    expect(held(app)).not.toContain("TOPSECRETPATH");
    expect(document.body.innerHTML).not.toContain("TOPSECRETPATH");
    expect(JSON.stringify(logged)).not.toContain("TOPSECRETPATH");
  });

  it("is gone with the dialog, so leaving the page keeps nothing", async () => {
    const user = userEvent.setup();
    const app = await openAdmin(newWorld(), "/admin/notify");
    await user.click(await screen.findByRole("button", { name: "Add channel" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Add a notification channel" });
    await paste(user, within(dialog).getByLabelText("Webhook URL"), URL_SECRET);
    app.unmount();
    expect(document.body.innerHTML).not.toContain("TOPSECRETPATH");
    expect(held(app)).not.toContain("TOPSECRETPATH");
  });

  it("never repeats the address in what it says is wrong with it", async () => {
    const user = userEvent.setup();
    const app = await openAdmin(newWorld(), "/admin/notify");
    await user.click(await screen.findByRole("button", { name: "Add channel" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Add a notification channel" });
    await fillNew(user, dialog, "http://corp.webhook.office.com/TOPSECRETPATH");
    await user.click(within(dialog).getByRole("button", { name: "Save channel" }));
    expect(await within(dialog).findByText("A webhook must be an https:// address")).toBeInTheDocument();
    expect(within(dialog).getByLabelText("Webhook URL")).toHaveAttribute("aria-invalid", "true");
    expect(dialog.textContent).not.toContain("TOPSECRETPATH");
    expect(writesOf(app.calls)).toHaveLength(0);
  });
});

describe("the form", () => {
  it("wants an id, a name, an address for a new channel and at least one event, and sends nothing until they are there", async () => {
    const user = userEvent.setup();
    const app = await openAdmin(newWorld(), "/admin/notify");
    await user.click(await screen.findByRole("button", { name: "Add channel" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Add a notification channel" });
    await user.click(within(dialog).getByRole("button", { name: "Save channel" }));
    expect(await within(dialog).findByText("Enter a channel id")).toBeInTheDocument();
    expect(within(dialog).getByText("Enter a name")).toBeInTheDocument();
    expect(within(dialog).getByText("Choose at least one event")).toBeInTheDocument();
    expect(within(dialog).getByText("Enter the webhook URL")).toBeInTheDocument();
    expect(writesOf(app.calls)).toHaveLength(0);
  });

  it("keeps the stored webhook when an existing channel is saved without a new address", async () => {
    const user = userEvent.setup();
    const world = newWorld();
    const app = await openAdmin(world, "/admin/notify");
    await user.click(within(await rowIn(await grid(), "Approvals")).getByRole("button", { name: "Edit channel approvals" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Edit channel approvals" });
    expect(within(dialog).getByLabelText("Channel id")).toHaveAttribute("readonly");
    expect(within(dialog).getByLabelText("Name")).toHaveValue("Approvals");
    expect(within(dialog).getByRole("checkbox", { name: "Proposal waiting for approval" })).toBeChecked();
    expect(within(dialog).getByText(/Leave empty to keep the saved webhook/)).toBeInTheDocument();
    await user.selectOptions(within(dialog).getByLabelText("Minimum severity"), "critical");
    await user.click(within(dialog).getByRole("button", { name: "Save channel" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    const body = JSON.parse(writesOf(app.calls)[0]?.body ?? "{}") as Record<string, unknown>;
    expect(body).not.toHaveProperty("webhook_url");
    expect(body["min_severity"]).toBe("critical");
    expect(world.webhooks["approvals"]).toBeUndefined();
    expect(within(await rowIn(await grid(), "Approvals")).getByText("Webhook set")).toBeInTheDocument();
  });

  it("keeps the dialog and the typed values when the hub is unreachable for a change with no address", async () => {
    const user = userEvent.setup();
    const world = newWorld();
    world.overrides.push({ match: (c) => c.method === "PUT", respond: () => problem(503, "unavailable"), once: true });
    await openAdmin(world, "/admin/notify");
    await user.click(within(await rowIn(await grid(), "Approvals")).getByRole("button", { name: "Edit channel approvals" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Edit channel approvals" });
    await user.click(within(dialog).getByRole("button", { name: "Save channel" }));
    expect(await within(dialog).findByText(/could not complete this change/i)).toBeInTheDocument();
    expect(within(dialog).getByLabelText("Name")).toHaveValue("Approvals");
  });
});

describe("switching a channel on or off", () => {
  it("sends the channel without any address, shows the hub's answer, and goes through the secret path", async () => {
    const user = userEvent.setup();
    const world = newWorld();
    const app = await openAdmin(world, "/admin/notify");
    const toggle = within(await rowIn(await grid(), "Critical findings")).getByRole("switch", { name: "Send Critical findings" });
    await user.click(toggle);
    await waitFor(() => expect(writesOf(app.calls)).toHaveLength(1));
    const body = JSON.parse(writesOf(app.calls)[0]?.body ?? "{}") as Record<string, unknown>;
    expect(writesOf(app.calls)[0]?.path).toBe("/api/v1/admin/notifications/findings");
    expect(body).toEqual({ name: "Critical findings", min_severity: "critical", kinds: ["critical_finding"], enabled: true });
    await waitFor(() => expect(within(screen.getByRole("grid", { name: "Notification channels" })).getByRole("switch", { name: "Send Critical findings" })).toBeChecked());
    expect(app.queryClient.getMutationCache().getAll()).toHaveLength(0);
  });

  it("says so, next to the list, when the hub refuses, and the switch shows what the hub holds", async () => {
    const user = userEvent.setup();
    const world = newWorld();
    world.overrides.push({ match: (c) => c.method === "PUT", respond: () => problem(403, "insufficient-role"), once: true });
    await openAdmin(world, "/admin/notify");
    const toggle = within(await rowIn(await grid(), "Critical findings")).getByRole("switch", { name: "Send Critical findings" });
    await user.click(toggle);
    expect(await screen.findByText(/role may have changed/i)).toBeInTheDocument();
    expect(toggle).not.toBeChecked();
  });
});

describe("deleting a channel", () => {
  it("says the webhook cannot be recovered, and removes the row once the hub has", async () => {
    const user = userEvent.setup();
    const app = await openAdmin(newWorld(), "/admin/notify");
    await user.click(within(await rowIn(await grid(), "Critical findings")).getByRole("button", { name: "Delete channel findings" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Delete channel findings" });
    expect(within(dialog).getByText(/The webhook is forgotten and can't be recovered/)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Delete channel" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(writesOf(app.calls)[0]).toMatchObject({ method: "DELETE", path: "/api/v1/admin/notifications/findings" });
    expect(await screen.findByText("2 channels")).toBeInTheDocument();
  });
});

describe("accessibility and roles", () => {
  it("has no structural violations with the list and the form open", async () => {
    const user = userEvent.setup();
    await openAdmin(newWorld(), "/admin/notify");
    await rowIn(await grid(), "Approvals");
    await expectNoAxeViolations();
    await user.click(await screen.findByRole("button", { name: "Add channel" }));
    await screen.findByRole("alertdialog", { name: "Add a notification channel" });
    await expectNoAxeViolations();
  });

  it("asks the hub for nothing of the Admin area for an Editor", async () => {
    const app = renderApp("/admin/notify", worldApi(newWorld("editor")));
    expect(await screen.findByRole("heading", { level: 1, name: "Admin needs the Admin role" }, FIRST)).toBeInTheDocument();
    expect(app.calls.some((c) => c.path.startsWith("/api/v1/admin"))).toBe(false);
  });
});

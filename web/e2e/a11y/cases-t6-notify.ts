// The states of the notify and restart pieces of T6 (slice B): the Services tab of a swimlane (the "Notify services" button, the
// read-only auto-notify line, the config-server restart banner), the dialogs they open (notify: what will be announced and which
// services wait for it, then the answer; the config-server restart: the Deployment's name typed, then the answer), the file page after
// a write (the "Saved" banner with "Notify services", "Restart services" or "Restart config-server"; the restart-all dialog that
// types the swimlane's id and stops at the first refusal) and the one-service restart dialog of T4, whose confirm button follows the
// gate now. The data is `design-notify-*` (the Services tab and its answers) and `design-notify-file-*` (the file page).
import { expect } from "@playwright/test";
import type { Page } from "@playwright/test";
import type { Case } from "./helpers.ts";
import { hold, modal } from "./helpers.ts";
import {
  API,
  CORE_FILE,
  SERVICES_URL,
  bannerButtons,
  bannerRestart,
  confirmNotify,
  dropped,
  frame,
  goOffline,
  manyFiles,
  notifyButton,
  openConfigServerRestart,
  openNotify,
  patchJson,
  patchLane,
  refuse,
  refuseAfter,
  saveFile,
  svc,
  typeToConfirm,
} from "./t6.ts";

const TAB = "design-notify-services-tab";
const CS_DEPLOYMENT = "csp-configuration-server";

/** The dialog's button that sends (or confirms). */
const send = (page: Page, name: string | RegExp) => modal(page).getByRole("button", { name }).last();

/** Press the button of the file page's banner and wait for its dialog. */
async function openFromBanner(page: Page, name: string | RegExp): Promise<void> {
  await saveFile(page);
  await page.getByRole("button", { name }).first().click();
  await expect(modal(page)).toBeVisible();
}

/** The agent of sitb goes away while the page is open: the hub says so by an event and the page reads the swimlane again. */
async function agentGoesAway(page: Page, emit: (text: string) => void, streams: () => number): Promise<void> {
  await expect.poll(streams, { timeout: 10_000 }).toBeGreaterThan(0);
  await patchLane(page, (lane) => ({ ...lane, agent: "disconnected" }));
  emit(frame(910, { type: "agent_status_changed", swimlane: "sitb", status: "disconnected" }));
}

export const t6NotifyCases: Case[] = [
  // ---- The Services tab: where a notification starts, and what auto-notify says --------------------------------------------------------------
  {
    id: "notify / Services tab: Notify services and the auto-notify line, auto-notify on (an Admin: a link to change it)",
    scenario: TAB,
    url: SERVICES_URL,
    async open({ page }) {
      await expect(notifyButton(page)).toBeEnabled();
      const line = page.getByRole("group", { name: "Auto-notify" });
      await expect(line).toContainText("Auto-notify on");
      await expect(line.getByRole("link", { name: "Change in Admin" })).toBeVisible();
    },
  },
  {
    id: "notify / Services tab: auto-notify off",
    scenario: "design-notify-auto-notify-off",
    url: SERVICES_URL,
    async open({ page }) {
      await expect(page.getByRole("group", { name: "Auto-notify" })).toContainText("Auto-notify off");
    },
  },
  {
    id: "notify / Services tab: a Viewer (the buttons are off and say why, auto-notify is read-only with no link)",
    scenario: "design-notify-viewer",
    url: SERVICES_URL,
    async open({ page }) {
      await expect(notifyButton(page)).toHaveAttribute("aria-disabled", "true");
      await expect(page.getByRole("group", { name: "Auto-notify" })).toContainText("Only an Admin can change this.");
      await expect(page.getByRole("group", { name: "Auto-notify" }).getByRole("link")).toHaveCount(0);
    },
  },
  {
    id: "notify / Services tab: an Operator (auto-notify is read-only, no link)",
    scenario: TAB,
    url: SERVICES_URL,
    async before({ page }) {
      await patchJson(page, API.me, (me) => ({ ...me, role: "operator" }));
    },
    async open({ page }) {
      await expect(notifyButton(page)).toBeEnabled();
      await expect(page.getByRole("group", { name: "Auto-notify" })).toContainText("Only an Admin can change this.");
    },
  },
  {
    id: "notify / Services tab: the tooltip of a button that is off (the reason, in words)",
    scenario: "design-notify-viewer",
    url: SERVICES_URL,
    async open({ page }) {
      await notifyButton(page).hover();
      await expect(page.getByRole("tooltip").first()).toContainText("Operator");
    },
  },
  {
    id: "notify / Services tab: the agent is offline (the button is off: Unavailable while the agent is offline)",
    scenario: TAB,
    url: SERVICES_URL,
    async before({ page }) {
      await patchLane(page, (lane) => ({ ...lane, agent: "disconnected" }));
    },
    async open({ page }) {
      await expect(notifyButton(page)).toHaveAttribute("aria-disabled", "true");
      await expect(notifyButton(page)).toHaveAccessibleDescription("Unavailable while the agent is offline");
    },
  },
  {
    id: "notify / Services tab: live updates are paused (the buttons are off: Unavailable offline)",
    scenario: TAB,
    url: SERVICES_URL,
    async open(ctx) {
      await goOffline(ctx);
      await expect(notifyButton(ctx.page)).toHaveAttribute("aria-disabled", "true");
      await expect(notifyButton(ctx.page)).toHaveAccessibleDescription("Unavailable offline");
    },
  },
  {
    id: "notify / Services tab: no service waits on a file (nothing to announce: the button is off)",
    scenario: TAB,
    url: SERVICES_URL,
    async before({ page }) {
      await patchLane(page, (lane) => ({ ...lane, services: [svc("tx-infinity-core"), svc("ui-service")], pickup_counts: [] }));
    },
    async open({ page }) {
      await expect(notifyButton(page)).toHaveAccessibleDescription("No service is waiting on a file.");
    },
  },

  // ---- Notify services: the dialog and every answer -----------------------------------------------------------------------------------------
  {
    id: "notify dialog / confirm: the files to announce and the services waiting on them",
    scenario: TAB,
    url: SERVICES_URL,
    async open({ page }) {
      await openNotify(page);
      await expect(modal(page).getByText("2 files to announce")).toBeVisible();
      await expect(modal(page).getByText("2 services waiting on these files")).toBeVisible();
      await expect(send(page, "Notify services")).toBeEnabled();
    },
  },
  {
    id: "notify dialog / confirm: more files wait than one request holds (150 of them, 100 in the request)",
    scenario: TAB,
    url: SERVICES_URL,
    async before({ page }) {
      await patchLane(page, (lane) => ({ ...lane, services: [svc("tx-infinity-core", { pickup: "needs_notify_or_restart", pending_since: 1, pending_files: manyFiles(150) }), svc("ui-service")] }));
    },
    async open({ page }) {
      await openNotify(page);
      await expect(modal(page).getByText(/50 more files are waiting/)).toBeVisible();
    },
  },
  {
    id: "notify dialog / confirm: a person whose changes need approval (it goes to an Admin as a request)",
    scenario: TAB,
    url: SERVICES_URL,
    async before({ page }) {
      await patchJson(page, API.me, (me) => ({ ...me, requires_approval: true }));
    },
    async open({ page }) {
      await openNotify(page);
      await expect(modal(page).getByText(/Your changes need approval/)).toBeVisible();
    },
  },
  {
    id: "notify dialog / working (the request is out, never optimistic)",
    scenario: TAB,
    url: SERVICES_URL,
    async before({ page }) {
      await hold(page, API.notify);
    },
    async open({ page }) {
      await openNotify(page);
      await confirmNotify(page);
      await expect(modal(page).getByRole("button", { name: /Notifying/ })).toBeVisible();
    },
  },
  {
    id: "notify dialog / sent (the hub accepted the paths, and the audit event)",
    scenario: TAB,
    url: SERVICES_URL,
    async open({ page }) {
      await openNotify(page);
      await confirmNotify(page);
      await expect(modal(page).getByText("Notification sent")).toBeVisible();
      await expect(modal(page).getByText("Audit event")).toBeVisible();
    },
  },
  {
    id: "notify dialog / partly accepted (a count below what was sent; the answer does not say which)",
    scenario: "design-notify-partial",
    url: SERVICES_URL,
    async open({ page }) {
      await openNotify(page);
      await confirmNotify(page);
      await expect(modal(page).getByText("Notification partly accepted")).toBeVisible();
      await expect(modal(page).getByText(/does not say which paths it left out/)).toBeVisible();
    },
  },
  {
    id: "notify dialog / sent for approval (nothing is sent to the config-server)",
    scenario: "design-notify-approval",
    url: SERVICES_URL,
    async open({ page }) {
      await openNotify(page);
      await confirmNotify(page);
      await expect(modal(page).getByText("Notification sent for approval")).toBeVisible();
    },
  },
  {
    id: "notify dialog / refused: locked (423)",
    scenario: "design-notify-locked",
    url: SERVICES_URL,
    async open({ page }) {
      await openNotify(page);
      await confirmNotify(page);
      await expect(modal(page).getByText(/is locked/)).toBeVisible();
    },
  },
  {
    id: "notify dialog / refused: the server did not accept it (422)",
    scenario: TAB,
    url: SERVICES_URL,
    async before({ page }) {
      await refuse(page, API.notify, 422, "Unprocessable");
    },
    async open({ page }) {
      await openNotify(page);
      await confirmNotify(page);
      await expect(modal(page).getByText(/did not accept this notification/)).toBeVisible();
      await expect(modal(page).getByText("req-a11y-0001")).toBeVisible();
    },
  },
  {
    id: "notify dialog / refused: too many requests (429, wait 30 seconds)",
    scenario: TAB,
    url: SERVICES_URL,
    async before({ page }) {
      await refuse(page, API.notify, 429, "Too many requests", { "retry-after": "30" });
    },
    async open({ page }) {
      await openNotify(page);
      await confirmNotify(page);
      await expect(modal(page).getByText(/Wait 30 seconds/)).toBeVisible();
    },
  },
  {
    id: "notify dialog / the server failed (5xx: it may or may not have been sent)",
    scenario: TAB,
    url: SERVICES_URL,
    async before({ page }) {
      await refuse(page, API.notify, 503, "Service unavailable");
    },
    async open({ page }) {
      await openNotify(page);
      await confirmNotify(page);
      await expect(modal(page).getByText(/may or may not have been sent/)).toBeVisible();
    },
  },
  {
    id: "notify dialog / no answer (the network failed: it may or may not have been sent)",
    scenario: TAB,
    url: SERVICES_URL,
    async before({ page }) {
      await dropped(page, API.notify);
    },
    async open({ page }) {
      await openNotify(page);
      await confirmNotify(page);
      await expect(modal(page).getByText(/Couldn't reach the server/)).toBeVisible({ timeout: 20_000 });
    },
  },
  {
    id: "notify dialog / live updates pause while it is open (the button is off and says why)",
    scenario: TAB,
    url: SERVICES_URL,
    async open(ctx) {
      await openNotify(ctx.page);
      await goOffline(ctx);
      await expect(send(ctx.page, "Notify services")).toHaveAttribute("aria-disabled", "true");
      await expect(modal(ctx.page)).toContainText("Unavailable offline");
    },
  },
  {
    id: "notify dialog / the agent goes away while it is open (the button is off and says why)",
    scenario: TAB,
    url: SERVICES_URL,
    async open({ page, harness }) {
      await openNotify(page);
      await agentGoesAway(page, (text) => { harness.emit(text); }, () => harness.streamCount());
      await expect(modal(page)).toContainText("Unavailable while the agent is offline", { timeout: 15_000 });
      await expect(send(page, "Notify services")).toHaveAttribute("aria-disabled", "true");
    },
  },

  // ---- The config-server restart: the banner, the dialog and its answers -------------------------------------------------------------------
  {
    id: "config-server / the banner on the Services tab (check C11 is open)",
    scenario: "design-notify-config-server",
    url: SERVICES_URL,
    async open({ page }) {
      await expect(page.getByText("Config-server restart needed")).toBeVisible();
      await expect(page.getByText(/Check C11 is open/)).toBeVisible();
      await expect(bannerRestart(page)).toBeEnabled();
    },
  },
  {
    id: "config-server / the banner for a Viewer (the button is off and says which role it needs)",
    scenario: "design-notify-viewer",
    url: SERVICES_URL,
    async before({ page }) {
      await patchLane(page, (lane) => ({ ...lane, services: [svc("tx-infinity-core", { pickup: "needs_config_server_restart", pending_since: 1, pending_files: ["channels.yml"] }), svc("ui-service")] }));
    },
    async open({ page }) {
      await expect(bannerRestart(page)).toHaveAttribute("aria-disabled", "true");
      await expect(bannerRestart(page)).toHaveAccessibleDescription(/Operator/);
    },
  },
  {
    id: "config-server / the banner when only a service says it waits (check C11 is not open)",
    scenario: "design-notify-config-server",
    url: SERVICES_URL,
    async before({ page }) {
      await patchJson(page, API.findings, (list) => ({ ...list, items: [] }));
    },
    async open({ page }) {
      await expect(page.getByText(/services report needing a config-server restart/)).toBeVisible();
      await expect(page.getByText(/Check C11 is open/)).toHaveCount(0);
    },
  },
  {
    id: "config-server dialog / confirm: the services it takes away, and the Deployment's name to type",
    scenario: "design-notify-config-server",
    url: SERVICES_URL,
    async open({ page }) {
      await openConfigServerRestart(page);
      await expect(modal(page).getByText(`Restart ${CS_DEPLOYMENT} in sitb?`)).toBeVisible();
      await expect(modal(page).getByText("4 other services affected")).toBeVisible();
      await expect(send(page, `Restart ${CS_DEPLOYMENT}`)).toHaveAttribute("aria-disabled", "true");
    },
  },
  {
    id: "config-server dialog / confirm: the name typed (the button is on)",
    scenario: "design-notify-config-server",
    url: SERVICES_URL,
    async open({ page }) {
      await openConfigServerRestart(page);
      await typeToConfirm(page, CS_DEPLOYMENT);
      await expect(send(page, `Restart ${CS_DEPLOYMENT}`)).not.toHaveAttribute("aria-disabled", "true");
    },
  },
  {
    id: "config-server dialog / the list of affected services is not the whole of them (truncated)",
    scenario: "design-notify-config-server-truncated",
    url: SERVICES_URL,
    async open({ page }) {
      await openConfigServerRestart(page);
      await expect(modal(page).getByText(/not the whole list/).first()).toBeVisible();
    },
  },
  {
    id: "config-server dialog / the hub offers no restart (it does not guess)",
    scenario: "design-notify-config-server-none",
    url: SERVICES_URL,
    async open({ page }) {
      await openConfigServerRestart(page);
      await expect(modal(page).getByText("No config-server restart is offered")).toBeVisible();
    },
  },
  {
    id: "config-server dialog / reading the offer (held)",
    scenario: "design-notify-config-server",
    url: SERVICES_URL,
    async before({ page }) {
      await hold(page, API.configServerOffer);
    },
    async open({ page }) {
      await openConfigServerRestart(page);
      await expect(modal(page).getByText("Reading what the restart would take away.")).toBeVisible();
    },
  },
  {
    id: "config-server dialog / the offer could not be read (500: Retry)",
    scenario: "design-notify-config-server",
    url: SERVICES_URL,
    async before({ page }) {
      await refuse(page, API.configServerOffer, 500, "Internal error");
    },
    async open({ page }) {
      await openConfigServerRestart(page);
      await expect(modal(page).getByText(/Couldn't read the config-server restart offer/)).toBeVisible();
      await expect(modal(page).getByRole("button", { name: "Retry" })).toBeVisible();
    },
  },
  {
    id: "config-server dialog / the offer is refused (403: it needs the Operator role)",
    scenario: "design-notify-config-server",
    url: SERVICES_URL,
    async before({ page }) {
      await refuse(page, API.configServerOffer, 403, "Forbidden");
    },
    async open({ page }) {
      await openConfigServerRestart(page);
      await expect(modal(page).getByText(/Reading it needs the Operator role/)).toBeVisible();
    },
  },
  {
    id: "config-server dialog / accepted (no progress is shown, and the audit event)",
    scenario: "design-notify-config-server",
    url: SERVICES_URL,
    async open({ page }) {
      await openConfigServerRestart(page);
      await typeToConfirm(page, CS_DEPLOYMENT);
      await send(page, `Restart ${CS_DEPLOYMENT}`).click();
      await expect(modal(page).getByText("Restart requested")).toBeVisible();
      await expect(modal(page).getByText("Audit event")).toBeVisible();
    },
  },
  {
    id: "config-server dialog / sent for approval",
    scenario: "design-notify-config-server-approval",
    url: SERVICES_URL,
    async open({ page }) {
      await openConfigServerRestart(page);
      await typeToConfirm(page, CS_DEPLOYMENT);
      await send(page, `Restart ${CS_DEPLOYMENT}`).click();
      await expect(modal(page).getByText("Config-server restart sent for approval")).toBeVisible();
    },
  },
  {
    id: "config-server dialog / refused: locked (423)",
    scenario: "design-notify-config-server",
    url: SERVICES_URL,
    async before({ page }) {
      await refuse(page, API.restart, 423, "Locked");
    },
    async open({ page }) {
      await openConfigServerRestart(page);
      await typeToConfirm(page, CS_DEPLOYMENT);
      await send(page, `Restart ${CS_DEPLOYMENT}`).click();
      await expect(modal(page).getByText(/locked/i).first()).toBeVisible();
    },
  },
  {
    id: "config-server dialog / the server failed (5xx)",
    scenario: "design-notify-config-server",
    url: SERVICES_URL,
    async before({ page }) {
      await refuse(page, API.restart, 503, "Service unavailable");
    },
    async open({ page }) {
      await openConfigServerRestart(page);
      await typeToConfirm(page, CS_DEPLOYMENT);
      await send(page, `Restart ${CS_DEPLOYMENT}`).click();
      await expect(modal(page).getByText("req-a11y-0001")).toBeVisible();
    },
  },
  {
    id: "config-server dialog / live updates pause while it is open, with the name typed (the button is off and says why)",
    scenario: "design-notify-config-server",
    url: SERVICES_URL,
    async open(ctx) {
      await openConfigServerRestart(ctx.page);
      await typeToConfirm(ctx.page, CS_DEPLOYMENT);
      await goOffline(ctx);
      await expect(send(ctx.page, `Restart ${CS_DEPLOYMENT}`)).toHaveAttribute("aria-disabled", "true");
      await expect(modal(ctx.page)).toContainText("Unavailable offline");
    },
  },

  // ---- T4's one-service restart dialog: its confirm button follows the gate ----------------------------------------------------------------
  {
    id: "restart / live updates pause while the dialog is open (the confirm button is off and says why)",
    scenario: "design-swimlanes-six",
    url: SERVICES_URL,
    async open(ctx) {
      await ctx.page.getByRole("button", { name: "Restart tx-infinity-core" }).click();
      await expect(ctx.page.getByRole("alertdialog", { name: /Restart tx-infinity-core in sitb\?/ })).toBeVisible();
      await goOffline(ctx);
      await expect(send(ctx.page, "Restart tx-infinity-core")).toHaveAttribute("aria-disabled", "true");
      await expect(modal(ctx.page)).toContainText("Unavailable offline");
    },
  },

  // ---- The file page after a write: the banner and what it offers ---------------------------------------------------------------------------
  {
    id: "file saved / the banner offers Notify services and Restart services (the file waits for a notify or a restart)",
    scenario: "design-notify-file-saved",
    url: CORE_FILE,
    async open({ page }) {
      await saveFile(page);
      await expect(bannerButtons(page)).toHaveCount(2);
    },
  },
  {
    id: "file saved / the banner offers nothing (the file is Live: there is nothing to do)",
    scenario: "design-notify-file-live",
    url: CORE_FILE,
    async open({ page }) {
      await saveFile(page);
      await expect(bannerButtons(page)).toHaveCount(0);
    },
  },
  {
    id: "file saved / the banner offers Restart config-server (the file waits for the config-server)",
    scenario: "design-notify-file-config-server",
    url: CORE_FILE,
    async open({ page }) {
      await saveFile(page);
      await expect(bannerRestart(page)).toBeVisible();
      await expect(bannerButtons(page)).toHaveCount(1);
    },
  },
  {
    id: "file saved / Notify services for this file: confirm",
    scenario: "design-notify-file-saved",
    url: CORE_FILE,
    async open({ page }) {
      await openFromBanner(page, "Notify services");
      await expect(modal(page).getByText("1 file to announce")).toBeVisible();
    },
  },
  {
    id: "file saved / Notify services for this file: sent",
    scenario: "design-notify-file-saved",
    url: CORE_FILE,
    async open({ page }) {
      await openFromBanner(page, "Notify services");
      await confirmNotify(page);
      await expect(modal(page).getByText("Notification sent")).toBeVisible();
    },
  },
  {
    id: "file saved / Notify services for this file: refused, locked (423)",
    scenario: "design-notify-file-locked",
    url: CORE_FILE,
    async open({ page }) {
      await openFromBanner(page, "Notify services");
      await confirmNotify(page);
      await expect(modal(page).getByText(/is locked/)).toBeVisible();
    },
  },
  {
    id: "file saved / Restart config-server for this file: the name typed",
    scenario: "design-notify-file-config-server",
    url: CORE_FILE,
    async open({ page }) {
      await saveFile(page);
      await openConfigServerRestart(page);
      await typeToConfirm(page, CS_DEPLOYMENT);
      await expect(send(page, `Restart ${CS_DEPLOYMENT}`)).not.toHaveAttribute("aria-disabled", "true");
    },
  },

  // ---- Restart services (restart all): the dialog types the swimlane's id and stops at the first refusal -------------------------------
  {
    id: "restart all / confirm: the services the hub lists, and the swimlane's id to type",
    scenario: "design-notify-file-saved",
    url: CORE_FILE,
    async open({ page }) {
      await openFromBanner(page, "Restart services");
      await expect(modal(page).getByText("Restart 2 services in sitb?")).toBeVisible();
      await expect(modal(page).getByText("2 services to restart")).toBeVisible();
      await expect(send(page, "Restart 2 services")).toHaveAttribute("aria-disabled", "true");
    },
  },
  {
    id: "restart all / confirm: the id typed (the button is on)",
    scenario: "design-notify-file-saved",
    url: CORE_FILE,
    async open({ page }) {
      await openFromBanner(page, "Restart services");
      await typeToConfirm(page, "sitb");
      await expect(send(page, "Restart 2 services")).not.toHaveAttribute("aria-disabled", "true");
    },
  },
  {
    id: "restart all / the list is not the whole of the swimlane's services (truncated)",
    scenario: "design-notify-file-saved",
    url: CORE_FILE,
    async before({ page }) {
      await patchJson(page, API.restartOffer, (offer) => ({ ...offer, truncated: true }));
    },
    async open({ page }) {
      await openFromBanner(page, "Restart services");
      await expect(modal(page).getByText(/not the whole list/).first()).toBeVisible();
    },
  },
  {
    id: "restart all / the hub lists no service to restart",
    scenario: "design-notify-file-saved",
    url: CORE_FILE,
    async before({ page }) {
      await patchJson(page, API.restartOffer, (offer) => ({ ...offer, services: [] }));
    },
    async open({ page }) {
      await openFromBanner(page, "Restart services");
      await expect(modal(page).getByText("No services to restart")).toBeVisible();
    },
  },
  {
    id: "restart all / reading the offer (held)",
    scenario: "design-notify-file-saved",
    url: CORE_FILE,
    async before({ page }) {
      await hold(page, API.restartOffer);
    },
    async open({ page }) {
      await saveFile(page);
      await page.getByRole("button", { name: "Restart services" }).click();
      await expect(modal(page).getByText("Reading which services read this file.")).toBeVisible();
    },
  },
  {
    id: "restart all / the offer could not be read (500: Retry)",
    scenario: "design-notify-file-saved",
    url: CORE_FILE,
    async before({ page }) {
      await refuse(page, API.restartOffer, 500, "Internal error");
    },
    async open({ page }) {
      await openFromBanner(page, "Restart services");
      await expect(modal(page).getByText(/Couldn't read the services to restart/)).toBeVisible();
      await expect(modal(page).getByRole("button", { name: "Retry" })).toBeVisible();
    },
  },
  {
    id: "restart all / the first request is out (one at a time: Request 1 of 2 sent)",
    scenario: "design-notify-file-saved",
    url: CORE_FILE,
    async before({ page }) {
      await hold(page, API.restart);
    },
    async open({ page }) {
      await openFromBanner(page, "Restart services");
      await typeToConfirm(page, "sitb");
      await send(page, "Restart 2 services").click();
      await expect(modal(page).getByText("Request 1 of 2 sent.")).toBeVisible();
    },
  },
  {
    id: "restart all / both requests accepted (no progress is shown)",
    scenario: "design-notify-file-saved",
    url: CORE_FILE,
    async open({ page }) {
      await openFromBanner(page, "Restart services");
      await typeToConfirm(page, "sitb");
      await send(page, "Restart 2 services").click();
      await expect(modal(page).getByText("Restart requested for 2 services")).toBeVisible();
    },
  },
  {
    id: "restart all / stopped at the first refusal (the second is locked, and says which was not sent)",
    scenario: "design-notify-file-saved",
    url: CORE_FILE,
    async before({ page }) {
      await refuseAfter(page, API.restart, 1, 423, "Locked");
    },
    async open({ page }) {
      await openFromBanner(page, "Restart services");
      await typeToConfirm(page, "sitb");
      await send(page, "Restart 2 services").click();
      await expect(modal(page).getByText("Restart stopped early")).toBeVisible();
      await expect(modal(page).getByText("Nothing was sent again.")).toBeVisible();
    },
  },
  {
    id: "restart all / stopped at the first request (it is refused: none was sent after it)",
    scenario: "design-notify-file-saved",
    url: CORE_FILE,
    async before({ page }) {
      await refuse(page, API.restart, 503, "Service unavailable");
    },
    async open({ page }) {
      await openFromBanner(page, "Restart services");
      await typeToConfirm(page, "sitb");
      await send(page, "Restart 2 services").click();
      await expect(modal(page).getByText("Restart stopped early")).toBeVisible();
    },
  },
  {
    id: "restart all / both requests sent for approval (nothing restarts, and an Admin can open the requests)",
    scenario: "design-notify-file-saved",
    url: CORE_FILE,
    async before({ page }) {
      await page.route(API.restart, (route) => route.fulfill({ status: 200, json: { outcome: "proposal_created", id: 2050 } }));
    },
    async open({ page }) {
      await openFromBanner(page, "Restart services");
      await typeToConfirm(page, "sitb");
      await send(page, "Restart 2 services").click();
      await expect(modal(page).getByText("Restart sent for approval")).toBeVisible();
      await expect(modal(page).getByRole("link", { name: "View requests" })).toBeVisible();
    },
  },
  {
    id: "restart all / live updates pause while it is open, with the id typed (the button is off and says why)",
    scenario: "design-notify-file-saved",
    url: CORE_FILE,
    async open(ctx) {
      await openFromBanner(ctx.page, "Restart services");
      await typeToConfirm(ctx.page, "sitb");
      await goOffline(ctx);
      await expect(send(ctx.page, "Restart 2 services")).toHaveAttribute("aria-disabled", "true");
      await expect(modal(ctx.page)).toContainText("Unavailable offline");
    },
  },
];

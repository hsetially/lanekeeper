// The frames of notify and restart (T6, slice B): telling the config-server which files changed (Notify services), what the swimlane does by
// itself (auto-notify, a line that is read-only), the config-server restart that finding C11 asks for, and the restart of the services that read
// a file that was just written (restart all). Each screen idle, and each dialog open, in every state that the States index lists for a write:
// loading, an offer that cannot be read (403, a server error), none offered, a list that is not the whole of it, below Operator, live updates
// paused, the agent offline, locked (423), refused (422), too many requests (429), a server error, no answer, sent for approval, partly accepted.
//
// None of them is a comparison with the design: the handoff has no frame for any of these (plan W6, handoff gap), so each frame is the app
// against its own baseline, which CI generates; the rows of design/DEVIATIONS.md that make it so are named in each frame's `why`. The
// scenarios are the notify slice's (`design-notify-*`, and the four `design-notify-file-*` of the file page after a save); a state a scenario
// has no variant for is made by changing the one answer that carries it, in the browser.
//
// What is never done here, because the page does not do it: nothing is optimistic (a dialog says what the server answered and nothing more),
// nothing is sent again by itself, and a type-to-confirm dialog is never confirmed by Enter (typeName presses nothing).
import type { Locator, Page } from "@playwright/test";
import type { ServiceState } from "../../../src/api/generated/model/index.ts";
import { expect } from "../../fixtures/test";
import { expectPaused, pauseStream, pausedMasks } from "../search/steps";
import { frame, pair, then, type Frame, type Step } from "../pickup/runner";
import {
  CONFIG_SERVER,
  CONFIG_SERVER_OFFER,
  FILE,
  NOTIFY,
  RESTART,
  RESTART_OFFER,
  TAB,
  agentOffline,
  as,
  asking,
  bannerConfigServer,
  bannerNotify,
  bannerRestart,
  configServerBanner,
  configServerButton,
  confirmConfigServer,
  confirmIn,
  confirmRestartFile,
  confirmRestartService,
  cutOff,
  dialog,
  held,
  hover,
  notifyButton,
  offerIs,
  openConfigServer,
  openNotify,
  openNotifyFromBanner,
  press,
  refuse,
  restartRow,
  saveCoreFile,
  saved,
  servicesAre,
  streamDown,
  tipSays,
  typeName,
  unreadable,
} from "./steps";

const SERVICES_WHY = "the Services tab has a Notify services button, an auto-notify line and a config-server banner that the design does not draw (design/DEVIATIONS.md: \"Swimlane detail: Services tab\", \"Notify services: where it starts\", \"Swimlane detail: auto-notify line\")";
const NOTIFY_WHY = "the notify dialog is new and has no handoff frame; its result is a count and an audit event, with no per-service result (design/DEVIATIONS.md: \"Notify services: dialog\", \"Notify services: where it starts\")";
const CONFIG_WHY = "the config-server restart is new and has no handoff frame: the hub's offer as it is, and the Deployment's name typed (design/DEVIATIONS.md: \"Swimlane detail: config-server restart\", \"Swimlane detail: Services tab\")";
const FILE_WHY = "the file page's banner offers what the file's pickup state calls for, and its dialogs are new (design/DEVIATIONS.md: \"Notify services: where it starts\", \"Notify services: dialog\", \"File: restart services\")";
const RESTART_ALL_WHY = "restart services lists the hub's offer and restarts one request at a time (design/DEVIATIONS.md: \"File: restart services\", \"Notify services: where it starts\")";
const GATE_WHY = "a button that a person cannot press is shown disabled with its reason, and a dialog that is open follows the same gate (design/DEVIATIONS.md: \"Notify services: where it starts\", \"Swimlane detail: restart dialog\")";
const RESTART_WHY = "the restart dialog says \"accepted\" and shows no progress, and its confirm button follows the gate while it is open (design/DEVIATIONS.md: \"Swimlane detail: restart dialog\", \"Swimlane detail: Services tab\")";

const NARROW = { width: 1024, height: 768 } as const;

// ----- reads -----------------------------------------------------------------------------------------------------------------------

const alert = (dialogOf: (page: Page) => Locator, ...words: Array<string | RegExp>): Step => async ({ page }) => {
  const within = dialogOf(page);
  await expect(within.getByRole("alert")).toBeVisible();
  for (const word of words) await expect(within).toContainText(word);
};

/** A result dialog says what happened in its description, and nothing it was not told. */
const result = (title: string, ...words: Array<string | RegExp>): Step => async ({ page }) => {
  const done = dialog(page, title);
  await expect(done).toBeVisible();
  for (const word of words) await expect(done).toContainText(word);
};

/**
 * The warning that other people will be interrupted is one sentence on one line (or two) in its box. An Alert lays its children out as a
 * column, so a sentence passed as bare children, with the swimlane's name in an element in the middle of it, was three lines of its own.
 */
const interruptsOthers = (dialogOf: (page: Page) => Locator): Step => async ({ page }) => {
  const warning = dialogOf(page).locator("[data-slot='alert']").filter({ hasText: "Other people testing in" });
  await expect(warning).toBeVisible();
  const box = await warning.boundingBox();
  expect(box, "the warning's box").not.toBeNull();
  // One line of text is about 20 px in a box of about 36: three lines would be about 76.
  expect(box?.height ?? 0, "the warning is one sentence, not three lines").toBeLessThan(56);
};

/** The tab is idle: the strip, the auto-notify line, and what the hub reports for each service. */
const tabIdle = (auto: "on" | "off", bannerShown: boolean): Step => async ({ page }) => {
  await expect(notifyButton(page)).toBeVisible();
  await expect(page.getByRole("group", { name: "Auto-notify" })).toContainText(auto === "on" ? "Auto-notify on" : "Auto-notify off");
  if (bannerShown) await expect(configServerBanner(page)).toBeVisible();
  else await expect(configServerBanner(page)).toHaveCount(0);
};

/** The notify dialog names the files it will announce and the services that wait on them, with each agent's notification setting as it reported it. */
const notifyAsks: Step = async ({ page }) => {
  const ask = asking(page);
  await expect(ask).toContainText("Tell the config-server that these 2 files changed in sitb.");
  await expect(ask.getByRole("region", { name: "2 files to announce" })).toContainText("application.properties");
  await expect(ask.getByRole("region", { name: /^2 services waiting on these files/ })).toContainText("notifications reported off");
  await expect(ask.getByRole("button", { name: "Notify services" })).not.toHaveAttribute("aria-disabled", "true");
};

// ----- the Services tab: idle and with the dialog open ------------------------------------------------------------------------------

const tabScenarios: Frame[] = [
  // Notify services: the ordinary case, and what comes of confirming it.
  ...pair("N01", { scenario: "design-notify-services-tab", path: TAB, title: "Services tab: Notify services in the strip, auto-notify on, two services waiting", why: SERVICES_WHY, check: tabIdle("on", false) }),
  ...pair("N02", { scenario: "design-notify-services-tab", path: TAB, title: "Notify dialog open: the files to announce, the services waiting and the audit note; nothing sent", why: NOTIFY_WHY, steps: openNotify, check: notifyAsks }),
  ...pair("N03", { scenario: "design-notify-services-tab", path: TAB, title: "Notified: the count of paths the hub accepted and the audit event, and that it does not say which services picked it up", why: NOTIFY_WHY, steps: then(openNotify, confirmIn(asking, "Notify services")), check: result("Notification sent", "The hub accepted 2 paths for sitb.", "Audit event", "does not say which services") }),
  ...pair("N04", { scenario: "design-notify-auto-notify-off", path: TAB, title: "Services tab: auto-notify off says nobody is notified automatically", why: SERVICES_WHY, check: tabIdle("off", false) }),
  ...pair("N05", { scenario: "design-notify-auto-notify-off", path: TAB, title: "Notify dialog with auto-notify off", why: NOTIFY_WHY, steps: openNotify, check: notifyAsks }),
  // A Viewer: the buttons are on screen and disabled, with the reason in a tooltip, and the line has no link.
  ...pair("N06", {
    scenario: "design-notify-viewer",
    path: TAB,
    title: "Viewer: Notify services and every Restart are disabled, and auto-notify is read-only with no link",
    why: GATE_WHY,
    check: async ({ page }) => {
      await expect(notifyButton(page)).toHaveAttribute("aria-disabled", "true");
      await expect(restartRow(page, "tx-infinity-core")).toHaveAttribute("aria-disabled", "true");
      await expect(page.getByRole("group", { name: "Auto-notify" }).getByRole("link")).toHaveCount(0);
      await expect(page.getByRole("group", { name: "Auto-notify" })).toContainText("Only an Admin can change this.");
    },
  }),
  ...pair("N07", { scenario: "design-notify-viewer", path: TAB, hold: true, title: "Viewer: the tooltip of Notify services says it needs the Operator role", why: GATE_WHY, steps: then(hover(notifyButton), tipSays("Needs the Operator role. Ask an Admin to grant it.")) }),
  ...pair("N08", { scenario: "design-notify-viewer", path: TAB, hold: true, title: "Viewer: the tooltip of a Restart says the same", why: GATE_WHY, steps: then(hover((page) => restartRow(page, "ui-service")), tipSays("Needs the Operator role")) }),
  // Sent for approval: nothing is sent to the config-server.
  ...pair("N09", { scenario: "design-notify-approval", path: TAB, title: "Services tab, a swimlane whose notifications are answered with a request", why: SERVICES_WHY, check: tabIdle("on", false) }),
  ...pair("N10", { scenario: "design-notify-approval", path: TAB, title: "Notify dialog before it is confirmed", why: NOTIFY_WHY, steps: openNotify, check: notifyAsks }),
  ...pair("N11", { scenario: "design-notify-approval", path: TAB, title: "Sent for approval: nothing is sent to the config-server until an Admin approves, and the request is named", why: NOTIFY_WHY, steps: then(openNotify, confirmIn(asking, "Notify services")), check: result("Notification sent for approval", "Nothing is sent to the config-server until an Admin approves request AP-2043", "View request") }),
  // Partly accepted: the hub's count is lower than what was sent and it does not say which.
  ...pair("N12", { scenario: "design-notify-partial", path: TAB, title: "Services tab, a swimlane whose notifications are partly accepted", why: SERVICES_WHY, check: tabIdle("on", false) }),
  ...pair("N13", { scenario: "design-notify-partial", path: TAB, title: "Notify dialog before it is confirmed", why: NOTIFY_WHY, steps: openNotify, check: notifyAsks }),
  ...pair("N14", { scenario: "design-notify-partial", path: TAB, title: "Partly accepted: 1 of 2 paths, and \"Its answer does not say which paths it left out\"; nothing is sent again", why: NOTIFY_WHY, steps: then(openNotify, confirmIn(asking, "Notify services")), check: result("Notification partly accepted", "The hub accepted 1 of 2 paths for sitb.", "does not say which paths it left out", "Nothing was sent again") }),
  // Locked: refused inside the dialog, with the request id.
  ...pair("N15", { scenario: "design-notify-locked", path: TAB, title: "Services tab, a locked swimlane", why: SERVICES_WHY, check: tabIdle("on", false) }),
  ...pair("N16", { scenario: "design-notify-locked", path: TAB, title: "Notify dialog before it is confirmed", why: NOTIFY_WHY, steps: openNotify, check: notifyAsks }),
  ...pair("N17", { scenario: "design-notify-locked", path: TAB, title: "423: the dialog stays, says nothing was sent, and gives the request id", why: NOTIFY_WHY, steps: then(openNotify, confirmIn(asking, "Notify services")), check: alert(asking, "sitb is locked", "Nothing was sent", "req-mock-0001") }),
];

// ----- the config-server restart ----------------------------------------------------------------------------------------------------

const confirmsConfigServer: Step = then(async ({ page }) => {
  const ask = confirmConfigServer(page);
  await expect(ask.getByRole("region", { name: /^4 other services affected$/ })).toContainText("sitb/tx-infinity-core");
  await expect(ask).toContainText("Other people testing in sitb will be interrupted.");
  // Nothing is confirmed until the Deployment's name is typed, and typing it does not confirm.
  await expect(ask.getByRole("button", { name: `Restart ${CONFIG_SERVER}` })).toHaveAttribute("aria-disabled", "true");
}, interruptsOthers(confirmConfigServer));

const configScenarios: Frame[] = [
  ...pair("N20", {
    scenario: "design-notify-config-server",
    path: TAB,
    title: "Services tab: the warning that a config-server restart is needed (C11 is open, a service waits for it) and its button",
    why: CONFIG_WHY,
    check: async ({ page }) => {
      await expect(configServerBanner(page)).toContainText("Check C11 is open");
      await expect(configServerBanner(page)).toContainText("2 services report needing a config-server restart.");
      await expect(configServerButton(page)).toBeEnabled();
    },
  }),
  ...pair("N21", { scenario: "design-notify-config-server", path: TAB, title: "Restart dialog: the Deployment the hub offers, the four services that cannot be served meanwhile, and the name to type", why: CONFIG_WHY, steps: openConfigServer, check: confirmsConfigServer }),
  ...pair("N22", { scenario: "design-notify-config-server", path: TAB, title: "The name is typed and the button is on: Enter would not confirm it", why: CONFIG_WHY, steps: then(openConfigServer, typeName(confirmConfigServer, CONFIG_SERVER)) }),
  ...pair("N23", {
    scenario: "design-notify-config-server",
    path: TAB,
    title: "Accepted: the hub accepted the request, and shows no progress (a service has no restarting state)",
    why: CONFIG_WHY,
    steps: then(openConfigServer, typeName(confirmConfigServer, CONFIG_SERVER), confirmIn(confirmConfigServer, `Restart ${CONFIG_SERVER}`)),
    check: result("Restart requested", `The hub accepted the restart of ${CONFIG_SERVER} in sitb.`, "reports no restart progress", "Audit event"),
  }),
  ...pair("N24", { scenario: "design-notify-config-server-truncated", path: TAB, title: "A truncated offer: the list is marked \"(not the whole list)\" and a warning says the others are not named", why: CONFIG_WHY, steps: openConfigServer, check: async ({ page }) => {
    await expect(confirmConfigServer(page).getByRole("region", { name: "4 other services affected (not the whole list)" })).toBeVisible();
    await expect(confirmConfigServer(page)).toContainText("not the whole of the services affected");
  } }),
  ...pair("N25", { scenario: "design-notify-config-server-none", path: TAB, title: "The hub offers no restart: it says so, and that it does not say why, and offers nothing to confirm", why: CONFIG_WHY, steps: press(configServerButton, (page) => dialog(page, "No config-server restart is offered")), check: async ({ page }) => {
    await expect(dialog(page, "No config-server restart is offered")).toContainText("does not say which of the two is missing");
    await expect(dialog(page, "No config-server restart is offered").getByRole("button", { name: /^Restart/ })).toHaveCount(0);
  } }),
  ...pair("N26", { scenario: "design-notify-config-server-approval", path: TAB, title: "The restart is sent for approval: nothing restarts until an Admin approves, and the request is named", why: CONFIG_WHY, steps: then(openConfigServer, typeName(confirmConfigServer, CONFIG_SERVER), confirmIn(confirmConfigServer, `Restart ${CONFIG_SERVER}`)), check: result("Config-server restart sent for approval", "Nothing restarts until an Admin approves request AP-2044") }),
  ...pair("N28", { scenario: "design-notify-config-server-truncated", path: TAB, title: "Services tab, a swimlane whose offer is truncated", why: CONFIG_WHY, check: tabIdle("on", true) }),
  ...pair("N29", { scenario: "design-notify-config-server-none", path: TAB, title: "Services tab, a swimlane for which the hub offers no restart (the warning stands: it does not hide the need)", why: CONFIG_WHY, check: tabIdle("on", true) }),
  ...pair("N30", { scenario: "design-notify-config-server-approval", path: TAB, title: "Services tab, a swimlane whose config-server restart is answered with a request", why: CONFIG_WHY, check: tabIdle("on", true) }),
  ...pair("N31", { scenario: "design-notify-config-server-approval", path: TAB, title: "Restart dialog, before it is confirmed (the hub will answer with a request)", why: CONFIG_WHY, steps: openConfigServer, check: confirmsConfigServer }),
  frame("N27", { scenario: "design-notify-config-server", path: TAB, size: NARROW, title: "1024 px wide: the banner keeps its button and the strip wraps", why: CONFIG_WHY, check: tabIdle("on", true) }),
];

// ----- the file page, after a save --------------------------------------------------------------------------------------------------

const fileScenarios: Frame[] = [
  ...pair("N40", {
    scenario: "design-notify-file-saved",
    path: FILE,
    clock: "date",
    title: "After a save: two services wait on the file, and the banner offers Notify services and Restart services",
    why: FILE_WHY,
    steps: saveCoreFile,
    check: async ({ page }) => {
      await expect(saved(page)).toContainText("Services pick it up after a notify or a restart.");
      await expect(bannerNotify(page)).toBeEnabled();
      await expect(bannerRestart(page)).toBeEnabled();
      await expect(bannerConfigServer(page)).toHaveCount(0);
    },
  }),
  ...pair("N41", { scenario: "design-notify-file-saved", path: FILE, clock: "date", title: "The notify dialog opened from the banner names this one file", why: FILE_WHY, steps: then(saveCoreFile, openNotifyFromBanner), check: async ({ page }) => {
    await expect(asking(page)).toContainText("Tell the config-server that this file changed in sitb.");
    await expect(asking(page).getByRole("region", { name: "1 file to announce" })).toContainText("tx-infinity-api/tx-infinity-core-sit7.yml");
  } }),
  ...pair("N42", { scenario: "design-notify-file-saved", path: FILE, clock: "date", title: "Notified from the banner: one path accepted, and the audit event", why: FILE_WHY, steps: then(saveCoreFile, openNotifyFromBanner, confirmIn(asking, "Notify services")), check: result("Notification sent", "The hub accepted 1 path for sitb.", "41") }),
  ...pair("N43", { scenario: "design-notify-file-saved", path: FILE, clock: "date", title: "Restart services: the hub's offer, two services, restarted one request at a time, and the swimlane's id to type", why: RESTART_ALL_WHY, steps: then(saveCoreFile, press(bannerRestart, (page) => confirmRestartFile(page, 2))), check: then(async ({ page }) => {
    const ask = confirmRestartFile(page, 2);
    await expect(ask.getByRole("region", { name: "2 services to restart" })).toContainText("sitb/tx-infinity-core");
    await expect(ask.getByRole("button", { name: "Restart 2 services" })).toHaveAttribute("aria-disabled", "true");
    await expect(ask).toContainText("The requests are sent one at a time");
  }, interruptsOthers((page) => confirmRestartFile(page, 2))) }),
  ...pair("N44", { scenario: "design-notify-file-saved", path: FILE, clock: "date", title: "Restart services, the swimlane's id typed: the button is on", why: RESTART_ALL_WHY, steps: then(saveCoreFile, press(bannerRestart, (page) => confirmRestartFile(page, 2)), typeName((page) => confirmRestartFile(page, 2), "sitb")) }),
  ...pair("N45", {
    scenario: "design-notify-file-saved",
    path: FILE,
    clock: "date",
    title: "Restart requested: accepted for two services, one at a time, and no progress is claimed",
    why: RESTART_ALL_WHY,
    steps: then(saveCoreFile, press(bannerRestart, (page) => confirmRestartFile(page, 2)), typeName((page) => confirmRestartFile(page, 2), "sitb"), confirmIn((page) => confirmRestartFile(page, 2), "Restart 2 services")),
    check: result("Restart requested for 2 services", "accepted 2 restart requests, one at a time", "reports no restart progress"),
  }),
  ...pair("N46", { scenario: "design-notify-file-locked", path: FILE, clock: "date", title: "After a save in a swimlane whose notifications are refused: the banner is the same", why: FILE_WHY, steps: saveCoreFile, check: async ({ page }) => {
    await expect(bannerNotify(page)).toBeEnabled();
  } }),
  ...pair("N47", { scenario: "design-notify-file-locked", path: FILE, clock: "date", title: "Notified from the banner and refused (423): the dialog stays with the request id", why: FILE_WHY, steps: then(saveCoreFile, openNotifyFromBanner, confirmIn(asking, "Notify services")), check: alert(asking, "sitb is locked", "Nothing was sent", "req-mock-0001") }),
  ...pair("N48", { scenario: "design-notify-file-live", path: FILE, clock: "date", title: "After a save, once the services have the file: the banner says it is the new version and offers nothing", why: FILE_WHY, steps: saveCoreFile, check: async ({ page }) => {
    await expect(saved(page)).toContainText("The file on sitb NFS is the new version.");
    await expect(saved(page).getByRole("button", { name: /Notify services|Restart/ })).toHaveCount(0);
  } }),
  ...pair("N49", { scenario: "design-notify-file-config-server", path: FILE, clock: "date", title: "After a save of a file only the config-server's restart serves: the banner offers Restart config-server and nothing else", why: FILE_WHY, steps: saveCoreFile, check: async ({ page }) => {
    await expect(bannerConfigServer(page)).toBeEnabled();
    await expect(saved(page).getByRole("button", { name: /Notify services|Restart services/ })).toHaveCount(0);
  } }),
  ...pair("N50", { scenario: "design-notify-file-config-server", path: FILE, clock: "date", title: "The config-server's restart opened from the banner: the offer, and the name to type", why: FILE_WHY, steps: then(saveCoreFile, press(bannerConfigServer, confirmConfigServer)), check: async ({ page }) => {
    await expect(confirmConfigServer(page)).toContainText(`Restart ${CONFIG_SERVER} in sitb?`);
  } }),
];

// ----- loading, an offer that cannot be read, a list that is not the whole of it ----------------------------------------------------------

const offerStates: Frame[] = [
  ...pair("N60", { scenario: "design-notify-config-server", path: TAB, title: "Loading: the offer is being read; the dialog has no confirm button", why: CONFIG_WHY, steps: then(held(CONFIG_SERVER_OFFER), press(configServerButton, (page) => dialog(page, "Restart the config-server in sitb?"))), ready: async ({ page }) => {
    await expect(page.getByRole("status", { name: "Loading the config-server restart offer" })).toBeVisible();
    await expect(dialog(page, "Restart the config-server in sitb?").getByRole("button", { name: /^Restart/ })).toHaveCount(0);
  } }),
  ...pair("N61", { scenario: "design-notify-file-saved", path: FILE, clock: "date", title: "Loading: the services to restart are being read", why: RESTART_ALL_WHY, steps: then(saveCoreFile, held(RESTART_OFFER), press(bannerRestart, (page) => dialog(page, "Restart services for tx-infinity-core-sit7.yml in sitb?"))), ready: async ({ page }) => {
    await expect(page.getByRole("status", { name: "Loading the services to restart" })).toBeVisible();
  } }),
  ...pair("N62", { scenario: "design-notify-config-server", path: TAB, title: "The offer is refused (403): it says reading it needs the Operator role, with the request id and no Retry", why: CONFIG_WHY, steps: then(unreadable(CONFIG_SERVER_OFFER, 403, "Forbidden"), press(configServerButton, (page) => dialog(page, "Couldn't read the config-server restart offer"))), check: async ({ page }) => {
    const shown = dialog(page, "Couldn't read the config-server restart offer");
    await expect(shown).toContainText("Reading it needs the Operator role");
    await expect(shown.getByRole("button", { name: "Retry" })).toHaveCount(0);
    await expect(shown).toContainText("req-mock-0001");
  } }),
  ...pair("N63", { scenario: "design-notify-config-server", path: TAB, title: "The offer cannot be read (a server error): the request id and Retry, and nothing to confirm", why: CONFIG_WHY, steps: then(unreadable(CONFIG_SERVER_OFFER, 503, "Service unavailable"), press(configServerButton, (page) => dialog(page, "Couldn't read the config-server restart offer"))), check: async ({ page }) => {
    const shown = dialog(page, "Couldn't read the config-server restart offer");
    await expect(shown.getByRole("button", { name: "Retry" })).toBeVisible();
    await expect(shown).toContainText("req-mock-0001");
    await expect(shown.getByRole("button", { name: /^Restart/ })).toHaveCount(0);
  } }),
  ...pair("N64", { scenario: "design-notify-file-saved", path: FILE, clock: "date", title: "The services to restart cannot be read (a server error): the request id and Retry", why: RESTART_ALL_WHY, steps: then(saveCoreFile, unreadable(RESTART_OFFER, 500, "Internal error"), press(bannerRestart, (page) => dialog(page, "Couldn't read the services to restart"))), check: async ({ page }) => {
    await expect(dialog(page, "Couldn't read the services to restart").getByRole("button", { name: "Retry" })).toBeVisible();
  } }),
  ...pair("N65", {
    scenario: "design-notify-file-saved",
    path: FILE,
    clock: "date",
    title: "The hub lists no service to restart: it says so and offers nothing",
    why: RESTART_ALL_WHY,
    steps: then(saveCoreFile, offerIs({ services: [], truncated: false }), press(bannerRestart, (page) => dialog(page, "No services to restart"))),
    check: async ({ page }) => {
      await expect(dialog(page, "No services to restart")).toContainText("lists no service to restart");
    },
  }),
  ...pair("N66", {
    scenario: "design-notify-file-saved",
    path: FILE,
    clock: "date",
    title: "The list of services to restart is not the whole of them: it says so and a warning says the others stay as they are",
    why: RESTART_ALL_WHY,
    steps: then(saveCoreFile, offerIs({ services: [{ namespace: "sitb", name: "tx-infinity-api" }, { namespace: "sitb", name: "tx-infinity-core" }], truncated: true }), press(bannerRestart, (page) => confirmRestartFile(page, 2))),
    check: async ({ page }) => {
      await expect(confirmRestartFile(page, 2).getByRole("region", { name: "2 services to restart (not the whole list)" })).toBeVisible();
      await expect(confirmRestartFile(page, 2)).toContainText("Restarting these leaves the others as they are.");
    },
  }),
  ...pair("N67", {
    scenario: "design-notify-file-saved",
    path: FILE,
    clock: "date",
    title: "Restart stopped early: the hub refused the first request (423), and the others were never sent",
    why: RESTART_ALL_WHY,
    steps: then(saveCoreFile, refuse(RESTART, 423, "Locked"), press(bannerRestart, (page) => confirmRestartFile(page, 2)), typeName((page) => confirmRestartFile(page, 2), "sitb"), confirmIn((page) => confirmRestartFile(page, 2), "Restart 2 services")),
    check: result("Restart stopped early", "was not sent", "Nothing was sent again"),
  }),
  ...pair("N68", {
    scenario: "design-notify-file-saved",
    path: FILE,
    clock: "date",
    title: "Restart sent for approval: one request for each service, none of them done",
    why: RESTART_ALL_WHY,
    steps: then(
      saveCoreFile,
      async ({ page }) => {
        await page.route(RESTART, (route) => (route.request().method() === "GET" ? route.fallback() : route.fulfill({ json: { outcome: "proposal_created", id: 2051 } })));
      },
      press(bannerRestart, (page) => confirmRestartFile(page, 2)),
      typeName((page) => confirmRestartFile(page, 2), "sitb"),
      confirmIn((page) => confirmRestartFile(page, 2), "Restart 2 services"),
    ),
    check: result("Restart sent for approval", "Nothing restarts for these services until an Admin approves"),
  }),
];

// ----- who may press, and when -------------------------------------------------------------------------------------------------------

const gates: Frame[] = [
  ...pair("N70", {
    scenario: "design-notify-services-tab",
    path: TAB,
    before: as("editor"),
    title: "An Editor: \"Restart needs Operator\", Notify services and every Restart are disabled",
    why: GATE_WHY,
    check: async ({ page }) => {
      await expect(page.getByText("Restart needs Operator", { exact: true })).toBeVisible();
      await expect(notifyButton(page)).toHaveAttribute("aria-disabled", "true");
    },
  }),
  ...pair("N71", { scenario: "design-notify-services-tab", path: TAB, before: as("editor"), hold: true, title: "An Editor: the tooltip of Notify services names the role it needs", why: GATE_WHY, steps: then(hover(notifyButton), tipSays("Needs the Operator role")) }),
  ...pair("N72", {
    scenario: "design-notify-services-tab",
    path: TAB,
    before: pauseStream,
    steps: ({ page }) => expectPaused(page),
    mask: pausedMasks,
    title: "Live updates paused: Notify services and Restart are off, and say \"Unavailable offline\"",
    why: GATE_WHY,
    check: async ({ page }) => {
      await expect(notifyButton(page)).toHaveAttribute("aria-disabled", "true");
    },
  }),
  ...pair("N73", {
    scenario: "design-notify-services-tab",
    path: TAB,
    before: pauseStream,
    steps: then(({ page }) => expectPaused(page), hover(notifyButton), tipSays("Unavailable offline")),
    hold: true,
    mask: pausedMasks,
    title: "Live updates paused: the tooltip of Notify services",
    why: GATE_WHY,
  }),
  ...pair("N74", {
    scenario: "design-notify-services-tab",
    path: TAB,
    before: agentOffline,
    title: "The swimlane's agent is offline: the banner says so, and Notify services and Restart are off",
    why: GATE_WHY,
    check: async ({ page }) => {
      await expect(page.getByText(/Agent (for sitb )?offline|Agent offline since/).first()).toBeVisible();
      await expect(notifyButton(page)).toHaveAttribute("aria-disabled", "true");
    },
  }),
  ...pair("N75", { scenario: "design-notify-services-tab", path: TAB, before: agentOffline, hold: true, title: "The agent is offline: the tooltip of Notify services says so", why: GATE_WHY, steps: then(hover(notifyButton), tipSays("Unavailable while the agent is offline")) }),
  ...pair("N76", {
    scenario: "design-notify-services-tab",
    path: TAB,
    before: servicesAre((list) => list.map((s): ServiceState => ({ ...s, pickup: "live", pending_files: [], pending_since: null })), []),
    hold: true,
    title: "No service is waiting on a file: Notify services is off, and says why",
    why: GATE_WHY,
    steps: then(hover(notifyButton), tipSays("No service is waiting on a file.")),
  }),
  ...pair("N77", {
    scenario: "design-notify-services-tab",
    path: TAB,
    before: servicesAre((list) => list.map((s, i): ServiceState => (i === 0 ? { ...s, pending_files: Array.from({ length: 130 }, (_, n) => `resources/file-${String(n).padStart(3, "0")}.yml`) } : s))),
    steps: openNotify,
    title: "More files wait than one request holds: 100 are announced, and a warning says how many are left for the next",
    why: NOTIFY_WHY,
    check: async ({ page }) => {
      await expect(asking(page).getByRole("region", { name: /^100 files to announce/ })).toBeVisible();
      await expect(asking(page)).toContainText("more files are waiting and are not in this request");
    },
  }),
  // A dialog that is already open follows the gate: the stream goes down while it is open (the confirm button turns off and says why).
  ...pair("N78", {
    scenario: "design-notify-services-tab",
    path: TAB,
    mask: pausedMasks,
    title: "The notify dialog is open when live updates pause: the confirm button turns off and the dialog says \"Unavailable offline\"",
    why: GATE_WHY,
    steps: then(openNotify, streamDown),
    check: async ({ page }) => {
      await expect(asking(page).getByRole("button", { name: "Notify services" })).toHaveAttribute("aria-disabled", "true");
      await expect(asking(page)).toContainText("Unavailable offline");
      await expect(asking(page).getByRole("button", { name: "Cancel" })).toBeEnabled();
    },
  }),
  ...pair("N79", {
    scenario: "design-notify-config-server",
    path: TAB,
    mask: pausedMasks,
    title: "The config-server's restart dialog is open, its name typed, when live updates pause: the confirm button turns off",
    why: GATE_WHY,
    steps: then(openConfigServer, typeName(confirmConfigServer, CONFIG_SERVER), streamDown),
    check: async ({ page }) => {
      await expect(confirmConfigServer(page).getByRole("button", { name: `Restart ${CONFIG_SERVER}` })).toHaveAttribute("aria-disabled", "true");
      await expect(confirmConfigServer(page)).toContainText("Unavailable offline");
    },
  }),
];

// ----- the answers to a notification that is not an acceptance ------------------------------------------------------------------------

const refusals: Frame[] = [
  ...pair("N80", { scenario: "design-notify-services-tab", path: TAB, title: "422: the server did not accept the notification, and nothing was sent", why: NOTIFY_WHY, steps: then(refuse(NOTIFY, 422, "Unprocessable"), openNotify, confirmIn(asking, "Notify services")), check: alert(asking, "The server did not accept this notification", "Nothing was sent", "req-mock-0001") }),
  ...pair("N81", { scenario: "design-notify-services-tab", path: TAB, title: "429: too many requests, and how long to wait", why: NOTIFY_WHY, steps: then(refuse(NOTIFY, 429, "Too many requests", 30), openNotify, confirmIn(asking, "Notify services")), check: alert(asking, "Too many requests", "Wait 30 seconds, then try again.", "req-mock-0001") }),
  ...pair("N82", { scenario: "design-notify-services-tab", path: TAB, title: "A server error: it may or may not have been sent, so it says to check the services, and never claims either", why: NOTIFY_WHY, steps: then(refuse(NOTIFY, 503, "Service unavailable"), openNotify, confirmIn(asking, "Notify services")), check: alert(asking, "may or may not have been sent", "Check the services before you try again", "req-mock-0001") }),
  ...pair("N83", { scenario: "design-notify-services-tab", path: TAB, title: "No answer (the network went): it says the same, and the dialog stays so that the person can decide", why: NOTIFY_WHY, steps: then(cutOff(NOTIFY), openNotify, confirmIn(asking, "Notify services")), unanswered: [], check: alert(asking, "Couldn't reach the server", "may or may not have been sent") }),
  ...pair("N84", { scenario: "design-notify-config-server", path: TAB, title: "The restart is refused with 423: the dialog stays with the request id", why: CONFIG_WHY, steps: then(refuse(RESTART, 423, "Locked"), openConfigServer, typeName(confirmConfigServer, CONFIG_SERVER), confirmIn(confirmConfigServer, `Restart ${CONFIG_SERVER}`)), check: alert(confirmConfigServer, "sitb is locked", "Another operation is using it", "req-mock-0001") }),
];

// ----- the restart of one service (T4's dialog, which the gate now follows) ----------------------------------------------------------

const oneService: Frame[] = [
  ...pair("N90", { scenario: "design-notify-services-tab", path: TAB, title: "Restart of one service: what the restart picks up, and the warning that others are interrupted", why: RESTART_WHY, steps: press((page) => restartRow(page, "tx-infinity-core"), (page) => confirmRestartService(page, "tx-infinity-core")), check: then(async ({ page }) => {
    await expect(confirmRestartService(page, "tx-infinity-core")).toContainText("The restart picks up application.properties and 1 more file.");
  }, interruptsOthers((page) => confirmRestartService(page, "tx-infinity-core"))) }),
  ...pair("N91", {
    scenario: "design-notify-services-tab",
    path: TAB,
    mask: pausedMasks,
    title: "The restart dialog is open when live updates pause: the confirm button turns off and says why (the gate follows an open dialog)",
    why: RESTART_WHY,
    steps: then(press((page) => restartRow(page, "tx-infinity-core"), (page) => confirmRestartService(page, "tx-infinity-core")), streamDown),
    check: async ({ page }) => {
      await expect(confirmRestartService(page, "tx-infinity-core").getByRole("button", { name: "Restart tx-infinity-core" })).toHaveAttribute("aria-disabled", "true");
      await expect(confirmRestartService(page, "tx-infinity-core")).toContainText("Unavailable offline");
    },
  }),
];

// ----- stacking: a tooltip over a dialog --------------------------------------------------------------------------------------------

const stacking: Frame[] = [
  ...pair("N92", {
    scenario: "design-notify-services-tab",
    path: TAB,
    hold: true,
    title: "The tooltip of the copy button inside the result dialog is drawn above the dialog, not under it",
    why: NOTIFY_WHY,
    // The copy button's tooltip keeps its words in a hidden twin (role tooltip) and draws a content element of its own, which is what is looked at.
    steps: then(openNotify, confirmIn(asking, "Notify services"), async ({ page }) => {
      await dialog(page, "Notification sent").getByRole("button", { name: /copy/i }).hover();
      await page.locator("[data-slot='tooltip-content']").first().waitFor();
    }),
    check: async ({ page }) => {
      const tip = page.locator("[data-slot='tooltip-content']").first();
      await expect(tip).toBeVisible();
      await expect(tip).toContainText("Copy");
      const box = await tip.boundingBox();
      expect(box, "the tooltip's box").not.toBeNull();
      if (box === null) return;
      // The thing at the centre of the tooltip is the tooltip (or part of it): nothing of the dialog or its overlay is drawn over it.
      const topmost = await page.evaluate(({ x, y }) => document.elementFromPoint(x, y)?.closest("[role='tooltip'], [data-slot='tooltip-content']") !== null, { x: box.x + box.width / 2, y: box.y + box.height / 2 });
      expect(topmost, "the tooltip is the topmost thing at its own centre").toBe(true);
    },
  }),
];

export const frames: Frame[] = [...tabScenarios, ...configScenarios, ...fileScenarios, ...offerStates, ...gates, ...refusals, ...oneService, ...stacking];

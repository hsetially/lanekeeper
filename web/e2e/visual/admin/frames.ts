// The frames of Admin: the board frames of "Lanekeeper Admin.dc.html" (1a to 1bn) and the States-index rows of Admin (L11 E11 R11 P02
// C03). The design's frames are four per section (default, empty, editing, error, saved). The app draws the sections with the
// contract's operations, and design/DEVIATIONS.md lists how they differ ("Admin: Users", "Admin: Swimlanes", "Admin: Paths, Merge,
// Tiers", "Admin: Folders", "Admin: Notifications", "Admin: MCP", "Admin: join token", "Admin area"). Every Admin frame is therefore an
// app-owned baseline, and the frames the app does not draw at all are excluded below, each with its reason.
import type { Page } from "@playwright/test";
import type { Excluded, FrameDef, Step } from "../search/kit";
import { expectSkeleton, failRequests, holdRequests, expectPaused, pauseStream, pausedMasks } from "../search/steps";

const APP = (row: string): string => `the section is the app's own, built on the contract's operations (design/DEVIATIONS.md: "${row}")`;

/** A text field of the open dialog, by its label. */
const field = (page: Page, name: string) => page.getByRole("alertdialog").getByRole("textbox", { name, exact: true });

const dialogOf = (page: Page) => page.getByRole("alertdialog");

/** The first field the dialog marks as wrong. */
const invalidField = (page: Page) => page.getByRole("alertdialog").locator("[aria-invalid='true']").first().waitFor();

/** Open the add dialog of a section by its button, and wait for the dialog. */
const openDialog = (button: string | RegExp, dialog: string | RegExp): Step => async ({ page }) => {
  await page.getByRole("button", { name: button }).first().click();
  await page.getByRole("alertdialog", { name: dialog }).waitFor();
};

export const frames: FrameDef[] = [
  // Users and access requests.
  { board: "admin", frame: "1a", kind: "app", scenario: "design-admin-users", path: "/admin/users", title: "users", why: APP("Admin: Users") },
  { board: "admin", frame: "1b", kind: "app", scenario: "design-admin-users-empty", path: "/admin/users", title: "users, no request waiting", why: APP("Admin: Users") },
  {
    board: "admin", frame: "1c", kind: "app", scenario: "design-admin-users", path: "/admin/users", title: "approving as Editor names the consequence", why: APP("Admin: Users"),
    steps: async ({ page }) => {
      await page.getByRole("button", { name: /^Approve request from sam/ }).click();
      await page.getByRole("alertdialog").waitFor();
    },
  },
  {
    board: "admin", frame: "1e", kind: "app", scenario: "design-admin-users", path: "/admin/users", title: "saved: Sam Okafor approved", why: APP("Admin: Users"),
    steps: async ({ page }) => {
      await page.getByRole("button", { name: /^Approve request from sam/ }).click();
      const dialog = page.getByRole("alertdialog");
      await dialog.getByRole("button", { name: /Approve/ }).click();
      await page.getByText(/approved|Sam Okafor/i).first().waitFor();
    },
  },
  // Swimlanes and agents.
  { board: "admin", frame: "1f", kind: "app", scenario: "design-admin-swimlanes", path: "/admin/swimlanes", title: "swimlanes and agents", why: APP("Admin: Swimlanes") },
  { board: "admin", frame: "1g", kind: "app", scenario: "design-admin-swimlanes-empty", path: "/admin/swimlanes", title: "no swimlane", why: APP("Admin: Swimlanes") },
  {
    board: "admin", frame: "1j", kind: "app", scenario: "design-admin-swimlanes", path: "/admin/swimlanes", title: "join token shown once", why: APP("Admin: join token"),
    steps: async ({ page }) => {
      await page.getByRole("button", { name: /^Manage sitc/ }).click();
      await page.getByRole("button", { name: "Create join token" }).click();
      await page.getByRole("button", { name: "I have stored it" }).waitFor();
    },
  },
  // Path mappings.
  { board: "admin", frame: "1k", kind: "app", scenario: "design-admin-paths", path: "/admin/paths", title: "path mappings", why: APP("Admin: Paths, Merge, Tiers") },
  { board: "admin", frame: "1l", kind: "app", scenario: "design-admin-paths-empty", path: "/admin/paths", title: "no path mapping", why: APP("Admin: Paths, Merge, Tiers") },
  { board: "admin", frame: "1m", kind: "app", scenario: "design-admin-paths", path: "/admin/paths", title: "editing a rule", why: APP("Admin: Paths, Merge, Tiers"), steps: openDialog("Edit rule default", /Edit/) },
  {
    board: "admin", frame: "1n", kind: "app", scenario: "design-admin-paths", path: "/admin/paths", title: "validation error: the prefixes are wrong", why: APP("Admin: Paths, Merge, Tiers"),
    steps: async (env) => {
      await openDialog("Add rule", "Add a path mapping")(env);
      const { page } = env;
      await field(page, "Rule id").fill("bad rule");
      await field(page, "Base repo path").fill("/abs");
      await page.getByRole("button", { name: "Save rule" }).click();
      await invalidField(page);
    },
  },
  {
    board: "admin", frame: "1o", kind: "app", scenario: "design-admin-paths", path: "/admin/paths", title: "saved: rule saved", why: APP("Admin: Paths, Merge, Tiers"),
    steps: async (env) => {
      await openDialog("Add rule", "Add a path mapping")(env);
      const { page } = env;
      await field(page, "Rule id").fill("security");
      await field(page, "Base repo path").fill("security");
      await field(page, "Tenant repo path").fill("data/security");
      await page.getByRole("button", { name: "Save rule" }).click();
      await page.getByRole("alertdialog").waitFor({ state: "detached" });
    },
  },
  // Merge modes.
  { board: "admin", frame: "1p", kind: "app", scenario: "design-admin-merge", path: "/admin/merge", title: "merge modes", why: APP("Admin: Paths, Merge, Tiers") },
  { board: "admin", frame: "1q", kind: "app", scenario: "design-admin-merge-empty", path: "/admin/merge", title: "no merge rule", why: APP("Admin: Paths, Merge, Tiers") },
  { board: "admin", frame: "1r", kind: "app", scenario: "design-admin-merge", path: "/admin/merge", title: "editing the security rule", why: APP("Admin: Paths, Merge, Tiers"), steps: openDialog("Edit rule security", /Edit rule security/) },
  {
    board: "admin", frame: "1s", kind: "app", scenario: "design-admin-merge", path: "/admin/merge", title: "validation error: an invalid glob", why: APP("Admin: Paths, Merge, Tiers"),
    steps: async (env) => {
      await openDialog("Add rule", "Add a merge rule")(env);
      const { page } = env;
      await field(page, "Rule id").fill("odd");
      await field(page, "Path glob").fill("[");
      await page.getByRole("button", { name: "Save rule" }).click();
      await invalidField(page);
    },
  },
  {
    board: "admin", frame: "1t", kind: "app", scenario: "design-admin-merge", path: "/admin/merge", title: "saved: rules saved", why: APP("Admin: Paths, Merge, Tiers"),
    steps: async (env) => {
      await openDialog("Add rule", "Add a merge rule")(env);
      const { page } = env;
      await field(page, "Rule id").fill("images");
      await field(page, "Path glob").fill("images/**");
      await page.getByRole("button", { name: "Save rule" }).click();
      await page.getByRole("alertdialog").waitFor({ state: "detached" });
    },
  },
  // Host tiers.
  { board: "admin", frame: "1u", kind: "app", scenario: "design-admin-tiers", path: "/admin/tiers", title: "host tiers", why: APP("Admin: Paths, Merge, Tiers") },
  { board: "admin", frame: "1v", kind: "app", scenario: "design-admin-tiers-empty", path: "/admin/tiers", title: "no host pattern", why: APP("Admin: Paths, Merge, Tiers") },
  {
    board: "admin", frame: "1w", kind: "app", scenario: "design-admin-tiers", path: "/admin/tiers", title: "adding *-pre.example.net to uat", why: APP("Admin: Paths, Merge, Tiers"),
    steps: async (env) => {
      await openDialog("Add pattern", "Add a host pattern")(env);
      const { page } = env;
      await field(page, "Rule id").fill("pre");
      await field(page, "Host pattern").fill("*-pre.example.net");
    },
  },
  {
    board: "admin", frame: "1x", kind: "app", scenario: "design-admin-tiers", path: "/admin/tiers", title: "validation error: an empty pattern", why: APP("Admin: Paths, Merge, Tiers"),
    steps: async (env) => {
      await openDialog("Add pattern", "Add a host pattern")(env);
      await env.page.getByRole("button", { name: "Save pattern" }).click();
      await invalidField(env.page);
    },
  },
  {
    board: "admin", frame: "1y", kind: "app", scenario: "design-admin-tiers", path: "/admin/tiers", title: "saved: pattern saved", why: APP("Admin: Paths, Merge, Tiers"),
    steps: async (env) => {
      await openDialog("Add pattern", "Add a host pattern")(env);
      const { page } = env;
      await field(page, "Rule id").fill("pre");
      await field(page, "Host pattern").fill("*-pre.example.net");
      await dialogOf(page).getByLabel("Tier", { exact: true }).fill("uat");
      await page.getByRole("button", { name: "Save pattern" }).click();
      await page.getByRole("alertdialog").waitFor({ state: "detached" });
    },
  },
  // Folder mappings.
  { board: "admin", frame: "1z", kind: "app", scenario: "design-admin-folders", path: "/admin/folders", title: "folder mappings", why: APP("Admin: Folders") },
  { board: "admin", frame: "1aa", kind: "app", scenario: "design-admin-folders-empty", path: "/admin/folders", title: "no folder mapping", why: APP("Admin: Folders") },
  { board: "admin", frame: "1ab", kind: "app", scenario: "design-admin-folders", path: "/admin/folders", title: "editing video", why: APP("Admin: Folders"), steps: openDialog("Edit mapping f-video", /Edit mapping f-video/) },
  {
    board: "admin", frame: "1ac", kind: "app", scenario: "design-admin-folders", path: "/admin/folders", title: "validation error: no folder", why: APP("Admin: Folders"),
    steps: async (env) => {
      await openDialog("Add mapping", "Add a folder mapping")(env);
      await env.page.getByRole("button", { name: "Save mapping" }).click();
      await invalidField(env.page);
    },
  },
  {
    board: "admin", frame: "1ad", kind: "app", scenario: "design-admin-folders", path: "/admin/folders", title: "saved: mapping saved", why: APP("Admin: Folders"),
    steps: async (env) => {
      await openDialog("Add mapping", "Add a folder mapping")(env);
      const { page } = env;
      await field(page, "Mapping id").fill("f-new");
      await field(page, "NFS folder").fill("video");
      await field(page, "Deployments").fill("sitb/video-service");
      await page.getByRole("button", { name: "Save mapping" }).click();
      await page.getByRole("alertdialog").waitFor({ state: "detached" });
    },
  },
  // Notifications.
  { board: "admin", frame: "1ae", kind: "app", scenario: "design-admin-notify", path: "/admin/notify", title: "notification channels", why: APP("Admin: Notifications") },
  { board: "admin", frame: "1af", kind: "app", scenario: "design-admin-notify-empty", path: "/admin/notify", title: "no channel", why: APP("Admin: Notifications") },
  {
    board: "admin", frame: "1ag", kind: "app", scenario: "design-admin-notify", path: "/admin/notify", title: "entering a webhook", why: APP("Admin: Notifications"),
    steps: async (env) => {
      await openDialog("Add channel", "Add a notification channel")(env);
      await field(env.page, "Webhook URL").fill("https://example.webhook.office.com/webhookb2/00000000-0000");
    },
  },
  {
    board: "admin", frame: "1ah", kind: "app", scenario: "design-admin-notify", path: "/admin/notify", title: "validation error: not a Teams URL", why: APP("Admin: Notifications"),
    steps: async (env) => {
      await openDialog("Add channel", "Add a notification channel")(env);
      const { page } = env;
      await field(page, "Channel id").fill("ops");
      await field(page, "Name").fill("Ops");
      await field(page, "Webhook URL").fill("http://example.com/hook");
      await page.getByRole("button", { name: "Save channel" }).click();
      await dialogOf(page).getByText("A webhook must be an https:// address").waitFor();
    },
  },
  {
    board: "admin", frame: "1ai", kind: "app", scenario: "design-admin-notify", path: "/admin/notify", title: "saved: channel saved", why: APP("Admin: Notifications"),
    steps: async (env) => {
      await openDialog("Add channel", "Add a notification channel")(env);
      const { page } = env;
      await field(page, "Channel id").fill("ops");
      await field(page, "Name").fill("Ops");
      await dialogOf(page).getByRole("checkbox", { name: "Critical finding" }).check();
      await field(page, "Webhook URL").fill("https://example.webhook.office.com/webhookb2/00000000-0000");
      await page.getByRole("button", { name: "Save channel" }).click();
      await page.getByRole("alertdialog").waitFor({ state: "detached" });
    },
  },
  // MCP personal tokens.
  { board: "admin", frame: "1aj", kind: "app", scenario: "design-admin-mcp", path: "/admin/mcp", title: "personal tokens", why: APP("Admin: MCP") },
  { board: "admin", frame: "1ak", kind: "app", scenario: "design-admin-mcp-empty", path: "/admin/mcp", title: "no token", why: APP("Admin: MCP") },
  {
    board: "admin", frame: "1al", kind: "app", scenario: "design-admin-mcp", path: "/admin/mcp", title: "issuing a token", why: APP("Admin: MCP"),
    steps: async ({ page }) => {
      await page.getByRole("textbox").first().fill("cursor-priya-2");
    },
  },
  { board: "admin", frame: "1bi", kind: "app", scenario: "design-admin-users", path: "/admin/users", title: "loading", why: APP("Admin area"), before: ({ page }) => holdRequests(page, /\/api\/v1\/admin\/users(\?|$)/), ready: ({ page }) => expectSkeleton(page) },
  { board: "admin", frame: "1bj", kind: "app", scenario: "design-admin-viewer", path: "/admin/users", title: "not an Admin: Admin needs the Admin role", why: APP("Admin area") },
  { board: "admin", frame: "1bk", kind: "app", scenario: "design-admin-mcp-off", path: "/admin/mcp", title: "MCP mode off", why: APP("Admin: MCP") },
  { board: "admin", frame: "1bn", kind: "app", scenario: "design-admin-notify", path: "/admin/notify", title: "notification channels, dark", why: APP("Admin: Notifications") },
];

const T5 = (section: string): string => `the ${section} section is task T5's: the app lists it and says "Not available yet" (design/DEVIATIONS.md: "Admin area", "T5 scope")`;
const FORM = (what: string): string => `the app has no ${what} (design/DEVIATIONS.md: "Admin: Users", "Admin: Swimlanes"): the contract has no operation for it`;

export const excluded: Excluded[] = [
  { board: "admin", frame: "1d", why: FORM("GitHub login field to be linked or refused, and no approve-without-a-role error: the role is chosen in the dialog") },
  { board: "admin", frame: "1h", why: FORM("register-swimlane form with a live preview of the identity binding (plan CG15)") },
  { board: "admin", frame: "1i", why: FORM("register-swimlane form, so its validation errors cannot be drawn (plan CG15)") },
  ...(["1ao", "1ap", "1aq", "1ar", "1as"] as const).map((frame) => ({ board: "admin" as const, frame, why: T5("Sentinels") })),
  ...(["1at", "1au", "1av", "1aw", "1ax"] as const).map((frame) => ({ board: "admin" as const, frame, why: T5("OS Login user mapping") })),
  ...(["1ay", "1az", "1ba", "1bb", "1bc"] as const).map((frame) => ({ board: "admin" as const, frame, why: T5("Severity rules") })),
  ...(["1bd", "1be", "1bf", "1bg", "1bh"] as const).map((frame) => ({ board: "admin" as const, frame, why: T5("Retention") })),
  { board: "admin", frame: "1am", why: FORM("validation of a Viewer holding a token, a lifetime or a name format: the personal-token form takes a label only") },
  { board: "admin", frame: "1an", why: FORM("\"Token shown once\" frame of the admin table: the token is shown once in the shared personal-tokens component, whose frame is the MCP page's") },
  { board: "admin", frame: "1bl", why: T5("Sentinels (dark)") },
  { board: "admin", frame: "1bm", why: T5("Severity rules (dark)") },
];

// The States-index rows of Admin, light, as app-owned baselines (L11 E11 R11 P02 C03).
export const states: FrameDef[] = [
  {
    board: "states", frame: "L11", kind: "state", scenario: "design-admin-users", path: "/admin/users", title: "Admin, loading", why: APP("Admin area"),
    before: ({ page }) => holdRequests(page, /\/api\/v1\/admin\/users(\?|$)/),
    ready: ({ page }) => expectSkeleton(page),
  },
  { board: "states", frame: "E11", kind: "state", scenario: "design-admin-paths-empty", path: "/admin/paths", title: "Admin, empty", why: APP("Admin area") },
  { board: "states", frame: "R11", kind: "state", scenario: "design-admin-users", path: "/admin/users", title: "Admin, error with the request id", why: APP("Admin area"), before: ({ page }) => failRequests(page, /\/api\/v1\/admin\/users(\?|$)/) },
  { board: "states", frame: "P02", kind: "state", scenario: "design-admin-viewer", path: "/admin/users", title: "Admin, permission: needs Admin", why: APP("Admin area") },
  {
    board: "states", frame: "C03", kind: "state", scenario: "design-admin-users-conflict", path: "/admin/users", title: "Admin, 409: the person changed meanwhile", why: APP("Admin: Users, 409 (State C03)"),
    steps: async ({ page }) => {
      await page.getByLabel("Role for Lena Brandt").selectOption("viewer");
      await page.getByRole("alertdialog", { name: "Change Lena Brandt's role" }).getByRole("button", { name: /Change role|Confirm/ }).click();
      await page.getByRole("button", { name: "Reload user" }).waitFor();
    },
  },
  {
    board: "states", frame: "U-admin", kind: "state", scenario: "design-admin-paths", path: "/admin/paths", title: "Admin, live updates paused: writes are off with the reason", why: APP("Admin area"),
    before: pauseStream,
    steps: ({ page }) => expectPaused(page),
    mask: pausedMasks,
  },
];

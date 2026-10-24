// The frames of the four Admin sections that T5 adds (Sentinels, OS Login users, Severity rules, Retention): each section's states (loading, empty,
// error, permission, live updates paused), its dialogs and what they refuse, and the retention page's dry run in the states it can be in.
//
// The design's own frames of these sections (1ao to 1bm of the Admin board) are in e2e/visual/admin/frames.ts, as app-owned baselines too: the app
// builds each section on the contract's operations and differs from the design by the rows "Admin: Sentinels", "Admin: OS Login", "Admin: Severity
// rules" and "Admin: Retention" of design/DEVIATIONS.md, so none is a pixel comparison. What is here is the rest, the states the design does not draw.
import type { FrameDef, Step } from "../search/kit";
import { expectPaused, expectSkeleton, failRequests, holdRequests, pauseStream, pausedMasks } from "../search/steps";
import { api, rewrite } from "../access/lib/steps";
import {
  addMapping, addRule, checkKind, confirmSave, deleteMapping, dialog, tableHeaderStaysUnderDialog, dryRun, dryRunShown, editMapping, editRule, fillMapping, fillRule, invalidInDialog, mappingSaved, noPreview,
  nothingToDelete, osLoginPut, refuse, ruleSaved, saveMapping, saveRule, savePolicy, setPolicy, then, typeConfirm,
} from "./steps";

const SENTINELS = "/admin/sentinels";
const OSLOGIN = "/admin/oslogin";
const SEVERITY = "/admin/severity";
const RETENTION = "/admin/retention";
const SIZE = { width: 1440, height: 1060 } as const;

const S_WHY = "the section is the app's own, read-only on the contract's operations (design/DEVIATIONS.md: \"Admin: Sentinels\")";
const O_WHY = "the section is the app's own: the admin's mappings, with add, edit and delete (design/DEVIATIONS.md: \"Admin: OS Login\")";
const R_WHY = "the section is the app's own: the admin's rules, four levels, no order (design/DEVIATIONS.md: \"Admin: Severity rules\")";
const T_WHY = "the section is the app's own: two numbers, a dry run and a typed confirmation (design/DEVIATIONS.md: \"Admin: Retention\")";

const frame = (code: string, def: Omit<FrameDef, "board" | "frame" | "kind">): FrameDef => ({ board: "states", frame: code, kind: "app", size: SIZE, ...def });

/** The four states every section of the page has, whatever its data: loading, error with the request id, an Editor who may not open Admin, live updates paused. */
function states(prefix: string, section: string, path: string, scenario: string, listPattern: RegExp, why: string): FrameDef[] {
  return [
    frame(`${prefix}01`, { scenario, path, title: `${section}: loading`, why, before: ({ page }) => holdRequests(page, listPattern), ready: ({ page }) => expectSkeleton(page) }),
    frame(`${prefix}02`, { scenario, path, title: `${section}: error with the request id`, why, before: ({ page }) => failRequests(page, listPattern) }),
    frame(`${prefix}03`, { scenario: "design-admin-viewer", path, title: `${section}: an Editor opens Admin and is told it needs the Admin role`, why }),
    frame(`${prefix}04`, { scenario, path, title: `${section}: live updates paused, so a write is off with its reason`, why, before: pauseStream, steps: ({ page }) => expectPaused(page), mask: pausedMasks }),
  ];
}

/** One sentinel has never reported. */
const neverSeen: Step = async ({ page }) => {
  await rewrite(page, api("/admin/sentinels"), (body) => ({
    ...body,
    items: (body["items"] as Array<Record<string, unknown>>).map((row, i) => (i === 1 ? { ...row, last_seen_at: null, status: "disconnected" } : row)),
  }));
};

/** Every sentinel is connected. */
const allConnected: Step = async ({ page }) => {
  await rewrite(page, api("/admin/sentinels"), (body) => ({
    ...body,
    items: (body["items"] as Array<Record<string, unknown>>).map((row) => ({ ...row, status: "connected", last_seen_at: 1_760_000_000_000 - 5_000, spool_backlog: 0 })),
  }));
};

export const frames: FrameDef[] = [
  // Sentinels.
  ...states("D", "Sentinels", SENTINELS, "design-admin-t5-sentinels", /\/api\/v1\/admin\/sentinels(\?|$)/, S_WHY),
  frame("D05", { scenario: "design-admin-t5-sentinels", path: SENTINELS, title: "Sentinels: one has never reported", why: S_WHY, before: neverSeen }),
  frame("D06", { scenario: "design-admin-t5-sentinels", path: SENTINELS, title: "Sentinels: all connected, no alert", why: S_WHY, before: allConnected }),
  frame("D07", { scenario: "design-admin-t5-sentinels-empty", path: SENTINELS, theme: "dark", title: "Sentinels: none registered, dark", why: S_WHY }),

  // OS Login users.
  ...states("E", "OS Login", OSLOGIN, "design-admin-t5-oslogin", /\/api\/v1\/admin\/os-login-users(\?|$)/, O_WHY),
  frame("E05", { scenario: "design-admin-t5-oslogin", path: OSLOGIN, theme: "dark", title: "OS Login: the mappings, dark", why: O_WHY }),
  frame("E06", { scenario: "design-admin-t5-oslogin", path: OSLOGIN, title: "OS Login: add a mapping, empty", why: O_WHY, steps: addMapping }),
  frame("E07", { scenario: "design-admin-t5-oslogin", path: OSLOGIN, title: "OS Login: add a mapping, filled", why: O_WHY, steps: then(addMapping, fillMapping("dana_okoye_work", 1)) }),
  frame("E08", { scenario: "design-admin-t5-oslogin", path: OSLOGIN, title: "OS Login: a name that is not a login name, and no user", why: O_WHY, steps: then(addMapping, fillMapping("dana/okoye", null), saveMapping, invalidInDialog) }),
  frame("E09", { scenario: "design-admin-t5-oslogin", path: OSLOGIN, title: "OS Login: a name that is mapped already", why: O_WHY, steps: then(addMapping, fillMapping("lena_b_corp_example", 1), saveMapping, invalidInDialog) }),
  frame("E10", { scenario: "design-admin-t5-oslogin", path: OSLOGIN, title: "OS Login: edit a mapping, the name cannot change", why: O_WHY, steps: editMapping }),
  frame("E11", { scenario: "design-admin-t5-oslogin", path: OSLOGIN, title: "OS Login: delete a mapping asks first", why: O_WHY, steps: deleteMapping }),
  frame("E12", { scenario: "design-admin-t5-oslogin", path: OSLOGIN, title: "OS Login: the hub refuses the choice (422), what was typed stays", why: O_WHY, before: osLoginPut(422, "Unprocessable"), steps: then(addMapping, fillMapping("dana_okoye_work", 1), saveMapping, async ({ page }) => {
    await dialog(page).getByText(/could not be saved|Unprocessable/).first().waitFor();
  }) }),
  frame("E13", { scenario: "design-admin-t5-oslogin", path: OSLOGIN, title: "OS Login: someone else changed it (409)", why: O_WHY, before: osLoginPut(409, "Conflict"), steps: then(addMapping, fillMapping("dana_okoye_work", 1), saveMapping, async ({ page }) => {
    await dialog(page).getByText(/same time/).first().waitFor();
  }) }),
  frame("E14", { scenario: "design-admin-t5-oslogin", path: OSLOGIN, title: "OS Login: the list of users could not be read", why: O_WHY, before: ({ page }) => failRequests(page, /\/api\/v1\/admin\/users(\?|$)/), steps: then(addMapping, async ({ page }) => {
    await dialog(page).getByText(/The list of users could not be read/).first().waitFor();
  }) }),
  frame("E15", { scenario: "design-admin-t5-oslogin", path: OSLOGIN, title: "OS Login: saved", why: O_WHY, steps: then(addMapping, fillMapping("dana_okoye_work", 1), saveMapping, mappingSaved) }),

  // Severity rules.
  ...states("F", "Severity rules", SEVERITY, "design-admin-t5-severity", /\/api\/v1\/admin\/severity-rules(\?|$)/, R_WHY),
  frame("F05", { scenario: "design-admin-t5-severity", path: SEVERITY, theme: "dark", title: "Severity rules: the admin's rules, dark", why: R_WHY }),
  frame("F06", { scenario: "design-admin-t5-severity", path: SEVERITY, title: "Severity rules: add a rule, no condition yet: it would raise everything", why: R_WHY, steps: addRule }),
  frame("F07", { scenario: "design-admin-t5-severity", path: SEVERITY, title: "Severity rules: add a rule with a path and a finding type", why: R_WHY, steps: then(addRule, fillRule({ id: "ssl-critical", name: "SSL config is Critical", path: "security/ssl/**" }), checkKind("Duplicate keys"), tableHeaderStaysUnderDialog) }),
  frame("F08", { scenario: "design-admin-t5-severity", path: SEVERITY, title: "Severity rules: a change type has nothing to match yet (D103)", why: R_WHY, steps: then(addRule, fillRule({ id: "removed", name: "Removed settings" }), checkKind("Setting removed")) }),
  frame("F09", { scenario: "design-admin-t5-severity", path: SEVERITY, title: "Severity rules: no id and no name", why: R_WHY, steps: then(addRule, saveRule, invalidInDialog) }),
  frame("F10", { scenario: "design-admin-t5-severity", path: SEVERITY, title: "Severity rules: a glob that cannot be read", why: R_WHY, steps: then(addRule, fillRule({ id: "odd", name: "Odd", path: "[" }), saveRule, invalidInDialog) }),
  frame("F11", { scenario: "design-admin-t5-severity", path: SEVERITY, title: "Severity rules: edit a rule, the id cannot change", why: R_WHY, steps: editRule(/^Edit rule payments/) }),
  frame("F12", { scenario: "design-admin-t5-severity", path: SEVERITY, title: "Severity rules: delete a rule asks first", why: R_WHY, steps: async ({ page }) => {
    await page.getByRole("button", { name: /^Delete rule payments/ }).click();
    await dialog(page).waitFor();
  } }),
  frame("F13", { scenario: "design-admin-t5-severity", path: SEVERITY, title: "Severity rules: someone else changed the rules (409)", why: R_WHY, before: refuse(/\/api\/v1\/admin\/severity-rules\/[^/?]+$/, "PUT", 409, "Conflict", "conflict"), steps: then(addRule, fillRule({ id: "ssl-critical", name: "SSL config is Critical", path: "security/ssl/**" }), saveRule, async ({ page }) => {
    await dialog(page).getByText(/same time/).first().waitFor();
  }) }),
  frame("F14", { scenario: "design-admin-t5-severity", path: SEVERITY, title: "Severity rules: saved", why: R_WHY, steps: then(addRule, fillRule({ id: "ssl-critical", name: "SSL config is Critical", path: "security/ssl/**" }), saveRule, ruleSaved) }),

  // Retention.
  ...states("G", "Retention", RETENTION, "design-admin-t5-retention", /\/api\/v1\/admin\/retention(\?|$)/, T_WHY),
  frame("G05", { scenario: "design-admin-t5-retention", path: RETENTION, theme: "dark", title: "Retention: the saved policy and its dry run, dark", why: T_WHY }),
  frame("G06", { scenario: "design-admin-t5-retention-defaults", path: RETENTION, theme: "dark", title: "Retention: never changed, dark", why: T_WHY }),
  frame("G07", { scenario: "design-admin-t5-retention-defaults", path: RETENTION, title: "Retention: never changed, and a dry run of the defaults", why: T_WHY, steps: then(dryRun, dryRunShown) }),
  frame("G08", { scenario: "design-admin-t5-retention", path: RETENTION, title: "Retention: values changed, the dry run not yet run: Save is off with its reason", why: T_WHY, steps: setPolicy("90", "50") }),
  frame("G09", { scenario: "design-admin-t5-retention", path: RETENTION, title: "Retention: a dry run of 90 days: what would be deleted", why: T_WHY, steps: then(setPolicy("90", "50"), dryRun, dryRunShown) }),
  frame("G10", { scenario: "design-admin-t5-retention", path: RETENTION, theme: "dark", title: "Retention: a dry run, dark", why: T_WHY, steps: then(setPolicy("90", "50"), dryRun, dryRunShown) }),
  frame("G11", { scenario: "design-admin-t5-retention", path: RETENTION, title: "Retention: the values changed after the dry run", why: T_WHY, steps: then(setPolicy("90", "50"), dryRun, dryRunShown, setPolicy("60", "50")) }),
  frame("G12", { scenario: "design-admin-t5-retention", path: RETENTION, title: "Retention: a dry run that finds nothing to delete", why: T_WHY, before: nothingToDelete, steps: then(setPolicy("365", "50"), dryRun, dryRunShown) }),
  frame("G13", { scenario: "design-admin-t5-retention", path: RETENTION, title: "Retention: the hub sent no preview, so the policy cannot be saved", why: T_WHY, before: noPreview, steps: then(setPolicy("90", "50"), dryRun, async ({ page }) => {
    await page.getByText("The hub sent no preview for this policy").waitFor();
  }) }),
  frame("G14", { scenario: "design-admin-t5-retention", path: RETENTION, title: "Retention: the dry run could not be run (500)", why: T_WHY, before: refuse(/\/api\/v1\/admin\/retention\?.*dry_run=true/, "PUT", 500, "Internal error", "internal"), steps: then(setPolicy("90", "50"), dryRun, async ({ page }) => {
    await page.getByText(/could not be run|Nothing was changed/).first().waitFor();
  }) }),
  frame("G15", { scenario: "design-admin-t5-retention", path: RETENTION, title: "Retention: below the minimum, and a count missing", why: T_WHY, steps: then(setPolicy("20", ""), dryRun, async ({ page }) => {
    await page.getByText("Minimum is 30 days").waitFor();
  }) }),
  frame("G16", { scenario: "design-admin-t5-retention", path: RETENTION, title: "Retention: a policy that deletes asks for the words to be typed", why: T_WHY, steps: then(setPolicy("90", "50"), dryRun, dryRunShown, savePolicy) }),
  frame("G17", { scenario: "design-admin-t5-retention", path: RETENTION, theme: "dark", title: "Retention: the typed confirmation, words typed, dark", why: T_WHY, steps: then(setPolicy("90", "50"), dryRun, dryRunShown, savePolicy, typeConfirm) }),
  frame("G18", { scenario: "design-admin-t5-retention", path: RETENTION, title: "Retention: a policy that deletes nothing now is a plain confirmation", why: T_WHY, before: nothingToDelete, steps: then(setPolicy("365", "50"), dryRun, dryRunShown, savePolicy) }),
  frame("G19", { scenario: "design-admin-t5-retention", path: RETENTION, title: "Retention: someone else changed the policy (409)", why: T_WHY, before: refuse(/\/api\/v1\/admin\/retention$/, "PUT", 409, "Conflict", "conflict"), steps: then(setPolicy("90", "50"), dryRun, dryRunShown, savePolicy, typeConfirm, async ({ page }) => {
    await dialog(page).getByRole("button", { name: /^Save/ }).click();
    await dialog(page).getByText(/same time/).first().waitFor();
  }) }),
  frame("G20", { scenario: "design-admin-t5-retention", path: RETENTION, title: "Retention: saved", why: T_WHY, steps: then(setPolicy("90", "50"), dryRun, dryRunShown, savePolicy, typeConfirm, confirmSave) }),
];

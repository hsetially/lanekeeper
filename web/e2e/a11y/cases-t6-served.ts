// The states of "As served" (T6, slice C): the page of a swimlane that asks the config-server, through the swimlane's agent, what it
// would serve for one file, application, tenant and channel (`/swimlanes/:id/served`, the address the MCP tool `get_served_file` links
// to). The data is `design-served-*`: the text (with the placeholder it left and the file the engine reads), the same text with a
// value the hub masked, a file the hub withholds, an answer that is not text, an empty file, a selection the config-server has nothing
// for (404), and an offline agent (503). Around them the states of the form and of the request: nothing chosen yet, an address that
// cannot be a request, the tenants known or not, a held request, the error (503, 500, no answer), an answer that may be out of date,
// a refresh that failed, a request on its way again, a long text and long lists of sources and placeholders.
import { expect } from "@playwright/test";
import type { Case } from "./helpers.ts";
import { FROZEN_NOW, API, SERVED_BASE, longText, patchLane, patchServed, refuse, dropped, servedUrl } from "./t6.ts";
import { hold } from "./helpers.ts";
import { requestsQuiet } from "../layout/audit.ts";

const TEXT = "design-served-text";

/** The answer shows: the facts about it, one line above the text. */
const FACTS = "About this answer";

/** Sixteen sources (the most the hub sends), each a long path. */
const sources = (n: number) =>
  Array.from({ length: n }, (_, i) => ({ path: `tx-infinity-api/channels/beta/regions/eu-west/applications/tx-infinity-core/overrides/override-${String(i).padStart(2, "0")}-sit7.yml`, rank: i }));

/** Placeholders the config-server left, with long names (the banner names the first twelve and counts the rest). */
const placeholders = (n: number) => Array.from({ length: n }, (_, i) => `INTEGRATION_${String(i).padStart(2, "0")}_ENDPOINT_BASE_URL_FOR_THE_PARTNER_GATEWAY`);

export const t6ServedCases: Case[] = [
  // ---- The form, before anything is asked -------------------------------------------------------------------------------------------------------
  {
    id: "served / nothing chosen yet (the form is empty and nothing is asked)",
    scenario: TEXT,
    url: SERVED_BASE,
    async open({ page, harness }) {
      await expect(page.getByText("Choose what to ask for")).toBeVisible();
      await expect(page.getByRole("button", { name: "Show served text" })).toBeEnabled();
      expect(harness.requests().filter((r) => r.path.includes("/served"))).toEqual([]);
    },
  },
  {
    id: "served / opened from a file (the file and the tenant are filled in, the application is left to choose)",
    scenario: TEXT,
    url: `${SERVED_BASE}?file=${encodeURIComponent("tx-infinity-api/connection-pool.yml")}&tenant=sit7`,
    async open({ page }) {
      await expect(page.getByRole("textbox", { name: "File" })).toHaveValue("tx-infinity-api/connection-pool.yml");
      await expect(page.getByRole("combobox", { name: "Tenant" })).toHaveValue("sit7");
      await expect(page.getByText("Choose what to ask for")).toBeVisible();
    },
  },
  {
    id: "served / the address has an application that cannot be one (the field says what is wrong)",
    scenario: TEXT,
    url: servedUrl({ application: "not an application!" }),
    async open({ page }) {
      await expect(page.getByText("That selection can't be asked for")).toBeVisible();
      await expect(page.getByRole("textbox", { name: "Application" })).toHaveAttribute("aria-invalid", "true");
    },
  },
  {
    id: "served / the address has a file that leaves the root (every wrong field says so)",
    scenario: TEXT,
    url: servedUrl({ file: "../../etc/passwd", tenant: "SIT 7", channel: "no channel!" }),
    async open({ page }) {
      await expect(page.getByText("That selection can't be asked for")).toBeVisible();
      await expect(page.getByRole("textbox", { name: "File" })).toHaveAttribute("aria-invalid", "true");
      await expect(page.getByRole("textbox", { name: "Channel" })).toHaveAttribute("aria-invalid", "true");
    },
  },
  {
    id: "served / Show pressed with nothing filled in (the first wrong field has focus)",
    scenario: TEXT,
    url: SERVED_BASE,
    async open({ page }) {
      await page.getByRole("button", { name: "Show served text" }).click();
      await expect(page.getByRole("textbox", { name: "File" })).toBeFocused();
      await expect(page.getByText("A file is required")).toBeVisible();
      await expect(page.getByText("An application is required")).toBeVisible();
      await expect(page.getByText("A tenant is required")).toBeVisible();
    },
  },
  {
    id: "served / a tenant the swimlane does not have (kept in the list and named)",
    scenario: TEXT,
    url: servedUrl({ tenant: "sit3" }),
    async open({ page }) {
      await expect(page.getByRole("option", { name: /sit3 \(not a tenant of this swimlane\)/ })).toBeAttached();
      await expect(page.getByRole("group", { name: FACTS })).toBeVisible();
    },
  },
  {
    id: "served / the swimlane's tenants are not known (the tenant is typed)",
    scenario: TEXT,
    url: servedUrl(),
    async before({ page }) {
      await patchLane(page, (lane) => ({ ...lane, tenants: [] }));
    },
    async open({ page }) {
      await expect(page.getByRole("textbox", { name: "Tenant" })).toHaveValue("sit7");
      await expect(page.getByRole("group", { name: FACTS })).toBeVisible();
    },
  },

  // ---- The answer ----------------------------------------------------------------------------------------------------------------------------------
  {
    id: "served / the text (the facts, the placeholder it left, the lines, the files the engine reads)",
    scenario: TEXT,
    url: servedUrl(),
    async open({ page }) {
      await expect(page.getByRole("group", { name: FACTS })).toBeVisible();
      await expect(page.getByText("1 placeholder left as written")).toBeVisible();
      await expect(page.getByText("Read-only view of the config-server's answer")).toBeVisible();
      await expect(page.getByText("Comparison with the rendered view is not available yet.")).toBeVisible();
    },
  },
  {
    id: "served / a request with a channel (the answer names it)",
    scenario: TEXT,
    url: servedUrl({ tenant: "sit9", channel: "beta" }),
    async open({ page }) {
      await expect(page.getByRole("group", { name: FACTS })).toContainText("channel beta");
    },
  },
  {
    id: "served / a value the hub masked (a hidden value: no Reveal, no copy)",
    scenario: "design-served-masked",
    url: servedUrl(),
    async open({ page }) {
      await expect(page.getByRole("group", { name: FACTS })).toBeVisible();
      await expect(page.getByRole("button", { name: /Reveal/ })).toHaveCount(0);
    },
  },
  {
    id: "served / the hub withholds the file (the hash, no text, and no copy of the hash)",
    scenario: "design-served-withheld",
    url: servedUrl({ file: "security/keystore.jks" }),
    async open({ page }) {
      await expect(page.getByText("Content withheld")).toBeVisible();
    },
  },
  {
    id: "served / the answer is not text (a binary file)",
    scenario: "design-served-not-text",
    url: servedUrl({ file: "tx-infinity-api/logo.png" }),
    async open({ page }) {
      await expect(page.getByText("Not shown as text")).toBeVisible();
    },
  },
  {
    id: "served / the config-server served an empty file",
    scenario: "design-served-empty-file",
    url: servedUrl(),
    async open({ page }) {
      await expect(page.getByText("The config-server served an empty file")).toBeVisible();
    },
  },
  {
    id: "served / not served for this selection (404: a state, not a fault)",
    scenario: "design-served-not-served",
    url: servedUrl(),
    async open({ page }) {
      await expect(page.getByText("Not served for this selection")).toBeVisible();
      await expect(page.getByRole("button", { name: "Ask again" })).toBeVisible();
    },
  },
  {
    id: "served / the agent is offline (the banner, and the request fails: 503)",
    scenario: "design-served-agent-offline",
    url: servedUrl(),
    async open({ page }) {
      await expect(page.getByText("Agent for sitb is offline")).toBeVisible();
      await expect(page.getByText("req-", { exact: false }).first()).toBeVisible();
    },
  },
  {
    id: "served / a long text (a thousand lines, read in a window of them)",
    scenario: TEXT,
    url: servedUrl(),
    async before({ page }) {
      await patchServed(page, (served) => ({ ...served, text: longText(1000), masks: undefined }));
    },
    async open({ page }) {
      await expect(page.getByText("1,000 lines")).toBeVisible();
    },
  },
  {
    id: "served / sixteen sources and forty placeholders, with long names (the lists wrap inside the page)",
    scenario: TEXT,
    url: servedUrl(),
    async before({ page }) {
      await patchServed(page, (served) => ({ ...served, sources: sources(16), unresolved_placeholders: placeholders(40) }));
    },
    async open({ page }) {
      await expect(page.getByText("40 placeholders left as written")).toBeVisible();
      await expect(page.getByText("and 28 more")).toBeVisible();
      await expect(page.getByRole("list", { name: "Files the engine reads for this selection" }).getByRole("listitem")).toHaveCount(16);
    },
  },

  // ---- The request and its failures -------------------------------------------------------------------------------------------------------------
  {
    id: "served / the request is out (held: the skeleton and the button says it is asking)",
    scenario: TEXT,
    url: servedUrl(),
    async before({ page }) {
      await hold(page, API.served);
    },
    async open({ page }) {
      await expect(page.getByRole("button", { name: /Asking the config-server/ })).toBeVisible();
    },
  },
  {
    id: "served / the request failed (503 with a request id: shown at once, with Retry, never tried again by itself)",
    scenario: TEXT,
    url: servedUrl(),
    async before({ page }) {
      await refuse(page, API.served, 503, "Service unavailable");
    },
    async open({ page, harness }) {
      await expect(page.getByText("req-a11y-0001").first()).toBeVisible();
      await expect(page.getByRole("button", { name: "Retry" })).toBeVisible();
      expect(harness.requests().filter((r) => r.path.includes("/served")).length, "the page does not try a 503 again by itself").toBeLessThanOrEqual(1);
    },
  },
  {
    id: "served / the request failed (500)",
    scenario: TEXT,
    url: servedUrl(),
    async before({ page }) {
      await refuse(page, API.served, 500, "Something went wrong on the server");
    },
    async open({ page }) {
      await expect(page.getByText("req-a11y-0001").first()).toBeVisible();
    },
  },
  {
    id: "served / the request got no answer (the network failed)",
    scenario: TEXT,
    url: servedUrl(),
    async before({ page }) {
      await dropped(page, API.served);
    },
    async open({ page }) {
      await expect(page.getByRole("button", { name: "Retry" })).toBeVisible({ timeout: 20_000 });
    },
  },
  {
    id: "served / asked again (the second request is out: the text stays, the button says it is asking)",
    scenario: TEXT,
    url: servedUrl(),
    async open({ page }) {
      await expect(page.getByRole("group", { name: FACTS })).toBeVisible();
      await hold(page, API.served);
      await page.getByRole("button", { name: "Ask again" }).click();
      await expect(page.getByRole("button", { name: /Asking/ }).first()).toBeVisible();
      await expect(page.getByText("Read-only view of the config-server's answer")).toBeVisible();
    },
  },
  {
    id: "served / asked again and the config-server failed (the last answer stays, with why it may be old)",
    scenario: TEXT,
    url: servedUrl(),
    async open({ page }) {
      await expect(page.getByRole("group", { name: FACTS })).toBeVisible();
      await refuse(page, API.served, 503, "Service unavailable");
      await page.getByRole("button", { name: "Ask again" }).click();
      await expect(page.getByText("Couldn't refresh").first()).toBeVisible({ timeout: 15_000 });
      // The refresh that failed has a request ID, which a report names (the first load's error panel has one too).
      await expect(page.getByRole("status").filter({ hasText: "Couldn't refresh" }).getByText("req-a11y-0001")).toBeVisible();
      await expect(page.getByText("Read-only view of the config-server's answer")).toBeVisible();
    },
  },
  {
    // The answer is live and nothing tells the page it changed: after a minute it says it may be out of date, and the person asks again.
    id: "served / the answer is a minute old (it says it may be out of date)",
    scenario: TEXT,
    url: servedUrl(),
    async before({ page }) {
      await page.clock.install({ time: FROZEN_NOW });
    },
    async open({ page, harness }) {
      await expect(page.getByRole("group", { name: FACTS })).toBeVisible();
      await expect.poll(() => harness.streamCount(), { timeout: 10_000 }).toBeGreaterThan(0);
      // The first time the stream opens the page asks for what is on screen again: that is the answer whose age is measured.
      await requestsQuiet(page, harness);
      // A minute of the page's clock in steps of 14 seconds, each with the heartbeat the hub sends every 15: a jump of a minute with
      // nothing on the stream is a dead connection to the page, which asks everything again, and the answer would not be old.
      for (let i = 0; i < 5; i++) {
        harness.emit(": heartbeat\n\n");
        await page.waitForTimeout(60);
        await page.clock.runFor(14_000);
      }
      await expect(page.getByText("This view may be out of date").first()).toBeVisible({ timeout: 15_000 });
    },
  },
];

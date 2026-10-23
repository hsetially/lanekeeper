// The frames of "As served" (T6, slice C): a file as the config-server answers for one application, tenant and channel, on the page of its
// own, `/swimlanes/:id/served` (the address the hub's MCP link template sends people to). The scenario is the served slice's
// (`design-served-*`: a Viewer, a swimlane with two tenants, and one answer for every selection, which echoes what was asked).
//
// None of them is a comparison with the design: the handoff has no frame for the page (plan F8, W6, handoff gap H2), so each frame is the app
// against its own baseline, which CI generates; the rows of design/DEVIATIONS.md that make it so are named in each frame's `why`.
//
// What the frames do not pretend (M1, the plan's CG8): the real hub sends the served text as the config-server answered it, with `masks`
// only when it has them, so the plain-text frames show text with nothing masked and the masked frame shows what the hub MASKED, not what the
// web hides; the web has no Reveal on this page and no copy of the text; and there is no Rendered toggle and no comparison with the rendered
// view, so the one line that says so is asserted and nothing stands in for a control.
import type { Locator, Page } from "@playwright/test";
import type { ServedFile } from "../../../src/api/generated/model/index.ts";
import { expect } from "../../fixtures/test";
import { api, rewrite } from "../access/lib/steps";
import { FROZEN_NOW } from "../render-references.mjs";
import { expectSkeleton, failRequests, holdRequests, pauseStream, pausedMasks, expectPaused } from "../search/steps";
import { frame, pair, then, type Frame, type Step } from "../pickup/runner";
import { settle } from "../search/kit";

const FILE_PATH = "tx-infinity-api%2Fconnection-pool.yml";
const SERVED = "/swimlanes/sitb/served";
/** A complete address: the file, the application and the tenant are in it, so the page asks once when it opens. */
const ASKED = `${SERVED}?application=tx-infinity-core&tenant=sit7&file=${FILE_PATH}`;
const SERVED_API = api("/swimlanes/sitb/served");

const PAGE_WHY = "As served has no frame in the handoff (design/DEVIATIONS.md: \"As served (/swimlanes/:id/served)\", handoff gap H2)";
const PICKERS_WHY = "the pickers are typed fields and a select of the swimlane's own tenants, and nothing is asked until the button is pressed (design/DEVIATIONS.md: \"As served: pickers\", \"As served (/swimlanes/:id/served)\")";
const TEXT_WHY = "the served text is read-only, with no Reveal, no copy of the text and no copy of a withheld file's hash, and a 404 is a state (design/DEVIATIONS.md: \"As served: text and states\", \"As served (/swimlanes/:id/served)\", \"File: view\")";
const RETRY_WHY = "an error is shown at once with its request id and \"Ask again\", never tried again by itself (design/DEVIATIONS.md: \"As served: no automatic retry\", \"As served: text and states\")";
const LINK_WHY = "the link sits among the file actions, where the design has no place for it (design/DEVIATIONS.md: \"File: As served link\", \"File: view\")";

const NARROW = { width: 1024, height: 768 } as const;

// ----- steps and reads -------------------------------------------------------------------------------------------------------------

const form = (page: Page): Locator => page.getByRole("form", { name: "What to ask the config-server for" });
const answer = (page: Page): Locator => page.getByRole("region", { name: "The config-server's answer" });
const facts = (page: Page): Locator => page.getByRole("group", { name: "About this answer" });
const field = (page: Page, name: "File" | "Application" | "Tenant" | "Channel"): Locator => form(page).getByLabel(name, { exact: true });

/** What is never on this page, whatever the answer: a Reveal, a copy of the text, the Rendered toggle and a comparison control. */
const NEVER: Step = async ({ page }) => {
  const main = page.getByRole("main");
  await expect(main.getByRole("button", { name: /reveal/i })).toHaveCount(0);
  await expect(main.getByRole("button", { name: /copy (the )?(served )?text/i })).toHaveCount(0);
  await expect(main.getByRole("tab", { name: /rendered/i })).toHaveCount(0);
  await expect(main.getByRole("switch", { name: /rendered/i })).toHaveCount(0);
  await expect(main.getByRole("button", { name: /compare|rendered/i })).toHaveCount(0);
};

/** An answer is on screen: the facts bar, and the one line that says the comparison is not there yet. */
const answered: Step = async (env) => {
  const { page } = env;
  await expect(facts(page)).toBeVisible();
  await expect(answer(page).getByText("Comparison with the rendered view is not available yet.")).toBeVisible();
  await NEVER(env);
};

/** What the form asked for and what the page says it was asked for agree: the fields hold the address, the facts bar names the same. */
const asked = (values: { file: string; application: string; tenant: string; channel?: string }): Step => async ({ page }) => {
  await expect(field(page, "File")).toHaveValue(values.file);
  await expect(field(page, "Application")).toHaveValue(values.application);
  await expect(field(page, "Tenant")).toHaveValue(values.tenant);
  await expect(field(page, "Channel")).toHaveValue(values.channel ?? "");
};

const CORE: Parameters<typeof asked>[0] = { file: "tx-infinity-api/connection-pool.yml", application: "tx-infinity-core", tenant: "sit7" };

/** Nothing is loading and nothing more is asked: the first answer and the refresh the live stream's first frame brings have both arrived. */
const quiet: Step = async ({ page, harness }) => {
  await settle(page, harness);
};

/** The answer on screen is asked for again and the new request is held: the button and "Ask again" say so. */
const askAgainHeld: Step = async ({ page }) => {
  await page.route(SERVED_API, () => {
    // Left unanswered on purpose: the second request is out.
  });
  await answer(page).getByRole("button", { name: "Ask again" }).click();
  await expect(answer(page).getByRole("button", { name: "Asking…" })).toBeVisible();
};

/** The next ask is refused (503): the last answer stays on screen, and the page says it could not refresh it (the shared stale banner, which has Refresh and no request id). */
const askAgainFails: Step = async ({ page }) => {
  await failRequests(page, SERVED_API, 503, "Service unavailable");
  await answer(page).getByRole("button", { name: "Ask again" }).click();
  await expect(page.getByText("Couldn't refresh")).toBeVisible();
};

/** More than a minute goes by on screen: the answer is kept and says it may be out of date. The clock is the page's own (installed before it opens). */
const aMinuteLater: Step = async ({ page }) => {
  await page.clock.fastForward(61_000);
  await expect(page.getByText("This view may be out of date")).toBeVisible();
};

/** The page's clock is a fake one that starts at the design's instant and runs on, so that it can be moved on. */
const clockThatMoves: Step = async ({ page }) => {
  await page.clock.install({ time: FROZEN_NOW });
};

const typeInto = (name: Parameters<typeof field>[1], value: string): Step => async ({ page }) => {
  await field(page, name).fill(value);
};

const show = (page: Page): Locator => form(page).getByRole("button", { name: "Show served text" });

/** The hub's answer with more placeholders than the banner names, and text that is not what it looks like (an override, markup). */
const MANY_KEYS = Array.from({ length: 14 }, (_, i) => `KEY_${String(i + 1).padStart(2, "0")}`);
const hostileAnswer: Step = async ({ page }) => {
  await rewrite(page, SERVED_API, (body) => {
    const served = body as unknown as ServedFile;
    return {
      ...served,
      file: "tx-infinity-api/inv\u202Eoice\u200B.yml",
      application: "tx-infinity-core\u202E",
      unresolved_placeholders: ["AUDIT\u202ESINK", "<img src=x onerror=alert(1)>", "zero\u200Bwidth", ...MANY_KEYS],
      sources: [
        { path: "tx-infinity-api/inv\u202Eoice-sit7.yml", rank: 0 },
        { path: "tx-infinity-api/<script>alert(1)</script>.yml", rank: 1 },
        { path: `tx-infinity-api/${"deep/".repeat(40)}pool-sit7.yml`, rank: 2 },
      ],
    };
  });
};

// ----- frames ----------------------------------------------------------------------------------------------------------------------

/** The seven scenarios of the slice, each at a complete address. */
const SEVEN: ReadonlyArray<{ code: string; scenario: string; title: string; check: Step }> = [
  {
    code: "S01",
    scenario: "design-served-text",
    title: "a resource file as served: its facts, one placeholder left as written, the engine's source, and the line about the comparison",
    check: then(answered, asked(CORE), async ({ page }) => {
      await expect(facts(page)).toContainText("tx-infinity-core");
      await expect(facts(page)).toContainText("sit7");
      await expect(facts(page)).toContainText("no channel");
      await expect(facts(page)).toContainText("Resource file");
      await expect(page.getByText("1 placeholder left as written")).toBeVisible();
      await expect(answer(page).getByRole("list", { name: "Files the engine reads for this selection" })).toContainText("tx-infinity-api/connection-pool-sit7.yml");
      // The text is the hub's as it came: nothing here claims a mask the hub did not send.
      await expect(page.getByRole("table", { name: "Served text of connection-pool.yml" })).toContainText("password: ********");
      await expect(page.getByText("Asked 2025-10-09 08:53:20 UTC")).toBeVisible();
    }),
  },
  {
    code: "S02",
    scenario: "design-served-masked",
    title: "the same file with the password masked by the hub: a hidden value that the page cannot reveal",
    check: then(answered, async ({ page }) => {
      // The hub masked it: the text holds a marker and `masks` says where, and the page draws a token and never the marker's words.
      const token = page.getByRole("main").locator("[data-slot='hidden-value']");
      await expect(token).toHaveCount(1);
      await expect(token).toContainText("Hidden value");
      await expect(page.getByRole("main").getByText("generic_secret")).toHaveCount(0);
      await expect(page.getByRole("main").getByRole("button", { name: /reveal|show value/i })).toHaveCount(0);
    }),
  },
  {
    code: "S03",
    scenario: "design-served-withheld",
    title: "a keystore the hub never shows: its hash and no text, and the hash has no copy button",
    check: then(async ({ page }) => {
      await expect(page.getByText("Content withheld")).toBeVisible();
      await expect(facts(page)).toBeVisible();
      // A fact about a file the page will not show is nothing worth pasting elsewhere: the path has its copy button, the hash has none.
      await expect(facts(page).getByRole("button", { name: /copy hash/i })).toHaveCount(0);
      await expect(facts(page).getByRole("button", { name: /copy path/i })).toBeVisible();
      await expect(answer(page).getByText("Comparison with the rendered view is not available yet.")).toBeVisible();
    }, NEVER),
  },
  {
    code: "S04",
    scenario: "design-served-not-text",
    title: "an answer that is not text (a binary file): not withheld, and no text to draw",
    check: then(async ({ page }) => {
      await expect(page.getByText("Not shown as text")).toBeVisible();
      await expect(facts(page).getByRole("button", { name: /copy hash/i })).toBeVisible();
    }, NEVER),
  },
  {
    code: "S05",
    scenario: "design-served-empty-file",
    title: "the config-server served a file of 0 bytes",
    check: then(async ({ page }) => {
      await expect(page.getByText("The config-server served an empty file")).toBeVisible();
    }, NEVER),
  },
  {
    code: "S06",
    scenario: "design-served-not-served",
    title: "the config-server has nothing for the selection (404): a state, not a fault, with the request id and Ask again",
    check: then(async ({ page }) => {
      await expect(page.getByText("Not served for this selection")).toBeVisible();
      await expect(page.locator("[data-slot='not-served']").getByText(/req-mock-/)).toBeVisible();
      await expect(page.locator("[data-slot='not-served']").getByRole("button", { name: "Ask again" })).toBeVisible();
    }, NEVER),
  },
  {
    code: "S07",
    scenario: "design-served-agent-offline",
    title: "the swimlane's agent is offline, so the hub cannot ask (503): the warning, and the error with its request id",
    check: then(async ({ page }) => {
      await expect(page.getByText("Agent for sitb is offline")).toBeVisible();
      await expect(page.getByText(/req-mock-/).first()).toBeVisible();
    }, NEVER),
  },
];

const seven: Frame[] = SEVEN.flatMap((s) => pair(s.code, { scenario: s.scenario, path: ASKED, title: s.title, why: s.code === "S06" || s.code === "S03" || s.code === "S04" || s.code === "S05" ? TEXT_WHY : PAGE_WHY, check: s.check }));

const choose: Frame[] = [
  ...pair("S10", {
    scenario: "design-served-text",
    path: SERVED,
    title: "no query: \"Choose what to ask for\", the four fields empty, and nothing is asked",
    why: PICKERS_WHY,
    check: async ({ page, harness }) => {
      await expect(page.getByText("Choose what to ask for")).toBeVisible();
      await asked({ file: "", application: "", tenant: "", channel: "" })({ page, harness, size: { width: 1440, height: 900 } });
      expect(harness.requests().filter((r) => r.path.includes("/served?")), "nothing is asked of the config-server before the button is pressed").toEqual([]);
    },
  }),
  ...pair("S11", {
    scenario: "design-served-text",
    path: `${SERVED}?file=tx-infinity-api%2Ftx-infinity-core-sit7.yml&tenant=sit7`,
    title: "opened from the file view's link: the file and the tenant filled in, the application left to choose, nothing asked",
    why: LINK_WHY,
    check: async ({ page, harness }) => {
      await expect(page.getByText("Choose what to ask for")).toBeVisible();
      await expect(field(page, "File")).toHaveValue("tx-infinity-api/tx-infinity-core-sit7.yml");
      await expect(field(page, "Tenant")).toHaveValue("sit7");
      await expect(field(page, "Application")).toHaveValue("");
      expect(harness.requests().filter((r) => r.path.includes("/served?"))).toEqual([]);
    },
  }),
  ...pair("S12", {
    scenario: "design-served-text",
    path: `${SERVED}?application=bad%20name!&tenant=sit7&file=${FILE_PATH}`,
    title: "an application that cannot be one: \"That selection can't be asked for\", the field marked, and nothing asked",
    why: PICKERS_WHY,
    check: async ({ page, harness }) => {
      await expect(page.getByText("That selection can't be asked for")).toBeVisible();
      await expect(field(page, "Application")).toHaveAttribute("aria-invalid", "true");
      await expect(form(page).getByText(/Use letters, digits/)).toBeVisible();
      // The message names what is wrong and never repeats what was given.
      await expect(form(page).getByText("bad name!")).toHaveCount(0);
      expect(harness.requests().filter((r) => r.path.includes("/served?"))).toEqual([]);
    },
  }),
  ...pair("S13", {
    scenario: "design-served-text",
    path: `${SERVED}?application=%3Cscript%3Ealert(1)%3C%2Fscript%3E&tenant=%E2%80%AEsit7&file=..%2F..%2Fetc%2Fpasswd&channel=%00`,
    title: "a hostile address: a script as the application, an override as the tenant, a path out of the root, a NUL as the channel",
    why: PICKERS_WHY,
    check: async ({ page, harness }) => {
      await expect(page.getByText("That selection can't be asked for")).toBeVisible();
      for (const name of ["File", "Application", "Tenant", "Channel"] as const) await expect(field(page, name)).toHaveAttribute("aria-invalid", "true");
      expect(harness.requests().filter((r) => r.path.includes("/served?")), "an address that cannot be a request sends none").toEqual([]);
      // No element of the page was made by the address: the script is text in a field, and nothing ran.
      await expect(page.locator("main script")).toHaveCount(0);
    },
  }),
  ...pair("S14", {
    scenario: "design-served-text",
    path: SERVED,
    title: "a form that is pressed while it cannot be a request: the first wrong field is marked and has the focus, and nothing is asked",
    why: PICKERS_WHY,
    steps: then(typeInto("File", "/etc/passwd"), typeInto("Application", "tx-infinity-core"), async ({ page }) => {
      await show(page).click();
    }),
    check: async ({ page, harness }) => {
      await expect(field(page, "File")).toBeFocused();
      await expect(field(page, "File")).toHaveAttribute("aria-invalid", "true");
      await expect(field(page, "Tenant")).toHaveAttribute("aria-invalid", "true");
      await expect(page.getByText("A tenant is required")).toBeVisible();
      expect(harness.requests().filter((r) => r.path.includes("/served?"))).toEqual([]);
    },
  }),
  frame("S15", {
    scenario: "design-served-text",
    path: SERVED,
    size: NARROW,
    title: "1024 px wide, nothing chosen yet: the four fields wrap and the button keeps its place",
    why: PICKERS_WHY,
  }),
];

const states: Frame[] = [
  ...pair("S20", {
    scenario: "design-served-text",
    path: ASKED,
    title: "loading: the request is held, so the form is there and the answer is a skeleton",
    why: PAGE_WHY,
    before: ({ page }) => holdRequests(page, SERVED_API),
    ready: ({ page }) => expectSkeleton(page),
    check: async ({ page }) => {
      await expect(answer(page)).toBeVisible();
      // The form says a request is out, and the answer is a skeleton with nothing in it.
      await expect(form(page).getByRole("button", { name: "Asking the config-server…" })).toBeVisible();
      await expect(facts(page)).toHaveCount(0);
    },
  }),
  ...pair("S21", {
    scenario: "design-served-text",
    path: ASKED,
    title: "a 503 from the hub: the error with its request id and Retry, once, and not tried again by itself",
    why: RETRY_WHY,
    before: ({ page }) => failRequests(page, SERVED_API, 503, "Service unavailable"),
    check: async (env) => {
      await expect(env.page.getByText("req-mock-0001").first()).toBeVisible();
      await expect(answer(env.page).getByRole("button", { name: "Retry" })).toBeVisible();
      await NEVER(env);
    },
  }),
  ...pair("S22", {
    scenario: "design-served-text",
    path: ASKED,
    title: "the answer is more than a minute old: it is kept, and it says it may be out of date and since when",
    why: PAGE_WHY,
    before: clockThatMoves,
    steps: then(answered, aMinuteLater),
    check: async ({ page }) => {
      await expect(page.getByText("Asked 2025-10-09 08:53:20 UTC (1 min ago)")).toBeVisible();
      await expect(page.getByRole("table", { name: "Served text of connection-pool.yml" })).toContainText("pool:");
    },
  }),
  ...pair("S23", {
    scenario: "design-served-text",
    path: ASKED,
    title: "asked again: the request is out, the button says \"Asking…\" and the answer on screen stays",
    why: PAGE_WHY,
    steps: then(quiet, answered, askAgainHeld),
    ready: async ({ page }) => {
      await expect(answer(page).getByRole("button", { name: "Asking…" })).toBeVisible();
    },
  }),
  ...pair("S24", {
    scenario: "design-served-text",
    path: ASKED,
    title: "asked again and refused (503): the last answer stays, and the page says it could not refresh it and offers Refresh",
    why: RETRY_WHY,
    steps: then(quiet, answered, askAgainFails),
    check: async ({ page }) => {
      await expect(page.getByRole("table", { name: "Served text of connection-pool.yml" })).toContainText("pool:");
      await expect(page.getByText("Showing the last data that loaded.")).toBeVisible();
      await expect(page.getByRole("button", { name: "Refresh" })).toBeVisible();
    },
  }),
  ...pair("S25", {
    scenario: "design-served-text",
    path: ASKED,
    title: "live updates paused: the answer is kept and the page says it is read while the connection is down",
    why: PAGE_WHY,
    before: pauseStream,
    steps: then(answered, ({ page }) => expectPaused(page)),
    mask: pausedMasks,
  }),
  ...pair("S26", {
    scenario: "design-served-text",
    path: `${SERVED}?application=tx-infinity-core&tenant=sit9&channel=beta&file=${FILE_PATH}`,
    title: "a request with a channel: the facts bar names it, and the form holds it",
    why: PAGE_WHY,
    check: then(answered, async ({ page }) => {
      await expect(facts(page)).toContainText("channel");
      await expect(facts(page)).toContainText("beta");
      await expect(field(page, "Channel")).toHaveValue("beta");
      await expect(field(page, "Tenant")).toHaveValue("sit9");
    }),
  }),
  ...pair("S27", {
    scenario: "design-served-text",
    path: ASKED,
    title: "hostile answer: an override and a zero-width space in the name, placeholders and sources that are markup, and more keys than the banner names",
    why: PAGE_WHY,
    before: hostileAnswer,
    check: then(answered, async ({ page }) => {
      const text = await page.getByRole("main").innerText();
      expect(Array.from(text).filter((ch) => /[\u200B-\u200F\u202A-\u202E\u2066-\u2069\uFEFF]/.test(ch)), "no raw control character is drawn").toEqual([]);
      expect(text).toContain("<U+202E>");
      await expect(page.getByText("17 placeholders left as written")).toBeVisible();
      await expect(page.getByText("and 5 more")).toBeVisible();
      // The markup is text: the page has no element that the answer made.
      await expect(page.locator("main img[src='x']")).toHaveCount(0);
    }),
  }),
  frame("S28", {
    scenario: "design-served-text",
    path: ASKED,
    size: NARROW,
    title: "1024 px wide, an answer: the facts bar wraps and the text keeps the room",
    why: PAGE_WHY,
    check: answered,
  }),
];

const fromTheFile: Frame[] = [
  ...pair("S30", {
    scenario: "design-file-core",
    path: `/swimlanes/sitb/files?path=tx-infinity-api%2Ftx-infinity-core-sit7.yml`,
    title: "the file view has an \"As served\" link among its actions, for this file and its tenant",
    why: LINK_WHY,
    check: async ({ page }) => {
      const link = page.getByRole("link", { name: "As served" });
      await expect(link).toBeVisible();
      await expect(link).toHaveAttribute("href", "/swimlanes/sitb/served?file=tx-infinity-api%2Ftx-infinity-core-sit7.yml&tenant=sit7");
    },
  }),
  frame("S31", {
    scenario: "design-file-denied",
    path: "/swimlanes/sitb/files?path=security%2Fkeystore.jks",
    title: "a file whose content is withheld has no \"As served\" link",
    why: LINK_WHY,
    check: async ({ page }) => {
      await expect(page.getByRole("link", { name: "As served" })).toHaveCount(0);
    },
  }),
];

export const frames: Frame[] = [...seven, ...choose, ...states, ...fromTheFile];

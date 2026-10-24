// The frames of Approvals: the board frames 1a to 1u of "Lanekeeper Approvals and Audit.dc.html", and the States-index rows of
// Approvals (L09 E09 R09 P05 U05 C02).
import { SWITCHER, type Excluded, type FrameDef, type Mask } from "../search/kit";
import { expectSkeleton, failRequests, holdRequests, expectPaused, pauseStream, pausedMasks } from "../search/steps";
import { frame } from "../search/sse";

const QUEUE = "design-approvals-pending";
const DRAWER = "the drawer's consequence block, evidence and actions differ from the design by documented rows (design/DEVIATIONS.md: the \"Approvals: ...\" rows: \"If you approve\", drawer actions, copy, live arrival)";
const PERMISSION = "the permission state is the shared kit's: \"Approvals needs the Admin role\" with Request a role change, in the page frame, with Approvals hidden from the sidebar (design/DEVIATIONS.md: \"Permission state\", \"Sidebar: Audit and Approvals\")";
const EMPTY = "the empty queue draws no tab counts and no table header, and the design's frame keeps the counts of the full queue in its tabs (design/DEVIATIONS.md: \"Approvals: tabs\")";
const SKELETON = "the loading skeleton is the shared list skeleton with a \"Loading proposals\" line, six rows, the design draws eight (States index: skeletons match each screen's own grid)";
const APP = "the drawer and the queue differ from the design by documented rows (design/DEVIATIONS.md, \"Approvals: ...\")";

// What the design draws and the app does not, as regions painted over in both images.
const LIVE: Mask = { x: 1280, y: 86, width: 140, height: 22, row: "Approvals (the live line counts from the clock: \"updated 2 s ago\" in the design, \"just now\" in the app)" };
const TABS: Mask = { x: 440, y: 121, width: 175, height: 38, row: "Approvals: tabs (counts for Pending and Stale always, Expired and Decided only while open)" };
const QUEUE_ROWS: Mask = { x: 249, y: 162, width: 1166, height: 690, row: "queue rows: the shared virtualised table draws fixed 48 px rows and its own header, the design's rows are as tall as their content (no DEVIATIONS row yet: reported)" };

/** Open a proposal from the queue by its number, through the address the app uses for the drawer. */
const drawer = (n: number, tab = "pending"): Pick<FrameDef, "path"> => ({ path: tab === "pending" ? `/approvals/${n}` : `/approvals/${n}?state=${tab}` });

export const frames: FrameDef[] = [
  { board: "approvals", frame: "1a", kind: "tier1", scenario: QUEUE, path: "/approvals", title: "Pending", masks: [SWITCHER, LIVE, TABS, QUEUE_ROWS] },
  { board: "approvals", frame: "1b", kind: "tier1", scenario: QUEUE, path: "/approvals?state=stale", title: "Stale", masks: [SWITCHER, LIVE, TABS, QUEUE_ROWS] },
  { board: "approvals", frame: "1c", kind: "tier1", scenario: QUEUE, path: "/approvals?state=expired", title: "Expired", masks: [SWITCHER, LIVE, TABS, QUEUE_ROWS] },
  { board: "approvals", frame: "1d", kind: "app", scenario: QUEUE, path: "/approvals?state=decided", title: "Decided (the hub answers 400 until prompt 06)", why: "the Decided tab asks `decided=true`, which the hub refuses until prompt 06 implements it (design/DEVIATIONS.md: \"Approvals: Decided tab\")" },
  { board: "approvals", frame: "1e", kind: "app", scenario: QUEUE, ...drawer(2046), title: "Edit, consequence then diff" , why: DRAWER },
  {
    board: "approvals", frame: "1f", kind: "app", scenario: QUEUE, ...drawer(2046), title: "Reject, reason filled in", why: DRAWER,
    steps: async ({ page }) => {
      await page.getByRole("textbox", { name: /Reason/ }).fill("Raise maxRetries to 4 first. 5 hits the downstream limit.");
    },
  },
  { board: "approvals", frame: "1g", kind: "app", scenario: QUEUE, ...drawer(2047), title: "AI-made edit via Cursor" , why: DRAWER },
  { board: "approvals", frame: "1h", kind: "app", scenario: QUEUE, ...drawer(2040), title: "Upload" , why: DRAWER },
  { board: "approvals", frame: "1i", kind: "app", scenario: QUEUE, ...drawer(2045), title: "Delete" , why: DRAWER },
  { board: "approvals", frame: "1j", kind: "app", scenario: QUEUE, ...drawer(2043), title: "Restart" , why: DRAWER },
  { board: "approvals", frame: "1k", kind: "app", scenario: QUEUE, ...drawer(2044), title: "Your own proposal" , why: DRAWER },
  { board: "approvals", frame: "1l", kind: "app", scenario: QUEUE, ...drawer(2041, "stale"), title: "Stale, the file changed" , why: DRAWER },
  { board: "approvals", frame: "1m", kind: "app", scenario: QUEUE, ...drawer(2042, "stale"), title: "Stale, changed outside Lanekeeper" , why: DRAWER },
  { board: "approvals", frame: "1n", kind: "app", scenario: QUEUE, ...drawer(2037, "expired"), title: "Expired, read-only" , why: DRAWER },
  { board: "approvals", frame: "1o", kind: "app", scenario: QUEUE, ...drawer(2032, "decided"), title: "Decided, rejected with the reason", why: "the Decided tab is an error until prompt 06 implements `decided=true`; the drawer of a decided proposal is read from the proposal itself" },
  { board: "approvals", frame: "1p", kind: "app", scenario: "design-approvals-editor", path: "/approvals", title: "Editor: Approvals needs Admin", why: PERMISSION },
  {
    board: "approvals", frame: "1q", kind: "app", scenario: QUEUE, path: "/approvals", title: "Loading", why: SKELETON,
    before: ({ page }) => holdRequests(page, /\/api\/v1\/proposals(\?|$)/),
    ready: ({ page }) => expectSkeleton(page),
  },
  { board: "approvals", frame: "1r", kind: "app", scenario: "design-approvals-empty", path: "/approvals", title: "Empty", why: EMPTY },
  {
    board: "approvals", frame: "1s", kind: "app", scenario: QUEUE, path: "/approvals", title: "A proposal arrives: tinted, with New, after Show",
    why: "an arriving proposal waits behind an \"n updates\" chip and then shows tinted with New; the design inserts it at once (design/DEVIATIONS.md: \"Approvals: live arrival\")",
    steps: async ({ page, harness }) => {
      harness.setScenario("design-approvals-arrival");
      harness.emit(frame(500, { type: "proposal_changed", id: 2048, status: "pending" }));
      await page.getByRole("button", { name: "Show" }).click();
    },
  },
  {
    board: "approvals", frame: "1t", kind: "app", scenario: QUEUE, ...drawer(2046), title: "Someone else approved the proposal you have open",
    why: "the live notice and the drawer's banner are the app's own (design/DEVIATIONS.md: \"Approvals: live arrival\"); the design inserts at once",
    steps: async ({ page, harness }) => {
      await page.route(/\/api\/v1\/proposals\/2046$/, async (route) => {
        const response = await route.fetch();
        const body = (await response.json()) as Record<string, unknown>;
        await route.fulfill({ response, json: { ...body, status: "approved", decided_by: "11111111-1111-4111-8111-111111111111:cccccccc-cccc-4ccc-8ccc-cccccccccccc", decided_at: 1_759_999_000_000 } });
      });
      harness.emit(frame(501, { type: "proposal_changed", id: 2046, status: "approved" }));
      await page.getByText(/decided|approved/i).first().waitFor();
    },
  },
  { board: "approvals", frame: "1u", kind: "app", scenario: QUEUE, ...drawer(2047), title: "AI-made edit" , why: DRAWER },
];

export const excluded: Excluded[] = [];

// The States-index rows of Approvals, light, as app-owned baselines.
export const states: FrameDef[] = [
  {
    board: "states", frame: "L09", kind: "state", scenario: QUEUE, path: "/approvals", title: "Approvals, loading", why: APP,
    before: ({ page }) => holdRequests(page, /\/api\/v1\/proposals(\?|$)/),
    ready: ({ page }) => expectSkeleton(page),
  },
  { board: "states", frame: "E09", kind: "state", scenario: "design-approvals-empty", path: "/approvals", title: "Approvals, empty", why: APP },
  { board: "states", frame: "R09", kind: "state", scenario: QUEUE, path: "/approvals", title: "Approvals, error with the request id", why: APP, before: ({ page }) => failRequests(page, /\/api\/v1\/proposals(\?|$)/) },
  { board: "states", frame: "P05", kind: "state", scenario: "design-approvals-editor", path: "/approvals", title: "Approvals, permission: needs Admin", why: APP },
  { board: "states", frame: "C02", kind: "state", scenario: "design-approvals-conflict", ...drawer(2046), title: "Approvals, 409: the file changed first", why: APP,
    steps: async ({ page }) => {
      await page.getByRole("button", { name: "Approve" }).click();
      await page.getByRole("dialog").waitFor();
    },
  },
  { board: "states", frame: "K02", kind: "state", scenario: "design-approvals-locked", ...drawer(2046), title: "Approvals, 423: the swimlane is locked", why: APP,
    steps: async ({ page }) => {
      await page.getByRole("button", { name: "Approve" }).click();
      await page.getByRole("dialog").waitFor();
    },
  },
  {
    board: "states", frame: "U05", kind: "state", scenario: QUEUE, path: "/approvals", title: "Approvals, live updates paused", why: APP,
    before: pauseStream,
    steps: ({ page }) => expectPaused(page),
    mask: pausedMasks,
  },
];

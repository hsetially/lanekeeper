// The design-parity scenarios of slice S5: Search (text, settings, docs), the doc reader, the upload, replace and delete
// dialogs, and Feature status, with the data of `Lanekeeper Search Page.dc.html` and `Lanekeeper Feature Status.dc.html` (./data.ts).
// The Search and Status board (`Lanekeeper Search and Status.dc.html`) shows every state as a Shell frame with a role, a tab and a
// query; the frame's own state (tab, query, switches) is in the address the e2e test opens, so a scenario is the DATA behind it:
//
//   design-search-admin       frames 1a to 1n, 1p to 1r and 1u to 1x: Priya Shah, an Admin; the three searches, the docs, the flags
//   design-search-editor      frames 1k, 1l, 1s, 1t and 1w: an Editor, who can replace an uploaded doc
//   design-search-viewer      frame 1o and 1w's pointer: a Viewer, whose Upload is off with the reason
//   design-search-truncated   frame 1c: a text search that reaches the cap of 500 and says so
//   design-search-invalid     frame 1d: the engine refuses the pattern (400)
//   design-search-empty       States E05 and E06: nothing found, no docs, no flags
//   design-search-error       States R05 and R06: every read of these screens answers 500
//
// Loaded by Node (the e2e harness) as well as by Vite: relative imports with the `.ts` extension and `import type`.
import type { Me } from "../../../api/generated/model/index.ts";
import { meBody } from "../../me.ts";
import { problemBody, type MockContext, type MockResponse, type Scenario } from "../../scenario.ts";
import { MARCO, PRIYA, scenarios as shellScenarios } from "../shell/index.ts";
import { DOCS, FLAGS, docHitsFor, manyTextHits, renderedDoc, settingHitsFor, textHitsFor } from "./data.ts";

const csrfCookie = "__Host-lk_csrf=mockMockMockMockMockMockMockMockMockMo0; Path=/; Secure; SameSite=Lax";
const PAGED = new URLSearchParams("q=enableAccountSorting&limit=100");

/** The signed-in user, the six swimlanes of the design and (for an Admin) the six pending approvals the sidebar counts. */
function chrome(role: "admin" | "editor" | "viewer"): MockResponse[] {
  const source = shellScenarios.find((s) => s.id === (role === "admin" ? "design-shell-admin" : "design-shell-viewer"));
  // The Shell's own answers to the palette's searches are replaced by this slice's.
  const kept = (source?.responses ?? []).filter((r) => r.path !== "/api/v1/search/settings" && r.path !== "/api/v1/docs/search" && r.path !== "/api/v1/me");
  const me: Me = role === "admin" ? meBody("admin", { id: PRIYA, display_name: "Priya Shah", email: "priya.shah@example.test" }) : meBody(role, { id: MARCO, display_name: "Marco Diaz", email: "marco.diaz@example.test" });
  return [{ method: "GET", path: "/api/v1/me", body: me, headers: { "set-cookie": csrfCookie } }, ...kept];
}

const lanesOf = (query: URLSearchParams): string[] | undefined => {
  const raw = query.get("swimlanes");
  return raw === null ? undefined : raw.split(",").filter(Boolean);
};

/** The reads of Search, Docs and Feature status over the design's data. */
function reads(): MockResponse[] {
  return [
    {
      method: "GET",
      path: "/api/v1/search/text",
      body: (ctx: MockContext) => {
        const lanes = lanesOf(ctx.query);
        const glob = ctx.query.get("path_glob");
        const hits = textHitsFor({
          pattern: ctx.query.get("pattern") ?? "",
          regex: ctx.query.get("regex") === "true",
          caseSensitive: ctx.query.get("case_sensitive") === "true",
          ...(lanes === undefined ? {} : { lanes }),
          ...(glob === null ? {} : { glob }),
        });
        return { hits: hits.slice(0, 500), truncated: hits.length > 500 };
      },
      sample: { query: new URLSearchParams("pattern=enableAccountSorting") },
      samples: [{ query: new URLSearchParams("pattern=timeoutMs&swimlanes=sit,sitb&path_glob=**/*.yml") }, { query: new URLSearchParams("pattern=nothing-at-all") }],
    },
    {
      method: "GET",
      path: "/api/v1/search/settings",
      body: (ctx: MockContext) => ({ items: settingHitsFor(ctx.query.get("q") ?? "", lanesOf(ctx.query)), next: null }),
      sample: { query: PAGED },
      samples: [{ query: new URLSearchParams("q=timeoutMs&swimlanes=sit") }, { query: new URLSearchParams("q=enableAccountSortng") }],
    },
    {
      method: "GET",
      path: "/api/v1/docs/search",
      body: (ctx: MockContext) => ({ hits: docHitsFor(ctx.query.get("q") ?? "").slice(0, Number(ctx.query.get("limit") ?? 10)) }),
      sample: { query: new URLSearchParams("q=account sorting&limit=50") },
      samples: [{ query: new URLSearchParams("q=ledger sorting&limit=50") }],
    },
    {
      method: "GET",
      path: "/api/v1/docs",
      body: (ctx: MockContext) => {
        const source = ctx.query.get("source");
        const prefix = ctx.query.get("path_prefix") ?? "";
        return { items: DOCS.filter((d) => (source === null || d.source === source) && d.path.startsWith(prefix)), next: null };
      },
      sample: { query: new URLSearchParams("limit=100") },
      samples: [{ query: new URLSearchParams("source=git&limit=100") }, { query: new URLSearchParams("path_prefix=/uploads/instant-payments.md&limit=20") }],
    },
    {
      method: "GET",
      path: "/api/v1/docs/fs/render",
      body: (ctx: MockContext) => renderedDoc(DOCS.find((d) => d.path === ctx.query.get("path")) ?? DOCS[0] ?? { id: 0, title: "", path: "", source: "git", version: 1, size: 0, updated_at: 0, updated_by: null }),
      sample: { query: new URLSearchParams("path=/git/main/docs/account-sorting.md") },
      samples: [{ query: new URLSearchParams("path=/uploads/instant-payments.md") }],
      headers: { etag: '"doc-render-1"' },
    },
    { method: "GET", path: "/api/v1/features/status", body: { flags: FLAGS } },
  ];
}

/** The writes of the docs: a doc that is created, replaced or deleted comes back as the design shows it. */
function writes(): MockResponse[] {
  const upload = DOCS[1];
  return upload === undefined
    ? []
    : [
        { method: "POST", path: "/api/v1/docs", status: 201, body: { ...upload, id: 104, title: "Receipts v4", path: "/uploads/receipts-v4.md", version: 1 } },
        { method: "PUT", path: "/api/v1/docs/{id}", body: { ...upload, version: upload.version + 1 }, sample: { params: { id: "102" } } },
        { method: "DELETE", path: "/api/v1/docs/{id}", status: 204, sample: { params: { id: "102" } } },
      ];
}

const scenario = (id: string, description: string, responses: MockResponse[]): Scenario => ({ id, description, responses });

/** Answers that replace the reads for a state, put first so that they win over the ones in `reads()`. */
const over = (role: "admin" | "editor" | "viewer", replacements: MockResponse[]): MockResponse[] => {
  const replaced = new Set(replacements.map((r) => `${r.method} ${r.path}`));
  return [...chrome(role), ...replacements, ...reads().filter((r) => !replaced.has(`${r.method} ${r.path}`)), ...writes()];
};

const problem500: Pick<MockResponse, "status" | "body"> = { status: 500, body: problemBody(500, "internal", "Internal error") };

export const scenarios: readonly Scenario[] = [
  scenario("design-search-admin", "Search (text, settings and docs), the doc reader and Feature status for Priya Shah, an Admin, over the six swimlanes of the design.", [...chrome("admin"), ...reads(), ...writes()]),
  scenario("design-search-editor", "The same screens for an Editor, who may upload a doc and replace an uploaded one.", [...chrome("editor"), ...reads(), ...writes()]),
  scenario("design-search-viewer", "The same screens for a Viewer: Upload and Replace are off, with the role that is needed.", [...chrome("viewer"), ...reads(), ...writes()]),
  scenario(
    "design-search-truncated",
    "A text search that finds more than the server will send: 500 hits and the truncated flag (frame 1c).",
    over("admin", [{ method: "GET", path: "/api/v1/search/text", body: { hits: manyTextHits("timeout", 500), truncated: true } }]),
  ),
  scenario("design-search-invalid", "The engine refuses the pattern of a text search with a 400 (frame 1d).", over("admin", [{ method: "GET", path: "/api/v1/search/text", status: 400, body: problemBody(400, "bad_request", "Bad request") }])),
  scenario(
    "design-search-empty",
    "Nothing to find: no hits, no docs, no flags (States E05 and E06).",
    over("admin", [
      { method: "GET", path: "/api/v1/search/text", body: { hits: [], truncated: false } },
      { method: "GET", path: "/api/v1/search/settings", body: { items: [], next: null } },
      { method: "GET", path: "/api/v1/docs/search", body: { hits: [] } },
      { method: "GET", path: "/api/v1/docs", body: { items: [], next: null } },
      { method: "GET", path: "/api/v1/features/status", body: { flags: [] } },
    ]),
  ),
  scenario(
    "design-search-error",
    "Every read of these screens answers 500 (States R05 and R06).",
    over("admin", [
      { method: "GET", path: "/api/v1/search/text", ...problem500 },
      { method: "GET", path: "/api/v1/search/settings", ...problem500 },
      { method: "GET", path: "/api/v1/docs/search", ...problem500 },
      { method: "GET", path: "/api/v1/docs", ...problem500 },
      { method: "GET", path: "/api/v1/docs/fs/render", ...problem500 },
      { method: "GET", path: "/api/v1/features/status", ...problem500 },
    ]),
  ),
];

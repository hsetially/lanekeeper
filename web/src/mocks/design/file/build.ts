// The builder of the file view's scenarios (S3). One scenario is the answers of the hub for one file in swimlane `sitb`, for one
// person, with one answer to each write; the variants (a Viewer, an agent that is offline, a conflict, a lock, a refusal, an open
// pull request) differ in a field or two, so they are made here from options and listed in `index.ts`.
//
// A read that depends on the request (the content of NFS, Git and the baseline; the tree of a folder; the refs; the preview of a
// target) is one response with a function for a body and a `sample` for each variant, so the contract check in
// src/mocks/mocks.test.ts reads every variant.
import type { AgentStatus, BaselineState, Comparison, FileEntry, Me, Role, SwimlaneDetail } from "../../../api/generated/model/index.ts";
import { meBody } from "../../me.ts";
import { problemBody, type MockContext, type MockResponse, type Scenario } from "../../scenario.ts";
import { makeComparison } from "../../perf/diff.ts";
import { SHELL_LANES, SHELL_USERS } from "../shell/index.ts";
import {
  binaryComparison,
  coreBaselineText,
  coreComparison,
  coreDocs,
  coreFlags,
  coreGitText,
  coreHistory,
  coreFile,
  CORE_PATH,
  H_BASELINE,
  H_GIT,
  H_NEWER,
  H_NFS,
  LANE,
  lineEndingComparison,
  MARCO,
  NOW,
  openPr,
  prJob,
  prPreview,
  PRIYA,
  TENANT,
  type FileKit,
} from "./data.ts";

const csrfCookie = "__Host-lk_csrf=mockMockMockMockMockMockMockMockMockMo0; Path=/; Secure; SameSite=Lax";
const FILES = "/api/v1/swimlanes/{id}/files";
const LANE_PARAMS = { params: { id: LANE } };

export type WriteAnswer = "applied" | "conflict" | "locked" | "approval" | "secret" | "too-large";
export type PrAnswer = "job" | "exists" | "token-expired" | "no-access" | "failed";
/** "large": the comparison of a file of 6,000 lines with a change every ten lines, as the engine sends it (the P10 gate's diff). */
export type CompareAnswer = "diff" | "eol-only" | "binary" | "identical" | "large";

/** The 6,000-line comparison is made once: it is the same for every request, and the tag names it as the hub's names the two hashes. */
let largeComparison: Comparison | undefined;
const LARGE_ETAG = '"design-file-diff-large-1"';

export type FileScenarioOptions = {
  id: string;
  description: string;
  role?: Role;
  /** The person: Priya Shah (the Admin of the design) or Marco Diaz (the Viewer). */
  as?: "priya" | "marco";
  requiresApproval?: boolean;
  needsGithubToken?: boolean;
  file?: FileKit;
  agent?: AgentStatus;
  baseline?: BaselineState;
  /** How the hub answers an edit, a revert, an upload and a delete. */
  write?: WriteAnswer;
  pr?: PrAnswer;
  compare?: CompareAnswer;
  /** The file has docs and documented flags. */
  docs?: boolean;
  /** The file has the history of the design (four versions); otherwise it has one. */
  history?: boolean;
};

function lanes(over: { agent: AgentStatus; baseline: BaselineState }) {
  return SHELL_LANES.map((lane) => (lane.id === LANE ? { ...lane, agent: over.agent, baseline: { state: over.baseline, by: null, at: null } } : lane));
}

function detail(over: { agent: AgentStatus; baseline: BaselineState }): SwimlaneDetail {
  const lane = lanes(over).find((l) => l.id === LANE);
  return {
    ...(lane as NonNullable<typeof lane>),
    drift_counts: over.baseline === "confirmed" ? [["nfs_ahead", 4]] : [["unknown", 412]],
    services: [],
    auto_notify: false,
  };
}

function siblings(file: FileKit): FileEntry[] {
  const dir = file.path.slice(0, Math.max(0, file.path.lastIndexOf("/")));
  const base = (name: string, over: Partial<FileEntry> = {}): FileEntry => ({
    path: `${dir}/${name}`,
    is_dir: false,
    kind: "base",
    class: "structured",
    role: "property_source",
    size: 4_200,
    hash: "0".repeat(63) + name.length.toString(16).slice(-1),
    drift: "in_sync",
    format: { eol: "lf", encoding: "utf8", bom: false },
    content_withheld: false,
    attribution: null,
    severity: null,
    pickup_state: null,
    logical_file: `${dir}/${name}`,
    tenant: null,
    ...over,
  });
  return [file.entry, base("tx-infinity-core.yml"), base("tx-infinity-gateway.yml", { drift: "nfs_ahead" })].filter((e, i, all) => all.findIndex((o) => o.path === e.path) === i);
}

function folder(path: string): FileEntry {
  return { path, is_dir: true, kind: "base", class: "structured", role: "resource", size: 0, hash: null, drift: "in_sync", format: null, content_withheld: false, attribution: null, severity: null, pickup_state: null, logical_file: null, tenant: null };
}

function comparisonFor(answer: CompareAnswer, file: FileKit, left: string, right: string): Comparison {
  switch (answer) {
    case "diff":
      return coreComparison(left, right);
    case "eol-only":
      return lineEndingComparison(left, right);
    case "binary":
      return binaryComparison(left, right);
    case "identical":
      return { left, right, file: file.logical ?? file.path, class: file.content.class, identical: true, eol_only: false, hunks: [], setting_changes: [], binary: null };
    case "large":
      return (largeComparison ??= makeComparison({ lines: 6000, changeEvery: 10 }));
  }
}

function editResponses(answer: WriteAnswer): Pick<MockResponse, "status" | "body"> {
  switch (answer) {
    case "applied":
      return { body: { outcome: "applied", hash: H_NEWER, audit_id: 42 } };
    case "approval":
      return { body: { outcome: "proposal_created", id: 9 } };
    case "conflict":
      return { status: 409, body: problemBody(409, "conflict", "Conflict", { current: H_NEWER }) };
    case "locked":
      return { status: 423, body: problemBody(423, "locked", "Locked", { until: null }) };
    case "secret":
      return { status: 422, body: problemBody(422, "unprocessable", "Unprocessable", { reason: "secret_detected", issues: [{ kind: "secret_match", line: 120, rule: "private_key" }] }) };
    case "too-large":
      return { status: 413, body: problemBody(413, "payload_too_large", "Payload too large") };
  }
}

function prResponses(answer: PrAnswer): MockResponse {
  const base = { method: "POST", path: "/api/v1/prs" } as const;
  switch (answer) {
    case "job":
      return { ...base, status: 202, body: prJob("pending") };
    case "exists":
      return { ...base, status: 409, body: { ...problemBody(409, "conflict", "Conflict"), reason: "duplicate_pr", link: openPr } };
    case "token-expired":
      return { ...base, status: 422, body: problemBody(422, "token_expired", "Token expired") };
    case "no-access":
      return { ...base, status: 422, body: problemBody(422, "token_missing_access", "Token cannot push", { base: true, tenant: false }) };
    case "failed":
      return { ...base, status: 202, body: prJob("pending") };
  }
}

/** One scenario of the file view. */
export function fileScenario(options: FileScenarioOptions): Scenario {
  const {
    id,
    description,
    role = "admin",
    as = role === "viewer" ? "marco" : "priya",
    requiresApproval = false,
    needsGithubToken = false,
    file = coreFile({ masked: true }),
    agent = "connected",
    baseline = "confirmed",
    write = "applied",
    pr = "job",
    compare = "diff",
    docs = true,
    history = true,
  } = options;
  const me: Me = meBody(role, as === "priya" ? { id: PRIYA, display_name: "Priya Shah", email: "priya.shah@example.test", requires_approval: requiresApproval, needs_github_token: needsGithubToken } : { id: MARCO, display_name: "Marco Diaz", email: "marco.diaz@example.test", requires_approval: requiresApproval, needs_github_token: needsGithubToken });
  const lane = { agent, baseline };
  const dir = file.path.slice(0, Math.max(0, file.path.lastIndexOf("/")));
  const edit = editResponses(write);
  const jobState = pr === "failed" ? "failed" : "completed";

  const content: MockResponse = {
    method: "GET",
    path: `${FILES}/content`,
    sample: { ...LANE_PARAMS, query: new URLSearchParams({ path: file.path, source: "nfs" }) },
    samples: [
      { ...LANE_PARAMS, query: new URLSearchParams({ path: file.path, source: "git", repo: "tenant", ref: TENANT }) },
      { ...LANE_PARAMS, query: new URLSearchParams({ path: file.path, source: "baseline" }) },
      { ...LANE_PARAMS, query: new URLSearchParams({ path: file.path, source: "blob", hash: H_NFS }) },
    ],
    body: ({ query }: MockContext) => {
      const source = query.get("source");
      // Only the core file has a Git and a baseline copy that differ from NFS; every other file is the same in each place.
      if (file.path !== CORE_PATH) return file.content;
      // The masks are those of the NFS text (offsets into it): the Git and baseline copies are other texts and carry none.
      const { masks: _nfsMasks, ...copy } = file.content;
      if (source === "git") return { ...copy, hash: H_GIT, text: coreGitText(), format: { eol: "lf", encoding: "utf8", bom: false }, size: coreGitText().length };
      if (source === "baseline") return { ...copy, hash: H_BASELINE, text: coreBaselineText(), size: coreBaselineText().length };
      return file.content;
    },
  };

  const tree: MockResponse = {
    method: "GET",
    path: "/api/v1/swimlanes/{id}/tree",
    sample: { ...LANE_PARAMS, query: new URLSearchParams() },
    samples: [{ ...LANE_PARAMS, query: new URLSearchParams({ prefix: dir }) }],
    body: ({ query }: MockContext) => {
      const prefix = query.get("prefix") ?? "";
      if (prefix === "") return { items: [folder("document-service"), folder("security"), folder("tx-infinity-api")], next: null };
      return { items: prefix === dir ? siblings(file) : [], next: null };
    },
  };

  const responses: MockResponse[] = [
    { method: "GET", path: "/api/v1/me", body: me, headers: { "set-cookie": csrfCookie } },
    { method: "POST", path: "/auth/logout", status: 204 },
    { method: "GET", path: "/api/v1/swimlanes", body: { items: lanes(lane), next: null } },
    { method: "GET", path: "/api/v1/swimlanes/{id}", sample: LANE_PARAMS, body: ({ params }: MockContext) => ({ ...detail(lane), id: params["id"] ?? LANE }) },
    // For an Admin the sidebar's counts: pending proposals and the access requests that wait.
    ...(role === "admin"
      ? [
          { method: "GET", path: "/api/v1/proposals", body: { items: [], next: null } } satisfies MockResponse,
          { method: "GET", path: "/api/v1/admin/access-requests", body: { items: [], next: null } } satisfies MockResponse,
          // The names of the people the file's header and history say, which only an Admin may read.
          SHELL_USERS,
        ]
      : []),
    // The findings of the swimlane: none. The Services tab, where deleting a file leads back to, reads one finding of the config-server restart kind.
    { method: "GET", path: "/api/v1/findings", body: { items: [], next: null } },
    // The pull requests of the swimlane (the header's badge): none, unless the scenario is the one where this change has one open.
    {
      method: "GET",
      path: "/api/v1/prs",
      sample: { query: new URLSearchParams({ swimlane: LANE, state: "open", limit: "100" }) },
      samples: [{ query: new URLSearchParams({ swimlane: LANE, state: "merged", limit: "100" }) }],
      body: ({ query }: MockContext) => ({ items: pr === "exists" && query.get("state") === "open" ? [openPr] : [], next: null }),
    },
    tree,
    content,
    {
      method: "GET",
      path: `${FILES}/history`,
      sample: LANE_PARAMS,
      body: { items: history ? coreHistory() : coreHistory().slice(0, 1), next: null },
    },
    {
      method: "GET",
      path: "/api/v1/compare",
      sample: { query: new URLSearchParams({ left: `git:tenant@${TENANT}`, right: `nfs:${LANE}`, file: file.logical ?? file.path }) },
      // The hub tags a comparison, and the app asks again with the tag (a 304 keeps the answer it has).
      ...(compare === "large" ? { headers: { etag: LARGE_ETAG } } : {}),
      body: ({ query }: MockContext) => comparisonFor(compare, file, query.get("left") ?? `git:tenant@${TENANT}`, query.get("right") ?? `nfs:${LANE}`),
    },
    { method: "GET", path: "/api/v1/docs/for-file", body: { hits: docs ? coreDocs() : [] } },
    { method: "GET", path: "/api/v1/features/status", body: { flags: docs ? coreFlags() : [] } },
    {
      method: "GET",
      path: "/api/v1/git/refs",
      sample: { query: new URLSearchParams({ repo: "tenant", kind: "branch" }) },
      samples: [{ query: new URLSearchParams({ repo: "base", kind: "tag" }) }],
      body: ({ query }: MockContext) => {
        const entry = (name: string, kind: "tag" | "branch") => ({ name, kind, commit: "a3f9c21a3f9c21a3f9c21a3f9c21a3f9c21a3f9c", branch_kind: kind === "branch" ? ("deployable" as const) : null });
        return query.get("kind") === "tag" ? { items: [entry("v2.3.1", "tag"), entry("v2.3.0", "tag")], next: null } : { items: [entry("sit7", "branch"), entry("sit9", "branch")], next: null };
      },
    },
    // The writes. Each answers as the option says; none is applied twice, and the hub's own audit event is the server's.
    { method: "PUT", path: FILES, sample: LANE_PARAMS, ...edit },
    { method: "DELETE", path: FILES, sample: LANE_PARAMS, ...edit },
    { method: "POST", path: `${FILES}/revert`, sample: LANE_PARAMS, ...edit },
    { method: "POST", path: `${FILES}/upload`, sample: LANE_PARAMS, ...edit },
    {
      method: "POST",
      path: `${FILES}/preview`,
      sample: LANE_PARAMS,
      body: {
        preview_id: "pv-mock-0001",
        expires_at: NOW + 10 * 60_000,
        scope: { kind: "every_service" },
        warnings: [{ kind: "needs_notify_or_restart" }],
        changes: [
          {
            change: {
              path: "txInfinityOptions.remote-itm-teller.enableAccountSorting",
              kind: "changed",
              left: { text: "false", ty: "bool", flag: null },
              right: { text: "true", ty: "bool", flag: null },
              left_location: null,
              right_location: null,
              severity: null,
            },
            flagged: false,
          },
        ],
        changes_total: 1,
        effective_changed: true,
      },
    },
    {
      method: "POST",
      path: "/api/v1/prs/preview",
      sample: { body: { swimlane: LANE, paths: [file.path], target: { kind: "tenant", branch: TENANT } } },
      samples: [{ body: { swimlane: LANE, paths: [file.path], target: { kind: "base" } } }],
      body: ({ body }: MockContext) => {
        const target = (body as { target?: { kind?: string } } | undefined)?.target;
        return prPreview(target?.kind === "base" ? "base" : "tenant", pr === "exists" && target?.kind !== "base" ? { open_pr: openPr } : {});
      },
    },
    prResponses(pr),
    { method: "GET", path: "/api/v1/pr-jobs/{id}", sample: { params: { id: "7" } }, body: prJob(jobState, jobState === "failed" ? { error: "GitHub refused the push." } : {}) },
  ];

  return { id, description, responses };
}

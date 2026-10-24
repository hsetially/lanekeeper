// A hub for the journeys of the four Admin sections that task T5 adds (sentinels, OS Login mapping, severity rules, retention) that
// remembers what it was told, like ./admin-hub.ts does for the sections of T4: a mapping that is saved is in the list the next time
// it is asked for; a rule that is deleted is gone; a retention policy that is saved is the one that is read back. The design's
// scenarios answer every write with the same body whatever was sent, which a journey that says "the new rule is in the table"
// cannot use.
//
// It is the design's data (src/mocks/design/admin-t5/fixtures.ts) behind scenario responses that read and write a small state in the
// test's own process. Each call makes a hub of its own, so no journey sees another's changes. It is a scenario like any other
// (src/mocks/scenario.ts): the harness serves it, and the page cannot tell it from the hub.
//
// The retention dry run: the real answer is the engine's. This hub answers with the design's smooth model (`previewFor`), except
// that a policy that keeps everything for ten years or more finds nothing to delete, which is what lets a journey play both cases of
// the dialog (a policy that deletes, and one that does not).
import type { OsLoginUser, RetentionInput, RetentionSettings, SentinelInfo, SeverityRule, User } from "../../src/api/generated/model/index.ts";
import { IDS, people } from "../../src/mocks/design/admin/fixtures.ts";
import { NOW, osLoginUsers, previewFor, retention, sentinels, severityRules } from "../../src/mocks/design/admin-t5/fixtures.ts";
import { problemBody } from "../../src/mocks/scenario.ts";
import { derive, type MockContext, type MockResponse } from "./scenarios";

export type AdminT5Hub = {
  /** Serve this scenario. */
  readonly scenario: string;
  readonly users: User[];
  osLogin: OsLoginUser[];
  readonly sentinels: SentinelInfo[];
  severity: SeverityRule[];
  retention: RetentionSettings;
  /** The bodies of the dry runs the hub was asked for, and of the saves, in order. */
  readonly dryRuns: RetentionInput[];
  readonly saves: RetentionInput[];
};

const page = <T>(items: readonly T[]): { items: readonly T[]; next: null } => ({ items, next: null });
const bodyOf = (context: MockContext): Record<string, unknown> => (typeof context.body === "object" && context.body !== null ? (context.body as Record<string, unknown>) : {});
const paramOf = (context: MockContext, name: string): string => decodeURIComponent(context.params[name] ?? "");
const upsert = <T>(list: T[], item: T, same: (other: T) => boolean): T[] => (list.some(same) ? list.map((other) => (same(other) ? item : other)) : [...list, item]);

/** What a dry run finds: the design's model, and nothing for a policy that keeps everything for ten years. */
function previewOf(input: RetentionInput): RetentionSettings["dry_run_preview"] {
  const found = previewFor(input.keep_days, input.keep_last_versions);
  return input.keep_days >= 3650 ? { versions_to_delete: 0, bytes_to_free: 0, versions_kept_by_reference: found.versions_kept_by_reference } : found;
}

export function adminT5Hub(): AdminT5Hub {
  const hub: AdminT5Hub = {
    scenario: "",
    users: people(),
    osLogin: osLoginUsers(),
    sentinels: sentinels(),
    severity: severityRules(),
    retention: retention({ dry_run_preview: previewOf({ keep_days: 180, keep_last_versions: 50 }) }),
    dryRuns: [],
    saves: [],
  };

  const responses: MockResponse[] = [
    { method: "GET", path: "/api/v1/admin/users", body: () => page(hub.users) },
    { method: "GET", path: "/api/v1/admin/sentinels", body: () => page(hub.sentinels) },

    { method: "GET", path: "/api/v1/admin/os-login-users", body: () => page(hub.osLogin) },
    {
      method: "PUT",
      path: "/api/v1/admin/os-login-users/{login}",
      sample: { params: { login: "dana_okoye_work" }, body: { user: IDS.dana } },
      body: (context: MockContext) => {
        const login = paramOf(context, "login");
        const user = bodyOf(context)["user"];
        if (typeof user !== "string" || !hub.users.some((u) => u.id === user)) return problemBody(422, "unprocessable", "Unprocessable");
        const mapping: OsLoginUser = { os_login: login, user };
        hub.osLogin = upsert(hub.osLogin, mapping, (m) => m.os_login === login);
        return mapping;
      },
    },
    {
      method: "DELETE",
      path: "/api/v1/admin/os-login-users/{login}",
      status: 204,
      sample: { params: { login: "marco_diaz_contractor_example" } },
      body: (context: MockContext) => {
        const login = paramOf(context, "login");
        hub.osLogin = hub.osLogin.filter((m) => m.os_login !== login);
        return undefined;
      },
    },

    { method: "GET", path: "/api/v1/admin/severity-rules", body: () => page(hub.severity) },
    {
      method: "PUT",
      path: "/api/v1/admin/severity-rules/{id}",
      sample: { params: { id: "security-high" }, body: { name: "Security files are High", match: { path_glob: "security/**" }, severity: "high", priority: 100, enabled: true } },
      body: (context: MockContext) => {
        const id = paramOf(context, "id");
        const rule = { id, ...bodyOf(context) } as SeverityRule;
        hub.severity = upsert(hub.severity, rule, (r) => r.id === id);
        return rule;
      },
    },
    {
      method: "DELETE",
      path: "/api/v1/admin/severity-rules/{id}",
      status: 204,
      sample: { params: { id: "security-high" } },
      body: (context: MockContext) => {
        const id = paramOf(context, "id");
        hub.severity = hub.severity.filter((r) => r.id !== id);
        return undefined;
      },
    },

    { method: "GET", path: "/api/v1/admin/retention", body: () => hub.retention },
    {
      method: "PUT",
      path: "/api/v1/admin/retention",
      sample: { query: new URLSearchParams("dry_run=true"), body: { keep_days: 90, keep_last_versions: 50 } },
      samples: [{ query: new URLSearchParams(), body: { keep_days: 90, keep_last_versions: 50 } }],
      body: (context: MockContext) => {
        const input = bodyOf(context) as unknown as RetentionInput;
        const preview = previewOf(input);
        if (context.query.get("dry_run") === "true") {
          hub.dryRuns.push(input);
          return { ...hub.retention, dry_run_preview: preview };
        }
        hub.saves.push(input);
        hub.retention = { keep_days: input.keep_days, keep_last_versions: input.keep_last_versions, updated_at: NOW, updated_by: IDS.priya, dry_run_preview: preview };
        return hub.retention;
      },
    },
  ];

  const base = derive("design-admin-t5-oslogin", responses, "journey-admin-t5-hub");
  return Object.assign(hub, { scenario: base });
}

/** The same hub, except that every write to the retention policy is refused with `status`. */
export function refusingRetention(hub: AdminT5Hub, status: 429 | 503): string {
  const put: MockResponse = {
    method: "PUT",
    path: "/api/v1/admin/retention",
    status,
    ...(status === 429 ? { headers: { "retry-after": "20" } } : {}),
    body: problemBody(status, status === 429 ? "too_many_requests" : "service_unavailable", status === 429 ? "Too many requests" : "Service unavailable"),
    sample: { query: new URLSearchParams("dry_run=true"), body: { keep_days: 90, keep_last_versions: 50 } },
  };
  return derive(hub.scenario, [put], `journey-admin-t5-refusing-${String(status)}`);
}

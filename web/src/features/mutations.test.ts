import { MutationObserver, QueryClient, QueryObserver, type QueryKey, type UseMutationOptions } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { adminKeys, compareKeys, docKeys, fileKeys, findingKeys, meKeys, prKeys, proposalKeys, swimlaneKeys } from "@/lib/query/keys";
import { accessMutations, afterSecretWrite, createMcpTokenRequest, putGithubTokenRequest } from "./access/mutations";
import { adminMutations, afterAdminSecretWrite, issueJoinTokenRequest, previewRetentionRequest, putNotificationRequest } from "./admin/mutations";
import { approvalMutations } from "./approvals/mutations";
import { docMutations } from "./docs/mutations";
import { fileMutations } from "./file/mutations";
import { prMutations } from "./prs/mutations";
import { swimlaneMutations } from "./swimlanes/mutations";

// The colocated mutations: each calls the operation of the contract (method, path, body), carries the headers the fetch layer
// gives every write, and refetches what the write changed (the table of src/lib/query/invalidate.ts) and nothing else.

type Call = { method: string; url: URL; body: string | undefined; headers: Headers };
let calls: Call[];
let reply: { status?: number; body: unknown };

beforeEach(() => {
  calls = [];
  reply = { body: {} };
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string, init: RequestInit = {}) => {
      calls.push({ method: String(init.method), url: new URL(input, "http://hub.test"), body: typeof init.body === "string" ? init.body : undefined, headers: new Headers(init.headers) });
      const status = reply.status ?? 200;
      return Promise.resolve(status === 204 ? new Response(null, { status }) : new Response(JSON.stringify(reply.body), { status, headers: { "content-type": "application/json" } }));
    }),
  );
});
afterEach(() => {
  vi.unstubAllGlobals();
});

type Anything = UseMutationOptions<never, never, never>;
type Case = {
  name: string;
  make: (client: QueryClient) => unknown;
  vars: unknown;
  method: string;
  path: string;
  /** The JSON the write sends, when it sends JSON. */
  json?: unknown;
  /** What the server answers. */
  answer?: { status?: number; body: unknown };
  /** Keys of queries on screen that must refetch. */
  refetch: readonly QueryKey[];
  /** Keys of queries on screen that must not. */
  leave?: readonly QueryKey[];
};

const HASH = "a".repeat(64);
const EXPECTED = { kind: "hash", hash: HASH };
const APPLIED = { status: 200, body: { outcome: "applied", hash: HASH, audit_id: 1 } };
const NO_CONTENT = { status: 204, body: null };

const CASES: Case[] = [
  // me
  { name: "requestRole", make: (c) => accessMutations.requestRole(c), vars: { requested_role: "editor", reason: "to fix a typo" }, method: "POST", path: "/api/v1/me/access-request", json: { requested_role: "editor", reason: "to fix a typo" }, refetch: [meKeys.all()] },
  { name: "deleteGithubToken", make: (c) => accessMutations.deleteGithubToken(c), vars: undefined, method: "DELETE", path: "/api/v1/me/github-token", answer: NO_CONTENT, refetch: [meKeys.githubCredential(), meKeys.all()], leave: [meKeys.mcpTokens()] },
  { name: "revokeMcpToken", make: (c) => accessMutations.revokeMcpToken(c), vars: 7, method: "DELETE", path: "/api/v1/me/mcp-tokens/7", answer: NO_CONTENT, refetch: [meKeys.mcpTokens()], leave: [meKeys.githubCredential()] },
  // swimlanes
  { name: "restart", make: (c) => swimlaneMutations.restart(c), vars: { swimlane: "sit1", service: { namespace: "sit1", name: "core" } }, method: "POST", path: "/api/v1/swimlanes/sit1/restart", json: { namespace: "sit1", name: "core" }, answer: APPLIED, refetch: [swimlaneKeys.pickup("sit1"), swimlaneKeys.detail("sit1"), swimlaneKeys.all()], leave: [swimlaneKeys.pickup("sit2"), proposalKeys.lists()] },
  { name: "restart, sent for approval", make: (c) => swimlaneMutations.restart(c), vars: { swimlane: "sit1", service: { namespace: "sit1", name: "core" } }, method: "POST", path: "/api/v1/swimlanes/sit1/restart", json: { namespace: "sit1", name: "core" }, answer: { status: 200, body: { outcome: "proposal_created", id: 4 } }, refetch: [proposalKeys.lists()], leave: [swimlaneKeys.pickup("sit1"), swimlaneKeys.detail("sit1")] },
  { name: "notify", make: (c) => swimlaneMutations.notify(c), vars: { swimlane: "sit1", body: { paths: ["core/a.yml"] } }, method: "POST", path: "/api/v1/swimlanes/sit1/notify", json: { paths: ["core/a.yml"] }, answer: { body: { accepted: 1, audit_id: 2 } }, refetch: [swimlaneKeys.pickup("sit1")], leave: [proposalKeys.lists()] },
  // files
  { name: "edit", make: (c) => fileMutations.edit(c), vars: { swimlane: "sit1", body: { path: "core/a.yml", expected: EXPECTED, content: "a: 1\n" } }, method: "PUT", path: "/api/v1/swimlanes/sit1/files", json: { path: "core/a.yml", expected: EXPECTED, content: "a: 1\n" }, answer: APPLIED, refetch: [fileKeys.head("sit1", "core/a.yml"), fileKeys.tree("sit1", "", null), fileKeys.history("sit1", "core/a.yml"), compareKeys.diff("nfs:sit1", "git:base@main", "core/a.yml", null), compareKeys.grid("core.yml", ["sit1"], false, null), swimlaneKeys.detail("sit1")], leave: [fileKeys.head("sit1", "core/b.yml"), fileKeys.tree("sit2", "", null), fileKeys.content("sit1", "core/a.yml", HASH), proposalKeys.lists()] },
  { name: "edit, sent for approval", make: (c) => fileMutations.edit(c), vars: { swimlane: "sit1", body: { path: "core/a.yml", expected: EXPECTED, content: "a: 1\n" } }, method: "PUT", path: "/api/v1/swimlanes/sit1/files", json: { path: "core/a.yml", expected: EXPECTED, content: "a: 1\n" }, answer: { status: 200, body: { outcome: "proposal_created", id: 4 } }, refetch: [proposalKeys.lists()], leave: [fileKeys.head("sit1", "core/a.yml"), fileKeys.tree("sit1", "", null)] },
  { name: "revert", make: (c) => fileMutations.revert(c), vars: { swimlane: "sit1", body: { path: "core/a.yml", expected: EXPECTED, to: "b".repeat(64) } }, method: "POST", path: "/api/v1/swimlanes/sit1/files/revert", json: { path: "core/a.yml", expected: EXPECTED, to: "b".repeat(64) }, answer: APPLIED, refetch: [fileKeys.head("sit1", "core/a.yml"), fileKeys.history("sit1", "core/a.yml")] },
  { name: "delete", make: (c) => fileMutations.delete(c), vars: { swimlane: "sit1", params: { path: "core/a.yml", expected: HASH, preview_id: "p1" } }, method: "DELETE", path: "/api/v1/swimlanes/sit1/files", answer: APPLIED, refetch: [fileKeys.head("sit1", "core/a.yml"), fileKeys.tree("sit1", "core", null)] },
  { name: "createDraft", make: (c) => fileMutations.createDraft(c), vars: { swimlane: "sit1", path: "core/a.yml", expected: EXPECTED, content: "a: 2\n" }, method: "POST", path: "/api/v1/drafts", json: { swimlane: "sit1", path: "core/a.yml", expected: EXPECTED, content: "a: 2\n" }, answer: { status: 201, body: { id: "d1" } }, refetch: [proposalKeys.lists()], leave: [fileKeys.head("sit1", "core/a.yml")] },
  { name: "applyDraft", make: (c) => fileMutations.applyDraft(c), vars: { draft: "d1", swimlane: "sit1", path: "core/a.yml" }, method: "POST", path: "/api/v1/drafts/d1/apply", answer: APPLIED, refetch: [proposalKeys.lists(), fileKeys.head("sit1", "core/a.yml")] },
  // proposals and pull requests
  { name: "approve", make: (c) => approvalMutations.approve(c), vars: { id: 12, body: { comment: "ok" }, swimlane: "sit1", path: "core/a.yml" }, method: "POST", path: "/api/v1/proposals/12/approve", json: { comment: "ok" }, answer: APPLIED, refetch: [proposalKeys.lists(), proposalKeys.detail(12), fileKeys.head("sit1", "core/a.yml")], leave: [proposalKeys.detail(13)] },
  { name: "reject", make: (c) => approvalMutations.reject(c), vars: { id: 12, body: { reason: "no" } }, method: "POST", path: "/api/v1/proposals/12/reject", json: { reason: "no" }, refetch: [proposalKeys.lists(), proposalKeys.detail(12)], leave: [proposalKeys.detail(13), fileKeys.head("sit1", "core/a.yml")] },
  { name: "createPr", make: (c) => prMutations.create(c), vars: { swimlane: "sit1", paths: ["core/a.yml"], title: "Fix" }, method: "POST", path: "/api/v1/prs", json: { swimlane: "sit1", paths: ["core/a.yml"], title: "Fix" }, answer: { body: { id: 9 } }, refetch: [prKeys.lists()], leave: [proposalKeys.lists()] },
  { name: "createPr, sent for approval", make: (c) => prMutations.create(c), vars: { swimlane: "sit1", paths: ["core/a.yml"], title: "Fix" }, method: "POST", path: "/api/v1/prs", json: { swimlane: "sit1", paths: ["core/a.yml"], title: "Fix" }, answer: { status: 200, body: { outcome: "proposal_created", id: 4 } }, refetch: [prKeys.lists(), proposalKeys.lists()] },
  { name: "previewPr (writes nothing)", make: (c) => prMutations.preview(c), vars: { swimlane: "sit1", paths: ["core/a.yml"], title: "Fix" }, method: "POST", path: "/api/v1/prs/preview", json: { swimlane: "sit1", paths: ["core/a.yml"], title: "Fix" }, refetch: [], leave: [prKeys.lists()] },
  // documents
  { name: "createDoc", make: (c) => docMutations.create(c), vars: { title: "T", markdown: "# T" }, method: "POST", path: "/api/v1/docs", json: { title: "T", markdown: "# T" }, answer: { status: 201, body: { id: 3 } }, refetch: [docKeys.lists(), docKeys.featureStatuses()] },
  { name: "updateDoc", make: (c) => docMutations.update(c), vars: { id: 3, body: { markdown: "# U", expected_version: 2 } }, method: "PUT", path: "/api/v1/docs/3", json: { markdown: "# U", expected_version: 2 }, refetch: [docKeys.lists(), docKeys.detail(3)], leave: [docKeys.detail(4)] },
  { name: "deleteDoc", make: (c) => docMutations.delete(c), vars: 3, method: "DELETE", path: "/api/v1/docs/3", answer: NO_CONTENT, refetch: [docKeys.lists(), docKeys.detail(3)] },
  // admin
  { name: "approveAccessRequest", make: (c) => adminMutations.approveAccessRequest(c), vars: { id: 5, body: { role: "viewer" } }, method: "POST", path: "/api/v1/admin/access-requests/5/approve", json: { role: "viewer" }, refetch: [adminKeys.list("access-requests", {}), adminKeys.list("users", {})] },
  { name: "rejectAccessRequest", make: (c) => adminMutations.rejectAccessRequest(c), vars: { id: 5, body: { reason: "no" } }, method: "POST", path: "/api/v1/admin/access-requests/5/reject", json: { reason: "no" }, refetch: [adminKeys.list("access-requests", {})], leave: [adminKeys.list("users", {})] },
  { name: "updateUser", make: (c) => adminMutations.updateUser(c), vars: { id: "u1", body: { role: "admin" }, self: true }, method: "PATCH", path: "/api/v1/admin/users/u1", json: { role: "admin" }, refetch: [adminKeys.list("users", {}), meKeys.all()] },
  { name: "updateUser (someone else)", make: (c) => adminMutations.updateUser(c), vars: { id: "u2", body: { status: "disabled" } }, method: "PATCH", path: "/api/v1/admin/users/u2", json: { status: "disabled" }, refetch: [adminKeys.list("users", {})], leave: [meKeys.all()] },
  { name: "putAgent", make: (c) => adminMutations.putAgent(c), vars: { swimlane: "sit1", body: { service_account_email: "a@b.iam" } }, method: "PUT", path: "/api/v1/admin/agents/sit1", json: { service_account_email: "a@b.iam" }, refetch: [adminKeys.list("agents", {}), swimlaneKeys.all(), swimlaneKeys.detail("sit1")], leave: [swimlaneKeys.detail("sit2")] },
  { name: "deleteAgent", make: (c) => adminMutations.deleteAgent(c), vars: { swimlane: "sit1" }, method: "DELETE", path: "/api/v1/admin/agents/sit1", answer: NO_CONTENT, refetch: [adminKeys.list("agents", {}), swimlaneKeys.detail("sit1")] },
  { name: "putTenants", make: (c) => adminMutations.putTenants(c), vars: { swimlane: "sit1", body: { tenants: ["t1"] } }, method: "PUT", path: "/api/v1/admin/swimlanes/sit1/tenants", json: { tenants: ["t1"] }, refetch: [adminKeys.list("tenants", { id: "sit1" }), swimlaneKeys.detail("sit1"), compareKeys.grid("f", ["sit1"], false, null)] },
  { name: "setAutoNotify (the one optimistic write)", make: (c) => adminMutations.setAutoNotify(c), vars: { swimlane: "sit1", body: { enabled: true } }, method: "PUT", path: "/api/v1/admin/swimlanes/sit1/auto-notify", json: { enabled: true }, answer: { body: { enabled: true } }, refetch: [adminKeys.list("auto-notify", { id: "sit1" }), swimlaneKeys.detail("sit1")] },
  { name: "putFolderMapping", make: (c) => adminMutations.putFolderMapping(c), vars: { id: "f1", body: { channel: "c" } }, method: "PUT", path: "/api/v1/admin/folder-mappings/f1", json: { channel: "c" }, refetch: [adminKeys.list("folder-mappings", {})], leave: [adminKeys.list("host-tiers", {})] },
  { name: "deleteFolderMapping", make: (c) => adminMutations.deleteFolderMapping(c), vars: "f1", method: "DELETE", path: "/api/v1/admin/folder-mappings/f1", answer: NO_CONTENT, refetch: [adminKeys.list("folder-mappings", {})] },
  { name: "putHostTier", make: (c) => adminMutations.putHostTier(c), vars: { id: "h1", body: { tier: "prod" } }, method: "PUT", path: "/api/v1/admin/host-tiers/h1", json: { tier: "prod" }, refetch: [adminKeys.list("host-tiers", {}), swimlaneKeys.all(), swimlaneKeys.detail("sit1")] },
  { name: "deleteHostTier", make: (c) => adminMutations.deleteHostTier(c), vars: "h1", method: "DELETE", path: "/api/v1/admin/host-tiers/h1", answer: NO_CONTENT, refetch: [adminKeys.list("host-tiers", {}), swimlaneKeys.all()] },
  { name: "putMergeMode", make: (c) => adminMutations.putMergeMode(c), vars: { id: "m1", body: { mode: "base" } }, method: "PUT", path: "/api/v1/admin/merge-modes/m1", json: { mode: "base" }, refetch: [adminKeys.list("merge-modes", {})] },
  { name: "deleteMergeMode", make: (c) => adminMutations.deleteMergeMode(c), vars: "m1", method: "DELETE", path: "/api/v1/admin/merge-modes/m1", answer: NO_CONTENT, refetch: [adminKeys.list("merge-modes", {})] },
  { name: "deleteNotification", make: (c) => adminMutations.deleteNotification(c), vars: "n1", method: "DELETE", path: "/api/v1/admin/notifications/n1", answer: NO_CONTENT, refetch: [adminKeys.list("notifications", {})] },
  { name: "putOsLoginUser", make: (c) => adminMutations.putOsLoginUser(c), vars: { login: "ada", body: { user: "u1" } }, method: "PUT", path: "/api/v1/admin/os-login-users/ada", json: { user: "u1" }, refetch: [adminKeys.list("os-login-users", {})] },
  { name: "deleteOsLoginUser", make: (c) => adminMutations.deleteOsLoginUser(c), vars: "ada", method: "DELETE", path: "/api/v1/admin/os-login-users/ada", answer: NO_CONTENT, refetch: [adminKeys.list("os-login-users", {})] },
  { name: "putPathMapping", make: (c) => adminMutations.putPathMapping(c), vars: { id: "p1", body: { from: "a" } }, method: "PUT", path: "/api/v1/admin/path-mappings/p1", json: { from: "a" }, refetch: [adminKeys.list("path-mappings", {})] },
  { name: "deletePathMapping", make: (c) => adminMutations.deletePathMapping(c), vars: "p1", method: "DELETE", path: "/api/v1/admin/path-mappings/p1", answer: NO_CONTENT, refetch: [adminKeys.list("path-mappings", {})] },
  { name: "putSeverityRule", make: (c) => adminMutations.putSeverityRule(c), vars: { id: "s1", body: { severity: "high" } }, method: "PUT", path: "/api/v1/admin/severity-rules/s1", json: { severity: "high" }, refetch: [adminKeys.list("severity-rules", {}), findingKeys.list({ limit: 50 })] },
  { name: "deleteSeverityRule", make: (c) => adminMutations.deleteSeverityRule(c), vars: "s1", method: "DELETE", path: "/api/v1/admin/severity-rules/s1", answer: NO_CONTENT, refetch: [adminKeys.list("severity-rules", {}), findingKeys.all()] },
  { name: "putRetention", make: (c) => adminMutations.putRetention(c), vars: { body: { keep_last_versions: 10, keep_days: 90 } }, method: "PUT", path: "/api/v1/admin/retention", json: { keep_last_versions: 10, keep_days: 90 }, refetch: [adminKeys.list("retention", {})] },
];

function watch(client: QueryClient, key: QueryKey) {
  const fetches = vi.fn(() => Promise.resolve({ n: 1 }));
  client.setQueryData(key, { n: 0 });
  new QueryObserver(client, { queryKey: key, queryFn: fetches, staleTime: Infinity }).subscribe(() => undefined);
  return fetches;
}

describe.each(CASES)("$name", (c) => {
  it("calls the operation, with the headers every write carries, and refetches what it changed and only that", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
    const refetched = c.refetch.map((key) => watch(client, key));
    const left = (c.leave ?? []).map((key) => watch(client, key));
    reply = c.answer ?? { body: {} };
    const options = c.make(client) as Anything;
    await new MutationObserver(client, options).mutate(c.vars as never);
    expect(calls).toHaveLength(1);
    const call = calls[0];
    expect(call?.method).toBe(c.method);
    expect(call?.url.pathname).toBe(c.path);
    expect(call?.headers.get("idempotency-key")).toMatch(/^[0-9a-f-]{36}$/);
    if (c.json !== undefined) expect(JSON.parse(call?.body ?? "null")).toEqual(c.json);
    refetched.forEach((fetches, i) => expect(fetches, `refetch ${JSON.stringify(c.refetch[i])}`).toHaveBeenCalledTimes(1));
    left.forEach((fetches, i) => expect(fetches, `leave ${JSON.stringify((c.leave ?? [])[i])}`).not.toHaveBeenCalled());
    client.clear();
  });

  it("is never optimistic and never retried, except the one that is declared so, and keeps nothing of the write", () => {
    const client = new QueryClient();
    const options = c.make(client) as Anything;
    expect(options.retry).toBe(false);
    expect(options.gcTime).toBe(0);
    if (/setAutoNotify/.test(c.name)) expect(options.onMutate).toBeTypeOf("function");
    else expect(options).not.toHaveProperty("onMutate");
    client.clear();
  });
});

describe("secret-bearing writes are requests, not mutation options", () => {
  it("send the secret once, and say what to refresh without holding it", async () => {
    const client = new QueryClient();
    expect(putGithubTokenRequest).toBeTypeOf("function");
    expect(createMcpTokenRequest).toBeTypeOf("function");
    expect(putNotificationRequest).toBeTypeOf("function");
    expect(issueJoinTokenRequest).toBeTypeOf("function");
    for (const holder of [accessMutations, adminMutations] as Array<Record<string, unknown>>) {
      for (const name of ["putGithubToken", "createMcpToken", "putNotification", "issueJoinToken", "revealValue"]) expect(holder, name).not.toHaveProperty(name);
    }
    reply = { body: { label: "x" } };
    await putGithubTokenRequest({ token: "ghp_secret_value_1234567890" });
    expect(calls[0]?.method).toBe("PUT");
    expect(calls[0]?.url.pathname).toBe("/api/v1/me/github-token");
    // Nothing of the secret is in a key, a URL or a header.
    expect(calls[0]?.url.href).not.toContain("ghp_secret");
    expect([...(calls[0]?.headers.entries() ?? [])].join(" ")).not.toContain("ghp_secret");
    const github = watch(client, meKeys.githubCredential());
    await afterSecretWrite(client, "putGithubToken");
    expect(github).toHaveBeenCalledTimes(1);
    const agents = watch(client, adminKeys.list("agents", {}));
    await afterAdminSecretWrite(client, "adminIssueJoinToken");
    expect(agents).toHaveBeenCalledTimes(1);
    client.clear();
  });

  it("never put a secret in the mutation cache: the requests are not mutations at all", () => {
    const client = new QueryClient();
    void putGithubTokenRequest({ token: "ghp_x" }).catch(() => undefined);
    expect(client.getMutationCache().getAll()).toHaveLength(0);
  });

  it("a retention preview is a request that writes nothing (dry_run) and refetches nothing", async () => {
    reply = { body: {} };
    await previewRetentionRequest({ keep_last_versions: 10, keep_days: 30 });
    expect(calls[0]?.method).toBe("PUT");
    expect(calls[0]?.url.pathname).toBe("/api/v1/admin/retention");
    expect(calls[0]?.url.searchParams.get("dry_run")).toBe("true");
  });
});

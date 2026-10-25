import { QueryClient, QueryObserver, type QueryKey } from "@tanstack/react-query";
import { describe, expect, expectTypeOf, it, vi } from "vitest";
import { OPERATION_ROLES, type OperationId } from "@/api/roles.generated";
import { ApiProblem } from "@/api/problem";
import { NEVER_AUTO_RETRY_PATHS } from "@/api/client";
import { adminKeys, compareKeys, docKeys, fileKeys, findingKeys, isImmutableKey, meKeys, prKeys, proposalKeys, swimlaneKeys } from "./keys";
import { invalidateAfterWrite, MUTATING_OPERATIONS, SECRET_OPERATIONS, shouldRefetchAfterError, WRITE_INVALIDATION, writeTargets, type MutatingOperationId } from "./invalidate";
import { matches, type Target } from "./targets";

// What a write invalidates, declared per operation. The table is typed by the generated operation list, so a mutating
// operation without a row does not compile, and this file proves it at run time too, against the same list.

const mutating = Object.entries(OPERATION_ROLES)
  .filter(([, op]) => op.method !== "GET")
  .map(([id]) => id as OperationId)
  .sort();

describe("the table covers every mutating operation of the contract", () => {
  it("has a row for each operation that is not a GET, and for none that is", () => {
    expect(Object.keys(WRITE_INVALIDATION).sort()).toEqual(mutating);
    expect([...MUTATING_OPERATIONS].sort()).toEqual(mutating);
    for (const [id, op] of Object.entries(OPERATION_ROLES)) {
      if (op.method === "GET") expect(WRITE_INVALIDATION, id).not.toHaveProperty(id);
    }
  });

  it("is typed by the generated operation list: no mutating operation can be missing, and no GET can be in it", () => {
    type Missing = Exclude<MutatingOperationId, keyof typeof WRITE_INVALIDATION>;
    type Extra = Exclude<keyof typeof WRITE_INVALIDATION, MutatingOperationId>;
    expectTypeOf<Missing>().toEqualTypeOf<never>();
    expectTypeOf<Extra>().toEqualTypeOf<never>();
    expectTypeOf<"listSwimlanes">().not.toExtend<MutatingOperationId>();
    expectTypeOf<"editFile">().toExtend<MutatingOperationId>();
  });

  it("says why for every operation that invalidates nothing", () => {
    const none = Object.entries(WRITE_INVALIDATION).filter(([, row]) => "none" in row);
    expect(none.map(([id]) => id).sort()).toEqual(["authLogout", "githubWebhook", "previewFileEffect", "previewPr", "revealValue"]);
    for (const [id, row] of none) expect("none" in row && row.none.length, id).toBeGreaterThan(40);
  });

  it("builds every target of every operation from the key factories: a known root, never immutable, never empty", () => {
    const roots = new Set(["me", "swimlanes", "swimlane", "pickup", "tree", "file-head", "history", "compare", "compare-tree", "grid", "findings", "proposals", "proposal", "prs", "pr-job", "docs", "doc", "features", "audit", "admin", "me-github-credential", "me-mcp-tokens", "git-refs", "content-ref", "activity"]);
    for (const id of MUTATING_OPERATIONS) {
      for (const target of everyContext(id)) {
        if (target.kind === "key") {
          expect(target.key.length, id).toBeGreaterThan(0);
          expect(roots.has(String(target.key[0])), `${id}: ${String(target.key[0])}`).toBe(true);
          expect(isImmutableKey(target.key), id).toBe(false);
        }
      }
    }
  });
});

/** The targets of an operation for a context that names everything the operation can use. */
function everyContext(id: MutatingOperationId): readonly Target[] {
  const ctx = { swimlane: "sit1", path: "core/a.yml", id: 7, self: true, outcome: "applied" as const };
  return writeTargets(id, ctx as never);
}

const names = (targets: readonly Target[]) => targets.map((t) => (t.kind === "key" ? t.key : t));

describe("what each kind of write invalidates", () => {
  it("a file write: the head, the trees above, history, compares and grids of the swimlane, the swimlane and the list", () => {
    for (const id of ["editFile", "uploadFile", "revertFile", "deleteFile"] as const) {
      const targets = writeTargets(id, { swimlane: "sit1", path: "core/a.yml" });
      expect(names(targets), id).toEqual([
        fileKeys.head("sit1", "core/a.yml"),
        { kind: "trees", swimlane: "sit1", paths: ["core/a.yml"] },
        fileKeys.history("sit1", "core/a.yml"),
        { kind: "compares", swimlane: "sit1" },
        { kind: "grids", swimlane: "sit1" },
        swimlaneKeys.detail("sit1"),
        swimlaneKeys.all(),
      ]);
    }
  });

  it("a write that was SENT FOR APPROVAL (202, outcome proposal_created) changed no file: only the proposals", () => {
    for (const id of ["editFile", "uploadFile", "revertFile", "deleteFile", "restartService", "notifyServices"] as const) {
      const ctx = id === "restartService" || id === "notifyServices" ? { swimlane: "sit1", outcome: "proposal_created" as const } : { swimlane: "sit1", path: "core/a.yml", outcome: "proposal_created" as const };
      expect(names(writeTargets(id, ctx as never)), id).toEqual([proposalKeys.lists()]);
    }
  });

  it("restart and notify: the pickup of the swimlane, the swimlane and the list", () => {
    for (const id of ["restartService", "notifyServices"] as const) {
      expect(names(writeTargets(id, { swimlane: "sit1" })), id).toEqual([swimlaneKeys.pickup("sit1"), swimlaneKeys.detail("sit1"), swimlaneKeys.all()]);
    }
  });

  it("deciding a proposal: the queue and that proposal, and the file when approving applied it", () => {
    expect(names(writeTargets("rejectProposal", { id: 7 }))).toEqual([proposalKeys.lists(), proposalKeys.detail(7)]);
    const approved = names(writeTargets("approveProposal", { id: 7, swimlane: "sit1", path: "core/a.yml", outcome: "applied" }));
    expect(approved.slice(0, 2)).toEqual([proposalKeys.lists(), proposalKeys.detail(7)]);
    expect(approved).toContainEqual(fileKeys.head("sit1", "core/a.yml"));
    // Approving can itself be sent on for another approval: then nothing was applied.
    expect(names(writeTargets("approveProposal", { id: 7, swimlane: "sit1", path: "core/a.yml", outcome: "proposal_created" }))).toEqual([proposalKeys.lists(), proposalKeys.detail(7)]);
    expect(names(writeTargets("approveProposal", { id: 7 }))).toEqual([proposalKeys.lists(), proposalKeys.detail(7), swimlaneKeys.all()]);
  });

  it("a draft: the queue; applying it is a file write", () => {
    expect(names(writeTargets("createDraft", {}))).toEqual([proposalKeys.lists()]);
    expect(names(writeTargets("applyDraft", { swimlane: "sit1", path: "core/a.yml" }))).toContainEqual(fileKeys.head("sit1", "core/a.yml"));
    expect(names(writeTargets("applyDraft", { swimlane: "sit1", path: "core/a.yml" }))).toContainEqual(proposalKeys.lists());
  });

  it("a pull request: the pull requests, and the queue when it was sent for approval", () => {
    expect(names(writeTargets("createPr", {}))).toEqual([prKeys.lists()]);
    expect(names(writeTargets("createPr", { outcome: "proposal_created" }))).toEqual([prKeys.lists(), proposalKeys.lists()]);
  });

  it("documents: the list, the document and the feature status", () => {
    expect(names(writeTargets("createDoc", {}))).toEqual([docKeys.lists(), docKeys.featureStatuses()]);
    for (const id of ["updateDoc", "deleteDoc"] as const) expect(names(writeTargets(id, { id: 3 })), id).toEqual([docKeys.lists(), docKeys.detail(3), docKeys.featureStatuses()]);
  });

  it("my account: the GitHub token and the user (needs_github_token), the MCP token list, the access request", () => {
    for (const id of ["putGithubToken", "deleteGithubToken"] as const) expect(names(writeTargets(id, {})), id).toEqual([meKeys.githubCredential(), meKeys.all()]);
    for (const id of ["createMcpToken", "revokeMcpToken"] as const) expect(names(writeTargets(id, {})), id).toEqual([meKeys.mcpTokens()]);
    expect(names(writeTargets("createAccessRequest", {}))).toEqual([meKeys.all()]);
  });

  it("admin lists: the section that was written, and what shows it elsewhere", () => {
    expect(names(writeTargets("adminApproveAccessRequest", {}))).toEqual([adminKeys.section("access-requests"), adminKeys.section("users")]);
    expect(names(writeTargets("adminRejectAccessRequest", {}))).toEqual([adminKeys.section("access-requests")]);
    expect(names(writeTargets("adminUpdateUser", {}))).toEqual([adminKeys.section("users")]);
    expect(names(writeTargets("adminUpdateUser", { self: true }))).toEqual([adminKeys.section("users"), meKeys.all()]);
    expect(names(writeTargets("adminPutAutoNotify", { swimlane: "sit1" }))).toEqual([adminKeys.section("auto-notify"), swimlaneKeys.detail("sit1")]);
    expect(names(writeTargets("adminPutTenants", { swimlane: "sit1" }))).toEqual([adminKeys.section("tenants"), swimlaneKeys.detail("sit1"), swimlaneKeys.all(), compareKeys.grids()]);
    expect(names(writeTargets("adminPutSeverityRules", {}))).toEqual([adminKeys.section("severity-rules"), findingKeys.all()]);
    expect(names(writeTargets("adminPutHostTiers", {}))).toEqual([adminKeys.section("host-tiers"), swimlaneKeys.all(), swimlaneKeys.details()]);
    expect(names(writeTargets("adminPutAgent", { swimlane: "sit1" }))).toEqual([adminKeys.section("agents"), swimlaneKeys.all(), swimlaneKeys.detail("sit1")]);
    expect(names(writeTargets("adminPutRetention", {}))).toEqual([adminKeys.section("retention")]);
  });

  it("every other admin list invalidates its own section", () => {
    const sections = {
      adminPutFolderMappings: "folder-mappings",
      adminDeleteFolderMappings: "folder-mappings",
      adminPutMergeModes: "merge-modes",
      adminDeleteMergeModes: "merge-modes",
      adminPutNotification: "notifications",
      adminDeleteNotification: "notifications",
      adminPutOsLoginUser: "os-login-users",
      adminDeleteOsLoginUser: "os-login-users",
      adminPutPathMappings: "path-mappings",
      adminDeletePathMappings: "path-mappings",
      adminDeleteSeverityRules: "severity-rules",
      adminDeleteHostTiers: "host-tiers",
    } as const;
    for (const [id, section] of Object.entries(sections)) {
      expect(names(writeTargets(id as MutatingOperationId, {}))[0], id).toEqual(adminKeys.section(section));
    }
  });

  it("returns nothing for an operation that declares `none`", () => {
    for (const id of ["authLogout", "githubWebhook", "previewFileEffect", "previewPr", "revealValue"] as const) expect(writeTargets(id, {})).toEqual([]);
  });
});

describe("secret-bearing operations (the T1 rules)", () => {
  it("are the operations whose paths the fetch layer never retries and never caches", () => {
    const byPath = mutating.filter((id) => {
      const path = OPERATION_ROLES[id].path.replace(/\{[^}]+\}/g, "x");
      return NEVER_AUTO_RETRY_PATHS.some((re) => re.test(path));
    });
    // The retry set is by path, so it also holds the other verbs of those paths (a delete, a revoke): every secret operation is in it.
    for (const id of SECRET_OPERATIONS) expect(byPath, id).toContain(id);
  });

  it("never invalidate content, blobs or a file, and never put anything but the fixed keys in the cache", () => {
    for (const id of SECRET_OPERATIONS) {
      for (const target of everyContext(id)) {
        expect(target.kind, id).toBe("key");
        if (target.kind === "key") expect(["content", "blob", "file-head", "tree", "history"], id).not.toContain(target.key[0]);
      }
    }
  });
});

describe("applying the targets", () => {
  function watch(client: QueryClient, key: QueryKey) {
    const fetches = vi.fn(() => Promise.resolve({ n: 1 }));
    client.setQueryData(key, { n: 0 });
    new QueryObserver(client, { queryKey: key, queryFn: fetches, staleTime: Infinity }).subscribe(() => undefined);
    return fetches;
  }

  it("refetches what is on screen, marks the rest stale, and leaves content alone", async () => {
    const client = new QueryClient();
    const head = watch(client, fileKeys.head("sit1", "core/a.yml"));
    const otherHead = watch(client, fileKeys.head("sit1", "core/b.yml"));
    const content = watch(client, fileKeys.content("sit1", "core/a.yml", "a".repeat(64)));
    client.setQueryData(swimlaneKeys.detail("sit1"), { cached: true });
    await invalidateAfterWrite(client, "editFile", { swimlane: "sit1", path: "core/a.yml" });
    expect(head).toHaveBeenCalledTimes(1);
    expect(otherHead).not.toHaveBeenCalled();
    expect(content).not.toHaveBeenCalled();
    expect(client.getQueryState(swimlaneKeys.detail("sit1"))?.isInvalidated).toBe(true);
    client.clear();
  });

  it("resolves only when the entries on screen have refetched, so a mutation can stay pending until the screen is right", async () => {
    const client = new QueryClient();
    let release: () => void = () => undefined;
    const slow = vi.fn(() => new Promise<{ n: number }>((resolve) => (release = () => resolve({ n: 1 }))));
    client.setQueryData(proposalKeys.lists(), { n: 0 });
    new QueryObserver(client, { queryKey: proposalKeys.lists(), queryFn: slow, staleTime: Infinity }).subscribe(() => undefined);
    let done = false;
    const settled = invalidateAfterWrite(client, "createDraft", {}).then(() => (done = true));
    await Promise.resolve();
    expect(done).toBe(false);
    release();
    await settled;
    expect(done).toBe(true);
    client.clear();
  });

  it("does nothing for an operation that declares none (no call to the cache at all)", async () => {
    const client = new QueryClient();
    const spy = vi.spyOn(client, "invalidateQueries");
    await invalidateAfterWrite(client, "revealValue", {});
    expect(spy).not.toHaveBeenCalled();
  });

  it("matches the same entries a live event would: the file-write targets are the live targets' vocabulary", () => {
    const targets = writeTargets("editFile", { swimlane: "sit1", path: "core/a.yml" });
    expect(targets.some((t) => matches(t, fileKeys.tree("sit1", "", null), undefined))).toBe(true);
    expect(targets.some((t) => matches(t, fileKeys.tree("sit2", "", null), undefined))).toBe(false);
    expect(targets.some((t) => matches(t, compareKeys.diff("nfs:sit1", "git:base@main", "a", null), undefined))).toBe(true);
  });
});

describe("after a failed write", () => {
  const problem = (status: number, kind = "x") => new ApiProblem({ status, kind, title: "t" });

  it("refetches when the cached data is probably out of date: a conflict, a lock, a precondition, a server error, or no answer (the write may have happened)", () => {
    for (const status of [409, 412, 423, 500, 502, 503, 504]) expect(shouldRefetchAfterError(problem(status)), String(status)).toBe(true);
    expect(shouldRefetchAfterError(problem(0, "network"))).toBe(true);
    expect(shouldRefetchAfterError(problem(0, "timeout"))).toBe(true);
  });

  it("does not refetch when the server said no and nothing changed: a bad request, no access, not found, a validation error, a limit", () => {
    for (const status of [400, 401, 403, 404, 413, 422, 429]) expect(shouldRefetchAfterError(problem(status)), String(status)).toBe(false);
    expect(shouldRefetchAfterError(problem(0, "aborted"))).toBe(false);
  });

  it("does not refetch for an error that is not a problem the hub sent", () => {
    expect(shouldRefetchAfterError(new Error("boom"))).toBe(false);
    expect(shouldRefetchAfterError(undefined)).toBe(false);
  });
});

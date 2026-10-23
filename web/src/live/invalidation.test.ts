import { QueryClient, QueryObserver, type QueryKey } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DomainEvent } from "@/api/generated/model";
import { adminKeys, compareKeys, docKeys, fileKeys, findingKeys, gitKeys, prKeys, proposalKeys, swimlaneKeys } from "@/lib/query/keys";
import { InvalidationBatcher, MAX_TARGETS, WINDOW_MS } from "./apply";
import { EVENT_KINDS } from "./events";
import { plan, type Notice, type Target } from "./invalidation";

// The plan's table ("Query keys, retry and SSE invalidation"), row by row, twice: what `plan(event)` says it will invalidate
// (pure data), and what happens to a real QueryClient with observers on those keys. A live event must refetch the queries a
// screen is showing and nothing else (P13, and "one SSE event triggering a refetch of everything on every client" is the
// anti-pattern the performance skill names).

const HASH = "a".repeat(64);
const OTHER = "b".repeat(64);
const SAMPLE: { [K in DomainEvent["type"]]: Extract<DomainEvent, { type: K }> } = {
  file_observed: { type: "file_observed", swimlane: "sit1", path: "core/a.yml", hash: HASH, source: "scan" },
  drift_changed: { type: "drift_changed", swimlane: "sit1", paths: ["core/a.yml"] },
  findings_changed: { type: "findings_changed", swimlane: "sit1" },
  pending_restart_changed: { type: "pending_restart_changed", swimlane: "sit1", services: [] },
  proposal_changed: { type: "proposal_changed", id: 12, status: "approved" },
  agent_status_changed: { type: "agent_status_changed", swimlane: "sit1", status: "disconnected" },
  git_head_moved: { type: "git_head_moved", repo: "base", branch: "main", from: null, to: "c".repeat(40) },
  access_request_created: { type: "access_request_created", user: "u1" },
  doc_indexed: { type: "doc_indexed", doc: 3 },
  resync: { type: "resync" },
  attribution_updated: { type: "attribution_updated", swimlane: "sit1", path: "core/a.yml", hash: HASH },
  pr_state_changed: { type: "pr_state_changed", link_id: 5, state: "merged" },
  pr_job_progress: { type: "pr_job_progress", job_id: 9, state: "opening" },
  sync_window_opened: { type: "sync_window_opened", swimlane: "sit1", job: { name: "sync-1", uid: "u" } },
  sync_window_closed: { type: "sync_window_closed", swimlane: "sit1", job: { name: "sync-1", uid: "u" } },
  pickup_changed: { type: "pickup_changed", swimlane: "sit1", services: [] },
  swimlane_changed: { type: "swimlane_changed", swimlane: "sit1" },
};

describe("plan: one row for each kind of event the contract has", () => {
  it("answers every kind, and every answer says what to refetch (a resync says everything)", () => {
    expect(Object.keys(SAMPLE).sort()).toEqual([...EVENT_KINDS].sort());
    for (const kind of EVENT_KINDS) {
      const p = plan(SAMPLE[kind]);
      if (kind === "resync") expect(p.all, kind).toBe(true);
      else {
        expect(p.all, kind).toBe(false);
        expect(p.targets.length, kind).toBeGreaterThan(0);
      }
    }
  });

  const keys = (event: DomainEvent): Target[] => [...plan(event).targets];

  it("file_observed: the head (unless it already has this hash), the trees above the path, history, compares and grids of the swimlane, the swimlane and the list", () => {
    expect(keys(SAMPLE.file_observed)).toEqual([
      { kind: "file-head", swimlane: "sit1", path: "core/a.yml", hash: HASH },
      { kind: "trees", swimlane: "sit1", paths: ["core/a.yml"] },
      { kind: "key", key: fileKeys.history("sit1", "core/a.yml") },
      { kind: "compares", swimlane: "sit1" },
      { kind: "grids", swimlane: "sit1" },
      { kind: "key", key: swimlaneKeys.detail("sit1") },
      { kind: "key", key: swimlaneKeys.all() },
    ]);
  });

  it("file_observed from a scan is an out-of-band change worth a notice; one from a tool write or a sync is not", () => {
    expect(plan(SAMPLE.file_observed).notices).toEqual([{ kind: "out-of-band-change", swimlane: "sit1", path: "core/a.yml" }]);
    expect(plan({ ...SAMPLE.file_observed, source: "tool_write" }).notices).toEqual([]);
    expect(plan({ ...SAMPLE.file_observed, source: "sync" }).notices).toEqual([]);
  });

  it("drift_changed: the list and the swimlane, the trees and heads of the paths, the compare trees of the swimlane", () => {
    expect(keys(SAMPLE.drift_changed)).toEqual([
      { kind: "key", key: swimlaneKeys.all() },
      { kind: "key", key: swimlaneKeys.detail("sit1") },
      { kind: "trees", swimlane: "sit1", paths: ["core/a.yml"] },
      { kind: "key", key: fileKeys.head("sit1", "core/a.yml") },
      { kind: "compare-trees", swimlane: "sit1" },
    ]);
  });

  it("drift_changed with more than 50 paths refetches every tree and every head of the swimlane instead of one target per path", () => {
    const paths = Array.from({ length: 51 }, (_, i) => `core/f${i}.yml`);
    const targets = keys({ type: "drift_changed", swimlane: "sit1", paths });
    expect(targets).toContainEqual({ kind: "key", key: fileKeys.trees("sit1") });
    expect(targets).toContainEqual({ kind: "key", key: fileKeys.heads("sit1") });
    expect(targets.length).toBeLessThan(10);
    expect(keys({ type: "drift_changed", swimlane: "sit1", paths: paths.slice(0, 50) }).filter((t) => t.kind === "key").length).toBeGreaterThan(50);
  });

  it("findings_changed: the findings, the swimlane and the list (the finding count)", () => {
    expect(keys(SAMPLE.findings_changed)).toEqual([
      { kind: "key", key: findingKeys.all() },
      { kind: "key", key: swimlaneKeys.detail("sit1") },
      { kind: "key", key: swimlaneKeys.all() },
    ]);
  });

  it.each(["pickup_changed", "pending_restart_changed"] as const)("%s: the pickup of the swimlane, the swimlane, the list, and the pickup mark of every file on screen", (kind) => {
    // The event names services and no paths, so the marks in the tree and in the head of a file (`FileEntry.pickup_state`) may all have changed.
    expect(keys(SAMPLE[kind])).toEqual([
      { kind: "key", key: swimlaneKeys.pickup("sit1") },
      { kind: "key", key: swimlaneKeys.detail("sit1") },
      { kind: "key", key: swimlaneKeys.all() },
      { kind: "key", key: fileKeys.trees("sit1") },
      { kind: "key", key: fileKeys.heads("sit1") },
    ]);
  });

  it("proposal_changed: the queue and that proposal, with a notice (the event has no author, so the app decides whose it is)", () => {
    expect(keys(SAMPLE.proposal_changed)).toEqual([
      { kind: "key", key: proposalKeys.lists() },
      { kind: "key", key: proposalKeys.detail(12) },
    ]);
    expect(plan(SAMPLE.proposal_changed).notices).toEqual([{ kind: "proposal-changed", id: 12, status: "approved" }]);
  });

  it("agent_status_changed: the list, the swimlane and the admin agents", () => {
    expect(keys(SAMPLE.agent_status_changed)).toEqual([
      { kind: "key", key: swimlaneKeys.all() },
      { kind: "key", key: swimlaneKeys.detail("sit1") },
      { kind: "key", key: adminKeys.section("agents") },
    ]);
  });

  it("git_head_moved: the list, git reads at that branch, and the ref lists of the repository only when the branch is new", () => {
    expect(keys(SAMPLE.git_head_moved)).toEqual([
      { kind: "key", key: swimlaneKeys.all() },
      { kind: "content-refs", repo: "base", branch: "main" },
      { kind: "key", key: gitKeys.repo("base") },
    ]);
    expect(keys({ ...SAMPLE.git_head_moved, from: "d".repeat(40) })).toEqual([
      { kind: "key", key: swimlaneKeys.all() },
      { kind: "content-refs", repo: "base", branch: "main" },
    ]);
  });

  it("access_request_created: the admin access requests", () => {
    expect(keys(SAMPLE.access_request_created)).toEqual([{ kind: "key", key: adminKeys.section("access-requests") }]);
  });

  it("doc_indexed: the documents, that document and the feature status", () => {
    expect(keys(SAMPLE.doc_indexed)).toEqual([
      { kind: "key", key: docKeys.lists() },
      { kind: "key", key: docKeys.detail(3) },
      { kind: "key", key: docKeys.featureStatuses() },
    ]);
  });

  it("attribution_updated: history, the trees above the path and the head", () => {
    expect(keys(SAMPLE.attribution_updated)).toEqual([
      { kind: "key", key: fileKeys.history("sit1", "core/a.yml") },
      { kind: "trees", swimlane: "sit1", paths: ["core/a.yml"] },
      { kind: "key", key: fileKeys.head("sit1", "core/a.yml") },
    ]);
  });

  it("pr_state_changed: the pull requests", () => {
    expect(keys(SAMPLE.pr_state_changed)).toEqual([{ kind: "key", key: prKeys.lists() }]);
  });

  it("pr_job_progress: that job, and the pull requests once the job is over (completed or failed)", () => {
    expect(keys(SAMPLE.pr_job_progress)).toEqual([{ kind: "key", key: prKeys.job(9) }]);
    for (const state of ["completed", "failed"] as const) {
      expect(keys({ ...SAMPLE.pr_job_progress, state })).toEqual([{ kind: "key", key: prKeys.job(9) }, { kind: "key", key: prKeys.lists() }]);
    }
  });

  it.each(["sync_window_opened", "sync_window_closed"] as const)("%s: the swimlane and the list", (kind) => {
    expect(keys(SAMPLE[kind])).toEqual([{ kind: "key", key: swimlaneKeys.detail("sit1") }, { kind: "key", key: swimlaneKeys.all() }]);
  });

  it("swimlane_changed (baseline, tier, tenants, auto-notify): the swimlane, the list, the admin swimlanes and that swimlane's admin settings", () => {
    expect(keys(SAMPLE.swimlane_changed)).toEqual([
      { kind: "key", key: swimlaneKeys.detail("sit1") },
      { kind: "key", key: swimlaneKeys.all() },
      { kind: "key", key: adminKeys.section("swimlanes") },
      { kind: "key", key: adminKeys.list("tenants", { id: "sit1" }) },
      { kind: "key", key: adminKeys.list("auto-notify", { id: "sit1" }) },
    ]);
  });

  it("never plans anything for an immutable key: content and blobs are functions of a hash", () => {
    for (const kind of EVENT_KINDS) {
      for (const target of plan(SAMPLE[kind]).targets) {
        if (target.kind === "key") expect(["content", "blob"], kind).not.toContain(target.key[0]);
      }
    }
  });
});

// ---- the plan applied to a real cache ----

type Watched = { key: QueryKey; fetches: ReturnType<typeof vi.fn> };

function watch(client: QueryClient, key: QueryKey, data: unknown = { ok: true }): Watched {
  const fetches = vi.fn(() => Promise.resolve(data));
  client.setQueryData(key, data);
  const observer = new QueryObserver(client, { queryKey: key, queryFn: fetches, staleTime: Infinity });
  observers.push(observer.subscribe(() => undefined));
  return { key, fetches };
}
const observers: Array<() => void> = [];

let client: QueryClient;
let batcher: InvalidationBatcher;
let notices: Notice[];

beforeEach(() => {
  vi.useFakeTimers();
  client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: Infinity } } });
  notices = [];
  batcher = new InvalidationBatcher(client, { onNotice: (n) => notices.push(n) });
});
afterEach(() => {
  for (const off of observers.splice(0)) off();
  batcher.dispose();
  client.clear();
  vi.useRealTimers();
});

async function deliver(...events: DomainEvent[]): Promise<void> {
  for (const event of events) batcher.push(plan(event));
  await vi.advanceTimersByTimeAsync(WINDOW_MS + 1);
}
const refetched = (...watched: Watched[]) => watched.map((w) => w.fetches.mock.calls.length);

describe("applying file_observed", () => {
  it("refetches the trees that list a folder above the path, and not another folder's, another swimlane's or a drift-filtered one that is not above", async () => {
    const root = watch(client, fileKeys.tree("sit1", "", null));
    const core = watch(client, fileKeys.tree("sit1", "core", null));
    const coreDrift = watch(client, fileKeys.tree("sit1", "core", "git_ahead"));
    const other = watch(client, fileKeys.tree("sit1", "other", null));
    const sit2 = watch(client, fileKeys.tree("sit2", "", null));
    await deliver(SAMPLE.file_observed);
    expect(refetched(root, core, coreDrift, other, sit2)).toEqual([1, 1, 1, 0, 0]);
  });

  it("refetches the head, unless the cached head already has the event's hash", async () => {
    const stale = watch(client, fileKeys.head("sit1", "core/a.yml"), { hash: OTHER });
    const same = watch(client, fileKeys.head("sit1", "core/b.yml"), { hash: HASH });
    await deliver(SAMPLE.file_observed, { ...SAMPLE.file_observed, path: "core/b.yml" });
    expect(refetched(stale, same)).toEqual([1, 0]);
  });

  it("refetches history, the swimlane, the list, and the compares and grids that have the swimlane, and nothing that has not", async () => {
    const history = watch(client, fileKeys.history("sit1", "core/a.yml"));
    const otherHistory = watch(client, fileKeys.history("sit1", "core/z.yml"));
    const detail = watch(client, swimlaneKeys.detail("sit1"));
    const otherDetail = watch(client, swimlaneKeys.detail("sit2"));
    const list = watch(client, swimlaneKeys.all());
    const cmpNfs = watch(client, compareKeys.diff("nfs:sit1", "git:base@main", "core/a.yml", null));
    const cmpOther = watch(client, compareKeys.diff("nfs:sit2", "git:base@main", "core/a.yml", null));
    const cmpGit = watch(client, compareKeys.diff("git:base@main", "git:tenant@main", "core/a.yml", null));
    const grid = watch(client, compareKeys.grid("core/a.yml", ["sit1", "sit2"], false, null));
    const gridOther = watch(client, compareKeys.grid("core/a.yml", ["sit2", "sit3"], false, null));
    const findings = watch(client, findingKeys.list({ limit: 50 }));
    await deliver(SAMPLE.file_observed);
    expect(refetched(history, otherHistory, detail, otherDetail, list)).toEqual([1, 0, 1, 0, 1]);
    expect(refetched(cmpNfs, cmpOther, cmpGit, grid, gridOther, findings)).toEqual([1, 0, 0, 1, 0, 0]);
  });
});

describe("applying the other events", () => {
  it("findings_changed refetches the findings of every filter, the swimlane and the list", async () => {
    const a = watch(client, findingKeys.list({ limit: 50 }));
    const b = watch(client, findingKeys.list({ swimlane: "sit1", limit: 50 }));
    const detail = watch(client, swimlaneKeys.detail("sit1"));
    const tree = watch(client, fileKeys.tree("sit1", "", null));
    await deliver(SAMPLE.findings_changed);
    expect(refetched(a, b, detail, tree)).toEqual([1, 1, 1, 0]);
  });

  it("proposal_changed refetches the queue and that proposal, not another", async () => {
    const queue = watch(client, proposalKeys.list({ status: "pending", limit: 50 }));
    const mine = watch(client, proposalKeys.detail(12));
    const other = watch(client, proposalKeys.detail(13));
    await deliver(SAMPLE.proposal_changed);
    expect(refetched(queue, mine, other)).toEqual([1, 1, 0]);
  });

  it("git_head_moved refetches a read at that branch of that repo only; ref lists only when the branch was new", async () => {
    const onBranch = watch(client, fileKeys.contentRef("sit1", "a.yml", "base", "main"));
    const otherRepo = watch(client, fileKeys.contentRef("sit1", "a.yml", "tenant", "main"));
    const otherRef = watch(client, fileKeys.contentRef("sit1", "a.yml", "base", "v1.0"));
    const refs = watch(client, gitKeys.refs("base", "branch", ""));
    const otherRefs = watch(client, gitKeys.refs("tenant", "branch", ""));
    await deliver({ ...SAMPLE.git_head_moved, from: "d".repeat(40) });
    expect(refetched(onBranch, otherRepo, otherRef, refs, otherRefs)).toEqual([1, 0, 0, 0, 0]);
    await deliver(SAMPLE.git_head_moved);
    expect(refetched(refs, otherRefs)).toEqual([1, 0]);
  });

  it("drift_changed with many paths refetches every tree of the swimlane", async () => {
    const trees = ["", "core", "core/deep", "other"].map((p) => watch(client, fileKeys.tree("sit1", p, null)));
    const sit2 = watch(client, fileKeys.tree("sit2", "", null));
    await deliver({ type: "drift_changed", swimlane: "sit1", paths: Array.from({ length: 60 }, (_, i) => `zzz/f${i}.yml`) });
    expect(refetched(...trees, sit2)).toEqual([1, 1, 1, 1, 0]);
  });

  it("a resync refetches every active query that is not immutable, and does not touch content or blobs", async () => {
    const list = watch(client, swimlaneKeys.all());
    const tree = watch(client, fileKeys.tree("sit1", "", null));
    const admin = watch(client, adminKeys.list("users", { limit: 50 }));
    const content = watch(client, fileKeys.content("sit1", "a.yml", HASH));
    const blob = watch(client, fileKeys.blob(HASH));
    batcher.pushAll();
    await vi.advanceTimersByTimeAsync(WINDOW_MS + 1);
    expect(refetched(list, tree, admin, content, blob)).toEqual([1, 1, 1, 0, 0]);
  });

  it("an event for another swimlane refetches nothing that shows this one", async () => {
    const detail = watch(client, swimlaneKeys.detail("sit1"));
    const tree = watch(client, fileKeys.tree("sit1", "", null));
    await deliver({ type: "findings_changed", swimlane: "sit9" }, { type: "drift_changed", swimlane: "sit9", paths: ["a.yml"] }, { type: "pickup_changed", swimlane: "sit9", services: [] });
    expect(refetched(detail, tree)).toEqual([0, 0]);
  });
});

describe("inactive queries", () => {
  it("are marked stale and not refetched: they refetch when someone mounts them", async () => {
    client.setQueryData(swimlaneKeys.detail("sit1"), { cached: true });
    const fetches = vi.fn(() => Promise.resolve({ cached: false }));
    client.getQueryCache().find({ queryKey: swimlaneKeys.detail("sit1") })?.setOptions({ queryFn: fetches });
    await deliver(SAMPLE.findings_changed);
    expect(fetches).not.toHaveBeenCalled();
    expect(client.getQueryState(swimlaneKeys.detail("sit1"))?.isInvalidated).toBe(true);
  });

  it("never mark an immutable entry stale", async () => {
    client.setQueryData(fileKeys.content("sit1", "a.yml", HASH), { text: "x" });
    batcher.pushAll();
    await vi.advanceTimersByTimeAsync(WINDOW_MS + 1);
    expect(client.getQueryState(fileKeys.content("sit1", "a.yml", HASH))?.isInvalidated).toBe(false);
  });
});

describe("coalescing (100 ms)", () => {
  it("a burst of events for the same things is one refetch, after the window and not before", async () => {
    const detail = watch(client, swimlaneKeys.detail("sit1"));
    const list = watch(client, swimlaneKeys.all());
    for (let i = 0; i < 40; i++) batcher.push(plan({ ...SAMPLE.file_observed, path: `core/f${i}.yml`, hash: `${i}`.padStart(64, "0") }));
    await vi.advanceTimersByTimeAsync(WINDOW_MS - 1);
    expect(refetched(detail, list)).toEqual([0, 0]);
    await vi.advanceTimersByTimeAsync(2);
    expect(refetched(detail, list)).toEqual([1, 1]);
  });

  it("an event after the window is a new window", async () => {
    const detail = watch(client, swimlaneKeys.detail("sit1"));
    await deliver(SAMPLE.findings_changed);
    await deliver(SAMPLE.findings_changed);
    expect(refetched(detail)).toEqual([2]);
  });

  it("falls back to every active query when one window names more than 200 distinct things", async () => {
    expect(MAX_TARGETS).toBe(200);
    const unrelated = watch(client, adminKeys.list("users", { limit: 50 }));
    const content = watch(client, fileKeys.content("sit1", "a.yml", HASH));
    for (let i = 0; i < 70; i++) batcher.push(plan({ ...SAMPLE.file_observed, path: `core/f${i}.yml`, hash: `${i}`.padStart(64, "0") })); // 3 or more targets each
    await vi.advanceTimersByTimeAsync(WINDOW_MS + 1);
    expect(refetched(unrelated, content)).toEqual([1, 0]);
  });

  it("does not fall back at 200 targets or fewer", async () => {
    const unrelated = watch(client, adminKeys.list("users", { limit: 50 }));
    for (let i = 0; i < 30; i++) batcher.push(plan({ ...SAMPLE.file_observed, path: `core/f${i}.yml`, hash: `${i}`.padStart(64, "0") }));
    await vi.advanceTimersByTimeAsync(WINDOW_MS + 1);
    expect(refetched(unrelated)).toEqual([0]);
  });

  it("dispose drops what is waiting and stops the timer", async () => {
    const detail = watch(client, swimlaneKeys.detail("sit1"));
    batcher.push(plan(SAMPLE.findings_changed));
    batcher.dispose();
    await vi.advanceTimersByTimeAsync(1000);
    expect(refetched(detail)).toEqual([0]);
    expect(vi.getTimerCount()).toBe(0);
  });
});

describe("notices", () => {
  it("are handed on at once, not after the window", () => {
    batcher.push(plan(SAMPLE.file_observed));
    batcher.push(plan(SAMPLE.proposal_changed));
    expect(notices).toEqual([
      { kind: "out-of-band-change", swimlane: "sit1", path: "core/a.yml" },
      { kind: "proposal-changed", id: 12, status: "approved" },
    ]);
  });

  it("a listener that throws does not stop the invalidation", async () => {
    const detail = watch(client, swimlaneKeys.detail("sit1"));
    const throwing = new InvalidationBatcher(client, {
      onNotice: () => {
        throw new Error("toaster broke");
      },
    });
    throwing.push(plan(SAMPLE.file_observed));
    await vi.advanceTimersByTimeAsync(WINDOW_MS + 1);
    expect(refetched(detail)).toEqual([1]);
    throwing.dispose();
  });
});

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { Role, type Me } from "@/api/generated/model";
import { SessionContext, type SessionState } from "@/auth/session";
import { proposalKeys } from "@/lib/query/keys";
import type { Notice } from "@/live/invalidation";
import { NoticeToasts } from "./notices";
import { ToastStore } from "./store";

// The live layer says what a person should be told (a scan saw a file change that did not come through this app; a proposal
// changed). This turns the two notices into toasts: an out-of-band change always, a proposal's decision only when the proposal
// is the viewer's own, and nothing else, and never from a server error.

let handler: ((notice: Notice) => void) | undefined;
vi.mock("@/live/LiveProvider", () => ({
  useLiveNotices: (h: (notice: Notice) => void) => {
    handler = h;
  },
}));

const ME = { id: "u1:u2", display_name: "Ada", email: "a@example.test", role: Role.editor, status: "active", requires_approval: false, github_login: null, last_seen_at: null } as unknown as Me;
let store: ToastStore;
let client: QueryClient;

function mount(role: Role = Role.editor) {
  const session: SessionState = { status: "ready", me: { ...ME, role }, role };
  render(
    <QueryClientProvider client={client}>
      <SessionContext.Provider value={session}>
        <NoticeToasts store={store} />
      </SessionContext.Provider>
    </QueryClientProvider>,
  );
}

const notify = async (notice: Notice) => {
  await act(async () => {
    handler?.(notice);
    await Promise.resolve();
  });
};

const proposal = (id: number, author: string) => ({ id, author, status: "approved" });

beforeEach(() => {
  handler = undefined;
  store = new ToastStore();
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
});

describe("out of band changes", () => {
  it("shows the swimlane, the file's name and the design's words, with a link to the swimlane", async () => {
    mount();
    await notify({ kind: "out-of-band-change", swimlane: "sitb", path: "tx-infinity-api/tx-infinity-core-sit1.yml" });
    const [item] = store.getSnapshot();
    expect(item?.action).toEqual({ label: "View swimlane", href: "/swimlanes/sitb" });
    const text = JSON.stringify(item?.title);
    expect(text).toContain("sitb");
    expect(text).toContain("tx-infinity-core-sit1.yml");
    expect(text).not.toContain("tx-infinity-api/");
    expect(text).toContain("changed on NFS (unknown author)");
  });

  it("is one toast for the same file, however often a scan sees it", async () => {
    mount();
    for (let i = 0; i < 3; i++) await notify({ kind: "out-of-band-change", swimlane: "sitb", path: "a.yml" });
    expect(store.getSnapshot()).toHaveLength(1);
  });

  it("has no link when the swimlane id is not one the router would accept", async () => {
    mount();
    await notify({ kind: "out-of-band-change", swimlane: "../x", path: "a.yml" });
    expect(store.getSnapshot()[0]?.action).toBeUndefined();
  });
});

describe("proposals", () => {
  it("tells the author that their proposal was decided, with a link only when they may open approvals", async () => {
    client.setQueryData(proposalKeys.detail(12), proposal(12, ME.id));
    mount(Role.admin);
    await notify({ kind: "proposal-changed", id: 12, status: "approved" });
    expect(JSON.stringify(store.getSnapshot()[0]?.title)).toContain("Your proposal #12 was approved");
    expect(store.getSnapshot()[0]?.action).toEqual({ label: "View approval", href: "/approvals/12" });
  });

  it("has no link for an Editor, who cannot open the approvals screen", async () => {
    client.setQueryData(proposalKeys.detail(12), proposal(12, ME.id));
    mount(Role.editor);
    await notify({ kind: "proposal-changed", id: 12, status: "rejected" });
    expect(JSON.stringify(store.getSnapshot()[0]?.title)).toContain("was rejected");
    expect(store.getSnapshot()[0]?.action).toBeUndefined();
  });

  it("says nothing about somebody else's proposal", async () => {
    client.setQueryData(proposalKeys.detail(13), proposal(13, "someone:else"));
    mount();
    await notify({ kind: "proposal-changed", id: 13, status: "approved" });
    expect(store.getSnapshot()).toEqual([]);
  });

  it("says nothing for a status that is not a decision (pending, draft)", async () => {
    client.setQueryData(proposalKeys.detail(14), proposal(14, ME.id));
    mount();
    await notify({ kind: "proposal-changed", id: 14, status: "pending" });
    await notify({ kind: "proposal-changed", id: 14, status: "draft" });
    expect(store.getSnapshot()).toEqual([]);
  });

  it("says nothing, and does not throw, when the proposal cannot be read", async () => {
    mount();
    // Nothing cached and the request fails: the query rejects with the app's problem; the notice is dropped.
    vi.stubGlobal("fetch", vi.fn(() => Promise.resolve(new Response(JSON.stringify({ type: "urn:lanekeeper:problem:forbidden", title: "No", status: 403 }), { status: 403, headers: { "content-type": "application/problem+json" } }))));
    await notify({ kind: "proposal-changed", id: 99, status: "approved" });
    expect(store.getSnapshot()).toEqual([]);
    vi.unstubAllGlobals();
  });

  it("reads the proposal once to learn its author and not again", async () => {
    const fetcher = vi.fn(() => Promise.resolve(new Response(JSON.stringify({ ...proposal(15, ME.id), swimlane: "sit1" }), { status: 200, headers: { "content-type": "application/json" } })));
    vi.stubGlobal("fetch", fetcher);
    mount();
    await notify({ kind: "proposal-changed", id: 15, status: "applied" });
    await notify({ kind: "proposal-changed", id: 15, status: "rejected" });
    expect(fetcher.mock.calls.length).toBeLessThanOrEqual(1);
    vi.unstubAllGlobals();
  });
});

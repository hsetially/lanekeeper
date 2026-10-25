import { screen, waitFor, act } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { DriftState } from "@/api/generated/model";
import { prKeys } from "@/lib/query/keys";
import { usePrBadges } from "./use-pr-badges";
import { prLink, problem, renderPrs, routes, type Call } from "./test-utils";

// The hook that joins a swimlane's pull-request links to its files. One request per state for the whole swimlane (never one per
// file), open ones followed to the end, merged ones the newest page only.

const PATH = "tx-infinity-api/tx-infinity-core-sit7.yml";
const OTHER = "application.properties";
const query = (call: Call) => new URL(call.path, "http://hub.test").searchParams;

type Probe = { path: string; hash?: string; drift?: DriftState };
function Probe({ swimlane, files, enabled }: { swimlane: string; files: Probe[]; enabled?: boolean }) {
  const badges = usePrBadges(swimlane, enabled === undefined ? {} : { enabled });
  return (
    <div>
      <p data-testid="ready">{String(badges.ready)}</p>
      <p data-testid="truncated">{String(badges.truncated)}</p>
      {files.map((f) => {
        const b = badges.badgeFor(f);
        return (
          <p key={f.path} data-testid={f.path}>
            {b === null ? "none" : `${b.kind}:${b.link.pr_number}${b.outdated ? ":outdated" : ""}`}
          </p>
        );
      })}
    </div>
  );
}

const server = (open: ReturnType<typeof prLink>[], merged: ReturnType<typeof prLink>[]) =>
  routes({ "GET /api/v1/prs": (call) => ({ items: query(call).get("state") === "merged" ? merged : open, next: null }) });

describe("usePrBadges", () => {
  it("asks for the open and the merged links of the swimlane, once each, and not for the files", async () => {
    const { calls } = renderPrs(<Probe swimlane="sitb" files={[{ path: PATH }, { path: OTHER }]} />, { api: server([prLink()], []) });
    await waitFor(() => expect(screen.getByTestId("ready")).toHaveTextContent("true"));
    const asked = calls.filter((c) => c.path.startsWith("/api/v1/prs")).map((c) => Object.fromEntries(query(c)));
    expect(asked).toHaveLength(2);
    expect(asked).toEqual(expect.arrayContaining([expect.objectContaining({ swimlane: "sitb", state: "open" }), expect.objectContaining({ swimlane: "sitb", state: "merged" })]));
    expect(screen.getByTestId(PATH)).toHaveTextContent("open:418");
    expect(screen.getByTestId(OTHER)).toHaveTextContent("none");
  });

  it("derives the merged badge from the file's drift and hash", async () => {
    const merged = prLink({ id: 2, state: "merged", pr_number: 417, closed_at: 5, path: OTHER });
    renderPrs(
      <Probe
        swimlane="sitb"
        files={[
          { path: OTHER, drift: "nfs_ahead", hash: "a".repeat(64) },
          { path: `${OTHER}.copy`, drift: "nfs_ahead" },
        ]}
      />,
      { api: server([], [merged]) },
    );
    await waitFor(() => expect(screen.getByTestId(OTHER)).toHaveTextContent("merged:417"));
    expect(screen.getByTestId(`${OTHER}.copy`)).toHaveTextContent("none");
  });

  it("marks an open pull request raised for an earlier version", async () => {
    renderPrs(<Probe swimlane="sitb" files={[{ path: PATH, hash: "b".repeat(64) }]} />, { api: server([prLink()], []) });
    await waitFor(() => expect(screen.getByTestId(PATH)).toHaveTextContent("open:418:outdated"));
  });

  it("follows the open links to the end, so a badge is not lost to the first page", async () => {
    const second = prLink({ id: 9, path: OTHER, pr_number: 430 });
    const api = routes({
      "GET /api/v1/prs": (call) => {
        const q = query(call);
        if (q.get("state") === "merged") return { items: [], next: null };
        return q.get("cursor") === null ? { items: [prLink()], next: "c1" } : { items: [second], next: null };
      },
    });
    renderPrs(<Probe swimlane="sitb" files={[{ path: PATH }, { path: OTHER }]} />, { api });
    await waitFor(() => expect(screen.getByTestId(OTHER)).toHaveTextContent("open:430"));
    expect(screen.getByTestId(PATH)).toHaveTextContent("open:418");
  });

  it("asks for nothing while it is not enabled", async () => {
    const { calls } = renderPrs(<Probe swimlane="sitb" files={[{ path: PATH }]} enabled={false} />, { api: server([prLink()], []) });
    await new Promise((r) => setTimeout(r, 50));
    expect(calls.filter((c) => c.path.startsWith("/api/v1/prs"))).toHaveLength(0);
    expect(screen.getByTestId("ready")).toHaveTextContent("false");
    expect(screen.getByTestId(PATH)).toHaveTextContent("none");
  });

  it("is not ready, and draws no badge, when the links could not be read", async () => {
    renderPrs(<Probe swimlane="sitb" files={[{ path: PATH }]} />, { api: () => problem(500, "internal") });
    await new Promise((r) => setTimeout(r, 200));
    expect(screen.getByTestId("ready")).toHaveTextContent("false");
    expect(screen.getByTestId(PATH)).toHaveTextContent("none");
  });

  it("follows a change of state: the list the live layer invalidates ('prs') is read again", async () => {
    let links = [prLink()];
    const { client, calls } = renderPrs(<Probe swimlane="sitb" files={[{ path: PATH, drift: "nfs_ahead" }]} />, {
      api: routes({ "GET /api/v1/prs": (call) => ({ items: query(call).get("state") === "merged" ? links.filter((l) => l.state === "merged") : links.filter((l) => l.state === "open"), next: null }) }),
    });
    await waitFor(() => expect(screen.getByTestId(PATH)).toHaveTextContent("open:418"));
    links = [prLink({ state: "merged", closed_at: 9 })];
    const before = calls.length;
    await act(async () => {
      await client.invalidateQueries({ queryKey: prKeys.lists() });
    });
    await waitFor(() => expect(screen.getByTestId(PATH)).toHaveTextContent("merged:418"));
    expect(calls.length).toBeGreaterThan(before);
  });

  it("does not say it is complete when a page of the open links was cut off", async () => {
    // Every page has a next one: the hook stops at its bound and says so.
    let n = 0;
    const api = routes({ "GET /api/v1/prs": (call) => (query(call).get("state") === "merged" ? { items: [], next: null } : { items: [prLink({ id: ++n, path: `f${n}.yml` })], next: `c${n}` }) });
    renderPrs(<Probe swimlane="sitb" files={[]} />, { api });
    await waitFor(() => expect(screen.getByTestId("truncated")).toHaveTextContent("true"), { timeout: 8000 });
    expect(n).toBeLessThanOrEqual(10);
  });
});

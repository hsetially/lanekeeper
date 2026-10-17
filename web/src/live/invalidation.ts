import type { DomainEvent } from "@/api/generated/model";
import { adminKeys, docKeys, fileKeys, findingKeys, gitKeys, prKeys, proposalKeys, swimlaneKeys, type QueryKey } from "@/lib/query/keys";
import type { Target } from "@/lib/query/targets";

export { matches, targetId, type Target } from "@/lib/query/targets";

// What a live event invalidates: the plan's table ("Query keys, retry and SSE invalidation") as a pure function. `plan` turns
// an event into DATA (which caches, and which notices to show). The vocabulary of targets and the one pass over the QueryClient
// are src/lib/query/targets.ts, shared with the write table; src/live/apply.ts does the coalescing. Keeping the decision as
// data is what lets the test read it row by row.
//
// The rule behind every row: an event refetches the queries a screen is SHOWING that it can change, and nothing else (P13;
// "one SSE event triggering a refetch of everything on every client" is the anti-pattern). Entries nobody is showing are only
// marked stale, so they refetch when a screen mounts them. Content and blobs are functions of a hash and never change, so
// no event names them.
//
// `DomainEvent` is the generated union of the kinds the hub streams (user_sessions_revoked is internal and never reaches a
// browser). The `switch` below has no default: a kind added to the contract without a row here is a compile error.

/** Something a person should be told, not a cache to refresh. The toaster (T4, S0) subscribes to these. */
export type Notice =
  /** A scan saw a file change that did not come through this app: "View swimlane". */
  | { readonly kind: "out-of-band-change"; readonly swimlane: string; readonly path: string }
  /** A proposal changed status. The event has no author, so the app checks whether it is the user's own. */
  | { readonly kind: "proposal-changed"; readonly id: number; readonly status: string };

export type Plan = {
  readonly targets: readonly Target[];
  readonly notices: readonly Notice[];
  /** Every active query that is not immutable (a resync). */
  readonly all: boolean;
};

/** Above this many paths one event refetches every tree and head of the swimlane instead of one target per path. */
export const MAX_PATH_TARGETS = 50;

const nothing: readonly Notice[] = [];
const some = (targets: Target[], notices: readonly Notice[] = nothing): Plan => ({ targets, notices, all: false });
const key = (k: QueryKey): Target => ({ kind: "key", key: k });

export function plan(event: DomainEvent): Plan {
  switch (event.type) {
    case "file_observed": {
      const { swimlane, path, hash } = event;
      return some(
        [
          { kind: "file-head", swimlane, path, hash },
          { kind: "trees", swimlane, paths: [path] },
          key(fileKeys.history(swimlane, path)),
          { kind: "compares", swimlane },
          { kind: "grids", swimlane },
          key(swimlaneKeys.detail(swimlane)),
          key(swimlaneKeys.all()),
        ],
        event.source === "scan" ? [{ kind: "out-of-band-change", swimlane, path }] : nothing,
      );
    }
    case "drift_changed": {
      const { swimlane, paths } = event;
      const head: Target[] = [key(swimlaneKeys.all()), key(swimlaneKeys.detail(swimlane))];
      const tail: Target = { kind: "compare-trees", swimlane };
      if (paths.length > MAX_PATH_TARGETS) return some([...head, key(fileKeys.trees(swimlane)), key(fileKeys.heads(swimlane)), tail]);
      return some([...head, { kind: "trees", swimlane, paths }, ...paths.map((path) => key(fileKeys.head(swimlane, path))), tail]);
    }
    case "findings_changed":
      return some([key(findingKeys.all()), key(swimlaneKeys.detail(event.swimlane)), key(swimlaneKeys.all())]);
    case "pickup_changed":
    case "pending_restart_changed":
      return some([key(swimlaneKeys.pickup(event.swimlane)), key(swimlaneKeys.detail(event.swimlane)), key(swimlaneKeys.all())]);
    case "proposal_changed":
      return some([key(proposalKeys.lists()), key(proposalKeys.detail(event.id))], [{ kind: "proposal-changed", id: event.id, status: event.status }]);
    case "agent_status_changed":
      return some([key(swimlaneKeys.all()), key(swimlaneKeys.detail(event.swimlane)), key(adminKeys.section("agents"))]);
    case "git_head_moved": {
      const targets: Target[] = [key(swimlaneKeys.all()), { kind: "content-refs", repo: event.repo, branch: event.branch }];
      // The lists of refs change only when the branch is new: a head that moved did not add or remove a name.
      if (event.from === null) targets.push(key(gitKeys.repo(event.repo)));
      return some(targets);
    }
    case "access_request_created":
      return some([key(adminKeys.section("access-requests"))]);
    case "doc_indexed":
      return some([key(docKeys.lists()), key(docKeys.detail(event.doc)), key(docKeys.featureStatuses())]);
    case "attribution_updated": {
      const { swimlane, path } = event;
      return some([key(fileKeys.history(swimlane, path)), { kind: "trees", swimlane, paths: [path] }, key(fileKeys.head(swimlane, path))]);
    }
    case "pr_state_changed":
      return some([key(prKeys.lists())]);
    case "pr_job_progress": {
      const targets: Target[] = [key(prKeys.job(event.job_id))];
      if (event.state === "completed" || event.state === "failed") targets.push(key(prKeys.lists()));
      return some(targets);
    }
    case "sync_window_opened":
    case "sync_window_closed":
      return some([key(swimlaneKeys.detail(event.swimlane)), key(swimlaneKeys.all())]);
    case "swimlane_changed":
      // Its baseline, tier, tenant set or auto-notify changed: the summary, the list, and the admin entries that show them.
      return some([
        key(swimlaneKeys.detail(event.swimlane)),
        key(swimlaneKeys.all()),
        key(adminKeys.section("swimlanes")),
        key(adminKeys.list("tenants", { id: event.swimlane })),
        key(adminKeys.list("auto-notify", { id: event.swimlane })),
      ]);
    case "resync":
      return { targets: [], notices: nothing, all: true };
  }
}


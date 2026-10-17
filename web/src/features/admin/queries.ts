import {
  adminGetAutoNotify,
  adminGetRetention,
  adminGetTenants,
  adminListAccessRequests,
  adminListAgents,
  adminListFolderMappings,
  adminListHostTiers,
  adminListMergeModes,
  adminListNotifications,
  adminListOsLoginUsers,
  adminListPathMappings,
  adminListSentinels,
  adminListSeverityRules,
  adminListUsers,
} from "@/api/generated/endpoints/admin/admin";
import type { AdminListAccessRequestsParams, AdminListUsersParams } from "@/api/generated/model";
import { EVENT_FED, POLLED_MS } from "@/lib/query/freshness";
import { adminKeys, type AdminSection } from "@/lib/query/keys";
import { pagedQuery, type Page } from "@/lib/query/paged";
import { apiQuery } from "@/lib/query/options";

// The admin screens (Admin only). Two lists are named by a live event, and are EVENT_FED: access requests
// (access_request_created) and agents (agent_status_changed). The per-swimlane settings (tenants, auto-notify) are named by
// swimlane_changed. Everything else is POLLED: an admin changes it, nobody is told, so a screen that gets old says so and
// offers Refresh, and the write that changed it refetches it at once (src/lib/query/invalidate.ts).

type WithCursor = { cursor?: string };

function list<TPage extends Page<TItem>, TItem, P extends Readonly<Record<string, unknown>>>(section: AdminSection, params: P, run: (params: P & WithCursor, signal: AbortSignal) => Promise<TPage>, staleTime: number) {
  return pagedQuery({
    queryKey: adminKeys.list(section, params),
    fetch: (cursor, signal) => run(cursor === undefined ? params : { ...params, cursor }, signal),
    staleTime,
  });
}

export const adminQueries = {
  users: (params: Omit<AdminListUsersParams, "cursor"> = {}) => list("users", params, (p, signal) => adminListUsers(p, { signal }), POLLED_MS),
  accessRequests: (params: Omit<AdminListAccessRequestsParams, "cursor"> = {}) => list("access-requests", params, (p, signal) => adminListAccessRequests(p, { signal }), EVENT_FED),
  agents: () => list("agents", {}, (p, signal) => adminListAgents(p, { signal }), EVENT_FED),
  folderMappings: () => list("folder-mappings", {}, (p, signal) => adminListFolderMappings(p, { signal }), POLLED_MS),
  hostTiers: () => list("host-tiers", {}, (p, signal) => adminListHostTiers(p, { signal }), POLLED_MS),
  mergeModes: () => list("merge-modes", {}, (p, signal) => adminListMergeModes(p, { signal }), POLLED_MS),
  notifications: () => list("notifications", {}, (p, signal) => adminListNotifications(p, { signal }), POLLED_MS),
  osLoginUsers: () => list("os-login-users", {}, (p, signal) => adminListOsLoginUsers(p, { signal }), POLLED_MS),
  pathMappings: () => list("path-mappings", {}, (p, signal) => adminListPathMappings(p, { signal }), POLLED_MS),
  sentinels: () => list("sentinels", {}, (p, signal) => adminListSentinels(p, { signal }), POLLED_MS),
  severityRules: () => list("severity-rules", {}, (p, signal) => adminListSeverityRules(p, { signal }), POLLED_MS),
  retention: () => apiQuery({ queryKey: adminKeys.list("retention", {}), queryFn: ({ signal }) => adminGetRetention({ signal }), staleTime: POLLED_MS }),
  /** The tenant set of one swimlane. */
  tenants: (swimlane: string) => apiQuery({ queryKey: adminKeys.list("tenants", { id: swimlane }), queryFn: ({ signal }) => adminGetTenants(swimlane, { signal }), staleTime: EVENT_FED }),
  /** The auto-notify switch of one swimlane. The key is the one `OPTIMISTIC.adminPutAutoNotify` patches. */
  autoNotify: (swimlane: string) => apiQuery({ queryKey: adminKeys.list("auto-notify", { id: swimlane }), queryFn: ({ signal }) => adminGetAutoNotify(swimlane, { signal }), staleTime: EVENT_FED }),
};

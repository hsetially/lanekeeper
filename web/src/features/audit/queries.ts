import { adminListAudit } from "@/api/generated/endpoints/admin/admin";
import type { AdminListAuditParams } from "@/api/generated/model";
import { POLLED_MS } from "@/lib/query/freshness";
import { auditKeys } from "@/lib/query/keys";
import { pagedQuery } from "@/lib/query/paged";

// The audit trail (Admin only). No event names it: the contract has no audit event (plan, CG9). It is POLLED, so a screen that
// shows it gets old says "may be out of date" with Refresh, and the "n updates" chip is fed by the events that make audit rows.

export const auditListQuery = (params: Omit<AdminListAuditParams, "cursor"> = {}) =>
  pagedQuery({
    queryKey: auditKeys.list(params),
    fetch: (cursor, signal) => adminListAudit({ ...params, ...(cursor === undefined ? {} : { cursor }) }, { signal }),
    staleTime: POLLED_MS,
  });

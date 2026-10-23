import type { AuditEvent, AuditEventVia } from "@/api/generated/model";

// Words for an audit row. Who made an event is the shared EventWho / AttributionChip (src/features/attribution), which say "Lanekeeper
// user" and the start of the person's id where the screen has no name, and the person's name for an Admin who has the user list.

export function newestFirst(events: readonly AuditEvent[]): AuditEvent[] {
  return [...events].sort((a, b) => b.at - a.at || b.id - a.id);
}

export function viaLabel(via: AuditEventVia): string {
  switch (via) {
    case "ui":
      return "Web";
    case "mcp":
      return "MCP";
    case "system":
      return "System";
    default:
      return "Other";
  }
}

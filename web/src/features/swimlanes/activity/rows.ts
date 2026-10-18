import type { AuditEvent, AuditEventVia } from "@/api/generated/model";

// Words for an audit row. The audit read carries the actor as an id and no name (CG9 would add one), so the screens say "user" and
// the start of the person's own id, and the full id is in the tooltip.

export function newestFirst(events: readonly AuditEvent[]): AuditEvent[] {
  return [...events].sort((a, b) => b.at - a.at || b.id - a.id);
}

/** `<Entra tenant>:<object id>` is the user id; the object id's first part is what tells two people apart. */
export function actorLabel(actor: string | null): string {
  if (actor === null) return "Lanekeeper";
  const object = actor.includes(":") ? actor.slice(actor.indexOf(":") + 1) : actor;
  return `user ${object.slice(0, 8)}`;
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

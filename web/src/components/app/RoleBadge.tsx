import { Badge } from "@/components/ui/badge";

/**
 * Viewer, Editor, Operator, Admin or "No role". The role is the session's own string (the API's Role enum): this file
 * names none of them but Admin, which has the approval outline style because it unlocks Admin-only screens.
 */
export function RoleBadge({ role }: { role: string | null | undefined }) {
  if (role === null || role === undefined) return <Badge variant="muted">No role</Badge>;
  const label = role.charAt(0).toUpperCase() + role.slice(1);
  return <Badge variant={role === "admin" ? "approval" : "outline"}>{label}</Badge>;
}

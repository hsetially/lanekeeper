import { AccessRequests } from "./AccessRequests";
import { People } from "./People";

/** Users and access requests: who waits for a role, and who has one. */
export function UsersSection({ q, role }: { q: string | undefined; role: string | undefined }) {
  return (
    <div className="flex flex-col gap-6">
      <AccessRequests />
      <People q={q} role={role} />
    </div>
  );
}

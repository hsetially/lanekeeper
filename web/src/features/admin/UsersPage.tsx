import type { PageProps } from "@/routes/props";
import { AdminFrame } from "./AdminFrame";
import { UsersSection } from "./users/UsersSection";

// Users and access requests (route /admin/users, Admin only, enforced by the route and by the hub on every call): the access
// requests that wait for an Admin, and the people with their roles. `q` (a name or an email, asked of the hub) and `role` (among the
// people loaded) are in the address so that a link to a search works.
export default function UsersPage({ search }: PageProps<"admin-users">) {
  return (
    <AdminFrame area="users">
      <UsersSection q={search.q} role={search.role} />
    </AdminFrame>
  );
}

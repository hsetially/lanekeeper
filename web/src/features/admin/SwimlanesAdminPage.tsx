import type { PageProps } from "@/routes/props";
import { AdminFrame } from "./AdminFrame";
import { SwimlanesSection } from "./swimlanes/SwimlanesSection";

// Swimlanes and agents (route /admin/swimlanes, Admin only): the swimlanes, the agent registered for each, and per swimlane its
// tenants, auto-notify switch, service account and fallback join token. Registering a swimlane is not here (CG15, no operation).
export default function SwimlanesAdminPage(_props: PageProps<"admin-swimlanes">) {
  return (
    <AdminFrame area="swimlanes">
      <SwimlanesSection />
    </AdminFrame>
  );
}

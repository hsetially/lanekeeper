import type { PageProps } from "@/routes/props";
import { AuditScreen } from "./AuditScreen";

// The audit log (Admin only; the route gate shows everyone else the permission screen). The screen is `AuditScreen`; this is the
// route's default export. The filters are in the address (`?actor=&swimlane=&action=&from=&to=`), so a link keeps them.
export default function AuditPage({ search }: PageProps<"audit">) {
  return <AuditScreen search={search} />;
}

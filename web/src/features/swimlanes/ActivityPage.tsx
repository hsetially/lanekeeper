import type { PageProps } from "@/routes/props";
import { AuditList } from "./activity/AuditList";
import { DetailFrame } from "./detail/DetailFrame";

// The Activity tab. The contract's only audit read is Admin-only, so this route is Admin-only too (the route table says so and the
// server enforces it); the tab shows for everyone else disabled, with the role it needs.
export default function ActivityPage({ params }: PageProps<"activity">) {
  return (
    <DetailFrame id={params.id} tab="activity" noun="activity">
      {() => <AuditList swimlane={params.id} variant="activity" />}
    </DetailFrame>
  );
}

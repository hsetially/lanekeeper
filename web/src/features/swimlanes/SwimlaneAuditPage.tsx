import { Button } from "@/components/ui/button";
import { SafeText } from "@/components/app/SafeText";
import type { PageProps } from "@/routes/props";
import { AppLink } from "@/routes/AppLink";
import { AuditList } from "./activity/AuditList";

// The audit trail of one swimlane: every audited event, with its sequence number and request ID. Admin-only, as the Audit screen
// is (the route table says so and the server enforces it).
export default function SwimlaneAuditPage({ params }: PageProps<"swimlane-audit">) {
  return (
    <div className="flex min-h-0 flex-1 flex-col px-6 pt-4">
      <div className="flex flex-none flex-wrap items-end justify-between gap-3 pb-3">
        <div className="flex flex-col gap-1">
          <h1 className="text-xl leading-[26px] font-semibold tracking-[-0.01em]">Audit</h1>
          <span className="text-muted-foreground">
            Every audited event on <SafeText text={params.id} className="font-mono" />
          </span>
        </div>
        <Button asChild variant="secondary">
          <AppLink route="swimlane" params={{ id: params.id }}>
            Back to the swimlane
          </AppLink>
        </Button>
      </div>
      <div className="mb-4 flex min-h-0 flex-1 flex-col overflow-hidden rounded-md border border-border bg-card">
        <AuditList swimlane={params.id} variant="audit" />
      </div>
    </div>
  );
}

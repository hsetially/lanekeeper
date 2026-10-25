import { Button } from "@/components/ui/button";
import type { PageProps } from "@/routes/props";
import { AppLink } from "@/routes/AppLink";
import { DetailFrame } from "./detail/DetailFrame";
import { FindingsTab } from "./findings/FindingsTab";
import { codeFilter } from "./findings/filter";

// The Findings tab. The address can narrow it (`code=C8`, `kind=duplicate_keys`, `severity=high` for high and above), which is how
// the other screens link to "the findings of this check on this swimlane"; a value it does not know is ignored, not guessed at.
export default function SwimlaneFindingsPage({ params, search }: PageProps<"swimlane-findings">) {
  const filter = codeFilter(search);
  return (
    <DetailFrame id={params.id} tab="findings" noun="findings">
      {(lane) => (
        <>
          {filter.label !== null ? (
            <div className="flex h-9 flex-none items-center gap-3 border-b border-border px-4 text-xs">
              <span>Showing {filter.label}.</span>
              <Button asChild variant="ghost" size="sm">
                <AppLink route="swimlane-findings" params={{ id: params.id }}>
                  Show all
                </AppLink>
              </Button>
            </div>
          ) : null}
          <FindingsTab lane={lane} kinds={filter.kinds} minSeverity={filter.minSeverity} />
        </>
      )}
    </DetailFrame>
  );
}


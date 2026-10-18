import { SafeText } from "@/components/app/SafeText";
import { Button } from "@/components/ui/button";
import { TriangleAlert, X } from "@/icons";
import { cn } from "@/lib/utils";

// The grid refuses more swimlanes than it can show and says how to fix it, in place of the grid (the design's frame 1h).

export function TooManyLanes({ lanes, limit, onKeepFirst, onChoose }: { lanes: readonly string[]; limit: number; onKeepFirst: () => void; onChoose: () => void }) {
  const over = lanes.length - limit;
  return (
    <div className="flex min-h-0 flex-1 justify-center overflow-auto px-6 py-8">
      <div role="alert" className="flex h-fit w-full max-w-[680px] flex-col gap-4 rounded-md border border-danger-border bg-card p-4">
        <div className="flex items-start gap-3">
          <span aria-hidden="true" className="inline-flex size-7 shrink-0 items-center justify-center rounded-sm border border-danger-border bg-danger-bg text-danger-fg">
            <TriangleAlert className="size-4" />
          </span>
          <div className="flex flex-col gap-1">
            <h2 className="text-base leading-[22px] font-semibold">Compare up to {limit} swimlanes at a time</h2>
            <p className="text-muted-foreground">
              You selected {lanes.length}. Each swimlane is one column, and past {limit} the values stop being readable. Remove {over} to continue, or keep the first {limit}.
            </p>
          </div>
        </div>
        <div className="flex flex-col gap-1.5">
          <span className="text-xs font-medium text-muted-foreground">
            Selected swimlanes · <span className="text-danger-fg">{over} over the limit</span>
          </span>
          <ul aria-label="Selected swimlanes" className="flex flex-wrap gap-1">
            {lanes.slice(0, 200).map((id, index) => (
              <li
                key={id}
                data-over={index >= limit || undefined}
                className={cn("inline-flex h-[22px] items-center gap-1 rounded-sm border px-1.5 font-mono text-xs", index >= limit ? "border-danger-border bg-danger-bg text-danger-fg" : "border-border bg-background text-foreground")}
              >
                <SafeText text={id} />
                {index >= limit ? (
                  <>
                    <X aria-hidden="true" className="size-2.5" />
                    <span className="sr-only">over the limit</span>
                  </>
                ) : null}
              </li>
            ))}
          </ul>
        </div>
        <div className="flex items-center gap-2">
          <Button onClick={onKeepFirst}>Keep the first {limit}</Button>
          <Button variant="secondary" onClick={onChoose}>
            Choose swimlanes…
          </Button>
          <span className="ml-auto text-xs text-muted-foreground tabular-nums">
            {lanes.length} / {limit}
          </span>
        </div>
      </div>
    </div>
  );
}

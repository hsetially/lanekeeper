import { cn } from "@/lib/utils";
import type { AttributionView } from "./attribution";

// Attribution confidence as four bars and a word (the design's chip): the word is the meaning, the bars are the quick read, and
// "Unknown" has a dashed outline, so the five levels differ without colour.

export function ConfidenceChip({ view }: { view: Pick<AttributionView, "bars" | "confidence"> }) {
  return (
    <span
      data-slot="confidence"
      data-level={view.confidence}
      title={`Attribution confidence: ${view.confidence}`}
      className={cn("inline-flex h-[18px] items-center gap-[5px] rounded-sm border px-1.5 text-xs leading-4", view.confidence === "Unknown" ? "border-dashed border-input" : "border-input")}
    >
      <span aria-hidden="true" className="inline-flex items-end gap-0.5">
        {[0, 1, 2, 3].map((i) => (
          <span key={i} className={cn("h-2 w-[3px] rounded-[1px] bg-current", i < view.bars ? "opacity-100" : "opacity-25")} />
        ))}
      </span>
      {view.confidence}
    </span>
  );
}

import type { Confidence } from "@/api/generated/model";
import { cn } from "@/lib/utils";
import { confidenceView } from "./model";

// How sure the server is about who changed a file, as a word and as a bar of four steps (a quarter for Low, the whole bar for
// Certain). The word is the meaning; the bars are the quick read and are hidden from assistive technology. The contract has four
// levels, not a percentage, so the bar moves a quarter at a time and never claims a number the server did not give.
//
// Two shapes, for the two places the design draws it:
//  - `bars`: a bordered chip with four small upright bars (history entries, drift rows, the file header). No score is a dashed
//    outline with empty bars, so it differs from every level without colour.
//  - `track`: a 36 by 4 track, after the audit table (Attribution column), filled by quarters (25, 50, 75 or 100%) with classes and
//    no inline style. The weaker half (Low, Medium) is the warning colour, the way the design tints a score under 70%; the stronger
//    half is the foreground.

export type ConfidenceMeterProps = {
  /** The level to show, or null for no score. */
  confidence: Confidence | null;
  variant?: "bars" | "track";
  className?: string;
};

const SEGMENTS = [0, 1, 2, 3] as const;

export function ConfidenceMeter({ confidence, variant = "bars", className }: ConfidenceMeterProps) {
  const view = confidenceView(confidence);
  const common = {
    "data-slot": "confidence",
    "data-variant": variant,
    "data-level": view.level ?? "none",
    "data-step": view.step,
    title: `Attribution confidence: ${view.label}`,
  } as const;
  const word = (
    <>
      <span className="sr-only">Confidence: </span>
      {view.label}
    </>
  );

  if (variant === "track") {
    const tint = view.step > 0 && view.step < 3 ? "bg-warning-fg" : "bg-foreground";
    return (
      <span {...common} className={cn("inline-flex items-center gap-1.5 font-mono text-xs leading-4 text-muted-foreground tabular-nums", className)}>
        <span aria-hidden="true" className="flex h-1 w-9 flex-none overflow-hidden rounded-[2px] bg-accent">
          {SEGMENTS.map((i) => (
            <span key={i} data-bar="" data-filled={i < view.step} className={cn("h-full flex-1", i < view.step && tint)} />
          ))}
        </span>
        {word}
      </span>
    );
  }
  return (
    <span {...common} className={cn("inline-flex h-[18px] items-center gap-[5px] rounded-sm border px-1.5 text-xs leading-4", view.level === null ? "border-dashed border-input" : "border-input", className)}>
      <span aria-hidden="true" className="inline-flex items-end gap-0.5">
        {SEGMENTS.map((i) => (
          <span key={i} data-bar="" data-filled={i < view.step} className={cn("h-2 w-[3px] rounded-[1px] bg-current", i < view.step ? "opacity-100" : "opacity-25")} />
        ))}
      </span>
      {word}
    </span>
  );
}

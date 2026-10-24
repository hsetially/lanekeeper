import type { CompareState } from "@/api/generated/model";
import { SafeText } from "@/components/app/SafeText";
import { ArrowLeftRight, Minus, Plus } from "@/icons";
import { cn } from "@/lib/utils";

// The state of a file between two sides, in an icon, a word and a colour (never the colour alone): changed, or only on one
// side (named). The colours are the diff's own: modified, removed, added.

const SPEC = {
  different: { Icon: ArrowLeftRight, classes: "border-warning-border bg-diff-mod-bg text-diff-mod-fg" },
  left_only: { Icon: Minus, classes: "border-danger-border bg-diff-del-bg text-diff-del-fg" },
  right_only: { Icon: Plus, classes: "border-success-border bg-diff-add-bg text-diff-add-fg" },
} as const;

export function StateChip({ state, leftName, rightName, className }: { state: Exclude<CompareState, "same">; leftName: string; rightName: string; className?: string }) {
  const { Icon, classes } = SPEC[state];
  return (
    <span data-state={state} className={cn("inline-flex h-5 shrink-0 items-center gap-1 rounded-sm border px-1.5 text-xs font-medium", classes, className)}>
      <Icon aria-hidden="true" className="size-3" />
      {state === "different" ? "changed" : <>only in <SafeText text={state === "left_only" ? leftName : rightName} /></>}
    </span>
  );
}

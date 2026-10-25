import { SafeText } from "@/components/app/SafeText";
import { Minus, Pencil, Plus } from "@/icons";
import { cn } from "@/lib/utils";
import type { Summary } from "./summary";

// The two summaries beside a diff (Diff, 1a): the settings that changed, as chips with the old value and the new, and the list
// of changes with their lines. Both are the engine's changes, worded; the values are data.

export function SettingChips({ summary }: { summary: Summary }) {
  if (summary.total === 0) return null;
  return (
    <div className="flex flex-none flex-col gap-2 border-b border-border px-4 py-2.5">
      <strong className="font-semibold">{summary.headline}</strong>
      <ul className="m-0 flex list-none flex-wrap gap-1.5 p-0">
        {summary.chips.map((chip, i) => (
          <li key={i} className="inline-flex h-[22px] items-center gap-1.5 rounded-sm border border-border bg-muted px-1.5 font-mono text-xs">
            <span className="text-muted-foreground">
              <SafeText text={chip.prefix} />
            </span>
            <span className="font-medium">
              <SafeText text={chip.key} />
            </span>
            <span className="text-muted-foreground">{chip.kind}</span>
            {chip.from === null ? null : (
              <span className="rounded-[2px] bg-diff-del-word px-0.5">
                <SafeText text={chip.from} />
              </span>
            )}
            {chip.to === null ? null : (
              <span className="rounded-[2px] bg-diff-add-word px-0.5">
                <SafeText text={chip.to} />
              </span>
            )}
          </li>
        ))}
        {summary.hiddenChips > 0 ? <li className="inline-flex h-[22px] items-center text-xs text-muted-foreground">+{summary.hiddenChips} more</li> : null}
      </ul>
    </div>
  );
}

const KIND = {
  Changed: { Icon: Pencil, classes: "text-diff-mod-fg" },
  Added: { Icon: Plus, classes: "text-diff-add-fg" },
  Removed: { Icon: Minus, classes: "text-diff-del-fg" },
} as const;

export function ChangeList({ summary }: { summary: Summary }) {
  return (
    <div className="flex w-[248px] flex-none flex-col overflow-y-auto border-r border-border bg-muted">
      <div className="flex h-8 flex-none items-center border-b border-border px-3 font-semibold">
        {summary.total} {summary.total === 1 ? "change" : "changes"}
      </div>
      <ul className="m-0 list-none p-0">
        {summary.list.map((item, i) => {
          const { Icon, classes } = KIND[item.kind];
          return (
            <li key={i} className="flex flex-col gap-0.5 border-b border-border px-3 py-2">
              <span className="font-mono text-xs font-medium [overflow-wrap:anywhere]">
                <SafeText text={item.key} />
              </span>
              <span className="flex items-center gap-1.5 text-xs text-muted-foreground">
                <span className={cn("inline-flex items-center gap-1 font-medium", classes)}>
                  <Icon aria-hidden="true" className="size-3" />
                  {item.kind}
                </span>
                {item.line === null ? null : <span>line {item.line}</span>}
              </span>
            </li>
          );
        })}
        {summary.hiddenList > 0 ? <li className="px-3 py-2 text-xs text-muted-foreground">and {summary.hiddenList} more</li> : null}
      </ul>
    </div>
  );
}

import { useId } from "react";
import { Button } from "@/components/ui/button";

// "Try instead": the next searches to try when one found nothing (the design's frames 1e, 1h and 1n). Each row says what it
// would do, what it would find when that is known, and has one button.

export type Suggestion = { id: string; label: string; note?: string; action: string; onApply: () => void };

export function SuggestionList({ items }: { items: readonly Suggestion[] }) {
  const heading = useId();
  if (items.length === 0) return null;
  return (
    <section aria-labelledby={heading} className="w-full max-w-[600px] overflow-hidden rounded-md border border-border bg-card">
      <h2 id={heading} className="m-0 flex h-7 items-center border-b border-border bg-muted px-3 text-xs font-medium text-muted-foreground">
        Try instead
      </h2>
      <ul className="m-0 list-none p-0">
        {items.map((item) => (
          <li key={item.id} className="flex min-h-10 items-center gap-3 border-b border-border px-3 py-1.5 last:border-b-0">
            <span className="min-w-0 flex-1 font-medium">{item.label}</span>
            {item.note ? <span className="whitespace-nowrap text-xs tabular-nums text-muted-foreground">{item.note}</span> : null}
            <Button type="button" variant="secondary" size="sm" className="text-xs" onClick={item.onApply}>
              {item.action}
            </Button>
          </li>
        ))}
      </ul>
    </section>
  );
}

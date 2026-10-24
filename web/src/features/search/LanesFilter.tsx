import { useId, useMemo, useState } from "react";
import { SafeText } from "@/components/app/SafeText";
import { TierChip } from "@/components/app/TierBadge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { TenantSet } from "@/features/shell/Tenants";
import { ChevronDown, Layers, Search } from "@/icons";
import { cn } from "@/lib/utils";
import type { LaneInfo } from "./lanes";

// "Swimlanes  All 6": which swimlanes a search covers (the design's multi-select, frames 1b and 1g). `selected` is null for all
// of them. The list has a filter field of its own because there can be forty swimlanes, and one row for each with its tier, its
// tenants and, when the search has results, how many matches are in it.

export function laneLabel(selected: readonly string[] | null, total: number): string {
  if (selected === null || (total > 0 && selected.length >= total)) return `All ${total}`;
  if (selected.length === 0) return "None";
  if (selected.length <= 2) return selected.join(", ");
  return `${selected.slice(0, 2).join(", ")} +${selected.length - 2}`;
}

type Props = {
  lanes: readonly LaneInfo[];
  selected: readonly string[] | null;
  onChange: (next: string[] | null) => void;
  /** Matches per swimlane in the answer on screen; absent when there is none. */
  counts?: ReadonlyMap<string, number> | undefined;
  className?: string;
};

function Row({ lane, checked, count, onToggle }: { lane: LaneInfo; checked: boolean; count: number | undefined; onToggle: (checked: boolean) => void }) {
  const id = useId();
  const countId = `${id}-count`;
  return (
    <div className="grid h-8 grid-cols-[16px_60px_minmax(0,1fr)_auto] items-center gap-2 rounded-sm px-2 hover:bg-accent">
      <Checkbox id={id} checked={checked} onCheckedChange={(next) => onToggle(next === true)} {...(count === undefined ? {} : { "aria-describedby": countId })} />
      <label htmlFor={id} className="contents">
        {lane.tier ? <TierChip tier={lane.tier} className="justify-center" /> : <span />}{" "}
        <span className="flex min-w-0 items-center gap-2">
          <SafeText text={lane.name} className="truncate font-mono text-xs font-medium" /> <TenantSet tenants={lane.tenants} />
        </span>
      </label>
      <span aria-hidden="true" className="text-xs tabular-nums text-muted-foreground">
        {count ?? ""}
      </span>
      {count === undefined ? null : (
        <span id={countId} className="sr-only">
          {count === 1 ? "1 match" : `${count} matches`}
        </span>
      )}
    </div>
  );
}

export function LanesFilter({ lanes, selected, onChange, counts, className }: Props) {
  const [filter, setFilter] = useState("");
  const total = lanes.length;
  const all = useMemo(() => lanes.map((l) => l.id), [lanes]);
  const effective = useMemo(() => new Set(selected ?? all), [selected, all]);
  const shown = useMemo(() => {
    const needle = filter.trim().toLowerCase();
    if (needle === "") return lanes;
    return lanes.filter((l) => [l.id, l.name, l.tier ?? "", ...l.tenants].some((t) => t.toLowerCase().includes(needle)));
  }, [lanes, filter]);

  function toggle(id: string, checked: boolean): void {
    const next = new Set(effective);
    if (checked) next.add(id);
    else next.delete(id);
    // Everything checked is "all", which the address does not spell out.
    onChange(next.size >= total && total > 0 ? null : all.filter((x) => next.has(x)));
  }

  return (
    <Popover onOpenChange={(open) => (open ? undefined : setFilter(""))}>
      <PopoverTrigger asChild>
        <button
          type="button"
          className={cn("inline-flex h-7 w-52 shrink-0 cursor-pointer items-center gap-1.5 rounded-sm border border-input bg-background px-2 text-sm hover:bg-accent data-[state=open]:border-ring data-[state=open]:bg-accent", className)}
        >
          <Layers aria-hidden="true" className="size-3.5 shrink-0 text-muted-foreground" />
          <span className="min-w-0 flex-1 truncate text-left">
            <span className="text-muted-foreground">Swimlanes</span> <span className="font-mono text-xs font-medium">{laneLabel(selected, total)}</span>
          </span>
          <ChevronDown aria-hidden="true" className="size-3 shrink-0 text-muted-foreground" />
        </button>
      </PopoverTrigger>
      <PopoverContent align="end" aria-label="Choose swimlanes" className="w-[344px] overflow-hidden p-0">
        <div className="flex h-9 items-center gap-2 border-b border-border px-2.5">
          <Search aria-hidden="true" className="size-3.5 shrink-0 text-muted-foreground" />
          <Input value={filter} onChange={(event) => setFilter(event.target.value)} aria-label="Filter swimlanes" placeholder="Filter swimlanes" className="h-6 border-0 bg-transparent px-0 shadow-none hover:border-0 focus-visible:shadow-none" />
          <span className="shrink-0 text-xs text-muted-foreground">
            {effective.size} of {total} selected
          </span>
        </div>
        <div role="group" aria-label="Swimlanes" className="flex max-h-72 flex-col overflow-auto p-1">
          {shown.map((lane) => (
            <Row key={lane.id} lane={lane} checked={effective.has(lane.id)} count={counts?.get(lane.id)} onToggle={(checked) => toggle(lane.id, checked)} />
          ))}
          {shown.length === 0 ? <p className="px-2 py-3 text-sm text-muted-foreground">No swimlane matches “{filter.trim()}”.</p> : null}
        </div>
        <div className="flex min-h-8 flex-wrap items-center gap-x-3 gap-y-0.5 border-t border-border bg-muted px-3 py-0.5 text-xs">
          <Button type="button" variant="ghost" size="sm" onClick={() => onChange(null)}>
            Select all
          </Button>
          <Button type="button" variant="ghost" size="sm" onClick={() => onChange([])}>
            Clear
          </Button>
          <span className="ml-auto text-muted-foreground">Counts are matches per swimlane</span>
        </div>
      </PopoverContent>
    </Popover>
  );
}

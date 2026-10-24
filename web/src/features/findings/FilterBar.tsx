import type { ReactNode } from "react";
import { Severity, type FindingKind } from "@/api/generated/model";
import { SafeText } from "@/components/app/SafeText";
import { TierChip } from "@/components/app/TierBadge";
import { MIN_SEVERITY_CHOICES } from "@/components/attribution/severity";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuCheckboxItem, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuRadioGroup, DropdownMenuRadioItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Plus, X } from "@/icons";
import { cn } from "@/lib/utils";
import { CodeChip } from "./cells";
import { NO_FILTERS, isFiltered, toggled, type Filters } from "./filters";
import { KINDS, codeOf, nameOf } from "./kinds";

// The toolbar of the Findings screen, after the design's: a dashed "+ Swimlane", "+ Check" and "+ Severity" button each, which
// fill in (a solid border and the first two values) once a choice is made, and a one-click "Clear filters". Each opens a menu of
// checkable items. The design's severity menu picks any of the levels; the server filters by "that and above" (`min_severity`),
// so this one picks that, and says so in its words.

/** A swimlane the menu offers: its tier and the number of open findings the swimlane list says it has, when known. */
export type LaneChoice = { id: string; tier: string | null; count: number | null };

/** The first two values and how many more, as the design's chips write them. */
const shortList = (values: readonly string[]): string => values.slice(0, 2).join(", ") + (values.length > 2 ? ` +${values.length - 2}` : "");

function FilterButton({ label, value, children }: { label: string; value: string | null; children: ReactNode }) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" className={cn("border-dashed border-input font-normal data-[state=open]:border-solid data-[state=open]:border-ring data-[state=open]:bg-accent", value !== null && "border-solid bg-background")}>
          <Plus data-icon="inline-start" />
          {label}
          {value === null ? null : <span className="border-l border-border pl-1.5 font-mono text-xs">{value}</span>}
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-72">
        {children}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

// A menu of several choices stays open while the person ticks them: the item's own select closes the menu, so it is cancelled.
const keepOpen = (event: Event): void => {
  event.preventDefault();
};

export function FilterBar({ filters, onChange, lanes, summary }: { filters: Filters; onChange: (next: Filters) => void; lanes: readonly LaneChoice[]; summary: string }) {
  return (
    <div role="group" aria-label="Filters" className="relative z-[3] flex h-10 flex-none items-center gap-2 border-b border-border bg-muted px-2">
      <FilterButton label="Swimlane" value={filters.lanes.length === 0 ? null : shortList(filters.lanes)}>
        {lanes.map((lane) => (
          <DropdownMenuCheckboxItem
            key={lane.id}
            checked={filters.lanes.includes(lane.id)}
            onCheckedChange={() => {
              onChange({ ...filters, lanes: toggled(filters.lanes, lane.id) });
            }}
            onSelect={keepOpen}
          >
            {lane.tier === null ? null : <TierChip tier={lane.tier} />} <SafeText text={lane.id} className="font-mono" />
            {lane.count === null ? null : (
              <>
                {" "}
                <span className="ml-auto text-xs text-muted-foreground tabular-nums">{lane.count}</span>
              </>
            )}
          </DropdownMenuCheckboxItem>
        ))}
        <DropdownMenuSeparator />
        <DropdownMenuLabel>Counts are all the open findings of the swimlane</DropdownMenuLabel>
        <DropdownMenuItem
          onSelect={() => {
            onChange({ ...filters, lanes: [] });
          }}
        >
          Clear
        </DropdownMenuItem>
      </FilterButton>

      <FilterButton label="Check" value={filters.kinds.length === 0 ? null : shortList(filters.kinds.map(codeOf))}>
        {KINDS.map((kind: FindingKind) => (
          <DropdownMenuCheckboxItem
            key={kind}
            checked={filters.kinds.includes(kind)}
            onCheckedChange={() => {
              onChange({ ...filters, kinds: KINDS.filter((k) => (k === kind ? !filters.kinds.includes(k) : filters.kinds.includes(k))) });
            }}
            onSelect={keepOpen}
          >
            <CodeChip code={codeOf(kind)} /> <span className="min-w-0 truncate">{nameOf(kind)}</span>
          </DropdownMenuCheckboxItem>
        ))}
        <DropdownMenuSeparator />
        <DropdownMenuItem
          onSelect={() => {
            onChange({ ...filters, kinds: [] });
          }}
        >
          Clear
        </DropdownMenuItem>
      </FilterButton>

      <FilterButton label="Severity" value={MIN_SEVERITY_CHOICES.find((choice) => choice.value === filters.minSeverity)?.label ?? null}>
        <DropdownMenuRadioGroup
          value={filters.minSeverity ?? "any"}
          onValueChange={(value) => {
            onChange({ ...filters, minSeverity: Object.values(Severity).find((s) => s === value) ?? null });
          }}
        >
          {MIN_SEVERITY_CHOICES.map((choice) => (
            <DropdownMenuRadioItem key={choice.value} value={choice.value}>
              {choice.label}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
        <DropdownMenuSeparator />
        <DropdownMenuLabel>Severity is set by the hub's rules</DropdownMenuLabel>
      </FilterButton>

      {isFiltered(filters) ? (
        <Button
          variant="ghost"
          size="sm"
          onClick={() => {
            onChange(NO_FILTERS);
          }}
        >
          <X data-icon="inline-start" />
          Clear filters
        </Button>
      ) : null}
      <span aria-live="polite" className="ml-auto text-xs text-muted-foreground tabular-nums">
        {summary}
      </span>
    </div>
  );
}

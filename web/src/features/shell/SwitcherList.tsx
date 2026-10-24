import { Command as CommandPrimitive } from "cmdk";
import { useEffect, useRef } from "react";
import type { SwimlaneSummary } from "@/api/generated/model";
import { SafeText } from "@/components/app/SafeText";
import { StatusBadge } from "@/components/app/StatusBadge";
import { TierChip } from "@/components/app/TierBadge";
import { ErrorState } from "@/components/app/ErrorState";
import { EmptyState } from "@/components/app/EmptyState";
import { driftSummary } from "@/components/app/drift";
import { Command, CommandEmpty, CommandItem, CommandList } from "@/components/ui/command";
import { Kbd } from "@/components/ui/kbd";
import { Skeleton } from "@/components/ui/skeleton";
import { swimlaneListQuery } from "@/features/swimlanes/queries";
import { Check, Layers, Search } from "@/icons";
import { cursorPagesSource, useQueryState } from "@/lib/query/use-query-state";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { TenantSet } from "./Tenants";

// The body of the swimlane switcher's popover (Shell frame 1h): a filter field and one row per swimlane with its tier, name,
// tenants and drift. It is a chunk of its own because it is built on cmdk, which the first screen does not need; the trigger
// loads it when the pointer or focus arrives.

export function SwimlaneRow({ swimlane, current }: { swimlane: SwimlaneSummary; current: boolean }) {
  const drift = driftSummary(swimlane.drift_counts);
  return (
    <div className="grid w-full grid-cols-[16px_60px_minmax(0,1fr)_auto_auto] items-center gap-2">
      <Check aria-hidden="true" className={current ? "size-3.5" : "size-3.5 opacity-0"} />
      {swimlane.tier ? <TierChip tier={swimlane.tier} className="justify-center" /> : <span />}
      <SafeText text={swimlane.display_name} className="truncate font-mono text-xs font-medium" />
      <TenantSet tenants={swimlane.tenants} />
      <StatusBadge variant={drift.variant} {...(drift.total > 0 ? { count: drift.total } : {})} label={drift.total > 0 ? drift.word : "In sync"} />
    </div>
  );
}

export default function SwitcherList({ currentId, onSelect }: { currentId: string | null; onSelect: (id: string) => void }) {
  const pages = useCursorPages({ ...swimlaneListQuery(), autoLoad: true });
  const state = useQueryState(cursorPagesSource(pages), { isEmpty: (items) => items.length === 0 });
  // The popover has focused itself by the time this chunk has loaded and drawn the field, so the field asks for focus itself:
  // a person who opens the switcher and types must be typing into the filter.
  const filter = useRef<HTMLInputElement>(null);
  useEffect(() => {
    filter.current?.focus();
  }, []);

  return (
    <Command label="Swimlanes" className="rounded-md">
      <div className="flex h-9 items-center gap-2 border-b border-border px-2.5">
        <Search aria-hidden="true" className="size-3.5 shrink-0 text-muted-foreground" />
        <CommandPrimitive.Input ref={filter} aria-label="Filter swimlanes" placeholder="Filter swimlanes…" className="min-w-0 flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground" />
        <Kbd>Esc</Kbd>
      </div>
      {state.state === "loading" ? (
        <div role="status" aria-busy="true" aria-label="Loading swimlanes" className="flex flex-col gap-1 p-2">
          {Array.from({ length: 6 }, (_, i) => (
            <Skeleton key={i} className="h-7" />
          ))}
        </div>
      ) : state.state === "error" ? (
        <ErrorState title="Couldn't load swimlanes" {...(state.error ? { problem: state.error } : {})} {...(state.retryable ? { onRetry: state.refetch } : {})} className="py-6" />
      ) : state.state === "empty" ? (
        <EmptyState icon={Layers} title="No swimlanes registered yet" className="py-6">
          A swimlane appears here once an agent registers for its Kubernetes namespace.
        </EmptyState>
      ) : (
        <CommandList className="max-h-[320px]">
          {(state.data ?? []).map((swimlane) => (
            <CommandItem
              key={swimlane.id}
              value={swimlane.id}
              keywords={[swimlane.display_name, ...swimlane.tenants, swimlane.tier ?? ""]}
              onSelect={() => {
                onSelect(swimlane.id);
              }}
              className="h-8 data-[selected=true]:bg-accent"
              data-current={swimlane.id === currentId ? "true" : undefined}
            >
              <SwimlaneRow swimlane={swimlane} current={swimlane.id === currentId} />
            </CommandItem>
          ))}
          <CommandEmpty>No swimlane matches.</CommandEmpty>
        </CommandList>
      )}
      <div className="flex h-7 items-center gap-4 border-t border-border bg-muted px-3 text-xs text-muted-foreground">
        <span>↑↓ navigate</span>
        <span>↵ switch</span>
        <span className="ml-auto">Remembered per session</span>
      </div>
    </Command>
  );
}

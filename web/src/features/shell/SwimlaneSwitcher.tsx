import { Suspense, lazy } from "react";
import { useState } from "react";
import { useRouterState } from "@tanstack/react-router";
import { SafeText } from "@/components/app/SafeText";
import { StatusBadge } from "@/components/app/StatusBadge";
import { TierChip } from "@/components/app/TierBadge";
import { driftSummary } from "@/components/app/drift";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Skeleton } from "@/components/ui/skeleton";
import { swimlaneListQuery } from "@/features/swimlanes/queries";
import { ChevronsUpDown } from "@/icons";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { hrefFor } from "@/routes/links";
import { useNavigateTo } from "@/routes/AppLink";
import { chooseSwimlane, useCurrentSwimlane } from "./current-swimlane";
import { TenantSet, tenantSummary } from "./Tenants";

// The swimlane switcher of the header (Shell frames 1a, 1h): the swimlane the person is working in, with its tier, its first
// tenant and a "+N" chip (the set is in a tooltip), and its drift; a click opens the list to change it. The design shows one
// branch; a swimlane has a set of tenants (D84), so the chip. The tier chip is there because the contract has the field
// (`SwimlaneSummary.tier`, CG4); a swimlane without one shows no chip.

/** The routes that belong to one swimlane: choosing another swimlane keeps the person on the same tab. */
const SWIMLANE_ROUTES = new Set(["swimlane", "files", "services", "drift", "swimlane-findings", "swimlane-audit", "activity", "adopt"]);

const List = lazy(() => import("./SwitcherList"));
const preloadList = (): void => {
  void import("./SwitcherList");
};

export function SwimlaneSwitcher() {
  const [open, setOpen] = useState(false);
  const navigate = useNavigateTo();
  const route = useRouterState({
    select: (s) => {
      const last = s.matches.at(-1);
      return { id: last?.staticData.routeId, swimlane: (last?.params as { id?: string } | undefined)?.id };
    },
  });
  const onSwimlane = route.id !== undefined && SWIMLANE_ROUTES.has(route.id);
  const currentId = useCurrentSwimlane(onSwimlane && route.swimlane !== undefined ? route.swimlane : null);
  const pages = useCursorPages({ ...swimlaneListQuery(), autoLoad: true });
  const current = currentId === null ? undefined : pages.items.find((s) => s.id === currentId);

  const choose = (id: string): void => {
    chooseSwimlane(id);
    setOpen(false);
    // Stay on the same tab of the new swimlane; anywhere else, open its overview.
    navigate(onSwimlane && route.id !== undefined ? hrefFor(route.id, { id }) : hrefFor("swimlane", { id }));
  };

  const drift = current ? driftSummary(current.drift_counts) : null;
  const label = current
    ? `Swimlane: ${current.display_name}${current.tier ? `, ${current.tier}` : ""}${current.tenants.length > 0 ? `, tenants ${tenantSummary(current.tenants).all}` : ""}${drift ? `, ${drift.total > 0 ? `${drift.total} ${drift.word}` : "in sync"}` : ""}`
    : currentId !== null
      ? `Swimlane: ${currentId}`
      : "Choose a swimlane";

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          aria-label={label}
          onPointerEnter={preloadList}
          onFocus={preloadList}
          className="inline-flex h-7 flex-none cursor-pointer items-center gap-2 rounded-sm border border-input bg-background px-2 hover:bg-accent"
        >
          {current ? (
            <>
              {current.tier ? <TierChip tier={current.tier} /> : null}
              <SafeText text={current.display_name} className="font-mono text-xs font-medium" />
              <TenantSet tenants={current.tenants} />
              {drift ? <StatusBadge variant={drift.variant} {...(drift.total > 0 ? { count: drift.total } : {})} label={drift.total > 0 ? drift.word : "In sync"} /> : null}
            </>
          ) : currentId !== null ? (
            <SafeText text={currentId} className="font-mono text-xs font-medium" />
          ) : (
            <span className="text-muted-foreground">Choose a swimlane</span>
          )}
          <ChevronsUpDown aria-hidden="true" className="size-3.5 shrink-0 text-muted-foreground" />
        </button>
      </PopoverTrigger>
      <PopoverContent align="end" aria-label="Choose a swimlane" className="w-[400px] max-w-[calc(100vw-2rem)] overflow-hidden p-0">
        <Suspense fallback={<Skeleton className="m-2 h-40" />}>
          <List currentId={currentId} onSelect={choose} />
        </Suspense>
      </PopoverContent>
    </Popover>
  );
}

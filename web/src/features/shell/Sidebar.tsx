import { Fragment } from "react";
import { useRouterState } from "@tanstack/react-router";
import type { Me, Role } from "@/api/generated/model";
import { IconButton } from "@/components/app/IconButton";
import { Kbd } from "@/components/ui/kbd";
import { Skeleton } from "@/components/ui/skeleton";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { ChevronDown, PanelLeft } from "@/icons";
import { cn } from "@/lib/utils";
import { AppLink } from "@/routes/AppLink";
import { formatBadge, navIdForRoute, visibleGroups, type NavEntry } from "./nav";
import { useNavBadges } from "./nav-badges";
import { useShell } from "./ShellContext";
import { UserMenu } from "./UserMenu";

// The sidebar (the Shell): 224 px, or a 48 px rail of icons with tooltips and count badges. It shows what the role may open
// (nav.ts) and marks the current item with aria-current. Collapsed or not is a layout preference (src/lib/prefs), the one
// thing here that is remembered.

const ITEM = "flex h-7 items-center gap-2 rounded-sm px-2 text-foreground no-underline hover:bg-accent hover:text-foreground";

function Count({ value, more, collapsed, noun }: { value: number | null; more: boolean; collapsed: boolean; noun: string }) {
  if (value === null || value === 0) return null;
  const text = formatBadge(value, more);
  return collapsed ? (
    <span aria-hidden="true" className="absolute -top-1 -right-[3px] min-w-4 rounded-full bg-foreground px-1 text-center text-xs leading-4 font-semibold text-background tabular-nums">
      {text}
    </span>
  ) : (
    <span data-slot="nav-count" className="inline-flex h-[18px] min-w-5 items-center justify-center rounded-sm border border-input bg-background px-[5px] text-xs font-medium tabular-nums">
      {text}
      <span className="sr-only"> {noun}</span>
    </span>
  );
}

/** What a count counts, in words: "open" findings, "pending" approvals, "pending requests" for the Admin item (one request in the singular). */
function nounOf(badge: NavEntry["badge"], count: number | null): string {
  if (badge === "approvals") return "pending";
  if (badge === "admin") return count === 1 ? "pending request" : "pending requests";
  return "open";
}

function Item({ entry, current, collapsed, count, more }: { entry: NavEntry; current: boolean; collapsed: boolean; count: number | null; more: boolean }) {
  const noun = nounOf(entry.badge, count);
  const Icon = entry.icon;
  if (collapsed) {
    const named = count !== null && count > 0 ? `${entry.label}, ${formatBadge(count, more)} ${noun}` : entry.label;
    return (
      <div className="relative flex justify-center">
        <Tooltip>
          <TooltipTrigger asChild>
            <AppLink
              route={entry.route as never}
              aria-label={named}
              {...(current ? { "aria-current": "page" as const } : {})}
              className={cn("relative flex h-7 w-8 items-center justify-center rounded-sm text-foreground hover:bg-accent hover:text-foreground", current && "bg-selection")}
            >
              <Icon aria-hidden="true" className="size-4" />
              <Count value={count} more={more} collapsed noun={noun} />
            </AppLink>
          </TooltipTrigger>
          <TooltipContent side="right">{count !== null && count > 0 ? `${entry.label} · ${formatBadge(count, more)} ${noun}` : entry.label}</TooltipContent>
        </Tooltip>
      </div>
    );
  }
  return (
    <>
      <AppLink route={entry.route as never} {...(current ? { "aria-current": "page" as const } : {})} className={cn(ITEM, current && "bg-selection font-medium")}>
        <Icon aria-hidden="true" className="size-3.5 shrink-0" />
        <span className="min-w-0 flex-1 truncate">{entry.label}</span>
        <Count value={count} more={more} collapsed={false} noun={noun} />
        {entry.children ? <ChevronDown aria-hidden="true" className="size-3 shrink-0 text-muted-foreground" /> : null}
      </AppLink>
      {entry.children?.map((child) => (
        <AppLink
          key={child.label}
          route="search"
          search={child.search}
          className="flex h-6 items-center gap-2 rounded-sm pr-2 pl-[30px] text-muted-foreground no-underline hover:bg-accent hover:text-foreground"
        >
          <span className="flex-1">{child.label}</span>
          {child.hint ? <Kbd className="bg-background">{child.hint}</Kbd> : null}
        </AppLink>
      ))}
    </>
  );
}

/** The sidebar while the session is still loading: its shape, no items (no role is known to choose them). */
export function SidebarSkeleton() {
  return (
    <aside aria-label="Primary" aria-busy="true" data-slot="sidebar" className="relative z-[5] flex w-56 flex-none flex-col border-r border-border bg-muted">
      <div className="flex h-11 flex-none items-center border-b border-border px-3">
        <span className="text-[14px] leading-5 font-semibold tracking-[-0.01em]">Lanekeeper</span>
      </div>
      <div className="flex flex-1 flex-col gap-2 p-2" aria-hidden="true">
        {Array.from({ length: 7 }, (_, i) => (
          <Skeleton key={i} className="h-7" />
        ))}
      </div>
    </aside>
  );
}

export function Sidebar({ me, role }: { me: Me; role: Role | null }) {
  const { sidebarCollapsed: collapsed, toggleSidebar } = useShell();
  const routeId = useRouterState({ select: (s) => s.matches.at(-1)?.staticData.routeId });
  const currentId = navIdForRoute(routeId);
  const groups = visibleGroups(role);
  const badges = useNavBadges(role);
  const counts: Record<string, { value: number | null; more: boolean }> = { findings: { value: badges.findings, more: false }, approvals: { value: badges.approvals, more: badges.approvalsMore }, admin: { value: badges.admin, more: badges.adminMore } };

  return (
    <aside aria-label="Primary" data-slot="sidebar" data-collapsed={collapsed} className={cn("relative z-[5] flex flex-none flex-col border-r border-border bg-muted", collapsed ? "w-12" : "w-56")}>
      <div className={cn("flex h-11 flex-none items-center gap-2 border-b border-border", collapsed ? "justify-center px-2" : "justify-between pr-2 pl-3")}>
        {collapsed ? null : <span className="text-[14px] leading-5 font-semibold tracking-[-0.01em]">Lanekeeper</span>}
        <IconButton label="Toggle sidebar" shortcut="⌘B" aria-expanded={!collapsed} aria-controls="primary-nav" onClick={toggleSidebar}>
          <PanelLeft className="size-4" />
        </IconButton>
      </div>
      <nav id="primary-nav" aria-label="Sections" className="flex flex-1 flex-col gap-0.5 overflow-y-auto p-2">
        {groups.map((group, index) => (
          <Fragment key={group[0]?.id ?? index}>
            {index > 0 ? <div aria-hidden="true" className={cn("my-1.5 h-px bg-border", collapsed && "mx-1")} /> : null}
            {group.map((entry) => (
              <Item key={entry.id} entry={entry} current={entry.id === currentId} collapsed={collapsed} count={entry.badge ? (counts[entry.badge]?.value ?? null) : null} more={entry.badge ? (counts[entry.badge]?.more ?? false) : false} />
            ))}
          </Fragment>
        ))}
      </nav>
      <div className="flex-none border-t border-border p-2">
        <UserMenu me={me} role={role} collapsed={collapsed} />
      </div>
    </aside>
  );
}

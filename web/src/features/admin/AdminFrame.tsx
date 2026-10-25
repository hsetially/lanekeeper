import type { ReactNode } from "react";
import { Badge } from "@/components/ui/badge";
import { ShieldCheck } from "@/icons";
import { cn } from "@/lib/utils";
import { AppLink } from "@/routes/AppLink";
import { ADMIN_NAV, areaCopy, type AdminArea, type NavItem } from "./nav";
import { usePendingRequests } from "./users/pending";
import { FOCUSABLE_SCROLLER } from "@/components/app/scroll-region";

// The Admin area as the design draws it (`Lanekeeper Admin Page`): the area's own title, a left list of sections in four groups with
// the count of waiting requests beside Users, and the section's heading and lead above its content. Every Admin route renders inside
// it. The page renders the one h1 of the screen ("Admin"); each section's own heading is an h2.
//
// The frame makes no write and holds no data of its own: the count is the Users section's own query (one cache entry), and the role
// gate is the route's (a person who is not an Admin never gets here; the permission state is shown instead, src/auth/session.tsx).

const itemClass = (current: boolean): string =>
  cn(
    "flex min-h-7 w-full items-center gap-2 rounded-sm px-2 py-1 text-left text-sm text-foreground hover:bg-accent focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring",
    current && "bg-selection font-medium",
  );

function Waiting({ count }: { count: number }) {
  return (
    <span
      aria-label={`${count} ${count === 1 ? "request" : "requests"} waiting`}
      className="inline-flex h-[18px] min-w-5 items-center justify-center rounded-sm border border-warning-border bg-warning-bg px-[5px] text-xs font-medium text-warning-fg tabular-nums"
    >
      {count}
    </span>
  );
}

function NavLink({ item, current, waiting }: { item: NavItem; current: boolean; waiting: number }) {
  const body = (
    <>
      <item.icon aria-hidden="true" className={cn("size-3.5 shrink-0", current ? "text-foreground" : "text-muted-foreground")} />
      <span className="min-w-0 flex-1">{item.label}</span>
      {item.id === "users" && waiting > 0 ? <Waiting count={waiting} /> : null}
    </>
  );
  const common = { className: itemClass(current), ...(current ? { "aria-current": "page" as const } : {}) };
  if (item.id === "users") return <AppLink route="admin-users" {...common}>{body}</AppLink>;
  if (item.id === "swimlanes") return <AppLink route="admin-swimlanes" {...common}>{body}</AppLink>;
  return (
    <AppLink route="admin-section" params={{ section: item.id }} {...common}>
      {body}
    </AppLink>
  );
}

export function AdminFrame({ area, children }: { area: AdminArea; children: ReactNode }) {
  const { pages, state } = usePendingRequests();
  const waiting = state.state === "ready" || state.state === "empty" ? pages.items.length : 0;
  const copy = areaCopy(area);
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-3 px-6 pt-4 pb-4">
      <header className="flex flex-wrap items-start justify-between gap-x-4 gap-y-2">
        <div>
          <h1 className="text-xl font-semibold tracking-tight">Admin</h1>
          <p className="mt-1 text-sm text-muted-foreground">Users, swimlanes, mapping rules and retention. Only Admins can open this area.</p>
        </div>
        <Badge variant="approval">
          <ShieldCheck aria-hidden="true" />
          Admin only
        </Badge>
      </header>
      <div className="grid min-h-0 flex-1 grid-cols-[240px_minmax(0,1fr)] overflow-hidden rounded-md border border-border bg-card">
        <nav aria-label="Admin sections" className="flex flex-col gap-0.5 overflow-y-auto border-r border-border bg-muted p-2">
          {ADMIN_NAV.map((group) => (
            <div key={group.label} role="group" aria-label={group.label} className="flex flex-col gap-0.5">
              <div aria-hidden="true" className="px-2 pt-3.5 pb-1 text-xs leading-4 font-semibold tracking-[0.04em] text-muted-foreground uppercase first:pt-1">
                {group.label}
              </div>
              {group.items.map((item) => (
                <NavLink key={item.id} item={item} current={item.id === area} waiting={waiting} />
              ))}
            </div>
          ))}
          <p className="mt-auto px-2 pt-3 pb-1 text-xs text-muted-foreground">Every change on these pages is written to the audit log.</p>
        </nav>
        <div role="region" aria-label={`Admin section: ${copy.title}`} {...FOCUSABLE_SCROLLER} className="min-w-0 overflow-y-auto">
          <div className="flex min-h-full flex-col gap-4 px-6 pt-5 pb-8">
            <div>
              <h2 className="text-lg font-semibold tracking-tight">
                {copy.title}
              </h2>
              <p className="mt-0.5 max-w-[680px] text-sm text-muted-foreground">{copy.lead}</p>
            </div>
            {children}
          </div>
        </div>
      </div>
    </div>
  );
}

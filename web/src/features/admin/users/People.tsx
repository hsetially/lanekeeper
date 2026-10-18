import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import type { Role, User, UserUpdate } from "@/api/generated/model";
import { EmptyState } from "@/components/app/EmptyState";
import { GatedButton } from "@/components/app/GatedButton";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { SafeText } from "@/components/app/SafeText";
import { StatusBadge } from "@/components/app/StatusBadge";
import type { Column } from "@/components/app/virtual/DataTable";
import { visibleText } from "@/components/app/safe-text";
import { useWriteGate } from "@/components/app/write-gate";
import { WriteError, type WriteErrorInfo } from "@/components/app/WriteError";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { NativeSelect } from "@/components/ui/native-select";
import { SearchInput } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { roleLabel } from "@/auth/roles";
import { useSession } from "@/auth/session";
import { AdminTable } from "@/features/admin/AdminTable";
import { adminMutations } from "@/features/admin/mutations";
import { adminQueries } from "@/features/admin/queries";
import { activeAdmins, ROLE_CHOICES, wouldLeaveNoAdmin } from "@/features/admin/roles";
import { adminWriteError } from "@/features/admin/write-error";
import { UserChangeDialog, type UserChange } from "@/features/admin/users/UserChangeDialog";
import { Info, Lock, ShieldCheck, User as PersonIcon } from "@/icons";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { cursorPagesSource, useQueryState } from "@/lib/query/use-query-state";
import { formatRelative, formatTime } from "@/lib/time";
import { useNow } from "@/live/use-now";
import { useNavigateTo } from "@/routes/AppLink";
import { linkTo } from "@/routes/links";

// The people who have a role (or had one): who they are, their role, whether their changes need an Admin's approval, whether they can
// sign in, when they were last seen and which GitHub account they are linked to. An Admin changes the first four.
//
// Nothing is optimistic. A new role, disabling, and no longer requiring approval name their consequence in a dialog first
// (UserChangeDialog); enabling and requiring approval are made at once, and say so here if the hub refuses. A select or a switch shows
// what the hub holds until the hub has answered and the list has been read again.
//
// Three safeguards are the hub's (S4): your own role cannot be changed, the last active admin cannot be removed, and the hub says so
// with a 409. The page says the same before it sends, for the case it can know (every person is loaded and no filter is on).
//
// The GitHub login is shown, not edited: the contract's UserUpdate has no such field (design/DEVIATIONS.md).

const SEARCH_DEBOUNCE_MS = 300;
/** The people asked for at a time; the footer says the same number. */
const PAGE_SIZE = 100;

const plural = (n: number, one: string, many: string): string => (n === 1 ? one : many);

function StatusCell({ user, you, busy, onDisable, onEnable }: { user: User; you: boolean; busy: boolean; onDisable: () => void; onEnable: (opener: Element) => void }) {
  const name = visibleText(user.display_name);
  if (user.status === "pending") return <StatusBadge variant="approval" label="Pending" />;
  if (user.status === "disabled") {
    return (
      <span className="flex items-center gap-2">
        <StatusBadge variant="locked" label="Disabled" />
        <GatedButton minRole="admin" size="sm" variant="ghost" aria-label={`Enable ${name}`} loading={busy} onClick={(event) => onEnable(event.currentTarget)}>
          Enable
        </GatedButton>
      </span>
    );
  }
  return (
    <span className="flex items-center gap-2">
      <StatusBadge variant="sync" label="Active" />
      <GatedButton
        size="sm"
        variant="ghost"
        aria-label={`Disable ${name}`}
        {...(you ? { gate: { allowed: false, reason: "You can't disable your own account" } } : { minRole: "admin" })}
        onClick={() => onDisable()}
      >
        Disable
      </GatedButton>
    </span>
  );
}

function RoleCell({ user, you, disabled, reason, onChange }: { user: User; you: boolean; disabled: boolean; reason: string | null; onChange: (to: Role) => void }) {
  const name = visibleText(user.display_name);
  if (you) {
    return (
      <Tooltip>
        <TooltipTrigger asChild>
          <button type="button" aria-disabled="true" className="inline-flex h-6 cursor-not-allowed items-center gap-1.5 rounded-sm border border-dashed border-input bg-transparent px-2 text-muted-foreground">
            <Lock aria-hidden="true" className="size-3" />
            {user.role === null ? "No role" : roleLabel(user.role)}
            <span className="sr-only"> You can't change your own role</span>
          </button>
        </TooltipTrigger>
        <TooltipContent>You can't change your own role</TooltipContent>
      </Tooltip>
    );
  }
  return (
    <NativeSelect
      aria-label={`Role for ${name}`}
      value={user.role ?? ""}
      disabled={disabled}
      {...(reason === null ? {} : { title: reason })}
      onChange={(event) => {
        const to = ROLE_CHOICES.find((choice) => choice === event.target.value);
        if (to !== undefined && to !== user.role) onChange(to);
      }}
    >
      {user.role === null ? (
        <option value="" disabled>
          No role yet
        </option>
      ) : null}
      {ROLE_CHOICES.map((choice) => (
        <option key={choice} value={choice}>
          {roleLabel(choice)}
        </option>
      ))}
    </NativeSelect>
  );
}

export function People({ q, role }: { q: string | undefined; role: string | undefined }) {
  const client = useQueryClient();
  const session = useSession();
  const meId = session.status === "ready" ? session.me.id : null;
  const gate = useWriteGate("admin");
  const navigate = useNavigateTo();
  // A write that applies at once (no dialog) disables its control while it runs, and a disabled control loses focus. When the hub
  // has answered, the control gets it back. (A dialog gives focus back to what opened it by itself: src/components/ui/return-focus.ts.)
  const applying = useRef<Element | null>(null);
  const refocus = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(refocus.current), []);
  const giveFocusBack = (): void => {
    const element = applying.current;
    applying.current = null;
    clearTimeout(refocus.current);
    refocus.current = setTimeout(() => {
      if (element instanceof HTMLElement && element.isConnected) element.focus();
    }, 0);
  };
  const now = useNow(true, 60_000);

  // The address holds the search (and the role); the box holds what is being typed. The address is changed a moment after the typing
  // stops, and the people are asked for from the address, so a link to a search and the back button both work.
  const [text, setText] = useState(q ?? "");
  const lastPushed = useRef(q ?? "");
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(
    () => () => {
      clearTimeout(timer.current);
    },
    [],
  );
  useEffect(() => {
    // The address changed by itself (back, forward, a link): the box follows it, unless it is the address this box just made.
    if ((q ?? "") !== lastPushed.current) {
      lastPushed.current = q ?? "";
      setText(q ?? "");
    }
  }, [q]);
  const roleFilter = ROLE_CHOICES.find((choice) => choice === role) ?? null;
  const goto = (nextQ: string, nextRole: Role | null): void => {
    lastPushed.current = nextQ;
    navigate(linkTo("admin-users", undefined, { ...(nextQ === "" ? {} : { q: nextQ }), ...(nextRole === null ? {} : { role: nextRole }) }));
  };

  const term = (q ?? "").trim();
  const pages = useCursorPages({ ...adminQueries.users({ limit: PAGE_SIZE, ...(term === "" ? {} : { q: term }) }) });
  const state = useQueryState(cursorPagesSource(pages), { isEmpty: (items) => items.length === 0 });
  const everyone = pages.items;
  // The hub is the judge of the last admin; the page can say so first only when it has seen every person and none was filtered out.
  const complete = !pages.hasMore && !pages.truncated && term === "";

  const update = useMutation(adminMutations.updateUser(client));
  const [change, setChange] = useState<UserChange | null>(null);
  const [guard, setGuard] = useState(false);
  const [refused, setRefused] = useState<{ who: string; info: WriteErrorInfo } | null>(null);
  const busyId = update.isPending ? update.variables.id : null;

  const applyNow = (user: User, body: UserUpdate, opener?: Element): void => {
    applying.current = opener ?? null;
    setGuard(false);
    setRefused(null);
    update.mutate(
      { id: user.id, body, self: false },
      {
        onSuccess: giveFocusBack,
        onError: (error) => {
          setRefused({ who: visibleText(user.display_name), info: adminWriteError(error, "The change could not be made") });
          giveFocusBack();
        },
      },
    );
  };
  const ask = (next: UserChange): void => {
    setGuard(false);
    setRefused(null);
    setChange(next);
  };

  const columns: Column<User>[] = [
    {
      id: "user",
      header: "User",
      width: "minmax(0,1.5fr)",
      minWidth: 200,
      cell: (user) => (
        <span className="flex min-w-0 items-baseline gap-2">
          <b className={user.status === "disabled" ? "truncate font-medium text-muted-foreground" : "truncate font-medium"}>
            <SafeText text={user.display_name} />
          </b>
          {user.id === meId ? <Badge>You</Badge> : null}
          <span className="truncate text-xs text-muted-foreground">
            <SafeText text={user.email} />
          </span>
        </span>
      ),
    },
    {
      id: "role",
      header: "Role",
      width: "120px",
      minWidth: 120,
      cell: (user) => (
        <RoleCell
          user={user}
          you={user.id === meId}
          disabled={!gate.allowed || busyId === user.id}
          reason={gate.reason}
          onChange={(to) => {
            if (complete && wouldLeaveNoAdmin(everyone, user.id, { role: to })) {
              setGuard(true);
              return;
            }
            ask({ kind: "role", user, to });
          }}
        />
      ),
    },
    {
      id: "approval",
      header: "Requires approval",
      width: "120px",
      minWidth: 120,
      cell: (user) =>
        user.role === null || user.role === "viewer" ? (
          <span className="text-muted-foreground">Not needed</span>
        ) : (
          <Switch
            aria-label={`Requires approval for ${visibleText(user.display_name)}`}
            checked={user.requires_approval}
            disabled={!gate.allowed || busyId === user.id}
            onCheckedChange={(checked) => {
              if (checked) {
                applyNow(user, { requires_approval: true }, document.activeElement ?? undefined);
              } else {
                setGuard(false);
                setRefused(null);
                setChange({ kind: "approval-off", user });
              }
            }}
          />
        ),
    },
    {
      id: "status",
      header: "Status",
      width: "216px",
      minWidth: 216,
      cell: (user) => (
        <StatusCell
          user={user}
          you={user.id === meId}
          busy={busyId === user.id}
          onDisable={() => {
            if (complete && wouldLeaveNoAdmin(everyone, user.id, { status: "disabled" })) {
              setGuard(true);
              return;
            }
            ask({ kind: "disable", user });
          }}
          onEnable={(opener) => {
            applyNow(user, { status: "active" }, opener);
          }}
        />
      ),
    },
    {
      id: "seen",
      header: "Last seen",
      width: "104px",
      minWidth: 104,
      cell: (user) => (user.last_seen_at === null ? <span className="text-muted-foreground">Never</span> : <span title={formatTime(user.last_seen_at)}>{formatRelative(user.last_seen_at, now)}</span>),
    },
    {
      id: "github",
      header: "GitHub login",
      width: "minmax(0,1fr)",
      minWidth: 110,
      cell: (user) => (user.github_login === null ? <Badge variant="muted">Not linked</Badge> : <SafeText text={`@${user.github_login}`} className="font-mono text-xs" />),
    },
  ];

  const filtering = term !== "" || roleFilter !== null;
  return (
    <section aria-label="Users" className="flex flex-col gap-2">
      <div className="flex min-h-6 flex-wrap items-center gap-2">
        <h3 className="text-sm font-semibold">Users</h3>
        <span className="text-xs text-muted-foreground">
          {state.state === "ready" ? `${pages.items.length}${pages.hasMore ? "+" : ""} ${plural(pages.items.length, "user", "users")}` : ""}
        </span>
        <div className="ml-auto flex flex-wrap items-center gap-2">
          <div className="w-64">
            <SearchInput
              aria-label="Search users"
              placeholder="Search by name or email"
              value={text}
              onChange={(event) => {
                const value = event.target.value;
                setText(value);
                clearTimeout(timer.current);
                timer.current = setTimeout(() => {
                  goto(value.trim(), roleFilter);
                }, SEARCH_DEBOUNCE_MS);
              }}
            />
          </div>
          <div className="w-32">
            <NativeSelect
              aria-label="Role"
              value={roleFilter ?? ""}
              onChange={(event) => {
                const next = ROLE_CHOICES.find((choice) => choice === event.target.value) ?? null;
                clearTimeout(timer.current);
                goto(text.trim(), next);
              }}
            >
              <option value="">All roles</option>
              {ROLE_CHOICES.map((choice) => (
                <option key={choice} value={choice}>
                  {roleLabel(choice)}
                </option>
              ))}
            </NativeSelect>
          </div>
        </div>
      </div>

      {guard ? (
        <Alert variant="danger">
          <AlertTitle>At least one active admin is required</AlertTitle>
          <AlertDescription>That change would leave no active Admin, so it wasn't made. Promote someone else first.</AlertDescription>
        </Alert>
      ) : null}
      {refused ? (
        <div className="flex flex-col gap-1">
          <WriteError {...refused.info} title={`${refused.who}: ${refused.info.title}`} />
          <div>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => {
                setRefused(null);
              }}
            >
              Dismiss
            </Button>
          </div>
        </div>
      ) : null}

      <QueryBoundaryView
        state={state}
        noun="users"
        source="the user list"
        empty={
          <div className="rounded-md border border-border bg-card">
            <EmptyState icon={PersonIcon} title={filtering ? "No one matches" : "No users yet"}>
              {filtering ? "Change or clear the search to see more people." : "People appear here once an Admin has approved their first sign-in."}
            </EmptyState>
          </div>
        }
      >
        {(items) => {
          const rows = roleFilter === null ? items : items.filter((user) => user.role === roleFilter);
          return rows.length === 0 ? (
            <div className="rounded-md border border-border bg-card">
              <EmptyState icon={PersonIcon} title="No one with that role">
                {pages.hasMore ? "Among the people loaded so far. Load more to look further." : "Choose another role to see more people."}
              </EmptyState>
            </div>
          ) : (
            <AdminTable
              label="Users"
              rows={rows}
              columns={columns}
              rowKey={(user) => user.id}
              footer={{ hasMore: pages.hasMore, onLoadMore: () => void pages.loadMore(), loading: pages.isFetching, truncated: pages.truncated, step: PAGE_SIZE }}
            />
          );
        }}
      </QueryBoundaryView>

      <div className="flex flex-wrap gap-x-5 gap-y-1.5 text-xs text-muted-foreground">
        <span className="inline-flex items-center gap-1.5">
          <Lock aria-hidden="true" className="size-3" />
          You can't change your own role
        </span>
        <span className="inline-flex items-center gap-1.5">
          <ShieldCheck aria-hidden="true" className="size-3" />
          At least one active admin is required ·{" "}
          <b className="font-semibold text-foreground">
            {activeAdmins(everyone)}
            {complete ? "" : "+"} active {activeAdmins(everyone) === 1 && complete ? "admin" : "admins"}
          </b>
        </span>
        <span className="inline-flex items-center gap-1.5">
          <Info aria-hidden="true" className="size-3" />
          Editors with “Requires approval” on need an Admin to approve each change
        </span>
      </div>

      <UserChangeDialog
        change={change}
        onClose={() => {
          setChange(null);
        }}
      />
    </section>
  );
}

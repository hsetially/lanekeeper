import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import type { OsLoginUser, User } from "@/api/generated/model";
import { EmptyState } from "@/components/app/EmptyState";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { SafeText } from "@/components/app/SafeText";
import { visibleText } from "@/components/app/safe-text";
import type { Column } from "@/components/app/virtual/DataTable";
import { AdminTable } from "@/features/admin/AdminTable";
import { adminMutations } from "@/features/admin/mutations";
import { adminQueries } from "@/features/admin/queries";
import { DeleteRuleDialog, RuleActions, RulesHeader, SavedNotice, type DeleteTarget } from "@/features/admin/rules";
import { Server } from "@/icons";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { cursorPagesSource, useQueryState } from "@/lib/query/use-query-state";
import { OsLoginDialog } from "./OsLoginDialog";
import { indexUsers, pathSegment, userStatusNote } from "./os-login-model";

// OS Login users: the admin's own mappings from the login name an NFS VM records for an edit to the Lanekeeper user it stands for.
// The contract has the mappings and nothing else, so the design's table of every account seen, its live preview, its Auto and Manual
// column and its "last session" are not drawn (design/DEVIATIONS.md). A mapping is saved or deleted after a dialog that names what
// that does; nothing is shown as done before the hub answers. Names and people are text the hub was given: SafeText.

const plural = (n: number, one: string, many: string): string => (n === 1 ? one : many);

function Person({ user, id, loading }: { user: User | undefined; id: string; loading: boolean }) {
  if (user === undefined) {
    return loading ? (
      <span className="text-muted-foreground">Loading…</span>
    ) : (
      <span className="flex min-w-0 items-baseline gap-2">
        <span className="text-warning-fg">Unknown user</span>
        <SafeText text={id} className="truncate font-mono text-xs text-muted-foreground" />
      </span>
    );
  }
  const note = userStatusNote(user.status);
  return (
    <span className="flex min-w-0 items-baseline gap-2">
      <b className="truncate font-medium">
        <SafeText text={user.display_name} />
      </b>
      <SafeText text={user.email} className="truncate text-xs text-muted-foreground" />
      {note === null ? null : <span className="shrink-0 rounded-sm border border-warning-border bg-warning-bg px-1 text-xs text-warning-fg">{note}</span>}
    </span>
  );
}

export function OsLoginSection() {
  const client = useQueryClient();
  const pages = useCursorPages({ ...adminQueries.osLoginUsers(), autoLoad: true });
  const state = useQueryState(cursorPagesSource(pages), { isEmpty: (items) => items.length === 0 });
  const users = useCursorPages({ ...adminQueries.users(), autoLoad: true });
  const remove = useMutation(adminMutations.deleteOsLoginUser(client));
  const [editing, setEditing] = useState<{ mapping: OsLoginUser | null } | null>(null);
  const [deleting, setDeleting] = useState<DeleteTarget | null>(null);
  const [saved, setSaved] = useState<string | null>(null);

  const byId = useMemo(() => indexUsers(users.items), [users.items]);
  const names = useMemo(() => new Set(pages.items.map((m) => m.os_login)), [pages.items]);

  const columns: Column<OsLoginUser>[] = [
    { id: "login", header: "OS Login username", width: "minmax(0,1.2fr)", minWidth: 200, cell: (row) => <SafeText text={row.os_login} className="font-mono text-xs font-medium" /> },
    { id: "user", header: "Lanekeeper user", width: "minmax(0,1.6fr)", minWidth: 240, cell: (row) => <Person user={byId.get(row.user)} id={row.user} loading={users.status === "pending"} /> },
    {
      id: "actions",
      header: <span className="sr-only">Actions</span>,
      width: "72px",
      minWidth: 72,
      align: "right",
      cell: (row) => (
        <RuleActions
          what={`mapping ${visibleText(row.os_login)}`}
          onEdit={() => {
            setSaved(null);
            setEditing({ mapping: row });
          }}
          onDelete={() => {
            setSaved(null);
            // The address of a mapping is its login name; the generated client writes it as it is, so it is encoded here.
            setDeleting({ id: pathSegment(row.os_login), what: `mapping ${visibleText(row.os_login)}` });
          }}
        />
      ),
    },
  ];

  const count = state.state === "ready" ? `${pages.items.length} ${plural(pages.items.length, "mapping", "mappings")}` : "";
  return (
    <div className="flex flex-col gap-3">
      <RulesHeader
        title="Mappings"
        count={count}
        addLabel="Add mapping"
        onAdd={() => {
          setSaved(null);
          setEditing({ mapping: null });
        }}
      />
      {saved === null ? null : (
        <SavedNotice
          title={`Mapping ${visibleText(saved)} saved`}
          onDismiss={() => {
            setSaved(null);
          }}
        >
          It applies to edits recorded from now on.
        </SavedNotice>
      )}
      <QueryBoundaryView
        state={state}
        noun="OS Login mappings"
        source="the OS Login mappings"
        empty={
          <div className="rounded-md border border-border bg-card">
            <EmptyState icon={Server} title="No OS Login mappings">
              Every login name is matched to a user by email where it can be. Add a mapping for a name that cannot be.
            </EmptyState>
          </div>
        }
      >
        {(rows) => <AdminTable label="OS Login mappings" rows={rows} columns={columns} rowKey={(row) => row.os_login} />}
      </QueryBoundaryView>
      <section aria-label="How a login name is matched" className="flex max-w-[680px] flex-col gap-1 text-xs text-muted-foreground">
        <p className="font-medium text-foreground">How a login name is matched to a user</p>
        <ol className="m-0 flex list-decimal flex-col gap-1 pl-4">
          <li>A row below is looked up first, by the exact name. The user's state is not asked: a disabled user still made the edits recorded under their name, so they stay attributed to them.</li>
          <li>Otherwise the name is matched to the one active user whose email has the same form: in lower case, with every character that is not a letter or a digit written as _, so dana.okoye@corp.example becomes dana_okoye_corp_example. When two users have the same form, nothing is guessed.</li>
          <li>Otherwise the name is shown on its own, as the OS Login user.</li>
        </ol>
      </section>
      {editing === null ? null : (
        <OsLoginDialog
          key={editing.mapping?.os_login ?? "new"}
          mapping={editing.mapping}
          existing={names}
          onSaved={setSaved}
          onClose={() => {
            setEditing(null);
          }}
        />
      )}
      <DeleteRuleDialog
        target={deleting}
        title={deleting === null ? "Delete mapping" : `Delete ${deleting.what}`}
        consequence="Edits recorded under this login name are matched by email from now on, or shown by the name alone."
        action="Delete mapping"
        mutation={remove}
        onClose={() => {
          setDeleting(null);
        }}
      />
    </div>
  );
}

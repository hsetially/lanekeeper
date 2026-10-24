import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { ConsequenceDialog } from "@/components/app/ConsequenceDialog";
import { ErrorState } from "@/components/app/ErrorState";
import { GatedButton } from "@/components/app/GatedButton";
import { SafeText } from "@/components/app/SafeText";
import { Button } from "@/components/ui/button";
import { Field, FieldControl, FieldDescription, FieldError, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { adminMutations } from "@/features/admin/mutations";
import { adminQueries } from "@/features/admin/queries";
import { checkTenant } from "@/features/admin/validate";
import { adminWriteError } from "@/features/admin/write-error";
import { GitBranch, Pencil, X } from "@/icons";
import { isRetryableProblem } from "@/lib/query/use-query-state";

// The tenant set of one swimlane (D84: a swimlane deploys a SET of tenants, each the name of a tenant Git branch). Shown as chips,
// changed in a dialog that says what that does. The whole set is sent (PUT replaces it), at least one tenant is required (the
// contract's minItems), and nothing is shown as changed until the hub has answered and the set has been read again.

const MAX_TENANTS = 100;

function TenantsDialog({ swimlane, current, onClose }: { swimlane: string; current: readonly string[]; onClose: () => void }) {
  const client = useQueryClient();
  const save = useMutation(adminMutations.putTenants(client));
  const [tenants, setTenants] = useState<string[]>([...current]);
  const [draft, setDraft] = useState("");
  const [problem, setProblem] = useState<string | null>(null);

  const add = (): void => {
    const value = draft.trim();
    const bad = checkTenant(value);
    if (bad !== null) {
      setProblem(bad);
      return;
    }
    if (tenants.includes(value)) {
      setProblem(`${value} is already in the list`);
      return;
    }
    if (tenants.length >= MAX_TENANTS) {
      setProblem(`At most ${MAX_TENANTS} tenants`);
      return;
    }
    setTenants([...tenants, value]);
    setDraft("");
    setProblem(null);
  };

  return (
    <ConsequenceDialog
      initialFocus="first-field"
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
      title={`Change the tenants of ${swimlane}`}
      consequence="Changes which tenant branches this swimlane reads. Its drift, its comparisons and its effective config are worked out again for the new set."
      actionLabel="Save tenants"
      workingLabel="Saving…"
      working={save.isPending}
      error={save.error === null ? null : adminWriteError(save.error, "The tenants could not be saved")}
      onConfirm={() => {
        if (tenants.length === 0) {
          setProblem("A swimlane needs at least one tenant");
          return;
        }
        save.mutate({ swimlane, body: { tenants } }, { onSuccess: onClose });
      }}
    >
      <Field invalid={problem !== null}>
        <FieldLabel>Add a tenant</FieldLabel>
        <div className="flex items-start gap-2">
          <div className="flex-1">
            <FieldControl>
              <Input
                value={draft}
                autoComplete="off"
                spellCheck={false}
                placeholder="sit8"
                className="font-mono"
                onChange={(event) => {
                  setDraft(event.target.value);
                  setProblem(null);
                }}
                onKeyDown={(event) => {
                  // Enter adds the tenant; it does not send the dialog.
                  if (event.key === "Enter") {
                    event.preventDefault();
                    add();
                  }
                }}
              />
            </FieldControl>
          </div>
          <Button variant="secondary" onClick={add}>
            Add tenant
          </Button>
        </div>
        <FieldError>{problem}</FieldError>
        <FieldDescription>The name of the tenant's Git branch, such as sit7.</FieldDescription>
      </Field>
      <ul aria-label="Tenants" className="m-0 flex list-none flex-wrap gap-1.5 p-0">
        {tenants.map((tenant) => (
          <li key={tenant} className="inline-flex h-6 items-center gap-1 rounded-sm border border-border bg-muted pr-0.5 pl-2 font-mono text-xs">
            <SafeText text={tenant} />
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label={`Remove ${tenant}`}
              className="size-5"
              onClick={() => {
                setTenants(tenants.filter((t) => t !== tenant));
                setProblem(null);
              }}
            >
              <X />
            </Button>
          </li>
        ))}
      </ul>
    </ConsequenceDialog>
  );
}

export function TenantsEditor({ swimlane }: { swimlane: string }) {
  const query = useQuery(adminQueries.tenants(swimlane));
  const [editing, setEditing] = useState(false);
  const tenants = query.data?.tenants;

  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center gap-2">
        <h4 className="text-sm font-semibold">Tenants</h4>
        {tenants === undefined ? null : (
          <GatedButton
            minRole="admin"
            size="sm"
            variant="secondary"
            aria-label={`Edit tenants of ${swimlane}`}
            onClick={() => {
              setEditing(true);
            }}
          >
            <Pencil data-icon="inline-start" />
            Edit
          </GatedButton>
        )}
      </div>
      {query.isPending ? (
        <div role="status" aria-busy="true" aria-label="Loading tenants">
          <Skeleton className="h-6 w-48" />
        </div>
      ) : query.isError ? (
        <ErrorState title="Couldn't load the tenants" problem={query.error} {...(isRetryableProblem(query.error) ? { onRetry: () => void query.refetch() } : {})} />
      ) : (
        <ul aria-label={`Tenants of ${swimlane}`} className="m-0 flex list-none flex-wrap gap-1.5 p-0">
          {query.data.tenants.map((tenant) => (
            <li key={tenant} className="inline-flex h-6 items-center gap-1 rounded-sm border border-border bg-muted px-2 font-mono text-xs">
              <GitBranch aria-hidden="true" className="size-3 text-muted-foreground" />
              <SafeText text={tenant} />
            </li>
          ))}
        </ul>
      )}
      {editing && tenants !== undefined ? (
        <TenantsDialog
          swimlane={swimlane}
          current={tenants}
          onClose={() => {
            setEditing(false);
          }}
        />
      ) : null}
    </div>
  );
}

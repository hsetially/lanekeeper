import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import type { FolderMapping } from "@/api/generated/model";
import { ConsequenceDialog } from "@/components/app/ConsequenceDialog";
import { EmptyState } from "@/components/app/EmptyState";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { SafeText } from "@/components/app/SafeText";
import { visibleText } from "@/components/app/safe-text";
import type { Column } from "@/components/app/virtual/DataTable";
import { Field, FieldControl, FieldDescription, FieldError, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { AdminTable } from "@/features/admin/AdminTable";
import { adminMutations } from "@/features/admin/mutations";
import { adminQueries } from "@/features/admin/queries";
import { DeleteRuleDialog, RuleActions, RulesHeader, SavedNotice, type DeleteTarget } from "@/features/admin/rules";
import { checkConfigKey, checkFolder, parseDeployments } from "@/features/admin/validate";
import { adminWriteError } from "@/features/admin/write-error";
import { Folder } from "@/icons";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { cursorPagesSource, useQueryState } from "@/lib/query/use-query-state";

// Folder mappings: which Deployments read an NFS folder, so that a pending restart, or the notify after a change in it, targets the
// right services (D105: a file is read by the Deployments linked to the deepest mapped folder above it). The design draws
// suggestions the tool made from folder names, each with a similarity score to Confirm or Correct, and a bulk confirm. The
// contract has only the mappings, so those are not drawn (design/DEVIATIONS.md).

const plural = (n: number, one: string, many: string): string => (n === 1 ? one : many);
const SHOWN = 2;

type Errors = { id?: string; folder?: string; deployments?: string };

function FolderDialog({ mapping, onClose, onSaved }: { mapping: FolderMapping | null; onClose: () => void; onSaved: (id: string) => void }) {
  const client = useQueryClient();
  const save = useMutation(adminMutations.putFolderMapping(client));
  const editing = mapping !== null;
  const [id, setId] = useState(mapping?.id ?? "");
  const [folder, setFolder] = useState(mapping?.folder ?? "");
  const [text, setText] = useState(mapping === null ? "" : mapping.deployments.map((d) => `${d.namespace}/${d.name}`).join("\n"));
  const [errors, setErrors] = useState<Errors>({});

  const submit = (): void => {
    const found: Errors = {};
    if (!editing) {
      const bad = checkConfigKey(id.trim(), "Enter a mapping id");
      if (bad !== null) found.id = bad;
    }
    const badFolder = checkFolder(folder.trim());
    if (badFolder !== null) found.folder = badFolder;
    const parsed = parseDeployments(text);
    if (!parsed.ok) found.deployments = parsed.error;
    setErrors(found);
    if (Object.keys(found).length > 0 || !parsed.ok) return;
    const mappingId = editing ? mapping.id : id.trim();
    save.mutate(
      { id: mappingId, body: { folder: folder.trim(), deployments: parsed.value } },
      {
        onSuccess: () => {
          onSaved(mappingId);
          onClose();
        },
      },
    );
  };

  return (
    <ConsequenceDialog
      initialFocus="first-field"
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
      title={editing ? `Edit mapping ${visibleText(mapping.id)}` : "Add a folder mapping"}
      consequence="Files in this folder, and in folders below it that have no mapping of their own, are read by these Deployments: a pending notify or restart for a change there targets them."
      actionLabel="Save mapping"
      workingLabel="Saving…"
      working={save.isPending}
      error={save.error === null ? null : adminWriteError(save.error, "The mapping could not be saved")}
      onConfirm={submit}
    >
      <Field invalid={errors.id !== undefined}>
        <FieldLabel>Mapping id</FieldLabel>
        <FieldControl>
          <Input value={id} readOnly={editing} autoComplete="off" spellCheck={false} placeholder="video" className="font-mono" onChange={(event) => setId(event.target.value)} />
        </FieldControl>
        <FieldError>{errors.id}</FieldError>
        {editing ? <FieldDescription>The id cannot change. Delete the mapping and add another to rename it.</FieldDescription> : null}
      </Field>
      <Field invalid={errors.folder !== undefined}>
        <FieldLabel>NFS folder</FieldLabel>
        <FieldControl>
          <Input value={folder} autoComplete="off" spellCheck={false} placeholder="video" className="font-mono" onChange={(event) => setFolder(event.target.value)} />
        </FieldControl>
        <FieldError>{errors.folder}</FieldError>
        <FieldDescription>Relative to the swimlane's NFS root, with no leading or trailing /.</FieldDescription>
      </Field>
      <Field invalid={errors.deployments !== undefined}>
        <FieldLabel>Deployments</FieldLabel>
        <FieldControl>
          <Textarea value={text} rows={4} spellCheck={false} placeholder="sitb/video-service" className="font-mono" onChange={(event) => setText(event.target.value)} />
        </FieldControl>
        <FieldError>{errors.deployments}</FieldError>
        <FieldDescription>One per line, as namespace/name.</FieldDescription>
      </Field>
    </ConsequenceDialog>
  );
}

export function FoldersSection() {
  const client = useQueryClient();
  const pages = useCursorPages({ ...adminQueries.folderMappings(), autoLoad: true });
  const state = useQueryState(cursorPagesSource(pages), { isEmpty: (items) => items.length === 0 });
  const remove = useMutation(adminMutations.deleteFolderMapping(client));
  const [editing, setEditing] = useState<{ mapping: FolderMapping | null } | null>(null);
  const [deleting, setDeleting] = useState<DeleteTarget | null>(null);
  const [saved, setSaved] = useState<string | null>(null);

  const columns: Column<FolderMapping>[] = [
    { id: "folder", header: "NFS folder", width: "minmax(0,1fr)", minWidth: 150, cell: (mapping) => <SafeText text={mapping.folder} className="font-mono text-xs font-medium" /> },
    {
      id: "deployments",
      header: "Deployments",
      width: "minmax(0,2fr)",
      minWidth: 240,
      cell: (mapping) => {
        const all = mapping.deployments.map((d) => `${d.namespace}/${d.name}`);
        return (
          <span title={visibleText(all.join(", "))} className="flex min-w-0 items-center gap-1.5">
            {all.slice(0, SHOWN).map((name) => (
              <span key={name} className="block h-5 max-w-full truncate rounded-sm border border-border bg-muted px-1.5 font-mono text-xs leading-[18px]">
                <SafeText text={name} />
              </span>
            ))}
            {all.length > SHOWN ? <span className="inline-flex h-5 items-center rounded-sm border border-border px-1 text-xs text-muted-foreground">+{all.length - SHOWN}</span> : null}
          </span>
        );
      },
    },
    { id: "id", header: "Mapping", width: "minmax(0,0.7fr)", minWidth: 100, cell: (mapping) => <SafeText text={mapping.id} className="font-mono text-xs text-muted-foreground" /> },
    {
      id: "actions",
      header: <span className="sr-only">Actions</span>,
      width: "72px",
      minWidth: 72,
      align: "right",
      cell: (mapping) => (
        <RuleActions
          what={`mapping ${visibleText(mapping.id)}`}
          onEdit={() => {
            setSaved(null);
            setEditing({ mapping });
          }}
          onDelete={() => {
            setSaved(null);
            setDeleting({ id: mapping.id, what: `mapping ${visibleText(mapping.id)}` });
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
          Pending restarts are worked out again with it.
        </SavedNotice>
      )}
      <QueryBoundaryView
        state={state}
        noun="folder mappings"
        source="the folder mappings"
        empty={
          <div className="rounded-md border border-border bg-card">
            <EmptyState icon={Folder} title="No folder mappings">
              No NFS folder is linked to a Deployment yet. A change in an unlinked folder is not tied to any service, so a restart cannot target one.
            </EmptyState>
          </div>
        }
      >
        {(mappings) => <AdminTable label="Folder mappings" rows={mappings} columns={columns} rowKey={(mapping) => mapping.id} />}
      </QueryBoundaryView>
      {editing === null ? null : (
        <FolderDialog
          key={editing.mapping?.id ?? "new"}
          mapping={editing.mapping}
          onSaved={setSaved}
          onClose={() => {
            setEditing(null);
          }}
        />
      )}
      <DeleteRuleDialog
        target={deleting}
        title={deleting === null ? "Delete mapping" : `Delete ${deleting.what}`}
        consequence="Files in this folder are no longer linked to these Deployments, so a restart does not target them."
        action="Delete mapping"
        mutation={remove}
        onClose={() => {
          setDeleting(null);
        }}
      />
    </div>
  );
}

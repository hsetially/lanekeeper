import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useId, useState } from "react";
import { MergeMode, type MergeModeRule } from "@/api/generated/model";
import { ConsequenceDialog } from "@/components/app/ConsequenceDialog";
import { EmptyState } from "@/components/app/EmptyState";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { SafeText } from "@/components/app/SafeText";
import { visibleText } from "@/components/app/safe-text";
import type { Column } from "@/components/app/virtual/DataTable";
import { Badge } from "@/components/ui/badge";
import { Field, FieldControl, FieldDescription, FieldError, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { AdminTable } from "@/features/admin/AdminTable";
import { adminMutations } from "@/features/admin/mutations";
import { adminQueries } from "@/features/admin/queries";
import { DeleteRuleDialog, RuleActions, RulesHeader, SavedNotice, type DeleteTarget } from "@/features/admin/rules";
import { checkConfigKey, checkGlob } from "@/features/admin/validate";
import { adminWriteError } from "@/features/admin/write-error";
import { Split } from "@/icons";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { cursorPagesSource, useQueryState } from "@/lib/query/use-query-state";

// Merge modes: whether a tenant file replaces its base file (whole_file, the default, D14) or is merged into it (spring_merge),
// chosen by a glob over the file's logical path. The rule that is listed first and matches wins. The order is the hub's: the
// contract's rule has no position, so the page cannot move a rule up or down (design/DEVIATIONS.md). The design's preview (the
// effective keys of a sample file under the rules) is the engine's work and has no operation, so it is not drawn.

const plural = (n: number, one: string, many: string): string => (n === 1 ? one : many);

const MODES: readonly { mode: MergeMode; help: string }[] = [
  { mode: MergeMode.whole_file, help: "The tenant file replaces its base file." },
  { mode: MergeMode.spring_merge, help: "The keys of the tenant file are merged over the base file's keys." },
];

function MergeRuleDialog({ rule, onClose, onSaved }: { rule: MergeModeRule | null; onClose: () => void; onSaved: (id: string) => void }) {
  const client = useQueryClient();
  const save = useMutation(adminMutations.putMergeMode(client));
  const uid = useId();
  const editing = rule !== null;
  const [id, setId] = useState(rule?.id ?? "");
  const [glob, setGlob] = useState(rule?.file_pattern ?? "");
  const [mode, setMode] = useState<MergeMode>(rule?.mode ?? MergeMode.whole_file);
  const [errors, setErrors] = useState<{ id?: string; glob?: string }>({});

  const submit = (): void => {
    const found: { id?: string; glob?: string } = {};
    if (!editing) {
      const bad = checkConfigKey(id.trim(), "Enter a rule id");
      if (bad !== null) found.id = bad;
    }
    const badGlob = checkGlob(glob);
    if (badGlob !== null) found.glob = badGlob;
    setErrors(found);
    if (Object.keys(found).length > 0) return;
    const ruleId = editing ? rule.id : id.trim();
    save.mutate(
      { id: ruleId, body: { file_pattern: glob, mode } },
      {
        onSuccess: () => {
          onSaved(ruleId);
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
      title={editing ? `Edit rule ${visibleText(rule.id)}` : "Add a merge rule"}
      consequence={
        editing
          ? "Changes how the files that match are compared and shown as effective config. It takes effect on the next sync."
          : "Files that match this pattern and no earlier rule get this mode. It changes how they are compared and shown as effective config."
      }
      actionLabel="Save rule"
      workingLabel="Saving…"
      working={save.isPending}
      error={save.error === null ? null : adminWriteError(save.error, "The rule could not be saved")}
      onConfirm={submit}
    >
      <Field invalid={errors.id !== undefined}>
        <FieldLabel>Rule id</FieldLabel>
        <FieldControl>
          <Input value={id} readOnly={editing} autoComplete="off" spellCheck={false} placeholder="security" className="font-mono" onChange={(event) => setId(event.target.value)} />
        </FieldControl>
        <FieldError>{errors.id}</FieldError>
        {editing ? <FieldDescription>The id cannot change. Delete the rule and add another to rename it.</FieldDescription> : null}
      </Field>
      <Field invalid={errors.glob !== undefined}>
        <FieldLabel>Path glob</FieldLabel>
        <FieldControl>
          <Input value={glob} autoComplete="off" spellCheck={false} placeholder="security/**" className="font-mono" onChange={(event) => setGlob(event.target.value)} />
        </FieldControl>
        <FieldError>{errors.glob}</FieldError>
        <FieldDescription>Matched against the file's logical path. * stays inside a folder, ** crosses folders.</FieldDescription>
      </Field>
      <fieldset className="m-0 flex min-w-0 flex-col gap-2 border-0 p-0">
        <legend className="mb-1 text-sm font-medium">Mode</legend>
        <RadioGroup
          value={mode}
          aria-label="Mode"
          onValueChange={(next) => {
            setMode(next === MergeMode.spring_merge ? MergeMode.spring_merge : MergeMode.whole_file);
          }}
        >
          {MODES.map((choice) => (
            <div key={choice.mode} className="flex items-start gap-2">
              <RadioGroupItem id={`${uid}-${choice.mode}`} value={choice.mode} aria-describedby={`${uid}-${choice.mode}-help`} className="mt-0.5" />
              <div className="flex flex-col">
                <Label htmlFor={`${uid}-${choice.mode}`} className="font-mono">
                  {choice.mode}
                </Label>
                <span id={`${uid}-${choice.mode}-help`} className="text-xs text-muted-foreground">
                  {choice.help}
                </span>
              </div>
            </div>
          ))}
        </RadioGroup>
      </fieldset>
    </ConsequenceDialog>
  );
}

export function MergeSection() {
  const client = useQueryClient();
  const pages = useCursorPages({ ...adminQueries.mergeModes(), autoLoad: true });
  const state = useQueryState(cursorPagesSource(pages), { isEmpty: (items) => items.length === 0 });
  const remove = useMutation(adminMutations.deleteMergeMode(client));
  const [editing, setEditing] = useState<{ rule: MergeModeRule | null } | null>(null);
  const [deleting, setDeleting] = useState<DeleteTarget | null>(null);
  const [saved, setSaved] = useState<string | null>(null);

  const columns: Column<MergeModeRule>[] = [
    { id: "n", header: "#", width: "32px", minWidth: 32, cell: (_rule, index) => <span className="text-muted-foreground tabular-nums">{index + 1}</span> },
    { id: "glob", header: "Path glob", width: "minmax(0,1.5fr)", minWidth: 180, cell: (rule) => <SafeText text={rule.file_pattern} className="font-mono text-xs" /> },
    {
      id: "mode",
      header: "Mode",
      width: "140px",
      minWidth: 140,
      cell: (rule) => <Badge className="font-mono">{rule.mode}</Badge>,
    },
    { id: "id", header: "Rule", width: "minmax(0,0.8fr)", minWidth: 110, cell: (rule) => <SafeText text={rule.id} className="font-mono text-xs text-muted-foreground" /> },
    {
      id: "actions",
      header: <span className="sr-only">Actions</span>,
      width: "72px",
      minWidth: 72,
      align: "right",
      cell: (rule) => (
        <RuleActions
          what={`rule ${visibleText(rule.id)}`}
          onEdit={() => {
            setSaved(null);
            setEditing({ rule });
          }}
          onDelete={() => {
            setSaved(null);
            setDeleting({ id: rule.id, what: `rule ${visibleText(rule.id)}` });
          }}
        />
      ),
    },
  ];

  const count = state.state === "ready" ? `${pages.items.length} ${plural(pages.items.length, "rule", "rules")}` : "";
  return (
    <div className="flex flex-col gap-3">
      <RulesHeader
        title="Rules"
        count={count}
        addLabel="Add rule"
        onAdd={() => {
          setSaved(null);
          setEditing({ rule: null });
        }}
      />
      {saved === null ? null : (
        <SavedNotice
          title={`Rule ${visibleText(saved)} saved`}
          onDismiss={() => {
            setSaved(null);
          }}
        >
          It takes effect on the next sync.
        </SavedNotice>
      )}
      <QueryBoundaryView
        state={state}
        noun="merge modes"
        source="the merge modes"
        empty={
          <div className="rounded-md border border-border bg-card">
            <EmptyState icon={Split} title="No merge rules">
              Every tenant file replaces its base file (whole_file). Add a rule to merge keys instead.
            </EmptyState>
          </div>
        }
      >
        {(rules) => <AdminTable label="Merge modes" rows={rules} columns={columns} rowKey={(rule) => rule.id} />}
      </QueryBoundaryView>
      <p className="text-xs text-muted-foreground">First match wins. Files that match no rule use whole_file.</p>
      {editing === null ? null : (
        <MergeRuleDialog
          key={editing.rule?.id ?? "new"}
          rule={editing.rule}
          onSaved={setSaved}
          onClose={() => {
            setEditing(null);
          }}
        />
      )}
      <DeleteRuleDialog
        target={deleting}
        title={deleting === null ? "Delete rule" : `Delete ${deleting.what}`}
        consequence="Files that matched this rule use the next rule that matches, or whole_file."
        mutation={remove}
        onClose={() => {
          setDeleting(null);
        }}
      />
    </div>
  );
}

import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useId, useState } from "react";
import type { HostTier } from "@/api/generated/model";
import { ConsequenceDialog } from "@/components/app/ConsequenceDialog";
import { EmptyState } from "@/components/app/EmptyState";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { SafeText } from "@/components/app/SafeText";
import { visibleText } from "@/components/app/safe-text";
import type { Column } from "@/components/app/virtual/DataTable";
import { Badge } from "@/components/ui/badge";
import { Field, FieldControl, FieldDescription, FieldError, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { AdminTable } from "@/features/admin/AdminTable";
import { adminMutations } from "@/features/admin/mutations";
import { adminQueries } from "@/features/admin/queries";
import { DeleteRuleDialog, RuleActions, RulesHeader, SavedNotice, type DeleteTarget } from "@/features/admin/rules";
import { checkConfigKey, checkHostPattern } from "@/features/admin/validate";
import { adminWriteError } from "@/features/admin/write-error";
import { Tag } from "@/icons";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { cursorPagesSource, useQueryState } from "@/lib/query/use-query-state";

// Host tiers: a host-name pattern puts the hosts it matches in a tier (dev, uat, prod, ...), and the C6 check says when a host of
// another tier is in a swimlane's config. The contract's rule is ONE pattern and one tier; the design's field of comma-separated
// patterns for a tier is not what the hub stores. The design's test field (which tier a name falls in) and its table of the hosts
// found in config are the engine's work and have no operation, so they are not drawn (design/DEVIATIONS.md).

const plural = (n: number, one: string, many: string): string => (n === 1 ? one : many);
const SUGGESTED = ["dev", "uat", "prod"] as const;

type Errors = { id?: string; pattern?: string; tier?: string };

function TierRuleDialog({ rule, onClose, onSaved }: { rule: HostTier | null; onClose: () => void; onSaved: (id: string) => void }) {
  const client = useQueryClient();
  const save = useMutation(adminMutations.putHostTier(client));
  const listId = useId();
  const editing = rule !== null;
  const [id, setId] = useState(rule?.id ?? "");
  const [pattern, setPattern] = useState(rule?.host_pattern ?? "");
  const [tier, setTier] = useState(rule?.tier ?? "");
  const [errors, setErrors] = useState<Errors>({});

  const submit = (): void => {
    const found: Errors = {};
    if (!editing) {
      const bad = checkConfigKey(id.trim(), "Enter a rule id");
      if (bad !== null) found.id = bad;
    }
    const badPattern = checkHostPattern(pattern.trim());
    if (badPattern !== null) found.pattern = badPattern;
    const badTier = checkConfigKey(tier.trim(), "Enter a tier");
    if (badTier !== null) found.tier = badTier;
    setErrors(found);
    if (Object.keys(found).length > 0) return;
    const ruleId = editing ? rule.id : id.trim();
    save.mutate(
      { id: ruleId, body: { host_pattern: pattern.trim(), tier: tier.trim() } },
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
      title={editing ? `Edit pattern ${visibleText(rule.id)}` : "Add a host pattern"}
      consequence="Hosts that match are put in this tier by the next scan. A host should match the patterns of one tier only: narrow the pattern if it also matches hosts of another."
      actionLabel="Save pattern"
      workingLabel="Saving…"
      working={save.isPending}
      error={save.error === null ? null : adminWriteError(save.error, "The pattern could not be saved")}
      onConfirm={submit}
    >
      <Field invalid={errors.id !== undefined}>
        <FieldLabel>Rule id</FieldLabel>
        <FieldControl>
          <Input value={id} readOnly={editing} autoComplete="off" spellCheck={false} placeholder="dev" className="font-mono" onChange={(event) => setId(event.target.value)} />
        </FieldControl>
        <FieldError>{errors.id}</FieldError>
        {editing ? <FieldDescription>The id cannot change. Delete the pattern and add another to rename it.</FieldDescription> : null}
      </Field>
      <Field invalid={errors.pattern !== undefined}>
        <FieldLabel>Host pattern</FieldLabel>
        <FieldControl>
          <Input value={pattern} autoComplete="off" spellCheck={false} placeholder="*-dev.example.net" className="font-mono" onChange={(event) => setPattern(event.target.value)} />
        </FieldControl>
        <FieldError>{errors.pattern}</FieldError>
        <FieldDescription>A host name where * matches any characters.</FieldDescription>
      </Field>
      <Field invalid={errors.tier !== undefined}>
        <FieldLabel>Tier</FieldLabel>
        <FieldControl>
          <Input value={tier} list={listId} autoComplete="off" spellCheck={false} placeholder="dev" className="font-mono" onChange={(event) => setTier(event.target.value)} />
        </FieldControl>
        <datalist id={listId}>
          {SUGGESTED.map((name) => (
            <option key={name} value={name} />
          ))}
        </datalist>
        <FieldError>{errors.tier}</FieldError>
        <FieldDescription>Such as dev, uat or prod.</FieldDescription>
      </Field>
    </ConsequenceDialog>
  );
}

export function TiersSection() {
  const client = useQueryClient();
  const pages = useCursorPages({ ...adminQueries.hostTiers(), autoLoad: true });
  const state = useQueryState(cursorPagesSource(pages), { isEmpty: (items) => items.length === 0 });
  const remove = useMutation(adminMutations.deleteHostTier(client));
  const [editing, setEditing] = useState<{ rule: HostTier | null } | null>(null);
  const [deleting, setDeleting] = useState<DeleteTarget | null>(null);
  const [saved, setSaved] = useState<string | null>(null);

  const columns: Column<HostTier>[] = [
    { id: "pattern", header: "Host pattern", width: "minmax(0,1.6fr)", minWidth: 200, cell: (rule) => <SafeText text={rule.host_pattern} className="font-mono text-xs" /> },
    { id: "tier", header: "Tier", width: "112px", minWidth: 112, cell: (rule) => <Badge className="font-mono"><SafeText text={rule.tier} /></Badge> },
    { id: "id", header: "Rule", width: "minmax(0,0.8fr)", minWidth: 110, cell: (rule) => <SafeText text={rule.id} className="font-mono text-xs text-muted-foreground" /> },
    {
      id: "actions",
      header: <span className="sr-only">Actions</span>,
      width: "72px",
      minWidth: 72,
      align: "right",
      cell: (rule) => (
        <RuleActions
          what={`pattern ${visibleText(rule.id)}`}
          onEdit={() => {
            setSaved(null);
            setEditing({ rule });
          }}
          onDelete={() => {
            setSaved(null);
            setDeleting({ id: rule.id, what: `pattern ${visibleText(rule.id)}` });
          }}
        />
      ),
    },
  ];

  const count = state.state === "ready" ? `${pages.items.length} ${plural(pages.items.length, "pattern", "patterns")}` : "";
  return (
    <div className="flex flex-col gap-3">
      <RulesHeader
        title="Patterns"
        count={count}
        addLabel="Add pattern"
        onAdd={() => {
          setSaved(null);
          setEditing({ rule: null });
        }}
      />
      {saved === null ? null : (
        <SavedNotice
          title={`Pattern ${visibleText(saved)} saved`}
          onDismiss={() => {
            setSaved(null);
          }}
        >
          Hosts are put in their tiers again by the next scan.
        </SavedNotice>
      )}
      <QueryBoundaryView
        state={state}
        noun="host tiers"
        source="the host tiers"
        empty={
          <div className="rounded-md border border-warning-border bg-warning-bg">
            <EmptyState icon={Tag} title="No host patterns">
              Every hostname is unclassified, so findings can't flag a prod host in a SIT file.
            </EmptyState>
          </div>
        }
      >
        {(rules) => <AdminTable label="Host tiers" rows={rules} columns={columns} rowKey={(rule) => rule.id} />}
      </QueryBoundaryView>
      <p className="text-xs text-muted-foreground">A pattern is a host name where * matches any characters, such as *-dev.example.net.</p>
      {editing === null ? null : (
        <TierRuleDialog
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
        title={deleting === null ? "Delete pattern" : `Delete ${deleting.what}`}
        consequence="Hosts this pattern matched are unclassified unless another pattern matches them."
        action="Delete pattern"
        mutation={remove}
        onClose={() => {
          setDeleting(null);
        }}
      />
    </div>
  );
}

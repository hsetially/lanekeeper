import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import type { PathMappingRule } from "@/api/generated/model";
import { EmptyState } from "@/components/app/EmptyState";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { SafeText } from "@/components/app/SafeText";
import { visibleText } from "@/components/app/safe-text";
import type { Column } from "@/components/app/virtual/DataTable";
import { AdminTable } from "@/features/admin/AdminTable";
import { adminMutations } from "@/features/admin/mutations";
import { adminQueries } from "@/features/admin/queries";
import { DeleteRuleDialog, RuleActions, RulesHeader, SavedNotice, type DeleteTarget } from "@/features/admin/rules";
import { ArrowLeftRight } from "@/icons";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { cursorPagesSource, useQueryState } from "@/lib/query/use-query-state";
import { PathRuleDialog } from "./PathRuleDialog";

// Path mappings: where the base files and the tenant files live in their Git repositories, and for which swimlanes. A rule is saved
// or deleted after a dialog that names what that does. The design's live preview (a sample path mapped both ways, and back) is not
// here: mapping a path is the engine's work (rule 7) and no operation gives its answer to the browser (design/DEVIATIONS.md).

const plural = (n: number, one: string, many: string): string => (n === 1 ? one : many);

export function PathsSection() {
  const client = useQueryClient();
  const pages = useCursorPages({ ...adminQueries.pathMappings(), autoLoad: true });
  const state = useQueryState(cursorPagesSource(pages), { isEmpty: (items) => items.length === 0 });
  const remove = useMutation(adminMutations.deletePathMapping(client));
  const [editing, setEditing] = useState<{ rule: PathMappingRule | null } | null>(null);
  const [deleting, setDeleting] = useState<DeleteTarget | null>(null);
  const [saved, setSaved] = useState<string | null>(null);

  const columns: Column<PathMappingRule>[] = [
    { id: "n", header: "#", width: "32px", minWidth: 32, cell: (_rule, index) => <span className="text-muted-foreground tabular-nums">{index + 1}</span> },
    { id: "id", header: "Rule", width: "minmax(0,0.8fr)", minWidth: 120, cell: (rule) => <SafeText text={rule.id} className="font-mono text-xs font-medium" /> },
    { id: "base", header: "Base repo path", width: "minmax(0,1fr)", minWidth: 140, cell: (rule) => <SafeText text={rule.base_root} className="font-mono text-xs" /> },
    { id: "tenant", header: "Tenant repo path", width: "minmax(0,1fr)", minWidth: 140, cell: (rule) => <SafeText text={rule.tenant_root} className="font-mono text-xs" /> },
    {
      id: "applies",
      header: "Applies to",
      width: "minmax(0,1fr)",
      minWidth: 140,
      cell: (rule) => (rule.swimlanes.length === 0 ? <span className="text-muted-foreground">Every swimlane</span> : <SafeText text={rule.swimlanes.join(", ")} className="font-mono text-xs" />),
    },
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
        noun="path mappings"
        source="the path mappings"
        empty={
          <div className="rounded-md border border-border bg-card">
            <EmptyState icon={ArrowLeftRight} title="No path mappings">
              Add a rule to say where the base files and the tenant files live in their repositories.
            </EmptyState>
          </div>
        }
      >
        {(rules) => <AdminTable label="Path mappings" rows={rules} columns={columns} rowKey={(rule) => rule.id} />}
      </QueryBoundaryView>
      <p className="text-xs text-muted-foreground">
        A tenant file sits beside its base file: the tenant's name is added before the extension, so tx-infinity-core.yml for tenant sit1 is tx-infinity-core-sit1.yml. A rule with no swimlanes applies to every swimlane.
      </p>
      {editing === null ? null : (
        <PathRuleDialog
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
        consequence="Files this rule covers are no longer matched by it. They use the next rule that applies, if there is one."
        mutation={remove}
        onClose={() => {
          setDeleting(null);
        }}
      />
    </div>
  );
}

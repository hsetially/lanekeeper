import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Fragment, useMemo, useState } from "react";
import type { SeverityRule } from "@/api/generated/model";
import { EmptyState } from "@/components/app/EmptyState";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { SafeText } from "@/components/app/SafeText";
import { visibleText } from "@/components/app/safe-text";
import type { Column } from "@/components/app/virtual/DataTable";
import { SeverityBadge } from "@/components/attribution";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { AdminTable } from "@/features/admin/AdminTable";
import { adminMutations } from "@/features/admin/mutations";
import { adminQueries } from "@/features/admin/queries";
import { DeleteRuleDialog, RuleActions, RulesHeader, SavedNotice, type DeleteTarget } from "@/features/admin/rules";
import { ShieldAlert } from "@/icons";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { cursorPagesSource, useQueryState } from "@/lib/query/use-query-state";
import { SeverityRuleDialog } from "./SeverityRuleDialog";
import { matchesEverything, matchParts, notRatedYet } from "./severity-model";

// Severity rules: the admin's own rules, added to the engine's built-in ones. The highest severity of the rules that match wins
// (D103), so a rule can raise what the built-in rules say and can never lower it or switch a built-in rule off; the page says so and
// has no control that would, and none to put the rules in order, because the order cannot matter. The built-in rules are not in the
// list the hub returns. The design's preview against recent changes is not here: rating a change is the engine's work (rule 7).
// Names, ids and globs are text an admin typed, so each goes through SafeText.

const plural = (n: number, one: string, many: string): string => (n === 1 ? one : many);

function Conditions({ rule }: { rule: SeverityRule }) {
  if (matchesEverything(rule.match)) return <span className="text-warning-fg">Every change and finding</span>;
  const parts = matchParts(rule.match);
  const whole = parts.map((p) => `${p.label} ${p.value}`).join(" · ");
  return (
    <span title={visibleText(whole)} className="block min-w-0 truncate">
      {parts.map((part, index) => (
        <Fragment key={part.label}>
          {index === 0 ? "" : " · "}
          <span className="text-muted-foreground">{part.label}</span> {part.glob ? <SafeText text={part.value} className="font-mono text-xs" /> : <SafeText text={part.value} className="text-xs" />}
        </Fragment>
      ))}
    </span>
  );
}

export function SeverityRulesSection() {
  const client = useQueryClient();
  const pages = useCursorPages({ ...adminQueries.severityRules(), autoLoad: true });
  const state = useQueryState(cursorPagesSource(pages), { isEmpty: (items) => items.length === 0 });
  const remove = useMutation(adminMutations.deleteSeverityRule(client));
  const [editing, setEditing] = useState<{ rule: SeverityRule | null } | null>(null);
  const [deleting, setDeleting] = useState<DeleteTarget | null>(null);
  const [saved, setSaved] = useState<string | null>(null);
  const ids = useMemo(() => new Set(pages.items.map((r) => r.id)), [pages.items]);

  const columns: Column<SeverityRule>[] = [
    {
      id: "rule",
      header: "Rule",
      width: "minmax(0,1.3fr)",
      minWidth: 200,
      cell: (rule) => (
        <span className="flex min-w-0 items-baseline gap-2">
          <b className="truncate font-medium">
            <SafeText text={rule.name} />
          </b>
          <SafeText text={rule.id} className="truncate font-mono text-xs text-muted-foreground" />
        </span>
      ),
    },
    { id: "applies", header: "Applies to", width: "minmax(0,1.5fr)", minWidth: 200, cell: (rule) => <Conditions rule={rule} /> },
    { id: "severity", header: "Severity", width: "104px", minWidth: 104, cell: (rule) => <SeverityBadge severity={rule.severity} /> },
    {
      id: "state",
      header: "State",
      width: "minmax(0,1.1fr)",
      minWidth: 190,
      cell: (rule) => (
        <span className="flex min-w-0 items-center gap-2">
          <span className={rule.enabled ? undefined : "text-muted-foreground"}>{rule.enabled ? "On" : "Off"}</span>
          {notRatedYet(rule.match) ? (
            <span
              title="The hub does not look at the individual setting changes of a file when it rates it, so a change type or a setting glob has nothing to match today."
              className="min-w-0 truncate rounded-sm border border-status-unknown-border bg-status-unknown-bg px-1 text-xs text-status-unknown-fg"
            >
              Not applied yet
            </span>
          ) : null}
        </span>
      ),
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
      <Alert variant="info" icon={<ShieldAlert aria-hidden="true" />}>
        <AlertTitle>Rules can raise a severity, never lower it</AlertTitle>
        <AlertDescription>
          The built-in rules always apply and are not listed here. When several rules match a change or a finding, the highest severity wins, so a rule you add can raise what the built-in rules say and can never lower it or switch a built-in rule off. The order of the rules does not matter. A change that no rule matches is Low.
        </AlertDescription>
      </Alert>
      <RulesHeader
        title="Your rules"
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
          Pending changes and open findings are rated again with it.
        </SavedNotice>
      )}
      <QueryBoundaryView
        state={state}
        noun="severity rules"
        source="the severity rules"
        empty={
          <div className="rounded-md border border-border bg-card">
            <EmptyState icon={ShieldAlert} title="No severity rules of your own">
              Only the built-in rules apply. Add a rule to raise the severity of a path or of a type of finding.
            </EmptyState>
          </div>
        }
      >
        {(rules) => <AdminTable label="Severity rules" rows={rules} columns={columns} rowKey={(rule) => rule.id} />}
      </QueryBoundaryView>
      {editing === null ? null : (
        <SeverityRuleDialog
          key={editing.rule?.id ?? "new"}
          rule={editing.rule}
          existing={ids}
          onSaved={setSaved}
          onClose={() => {
            setEditing(null);
          }}
        />
      )}
      <DeleteRuleDialog
        target={deleting}
        title={deleting === null ? "Delete rule" : `Delete ${deleting.what}`}
        consequence="Whatever this rule raised goes back to what the other rules say for pending changes and open findings. The built-in rules are not affected."
        mutation={remove}
        onClose={() => {
          setDeleting(null);
        }}
      />
    </div>
  );
}

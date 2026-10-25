import { useMemo, useState } from "react";
import type { SwimlaneSummary } from "@/api/generated/model";
import { AgentDot } from "@/components/app/AgentDot";
import { SafeText } from "@/components/app/SafeText";
import { TierChip } from "@/components/app/TierBadge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { agentView } from "@/features/swimlanes/agent";

// "Choose swimlanes": the swimlanes that are the columns of the grid, from the list the person may see. Each swimlane is one
// column, in the order of the list, and past `limit` the values stop being readable, so the dialog stops there and says so.

const MAX_SHOWN = 300;

type Props = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  all: readonly SwimlaneSummary[];
  selected: readonly string[];
  limit: number;
  onApply: (ids: string[]) => void;
};

export function LanePicker({ open, onOpenChange, all, selected, limit, onApply }: Props) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent size="lg">{open ? <Body all={all} selected={selected} limit={limit} onApply={onApply} onClose={() => onOpenChange(false)} /> : null}</DialogContent>
    </Dialog>
  );
}

function Body({ all, selected, limit, onApply, onClose }: Omit<Props, "open" | "onOpenChange"> & { onClose: () => void }) {
  const [draft, setDraft] = useState<ReadonlySet<string>>(() => new Set(selected));
  const [text, setText] = useState("");
  const needle = text.trim().toLowerCase();
  const shown = useMemo(
    () => all.filter((lane) => needle === "" || [lane.id, lane.display_name, lane.tier ?? "", ...lane.tenants].some((field) => field.toLowerCase().includes(needle))),
    [all, needle],
  );
  const full = draft.size >= limit;
  const toggle = (id: string) => {
    setDraft((current) => {
      const next = new Set(current);
      if (!next.delete(id) && next.size < limit) next.add(id);
      return next;
    });
  };
  // The columns follow the order of the list, not the order of the clicks; a swimlane the list does not have keeps its place at the end.
  const apply = () => {
    const inList = all.filter((lane) => draft.has(lane.id)).map((lane) => lane.id);
    const known = new Set(inList);
    onApply([...inList, ...[...draft].filter((id) => !known.has(id))]);
    onClose();
  };
  return (
    <>
      <DialogHeader>
        <DialogTitle>Choose swimlanes</DialogTitle>
        <DialogDescription>Each swimlane is one column of the grid. Up to {limit} at a time.</DialogDescription>
      </DialogHeader>
      <Input
        aria-label="Filter swimlanes"
        placeholder="Filter by name, tier or tenant"
        value={text}
        onChange={(event) => {
          setText(event.target.value);
        }}
      />
      <ul aria-label="Swimlanes" className="flex max-h-72 flex-col overflow-auto rounded-sm border border-border">
        {shown.slice(0, MAX_SHOWN).map((lane) => {
          const checked = draft.has(lane.id);
          const agent = agentView(lane.agent);
          return (
            <li key={lane.id} className="border-b border-border last:border-b-0">
              <label className="flex h-8 cursor-pointer items-center gap-2 px-2 text-sm hover:bg-accent">
                <Checkbox
                  checked={checked}
                  disabled={!checked && full}
                  onCheckedChange={() => {
                    toggle(lane.id);
                  }}
                  aria-label={lane.id}
                />
                {lane.tier == null ? null : <TierChip tier={lane.tier} />}
                <SafeText text={lane.id} className="font-mono font-medium" />
                <AgentDot state={agent.state} />
                <span className="ml-auto truncate font-mono text-xs text-muted-foreground">
                  <SafeText text={lane.tenants.slice(0, 2).join(", ")} />
                  {lane.tenants.length > 2 ? ` +${lane.tenants.length - 2}` : ""}
                </span>
              </label>
            </li>
          );
        })}
        {shown.length === 0 ? <li className="p-3 text-sm text-muted-foreground">No swimlane matches.</li> : null}
        {shown.length > MAX_SHOWN ? <li className="p-3 text-sm text-muted-foreground">Showing the first {MAX_SHOWN} of {shown.length}. Filter to see the rest.</li> : null}
      </ul>
      <DialogFooter className="justify-between">
        <span className="text-xs text-muted-foreground tabular-nums" role="status">
          {draft.size} / {limit} selected{full ? ". That is the most the grid shows." : ""}
        </span>
        <span className="flex gap-2">
          <Button variant="secondary" onClick={onClose}>
            Cancel
          </Button>
          <Button onClick={apply} disabled={draft.size === 0}>
            Compare
          </Button>
        </span>
      </DialogFooter>
    </>
  );
}

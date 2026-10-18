import { useMemo } from "react";
import type { SwimlaneSummary, User } from "@/api/generated/model";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { NativeSelect } from "@/components/ui/native-select";
import { X } from "@/icons";
import { actionView, KNOWN_ACTIONS, type Filters } from "./model";

// The bar under the quick filters: what the hub can filter the audit log on. A user, an action, a swimlane and a range of days
// (UTC). Each is a real control with a name, a value in the address, and one request when it changes. The design's Via, Attribution
// source and Minimum confidence filters have no parameter in the contract, so they are not here (the quick filters above work
// on the events that are loaded).

type Props = {
  filters: Filters;
  users: readonly User[];
  swimlanes: readonly SwimlaneSummary[];
  onChange: (next: Filters) => void;
  disabled?: boolean;
};

/** Two people with one name are told apart by their email. */
function userLabels(users: readonly User[]): Array<{ id: string; label: string }> {
  const count = new Map<string, number>();
  for (const u of users) count.set(u.display_name, (count.get(u.display_name) ?? 0) + 1);
  return users
    .map((u) => ({ id: u.id, label: (count.get(u.display_name) ?? 0) > 1 ? `${u.display_name} (${u.email})` : u.display_name }))
    .sort((a, b) => a.label.localeCompare(b.label));
}

const without = (filters: Filters, key: keyof Filters): Filters => {
  const next = { ...filters };
  Reflect.deleteProperty(next, key);
  return next;
};

export function FilterBar({ filters, users, swimlanes, onChange, disabled = false }: Props) {
  const people = useMemo(() => userLabels(users), [users]);
  const lanes = useMemo(() => swimlanes.map((l) => l.id).sort(), [swimlanes]);
  const set = (key: keyof Filters, value: string): void => {
    onChange(value === "" ? without(filters, key) : { ...filters, [key]: value });
  };
  const active = Object.keys(filters).length > 0;
  // A user or a swimlane that is in the address but not in the list is still a choice that was made: it stays in the list.
  const unknownUser = filters.actor !== undefined && !people.some((p) => p.id === filters.actor);
  const unknownLane = filters.swimlane !== undefined && !lanes.includes(filters.swimlane);
  const unknownAction = filters.action !== undefined && !KNOWN_ACTIONS.includes(filters.action);
  return (
    <div data-slot="filter-bar" className="flex min-h-10 flex-none flex-wrap items-center gap-2 border-b border-border bg-background px-2 py-1">
      <div className="w-44">
        <NativeSelect
          aria-label="User"
          disabled={disabled}
          value={filters.actor ?? ""}
          onChange={(event) => {
            set("actor", event.target.value);
          }}
        >
          <option value="">Any user</option>
          {unknownUser && filters.actor !== undefined ? <option value={filters.actor}>{filters.actor}</option> : null}
          {people.map((p) => (
            <option key={p.id} value={p.id}>
              {p.label}
            </option>
          ))}
        </NativeSelect>
      </div>
      <div className="w-52">
        <NativeSelect
          aria-label="Action"
          disabled={disabled}
          value={filters.action ?? ""}
          onChange={(event) => {
            set("action", event.target.value);
          }}
        >
          <option value="">Any action</option>
          {unknownAction && filters.action !== undefined ? <option value={filters.action}>{filters.action}</option> : null}
          {KNOWN_ACTIONS.map((a) => (
            <option key={a} value={a}>
              {`${actionView(a).label} (${a})`}
            </option>
          ))}
        </NativeSelect>
      </div>
      <div className="w-36">
        <NativeSelect
          aria-label="Swimlane"
          disabled={disabled}
          value={filters.swimlane ?? ""}
          onChange={(event) => {
            set("swimlane", event.target.value);
          }}
        >
          <option value="">Any swimlane</option>
          {unknownLane && filters.swimlane !== undefined ? <option value={filters.swimlane}>{filters.swimlane}</option> : null}
          {lanes.map((id) => (
            <option key={id} value={id}>
              {id}
            </option>
          ))}
        </NativeSelect>
      </div>
      <label className="flex items-center gap-1.5 text-xs text-muted-foreground">
        From
        <Input
          type="date"
          aria-label="From"
          disabled={disabled}
          value={filters.from ?? ""}
          max={filters.to}
          className="w-36"
          onChange={(event) => {
            set("from", event.target.value);
          }}
        />
      </label>
      <label className="flex items-center gap-1.5 text-xs text-muted-foreground">
        To
        <Input
          type="date"
          aria-label="To"
          disabled={disabled}
          value={filters.to ?? ""}
          min={filters.from}
          className="w-36"
          onChange={(event) => {
            set("to", event.target.value);
          }}
        />
      </label>
      <span className="text-xs text-muted-foreground">UTC days</span>
      {active ? (
        <Button
          variant="ghost"
          size="sm"
          onClick={() => {
            onChange({});
          }}
        >
          <X data-icon="inline-start" />
          Clear filters
        </Button>
      ) : null}
    </div>
  );
}

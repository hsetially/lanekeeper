import type { ReactNode, Ref } from "react";
import { GatedButton } from "@/components/app/GatedButton";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuCheckboxItem, DropdownMenuContent, DropdownMenuRadioGroup, DropdownMenuRadioItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { SearchInput } from "@/components/ui/input";
import { ChevronDown, Columns2, Plus, Split } from "@/icons";
import { cn } from "@/lib/utils";
import { AppLink } from "@/routes/AppLink";
import type { AgentFilter } from "./lanes";
import { COLUMN_IDS, COLUMN_LABEL, type ColumnId } from "./columns";

// The bar above the list (the design's 40 px bar): the filter field, the Tier and Agent status filters, Has drift, the column
// chooser, how many are selected and Compare selected. While the list loads, or when there is nothing to filter, it is drawn
// dimmed and does nothing, as the design draws it.

// The design's toolbar buttons: 28 px high, 8 px of padding, a 12 px icon (the shared Button's 10 px and 14 px are for its own size).
const chip = "px-2 [&_svg:not([class*='size-'])]:size-3";
const dashed = cn(chip, "border-dashed border-input bg-transparent font-normal hover:bg-accent");

type Props = {
  disabled: boolean;
  inputRef: Ref<HTMLInputElement>;
  text: string;
  onText: (text: string) => void;
  tiers: readonly string[];
  tier: string | null;
  onTier: (tier: string | null) => void;
  agent: AgentFilter | null;
  onAgent: (agent: AgentFilter | null) => void;
  hasDrift: boolean;
  onHasDrift: (on: boolean) => void;
  columns: readonly ColumnId[];
  onColumn: (id: ColumnId, on: boolean) => void;
  /** The ids of the selected swimlanes in the order of the list. */
  selected: readonly string[];
};

const AGENT_LABEL: Record<AgentFilter, string> = { online: "Online", offline: "Offline" };

/** A dashed "+ Tier" button that opens a menu of one choice; once a choice is made it reads "Tier: PRESIT" and the dash is gone. */
function FilterMenu({ label, shown, checked, children, onValue, disabled }: { label: string; shown: string | null; checked: string; children: ReactNode; onValue: (value: string) => void; disabled: boolean }) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" disabled={disabled} className={cn(dashed, shown !== null && "border-solid bg-background")}>
          {shown === null ? <Plus data-icon="inline-start" /> : null}
          {shown === null ? label : `${label}: ${shown}`}
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start">
        <DropdownMenuRadioGroup value={checked} onValueChange={onValue}>
          {children}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

const ALL = "__all";

export function Toolbar({ disabled, inputRef, text, onText, tiers, tier, onTier, agent, onAgent, hasDrift, onHasDrift, columns, onColumn, selected }: Props) {
  const canCompare = selected.length >= 2;
  return (
    <div data-slot="toolbar" className={cn("flex min-h-10 flex-none flex-wrap items-center gap-x-2 gap-y-1 border-b border-border bg-muted px-2 py-1", disabled && "opacity-50")}>
      <SearchInput
        ref={inputRef}
        aria-label="Filter swimlanes"
        placeholder="Filter swimlanes…"
        kbd="/"
        disabled={disabled}
        value={text}
        onChange={(event) => {
          onText(event.target.value);
        }}
        boxClassName="w-60 min-w-24 shrink"
      />
      <FilterMenu
        label="Tier"
        shown={tier === null ? null : tier.toUpperCase()}
        checked={tier === null ? ALL : tier.toUpperCase()}
        disabled={disabled}
        onValue={(value) => {
          onTier(value === ALL ? null : value.toLowerCase());
        }}
      >
        <DropdownMenuRadioItem value={ALL}>All tiers</DropdownMenuRadioItem>
        {tiers.map((name) => (
          <DropdownMenuRadioItem key={name} value={name}>
            {name}
          </DropdownMenuRadioItem>
        ))}
      </FilterMenu>
      <FilterMenu
        label="Agent status"
        shown={agent === null ? null : AGENT_LABEL[agent]}
        checked={agent ?? ALL}
        disabled={disabled}
        onValue={(value) => {
          onAgent(value === "online" || value === "offline" ? value : null);
        }}
      >
        <DropdownMenuRadioItem value={ALL}>Any status</DropdownMenuRadioItem>
        <DropdownMenuRadioItem value="online">Online</DropdownMenuRadioItem>
        <DropdownMenuRadioItem value="offline">Offline</DropdownMenuRadioItem>
      </FilterMenu>
      <Button
        variant="ghost"
        aria-pressed={hasDrift}
        disabled={disabled}
        onClick={() => {
          onHasDrift(!hasDrift);
        }}
        className={cn(chip, "border-input bg-background font-normal hover:bg-accent", hasDrift && "bg-selection")}
      >
        <Split data-icon="inline-start" />
        Has drift
      </Button>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="ghost" disabled={disabled} className={cn(chip, "border-input bg-background font-normal hover:bg-accent")}>
            <Columns2 data-icon="inline-start" />
            Columns
            <ChevronDown data-icon="inline-end" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-56">
          {COLUMN_IDS.map((id) => (
            <DropdownMenuCheckboxItem
              key={id}
              checked={columns.includes(id)}
              disabled={id === "swimlane"}
              onSelect={(event) => {
                event.preventDefault();
              }}
              onCheckedChange={(on) => {
                onColumn(id, on);
              }}
            >
              {COLUMN_LABEL[id]}
              {id === "swimlane" ? <span className="ml-auto text-xs text-muted-foreground">Always on</span> : null}
            </DropdownMenuCheckboxItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
      <span aria-live="polite" className="ml-auto text-xs tabular-nums text-muted-foreground">
        {selected.length > 0 ? `${selected.length} selected` : ""}
      </span>
      {canCompare ? (
        <Button asChild>
          <AppLink route="compare" search={{ lanes: selected.join(",") }}>
            <Columns2 data-icon="inline-start" />
            Compare selected
          </AppLink>
        </Button>
      ) : (
        <GatedButton variant="secondary" gate={{ allowed: false, reason: "Select two or more swimlanes to compare." }}>
          <Columns2 data-icon="inline-start" />
          Compare selected
        </GatedButton>
      )}
    </div>
  );
}

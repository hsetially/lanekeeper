import { useEffect, useState } from "react";
import type { SwimlaneSummary } from "@/api/generated/model";
import { AgentDot } from "@/components/app/AgentDot";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { SafeText } from "@/components/app/SafeText";
import { visibleText } from "@/components/app/safe-text";
import { TierChip } from "@/components/app/TierBadge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { agentView } from "@/features/swimlanes/agent";
import { ChevronsUpDown, GitBranch, Server, Tag } from "@/icons";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { cursorPagesSource, useQueryState } from "@/lib/query/use-query-state";
import { gitRefsQuery } from "../queries";
import type { Target } from "./sides";

// One side of a whole-swimlane comparison: a button that says what is chosen, and a list to choose from. A swimlane comes from
// the list the person may see (filtered here); a branch or a tag comes from the server's list of refs, which filters by prefix.

const MAX_SHOWN = 200;

type Props = {
  /** "Left side" or "Right side": the accessible name begins with it. */
  side: "Left" | "Right";
  target: Target;
  /** The swimlane id, or the branch or tag. */
  value: string | null;
  lanes: readonly SwimlaneSummary[];
  onChoose: (name: string) => void;
  disabled?: boolean;
  /** Open from outside (the empty state's button). */
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
};

const NOUN: Record<Target, string> = { lane: "swimlane", branch: "Git branch", tag: "Git tag" };
const PLURAL = { branch: "branches", tag: "tags" } as const;

export function SidePicker({ side, target, value, lanes, onChoose, disabled = false, open, onOpenChange }: Props) {
  const [own, setOwn] = useState(false);
  const isOpen = open ?? own;
  const [text, setText] = useState("");
  const lane = target === "lane" ? lanes.find((l) => l.id === value) : undefined;
  const Icon = target === "lane" ? Server : target === "branch" ? GitBranch : Tag;
  const change = (next: boolean) => {
    setOwn(next);
    onOpenChange?.(next);
    if (!next) setText("");
  };
  return (
    <Popover open={isOpen} onOpenChange={change}>
      <PopoverTrigger asChild>
        <Button variant="secondary" disabled={disabled} aria-haspopup="dialog" aria-label={`${side} side: ${value === null ? `choose a ${NOUN[target]}` : visibleText(value)}`} className="min-w-36 justify-start bg-background">
          <Icon aria-hidden="true" className="text-muted-foreground" />
          {lane?.tier == null ? null : <TierChip tier={lane.tier} />}
          <span className="truncate font-mono text-xs font-medium">{value === null ? `Choose a ${NOUN[target]}` : <SafeText text={value} />}</span>
          <ChevronsUpDown aria-hidden="true" className="ml-auto text-muted-foreground" />
        </Button>
      </PopoverTrigger>
      <PopoverContent aria-label={`${side} side: choose a ${NOUN[target]}`} className="flex w-80 flex-col gap-2 p-2">
        <Input
          aria-label={`Filter ${NOUN[target]}s`}
          placeholder={target === "lane" ? "Filter swimlanes" : `Start of the ${target} name`}
          value={text}
          maxLength={64}
          onChange={(event) => {
            setText(event.target.value);
          }}
          spellCheck={false}
        />
        {target === "lane" ? (
          <LaneList lanes={lanes} text={text} value={value} onChoose={(name) => { onChoose(name); change(false); }} />
        ) : (
          <RefList target={target} text={text} value={value} onChoose={(name) => { onChoose(name); change(false); }} />
        )}
      </PopoverContent>
    </Popover>
  );
}

function LaneList({ lanes, text, value, onChoose }: { lanes: readonly SwimlaneSummary[]; text: string; value: string | null; onChoose: (name: string) => void }) {
  const needle = text.trim().toLowerCase();
  const shown = lanes.filter((l) => needle === "" || [l.id, l.display_name, l.tier ?? "", ...l.tenants].some((f) => f.toLowerCase().includes(needle)));
  return (
    <ul aria-label="Swimlanes" className="flex max-h-64 flex-col overflow-auto">
      {shown.slice(0, MAX_SHOWN).map((l) => (
        <li key={l.id}>
          <button type="button" aria-current={l.id === value || undefined} onClick={() => onChoose(l.id)} className="flex h-7 w-full cursor-pointer items-center gap-2 rounded-sm px-2 text-left hover:bg-accent aria-[current]:bg-selection">
            {l.tier == null ? null : <TierChip tier={l.tier} />}
            <SafeText text={l.id} className="font-mono text-xs font-medium" />
            <AgentDot state={agentView(l.agent).state} />
            <span className="ml-auto truncate font-mono text-xs text-muted-foreground">
              <SafeText text={l.tenants[0] ?? ""} />
            </span>
          </button>
        </li>
      ))}
      {shown.length === 0 ? <li className="p-2 text-sm text-muted-foreground">No swimlane matches.</li> : null}
    </ul>
  );
}

function RefList({ target, text, value, onChoose }: { target: Exclude<Target, "lane">; text: string; value: string | null; onChoose: (name: string) => void }) {
  // The prefix goes to the server a moment after the person stops typing.
  const [prefix, setPrefix] = useState("");
  useEffect(() => {
    const timer = setTimeout(() => {
      setPrefix(text.trim());
    }, 250);
    return () => {
      clearTimeout(timer);
    };
  }, [text]);
  const repo = target === "branch" ? "tenant" : "base";
  const pages = useCursorPages({ ...gitRefsQuery(repo, target, prefix), maxPages: 1 });
  const state = useQueryState(cursorPagesSource(pages), { isEmpty: (refs) => refs.length === 0 });
  return (
    <QueryBoundaryView
      state={state}
      noun={PLURAL[target]}
      source={`the ${repo} repository`}
      staleBanner={false}
      empty={<p className="p-2 text-sm text-muted-foreground">{prefix === "" ? `The ${repo} repository has no ${PLURAL[target]}.` : `No ${target} starts with “${prefix}”.`}</p>}
    >
      {(refs) => (
        <ul aria-label={`${target === "branch" ? "Branches" : "Tags"} of the ${repo} repository`} className="flex max-h-64 flex-col overflow-auto">
          {refs.slice(0, MAX_SHOWN).map((ref) => (
            <li key={ref.name}>
              <button type="button" aria-current={ref.name === value || undefined} onClick={() => onChoose(ref.name)} className="flex h-7 w-full cursor-pointer items-center gap-2 rounded-sm px-2 text-left hover:bg-accent aria-[current]:bg-selection">
                <SafeText text={ref.name} className="min-w-0 flex-1 truncate font-mono text-xs font-medium" />
                <span className="font-mono text-xs text-muted-foreground">{ref.commit.slice(0, 7)}</span>
              </button>
            </li>
          ))}
          {pages.hasMore ? <li className="p-2 text-xs text-muted-foreground">More than this: type the start of the name.</li> : null}
        </ul>
      )}
    </QueryBoundaryView>
  );
}

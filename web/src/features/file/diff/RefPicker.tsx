import { Command as CommandPrimitive } from "cmdk";
import { useState, type ReactNode } from "react";
import { SafeText } from "@/components/app/SafeText";
import { Command, CommandEmpty, CommandGroup, CommandItem, CommandList } from "@/components/ui/command";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Skeleton } from "@/components/ui/skeleton";
import { gitRefsQuery } from "@/features/compare/queries";
import { swimlaneListQuery } from "@/features/swimlanes/queries";
import { ChevronDown, GitBranch, Layers, Search, ShieldCheck, Tag } from "@/icons";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { cn } from "@/lib/utils";
import type { Side } from "./refs";

// The ref picker of the Diff tab (File, 1a): which version the other side is. Sources (the file in Git, the baseline), another
// swimlane, tags and branches. Choosing one only says which; the screen puts it in the URL and the engine does the comparing.
// Names of refs and swimlanes come from the server and from Git, so they are data: SafeText.

export type RefPickerProps = {
  label: "Left" | "Right";
  /** What the side is now, as the button reads. */
  text: string;
  /** This file's swimlane: left out of "Another swimlane", and the baseline's. */
  lane: string;
  /** The file's tenant, or null for a base file. Decides which repository's branches are listed. */
  tenant: string | null;
  /** How the Git source reads: "Git (sit7 head)". */
  gitLabel: string;
  onSelect: (side: Side) => void;
  /** The side cannot be changed (the right is this swimlane's NFS): a label, not a list. */
  fixed?: boolean;
};

const buttonClasses = "inline-flex h-7 items-center gap-1.5 rounded-sm border bg-background px-2";

function Item({ value, icon, children, onChoose }: { value: string; icon: ReactNode; children: string; onChoose: () => void }) {
  return (
    <CommandItem value={value} onSelect={onChoose} className="h-7">
      <span aria-hidden="true" className="text-muted-foreground">
        {icon}
      </span>
      <SafeText text={children} className={cn("flex-1 truncate", value.startsWith("source") ? "" : "font-mono text-xs")} />
    </CommandItem>
  );
}

function PickerList({ lane, tenant, gitLabel, onChoose }: { lane: string; tenant: string | null; gitLabel: string; onChoose: (side: Side) => void }) {
  const lanes = useCursorPages({ ...swimlaneListQuery(), autoLoad: true });
  const tags = useCursorPages({ ...gitRefsQuery("base", "tag", ""), maxPages: 2 });
  const branches = useCursorPages({ ...gitRefsQuery(tenant !== null ? "tenant" : "base", "branch", ""), maxPages: 2 });
  const refsFailed = tags.status === "error" || branches.status === "error";
  const refsLoading = tags.status === "pending" || branches.status === "pending";
  return (
    <Command label="Versions to compare with" className="rounded-md" loop>
      <div className="flex h-8 items-center gap-2 border-b border-border px-2">
        <Search aria-hidden="true" className="size-3.5 shrink-0 text-muted-foreground" />
        <CommandPrimitive.Input aria-label="Filter refs" placeholder="Filter…" className="min-w-0 flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground" />
      </div>
      <CommandList className="max-h-[320px]">
        <CommandEmpty>Nothing matches.</CommandEmpty>
        <CommandGroup heading="Sources">
          <Item value="source git" icon={<GitBranch className="size-3.5" />} onChoose={() => onChoose({ kind: "git" })}>
            {gitLabel}
          </Item>
          <Item value="source baseline" icon={<ShieldCheck className="size-3.5" />} onChoose={() => onChoose({ kind: "baseline", lane })}>
            Baseline
          </Item>
        </CommandGroup>
        {lanes.items.some((s) => s.id !== lane) ? (
          <CommandGroup heading="Another swimlane">
            {lanes.items
              .filter((s) => s.id !== lane)
              .map((s) => (
                <Item key={s.id} value={`lane ${s.id}`} icon={<Layers className="size-3.5" />} onChoose={() => onChoose({ kind: "lane", id: s.id })}>
                  {s.id}
                </Item>
              ))}
          </CommandGroup>
        ) : null}
        {tags.items.length > 0 ? (
          <CommandGroup heading="Tags">
            {tags.items.map((t) => (
              <Item key={t.name} value={`tag ${t.name}`} icon={<Tag className="size-3.5" />} onChoose={() => onChoose({ kind: "tag", name: t.name })}>
                {t.name}
              </Item>
            ))}
          </CommandGroup>
        ) : null}
        {branches.items.length > 0 ? (
          <CommandGroup heading="Branches">
            {branches.items.map((b) => (
              <Item key={b.name} value={`branch ${b.name}`} icon={<GitBranch className="size-3.5" />} onChoose={() => onChoose({ kind: "branch", name: b.name })}>
                {b.name}
              </Item>
            ))}
          </CommandGroup>
        ) : null}
      </CommandList>
      {refsLoading ? <Skeleton className="m-2 h-5" /> : null}
      {refsFailed ? <p className="border-t border-border px-3 py-2 text-xs text-muted-foreground">Tags and branches could not be read.</p> : null}
    </Command>
  );
}

export function RefPicker({ label, text, lane, tenant, gitLabel, onSelect, fixed = false }: RefPickerProps) {
  const [open, setOpen] = useState(false);
  if (fixed) {
    return (
      <span className={cn(buttonClasses, "border-input")}>
        <span className="text-muted-foreground">{label}</span>
        <span className="font-medium">{text}</span>
      </span>
    );
  }
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button type="button" aria-haspopup="listbox" aria-expanded={open} className={cn(buttonClasses, open ? "border-ring" : "border-input", "cursor-pointer")}>
          <span className="text-muted-foreground">{label}</span>
          <span className="font-medium">{text}</span>
          <ChevronDown aria-hidden="true" className="size-3 text-muted-foreground" />
        </button>
      </PopoverTrigger>
      <PopoverContent aria-label="Choose a version to compare" className="w-[260px] p-0">
        <PickerList
          lane={lane}
          tenant={tenant}
          gitLabel={gitLabel}
          onChoose={(side) => {
            setOpen(false);
            onSelect(side);
          }}
        />
      </PopoverContent>
    </Popover>
  );
}

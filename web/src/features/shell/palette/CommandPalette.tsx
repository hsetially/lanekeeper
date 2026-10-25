import { Command as CommandPrimitive } from "cmdk";
import { useEffect, useMemo, useState, type KeyboardEvent } from "react";
import type { Role } from "@/api/generated/model";
import { roleAtLeast } from "@/auth/roles";
import { SafeText } from "@/components/app/SafeText";
import { CommandGroup, CommandItem, CommandList } from "@/components/ui/command";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import { Kbd } from "@/components/ui/kbd";
import { Spinner } from "@/components/ui/spinner";
import { swimlaneListQuery } from "@/features/swimlanes/queries";
import { Search, X } from "@/icons";
import { rowCaps } from "@/keyboard";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { cn } from "@/lib/utils";
import { highlight } from "./match";
import { GROUP, MAX_NUMBERED, buildPalette, type PaletteAction, type PaletteItem } from "./model";
import { fetchDocs, fetchSettings, fetchUsers, useRemoteGroup } from "./remote";

// The command palette (the Shell, frames 1c and 1d): ⌘K from anywhere, one box, results in groups. Keys: ↑ ↓ move, ↵ runs,
// ⌘↵ opens in a new tab, ⌘1 to ⌘9 run the nth result, `>` limits it to actions and Backspace on the empty box lifts that.
//
// It is built on cmdk with cmdk's own filter switched off: some groups come from the server and must not be filtered a second
// time, and ⌘1 to ⌘9 need to mean "the nth row on screen". This file is a chunk of its own (cmdk and the dialog are not
// part of the first screen); the shell loads it when idle and when ⌘K is pressed.

export type RunOptions = { newTab?: boolean };

type Props = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  role: Role | null;
  /** Do what a result says. The shell owns navigation and the chrome; the palette owns neither. */
  onRun: (action: PaletteAction, options: RunOptions) => void;
};

function Label({ item, text }: { item: PaletteItem; text: string }) {
  const { before, match, after } = highlight(text, item.label);
  return (
    <span className={cn("min-w-0 truncate", item.mono ? "font-mono text-xs" : "text-sm")}>
      <SafeText text={before} />
      {match ? <strong className="rounded-[2px] bg-selection font-semibold"><SafeText text={match} /></strong> : null}
      <SafeText text={after} />
    </span>
  );
}

function Row({ item, text, number }: { item: PaletteItem; text: string; number: number | null }) {
  const Icon = item.icon;
  const caps = item.hint ? rowCaps(item.hint.row, item.hint.binding) : number !== null ? ["⌘", String(number)] : [];
  return (
    <>
      <Icon aria-hidden="true" className="text-muted-foreground" />
      <Label item={item} text={text} />
      <span className="min-w-0 flex-1 truncate text-xs text-muted-foreground">{item.meta ? <SafeText text={item.meta} /> : null}</span>
      {item.chip ? <span className="inline-flex h-5 items-center rounded-sm border border-input px-1.5 font-mono text-xs font-medium">{item.chip}</span> : null}
      <span className="inline-flex items-center gap-0.5" aria-hidden="true">
        {caps.map((cap, i) => (cap === "then" ? <span key={i} className="px-0.5 text-xs text-muted-foreground">then</span> : <Kbd key={i}>{cap}</Kbd>))}
      </span>
    </>
  );
}

/** Open a link in a new tab the way the browser does for a link with a target (the palette has no anchor for a row to click). */
function openInNewTab(href: string): void {
  const anchor = document.createElement("a");
  anchor.href = href;
  anchor.target = "_blank";
  anchor.rel = "noopener noreferrer";
  anchor.click();
}

function Body({ role, onRun, onOpenChange }: Omit<Props, "open">) {
  const [text, setText] = useState("");
  const [actionsOnly, setActionsOnly] = useState(false);
  const [selected, setSelected] = useState("");
  const swimlanes = useCursorPages({ ...swimlaneListQuery(), autoLoad: true, enabled: role !== null });

  const query = `${actionsOnly ? ">" : ""}${text}`;
  const searching = !actionsOnly;
  const docs = useRemoteGroup(text, GROUP.docs, searching && roleAtLeast(role, GROUP.docs.minRole), fetchDocs);
  const settings = useRemoteGroup(text, GROUP.settings, searching && roleAtLeast(role, GROUP.settings.minRole), fetchSettings);
  const users = useRemoteGroup(text, GROUP.users, searching && roleAtLeast(role, GROUP.users.minRole), fetchUsers);

  const view = useMemo(() => buildPalette({ query, role, swimlanes: swimlanes.items, docs, settings, users }), [query, role, swimlanes.items, docs, settings, users]);

  // The highlighted row is the first one whenever the query changes or a different row comes to the top (the swimlane list
  // arriving after the actions were drawn); arrow keys move it from there. Without this cmdk keeps its choice on a row that is
  // now far down the list.
  const first = view.items[0]?.id ?? "";
  useEffect(() => {
    setSelected(first);
  }, [query, first]);

  const run = (item: PaletteItem, options: RunOptions = {}): void => {
    onOpenChange(false);
    if (options.newTab === true && item.action.kind === "go") {
      openInNewTab(item.action.href);
      return;
    }
    onRun(item.action, options);
  };

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>): void => {
    if ((event.metaKey || event.ctrlKey) && !event.altKey && !event.shiftKey && /^[1-9]$/.test(event.key)) {
      event.preventDefault();
      const item = view.numbered[Number(event.key) - 1];
      if (item && Number(event.key) <= MAX_NUMBERED) run(item);
      return;
    }
    if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
      event.preventDefault();
      const item = view.items.find((i) => i.id === selected);
      if (item) run(item, { newTab: true });
      return;
    }
    // Backspace in the empty box lifts the `>` that limits the results to actions.
    if (event.key === "Backspace" && text === "" && actionsOnly) {
      event.preventDefault();
      setActionsOnly(false);
    }
  };

  const onValueChange = (value: string): void => {
    if (!actionsOnly && value.startsWith(">")) {
      setActionsOnly(true);
      setText(value.slice(1).trimStart());
      return;
    }
    setText(value);
  };

  const nothing = view.items.length === 0 && view.sections.every((s) => s.status === "ready");
  const failed = view.sections.filter((section) => section.status === "error").map((section) => section.spec.title.toLowerCase());
  const running = view.sections.filter((section) => section.status === "loading").map((section) => section.spec.title.toLowerCase());
  const saying = failed.length > 0 ? `${failed.join(" and ")} search failed.`.replace(/^./, (c) => c.toUpperCase()) : running.length > 0 ? `Searching ${running.join(" and ")}…` : "";
  return (
    <CommandPrimitive label="Search or jump to" shouldFilter={false} loop value={selected} onValueChange={setSelected} onKeyDown={onKeyDown} className="flex h-full w-full flex-col overflow-hidden rounded-md bg-popover text-popover-foreground">
      <div className="flex h-11 items-center gap-2 border-b border-border px-3">
        <Search aria-hidden="true" className="size-4 shrink-0 text-muted-foreground" />
        {actionsOnly ? (
          <span data-slot="palette-chip" className="inline-flex h-5 shrink-0 items-center gap-1 rounded-sm border border-input bg-muted px-1.5 text-xs font-medium">
            Actions
            <button
              type="button"
              tabIndex={-1}
              aria-label="Show all results"
              className="inline-flex cursor-pointer items-center text-muted-foreground hover:text-foreground"
              onClick={() => {
                setActionsOnly(false);
              }}
            >
              <X aria-hidden="true" className="size-3" />
            </button>
          </span>
        ) : null}
        <CommandPrimitive.Input
          value={text}
          onValueChange={onValueChange}
          placeholder={actionsOnly ? "Run an action…" : "Search or jump to…"}
          className="h-full min-w-0 flex-1 bg-transparent text-base text-foreground outline-none placeholder:text-muted-foreground"
        />
        <Kbd>Esc</Kbd>
      </div>
      <CommandList hidden={nothing} className="max-h-[360px]">
        {view.sections.map((section) => (
          <CommandGroup key={section.spec.id} heading={section.spec.title}>
            {section.status === "loading" ? (
              <div aria-hidden="true" className="flex h-8 items-center gap-2 px-2 text-xs text-muted-foreground">
                <Spinner />
                Searching {section.spec.title.toLowerCase()}…
              </div>
            ) : section.status === "error" ? (
              <div aria-hidden="true" className="flex h-8 items-center px-2 text-xs text-danger-fg">
                Couldn&apos;t search {section.spec.title.toLowerCase()}.
              </div>
            ) : (
              section.items.map((item) => {
                const index = view.numbered.indexOf(item);
                return (
                  <CommandItem
                    key={item.id}
                    value={item.id}
                    onSelect={() => {
                      run(item);
                    }}
                  >
                    <Row item={item} text={view.text} number={index >= 0 ? index + 1 : null} />
                  </CommandItem>
                );
              })
            )}
          </CommandGroup>
        ))}
      </CommandList>
      {/* Outside the list: a listbox may hold only options and groups (axe `aria-required-children`), and with nothing to list the list is hidden. */}
      {/* The rows above are for the eye: a status may not sit inside a listbox, so a search that is running or has failed is said here, once. */}
      <div role="status" className="sr-only">
        {saying}
      </div>
      {nothing ? (
        <div role="status" className="px-3 py-6 text-center text-sm text-muted-foreground">
          {view.text === "" ? "Nothing to show yet." : `No results for “${view.text}”.`}
          <span className="mt-1 block text-xs">Start with &gt; to see only actions.</span>
        </div>
      ) : null}
      <div className="flex h-8 items-center gap-4 border-t border-border bg-muted px-3 text-xs text-muted-foreground">
        <span>↑↓ navigate</span>
        <span>↵ open</span>
        <span>⌘↵ new tab</span>
        <span>⌘1–9 jump to result</span>
        <span className="ml-auto">&gt; actions only</span>
      </div>
    </CommandPrimitive>
  );
}

export default function CommandPalette({ open, onOpenChange, role, onRun }: Props) {
  // The dialog of ui/command.tsx wraps its children in a Command of its own; here the Command is the body's, because it needs
  // the selection and the keys, so the dialog is put together from its parts.
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent size="palette" showCloseButton={false} className="gap-0 overflow-hidden p-0">
        <DialogTitle className="sr-only">Command palette</DialogTitle>
        <DialogDescription className="sr-only">Search swimlanes, settings and docs, or run an action. Type &gt; for actions only.</DialogDescription>
        <Body role={role} onRun={onRun} onOpenChange={onOpenChange} />
      </DialogContent>
    </Dialog>
  );
}

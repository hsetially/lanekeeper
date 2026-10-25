import { useState, type SyntheticEvent } from "react";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { SafeText } from "@/components/app/SafeText";
import { visibleText } from "@/components/app/safe-text";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { ChevronsUpDown, FileCode } from "@/icons";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { cursorPagesSource, useQueryState } from "@/lib/query/use-query-state";
import { isLogicalFile } from "../model";
import { treeComparisonQuery } from "../queries";

// The logical file of the grid. The contract has no list of every logical file, so the picker offers two things: a path to
// type or paste (checked before it is sent: a base-relative path, no `..`), and the files that differ between the first two
// swimlanes, which is where a grid has something to show. Typing narrows that list, and Enter takes what was typed.

const MAX_SHOWN = 200;

type Props = {
  file: string | null;
  /** The first two swimlanes of the grid: the files that differ between them are the suggestions. */
  sides: readonly string[];
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onChoose: (file: string) => void;
  disabled?: boolean;
};

export function FilePicker({ file, sides, open, onOpenChange, onChoose, disabled = false }: Props) {
  const [text, setText] = useState("");
  const [invalid, setInvalid] = useState(false);
  const submit = (event: SyntheticEvent) => {
    event.preventDefault();
    const path = text.trim();
    if (!isLogicalFile(path)) {
      setInvalid(true);
      return;
    }
    setInvalid(false);
    onChoose(path);
  };
  const choose = (path: string) => {
    setInvalid(false);
    onChoose(path);
  };
  return (
    <Popover open={open} onOpenChange={onOpenChange}>
      <PopoverTrigger asChild>
        <Button variant="secondary" disabled={disabled} aria-haspopup="dialog" aria-label={`Logical file: ${file === null ? "none chosen" : visibleText(file)}`} className="max-w-[28rem] justify-start bg-background">
          <FileCode aria-hidden="true" className="text-muted-foreground" />
          <span className="truncate font-mono text-xs font-medium">{file === null ? "Choose a file" : <SafeText text={file} />}</span>
          <ChevronsUpDown aria-hidden="true" className="text-muted-foreground" />
        </Button>
      </PopoverTrigger>
      <PopoverContent aria-label="Choose a logical file" className="flex w-[30rem] max-w-[calc(100vw-2rem)] flex-col gap-2 p-2">
        <form onSubmit={submit} className="flex flex-col gap-1">
          <Input
            aria-label="Logical file"
            placeholder="tx-infinity-api/tx-infinity-core.yml"
            value={text}
            aria-invalid={invalid || undefined}
            aria-describedby={invalid ? "compare-file-invalid" : undefined}
            onChange={(event) => {
              setText(event.target.value);
              setInvalid(false);
            }}
            className="font-mono text-xs"
            spellCheck={false}
            autoComplete="off"
          />
          {invalid ? (
            <p id="compare-file-invalid" role="alert" className="text-xs text-danger-fg">
              That is not a path below the config root. Use a path such as tx-infinity-api/tx-infinity-core.yml, with no .. and no leading slash.
            </p>
          ) : (
            <p className="text-xs text-muted-foreground">Type or paste a path and press Enter, or choose a file that differs.</p>
          )}
        </form>
        {sides.length < 2 ? <p className="text-xs text-muted-foreground">Choose two swimlanes to list the files that differ between them.</p> : <Suggestions left={sides[0] ?? ""} right={sides[1] ?? ""} text={text} onChoose={choose} />}
      </PopoverContent>
    </Popover>
  );
}

const STATE_LABEL = { different: "changed", left_only: "only in", right_only: "only in", same: "same" } as const;

function Suggestions({ left, right, text, onChoose }: { left: string; right: string; text: string; onChoose: (file: string) => void }) {
  const pages = useCursorPages({ ...treeComparisonQuery(`effective:${left}`, `effective:${right}`, ""), maxPages: 2 });
  const state = useQueryState(cursorPagesSource(pages), { isEmpty: (entries) => entries.every((entry) => entry.state === "same") });
  const needle = text.trim().toLowerCase();
  return (
    <QueryBoundaryView state={state} noun="files that differ" source={`the comparison of ${left} and ${right}`} staleBanner={false} empty={<p className="p-2 text-sm text-muted-foreground">No file differs between {left} and {right}.</p>}>
      {(entries) => {
        const matching = entries.filter((entry) => entry.state !== "same" && (needle === "" || entry.file.toLowerCase().includes(needle)));
        return (
          <div className="flex flex-col gap-1">
            <p className="text-xs text-muted-foreground">
              Files that differ between {left} and {right}
              {pages.hasMore ? " (the first of them)" : ""}
            </p>
            <ul aria-label="Files that differ" className="flex max-h-64 flex-col overflow-auto">
              {matching.slice(0, MAX_SHOWN).map((entry) => (
                <li key={entry.file}>
                  <button type="button" onClick={() => onChoose(entry.file)} className="flex h-7 w-full cursor-pointer items-center gap-2 rounded-sm px-2 text-left hover:bg-accent">
                    <SafeText text={entry.file} className="min-w-0 flex-1 truncate font-mono text-xs" />
                    <span className="shrink-0 text-xs text-muted-foreground">
                      {STATE_LABEL[entry.state]}
                      {entry.state === "left_only" ? ` ${left}` : entry.state === "right_only" ? ` ${right}` : ""}
                    </span>
                  </button>
                </li>
              ))}
              {matching.length === 0 ? <li className="p-2 text-sm text-muted-foreground">No file matches what you typed.</li> : null}
              {matching.length > MAX_SHOWN ? <li className="p-2 text-xs text-muted-foreground">Showing the first {MAX_SHOWN}. Type more of the path.</li> : null}
            </ul>
          </div>
        );
      }}
    </QueryBoundaryView>
  );
}

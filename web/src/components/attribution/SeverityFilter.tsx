import { useId } from "react";
import type { Severity } from "@/api/generated/model";
import { NativeSelect } from "@/components/ui/native-select";
import { cn } from "@/lib/utils";
import { MIN_SEVERITY_CHOICES, loadedScopeNote, parseSeverity } from "./severity";

// The severity menu: "that level and above", not a pick of any, because that is what the server filters by (`min_severity`).
//
//  - scope "server": the list is filtered by the server, so the answer is whole, however many pages there are. No note.
//  - scope "loaded": the list has no server filter (the drift list, a tree), so the choice keeps the rows that are loaded. While
//    the server has more, a line says so, and the control is described by it, so the screen never looks complete when it is not.
//
// A native select (Radix Select injects a <style> through innerHTML, which the CSP refuses, S12).

export type SeverityFilterProps = {
  value: Severity | null;
  onChange: (value: Severity | null) => void;
  scope: "server" | "loaded";
  /**
   * For scope "loaded": how many rows are loaded, whether the server has rows that are not (a next page, or a list that was cut
   * off at its page limit: `pages.hasMore || pages.truncated`), and what a row is called.
   */
  count?: number;
  hasMore?: boolean;
  noun?: string;
  label?: string;
  disabled?: boolean;
  className?: string;
};

export function SeverityFilter({ value, onChange, scope, count = 0, hasMore = false, noun, label = "Severity", disabled = false, className }: SeverityFilterProps) {
  const id = useId();
  const noteId = `${id}-note`;
  const note = scope === "loaded" && value !== null ? loadedScopeNote({ what: "filter", count, hasMore, ...(noun === undefined ? {} : { noun }) }) : null;
  return (
    <div data-slot="severity-filter" data-scope={scope} className={cn("flex flex-wrap items-center gap-x-2 gap-y-1", className)}>
      <label htmlFor={id} className="text-xs text-muted-foreground">
        {label}
      </label>
      {/* The width is on a box around the select: NativeSelect's wrapper is as wide as its container and draws the arrow at its own right edge. */}
      <div className="w-44">
        <NativeSelect id={id} value={value ?? "any"} disabled={disabled} aria-describedby={note === null ? undefined : noteId} onChange={(event) => onChange(parseSeverity(event.target.value) ?? null)}>
          {MIN_SEVERITY_CHOICES.map((choice) => (
            <option key={choice.value} value={choice.value}>
              {choice.label}
            </option>
          ))}
        </NativeSelect>
      </div>
      {note === null ? null : (
        <span id={noteId} className="text-xs text-muted-foreground">
          {note}
        </span>
      )}
    </div>
  );
}

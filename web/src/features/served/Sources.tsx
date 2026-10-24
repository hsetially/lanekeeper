import type { EffectiveSource } from "@/api/generated/model";
import { SafeText } from "@/components/app/SafeText";
import { AppLink } from "@/routes/AppLink";

// The files the engine reads for a selection (`ServedFile.sources`, at most 16): what it believes the answer is made of, next to what
// the config-server answered. Each is a link to the file view, where its own text, history and drift are. A path is untrusted text
// from the file system (SafeText); a swimlane the route table does not accept makes no link (AppLink), only the text.
//
// Two things are about room. A link is a line of 24 px (`leading-6`), not the 16 px of its text: stacked links 16 px high and 4 px apart
// fail WCAG 2.5.8 (axe `target-size`). And the list holds a few lines and scrolls in its own box (`max-h-28`): sixteen long paths took
// 570 px at 1024 wide and left the text of the answer 0 px high.

export function Sources({ swimlane, sources }: { swimlane: string; sources: readonly EffectiveSource[] }) {
  if (sources.length === 0) return null;
  return (
    <ul aria-label="Files the engine reads for this selection" className="m-0 flex max-h-28 min-w-0 flex-1 list-none flex-wrap items-center gap-x-4 gap-y-1 overflow-y-auto p-0">
      {sources.map((source) => (
        <li key={`${source.rank}:${source.path}`} className="inline-flex max-w-full min-w-0 items-center gap-1.5">
          <AppLink route="files" params={{ id: swimlane }} search={{ path: source.path }} className="min-w-0 truncate font-mono text-xs leading-6 text-link underline underline-offset-2">
            <SafeText text={source.path} />
          </AppLink>
          <span className="text-xs text-muted-foreground tabular-nums">rank {source.rank}</span>
        </li>
      ))}
    </ul>
  );
}

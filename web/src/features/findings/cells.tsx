import type { Finding, FindingKind } from "@/api/generated/model";
import { SafeText } from "@/components/app/SafeText";
import { WifiOff } from "@/icons";
import { AppLink } from "@/routes/AppLink";
import { cn } from "@/lib/utils";
import { nameOf } from "./kinds";
import { lineLabel } from "./lines";

// The cells of the findings table. Everything that came from the server (a path, a swimlane, a message) goes through SafeText.

/** `C8`: the check's code in the design's small bordered mono chip. */
export function CodeChip({ code, className }: { code: string; className?: string }) {
  return (
    <span data-slot="check-code" className={cn("inline-flex h-5 min-w-[26px] shrink-0 items-center justify-center rounded-sm border border-input px-1 font-mono text-xs font-medium tracking-[0.02em]", className)}>
      {code}
    </span>
  );
}

export function CheckCell({ kind, code }: { kind: FindingKind; code: string }) {
  return (
    <span className="flex min-w-0 items-center gap-2">
      <CodeChip code={code} />
      <span className="min-w-0 truncate">{nameOf(kind)}</span>
    </span>
  );
}

/** The swimlane's name, and a mark with its words when the agent that reports on it is offline (the findings are as of its last scan). */
export function SwimlaneCell({ id, offline }: { id: string; offline: boolean }) {
  return (
    <span className="flex min-w-0 items-center gap-1 font-mono text-xs">
      <SafeText text={id} className="truncate" />
      {offline ? (
        <>
          <WifiOff aria-hidden="true" className="size-3 shrink-0 text-warning-fg" />
          <span className="sr-only">agent offline</span>
        </>
      ) : null}
    </span>
  );
}

/** The file name in bold and its folder after it, as one line of mono. */
export function PathCell({ path }: { path: string | null }) {
  if (path === null) return <span className="text-muted-foreground">No file</span>;
  const cut = path.lastIndexOf("/");
  const dir = cut < 0 ? "" : path.slice(0, cut + 1);
  const name = path.slice(cut + 1);
  return (
    <span className="flex min-w-0 items-baseline gap-2 font-mono text-xs">
      {/* The folder gives way first; a name longer than the cell ends in an ellipsis (a flex box is not reached by the cell's own). */}
      <SafeText text={name} className="max-w-full shrink-0 truncate font-medium" />
      {dir === "" ? null : <SafeText text={dir} className="min-w-0 truncate text-muted-foreground" />}
    </span>
  );
}

/** The first place the finding names, as a link into the file at that line, and how many more there are. */
export function LinesCell({ finding }: { finding: Finding }) {
  const first = finding.locations[0];
  if (finding.path === null || first === undefined) return <span className="text-muted-foreground">None</span>;
  const label = lineLabel(first.lines);
  const more = finding.locations.length - 1;
  return (
    <span className="flex items-center font-mono text-xs">
      <AppLink route="files" params={{ id: finding.swimlane }} search={{ path: finding.path, line: String(first.lines.start) }} aria-label={`Open file at ${label}`} title={`Open file at ${label}`}>
        {label}
      </AppLink>
      {more > 0 ? (
        <>
          {" "}
          <span className="text-muted-foreground">+{more}</span>
        </>
      ) : null}
    </span>
  );
}

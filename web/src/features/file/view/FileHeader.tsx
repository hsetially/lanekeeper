import type { ReactNode } from "react";
import type { DriftState } from "@/api/generated/model";
import { DRIFT_VARIANT } from "@/components/app/drift";
import { HashChip, PathChip } from "@/components/app/chips";
import { CopyLink } from "@/components/app/CopyLink";
import { SafeText } from "@/components/app/SafeText";
import { StatusBadge } from "@/components/app/StatusBadge";
import { Badge } from "@/components/ui/badge";
import { Skeleton } from "@/components/ui/skeleton";
import { Layers, Pencil } from "@/icons";
import { cn } from "@/lib/utils";
import type { Fact } from "../model";

// The head of the file view (File, top block). The h1 is the logical file in mono, which is the file as the services see it;
// the path of the file that is actually here is the chip under it. `facts` is null while the file loads: the chips and the
// hash hold their place as skeletons, so nothing moves when they arrive. The file's name is untrusted text (S8): SafeText.

export type FileHeaderProps = {
  title: string;
  /** "Effective file for tx-infinity-core": set when this file is a tenant file that stands in for its base file. */
  marker: string | null;
  drift: DriftState | null;
  /** "Line endings only". */
  note: string | null;
  editing: boolean;
  path: string;
  facts: readonly Fact[] | null;
  hash: string | null;
  /** The facts and the hash are not coming (the file could not be read): no placeholder for them, which would say "loading" under an error. */
  unavailable?: boolean;
  actions: ReactNode;
  /** The app path of this file (made with `linkTo`): the permalink the "Copy link" button copies. Null or absent: no button. */
  link?: string | null;
  /** The line the link points at, when it points at one: the button says so. */
  linkLine?: number | null;
};

const factClasses = "inline-flex h-[22px] items-center rounded-sm border px-1.5 font-mono text-xs";

export function FileHeader({ title, marker, drift, note, editing, path, facts, hash, unavailable = false, actions, link = null, linkLine = null }: FileHeaderProps) {
  return (
    <section aria-label="File" className="flex flex-none flex-wrap items-start justify-between gap-x-4 gap-y-3 px-6 pt-3.5 pb-3">
      <div className="flex min-w-0 flex-col gap-2">
        <div className="flex flex-wrap items-center gap-2">
          <h1 className="m-0 font-mono text-xl leading-[26px] font-medium">
            <SafeText text={title} />
          </h1>
          {marker === null ? null : (
            <Badge className="border-info-border bg-info-bg text-info-fg">
              <Layers aria-hidden="true" />
              <SafeText text={marker} />
            </Badge>
          )}
          {drift === null ? null : <StatusBadge variant={DRIFT_VARIANT[drift]} />}
          {note === null ? null : <Badge className="border-dashed border-input">{note}</Badge>}
          {editing ? (
            <Badge className="border-primary font-semibold text-link">
              <Pencil aria-hidden="true" />
              Editing
            </Badge>
          ) : null}
        </div>
        <div className="flex flex-wrap items-center gap-x-2 gap-y-1.5">
          <PathChip path={path} />
          {facts === null ? (
            unavailable ? null : <>
              <Skeleton className="h-[22px] w-12" />
              <Skeleton className="h-[22px] w-14" />
              <Skeleton className="h-[22px] w-20" />
            </>
          ) : (
            facts.map((fact) => (
              <span key={fact.text} data-tone={fact.tone ?? "plain"} className={cn(factClasses, fact.tone === "info" ? "border-info-border bg-info-bg text-info-fg" : "border-border text-foreground")}>
                {fact.text}
              </span>
            ))
          )}
          {hash === null ? unavailable ? null : <Skeleton className="h-[22px] w-20" /> : <HashChip hash={hash} />}
          {link === null ? null : <CopyLink href={link} what={linkLine === null ? "link to this file" : `link to line ${linkLine}`} />}
        </div>
      </div>
      <div className="flex flex-wrap items-center justify-end gap-2">{actions}</div>
    </section>
  );
}

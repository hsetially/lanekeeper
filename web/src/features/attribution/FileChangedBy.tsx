import type { Attribution } from "@/api/generated/model";
import { AttributionChip, type AttributionChipProps } from "@/components/attribution/AttributionChip";
import { cn } from "@/lib/utils";

// "Changed by  Marco Diaz via Lanekeeper  [bars] Certain": the head of a file (File, top block). `attribution` is the file entry's
// (`FileEntry.attribution`): null for a file the hub has never seen change, which says "Not recorded" and invents no author.

export type FileChangedByProps = Pick<AttributionChipProps, "you" | "nameOf" | "className"> & {
  attribution: Attribution | null;
  label?: string;
};

export function FileChangedBy({ attribution, label = "Changed by", you, nameOf, className }: FileChangedByProps) {
  return (
    <span data-slot="file-changed-by" className={cn("inline-flex flex-wrap items-center gap-x-2 gap-y-1 text-xs leading-4", className)}>
      <span className="text-muted-foreground">{label}</span>
      <AttributionChip attribution={attribution} you={you} nameOf={nameOf} />
    </span>
  );
}

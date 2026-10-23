import { Fragment } from "react";
import type { Attribution } from "@/api/generated/model";
import { SafeText } from "@/components/app/SafeText";
import { cn } from "@/lib/utils";
import { SOURCE_MEANING, UPGRADE_NOTE, evidenceRows, type NameLookup } from "./model";

// The facts behind an attribution, as a definition list: the source, the confidence, who, and each piece of evidence the server
// recorded (the executable, the login identity, when it was seen). A drawer shows it in place; the chip shows it in a popover.
// Only what was recorded is a row, and the words never say more than the server did. Every value comes from an NFS VM, a cluster
// or a person, so every one goes through SafeText (S8).

export type AttributionEvidenceProps = {
  attribution: Attribution;
  /** The signed-in person's id (their own changes are "You"). */
  you?: string | null | undefined;
  nameOf?: NameLookup | undefined;
  className?: string;
};

export function AttributionEvidence({ attribution, you, nameOf, className }: AttributionEvidenceProps) {
  const rows = evidenceRows(attribution, { you, nameOf });
  const { evidence } = attribution;
  const recorded = [evidence.executable, evidence.login_identity, evidence.observed_at].some((value) => value !== undefined && value !== null);
  return (
    <div data-slot="attribution-evidence" className={cn("flex flex-col gap-2 text-xs leading-4", className)}>
      <dl className="m-0 grid grid-cols-[minmax(0,6.5rem)_minmax(0,1fr)] gap-x-3 gap-y-1">
        {rows.map((row) => (
          <Fragment key={row.label}>
            <dt className="text-muted-foreground">{row.label}</dt>
            <dd className={cn("m-0 min-w-0 break-words", row.mono === true && "font-mono")}>
              <SafeText text={row.value} />
            </dd>
          </Fragment>
        ))}
      </dl>
      {recorded ? null : <p className="m-0">No further evidence was recorded.</p>}
      <p className="m-0 text-muted-foreground">
        {SOURCE_MEANING[attribution.source]} {UPGRADE_NOTE}
      </p>
    </div>
  );
}

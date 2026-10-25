import type { EolStyle } from "@/api/generated/model";
import { GatedButton } from "@/components/app/GatedButton";
import { SafeText } from "@/components/app/SafeText";
import type { GateResult } from "@/components/app/write-gate";
import { Button } from "@/components/ui/button";
import { Info } from "@/icons";
import { eolLabel } from "../model";

// Two sides with the same text and different line endings (Diff, 1c). A diff would mark every line, which says nothing, so it
// is hidden behind this notice. The notice says which ending each side has, why the file shows as changed, and offers the two
// things a person may want: to see the diff anyway, or to convert the NFS file (a write: it goes through the Save pipeline).

export type LineEndingNoticeProps = {
  nfsEol: EolStyle;
  lines: number;
  /** The other side, as it reads: "Git", "sitc". */
  against: string;
  /** The server sent lines to show (it may send none for a line-endings-only change). */
  canShowAnyway: boolean;
  convertGate: GateResult;
  onShowAnyway: () => void;
  onConvert: (to: "lf" | "crlf") => void;
};

const OTHER: Record<"lf" | "crlf", "lf" | "crlf"> = { lf: "crlf", crlf: "lf" };

export function LineEndingNotice({ nfsEol, lines, against, canShowAnyway, convertGate, onShowAnyway, onConvert }: LineEndingNoticeProps) {
  const target: "lf" | "crlf" = nfsEol === "mixed" ? "lf" : OTHER[nfsEol];
  const isGit = against === "Git";
  return (
    <div className="flex flex-1 items-start justify-center px-6 py-12">
      <div role="status" className="flex w-[560px] max-w-full flex-col gap-3 rounded-md border border-info-border bg-info-bg p-4">
        <div className="flex items-center gap-2 text-info-fg">
          <Info aria-hidden="true" className="size-4 shrink-0" />
          <strong className="text-md font-semibold">Only line endings differ</strong>
        </div>
        <p className="m-0">
          NFS uses {nfsEol === "mixed" ? "mixed line endings" : <code className="font-mono text-xs">{eolLabel(nfsEol)}</code>}
          {nfsEol === "mixed" ? null : (
            <>
              {" "}
              and <SafeText text={against} /> uses <code className="font-mono text-xs">{eolLabel(OTHER[nfsEol])}</code>
            </>
          )}
          . All {lines.toLocaleString("en-US")} lines have the same text, so the diff is hidden.{isGit ? " This is why the file shows NFS ahead." : ""}
        </p>
        <div className="flex flex-wrap gap-2">
          {canShowAnyway ? (
            <Button variant="secondary" onClick={onShowAnyway}>
              Show diff anyway
            </Button>
          ) : null}
          <GatedButton variant="secondary" gate={convertGate} onClick={() => onConvert(target)}>
            Convert NFS file to {eolLabel(target)}
          </GatedButton>
        </div>
      </div>
    </div>
  );
}

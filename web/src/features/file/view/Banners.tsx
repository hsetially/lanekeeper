import { Banner } from "@/components/app/Banner";
import { SafeText } from "@/components/app/SafeText";
import { GatedButton } from "@/components/app/GatedButton";
import type { GateResult } from "@/components/app/write-gate";
import type { WithheldReason } from "@/components/masking/withheld";
import { Button } from "@/components/ui/button";
import { Eye, Lock, TriangleAlert, WifiOff } from "@/icons";
import { formatClock, formatRelative } from "@/lib/time";
import { AppLink } from "@/routes/AppLink";

// The notices across the top of the file view, in the words of the design (File, States P03 and O03).

/** A Viewer (States P03). The buttons are disabled with the reason; this says it once. */
export function ReadOnlyBanner() {
  return (
    <Banner variant="info" icon={<Eye aria-hidden="true" />} title="Read-only">
      Viewers can compare and download. Saving and raising PRs need Editor.
    </Banner>
  );
}

/** The swimlane's agent is not reporting (States O03): the file is the last known one, and a save would go nowhere. */
export function AgentOfflineBanner({ lane, asOf, now, canCheck }: { lane: string; asOf: number | null; now: number; canCheck: boolean }) {
  return (
    <Banner
      variant="warning"
      icon={<WifiOff aria-hidden="true" />}
      title={`Agent for ${lane} is offline`}
      {...(canCheck
        ? {
            action: (
              <Button asChild variant="secondary" size="sm">
                <AppLink route="admin-swimlanes">Check agent</AppLink>
              </Button>
            ),
          }
        : {})}
    >
      {asOf === null ? "Showing the data from the last scan." : `Showing data as of ${formatClock(asOf)} (${formatRelative(asOf, now)}).`} Saving on {lane} is off.
    </Banner>
  );
}

/**
 * A file whose content is not shown (D79): name, size, hash and who changed it are all there is. A file of the denied class is a
 * sensitive type the hub never reads; any other file whose content is withheld is one the hub could not check for secrets, and the
 * contract fails closed (S14b), so the banner does not call it a sensitive type.
 */
export function WithheldBanner({ reason = "denied" }: { reason?: WithheldReason }) {
  return (
    <div role="status" className="flex min-h-10 flex-none flex-wrap items-center gap-2 border-y border-status-locked-border bg-status-locked-bg px-6 py-1.5 text-status-locked-fg">
      <Lock aria-hidden="true" className="size-3.5 shrink-0" />
      <strong className="font-semibold">{reason === "denied" ? "Content withheld: sensitive file type" : "Content withheld: could not be checked for secrets"}</strong>
      <span>Lanekeeper shows only the name, size, hash and who changed it. There is no edit, upload or download.</span>
    </div>
  );
}

/** A big file opens read-only first so that the page stays fast (P10); editing it is one press away. */
export function LargeFileBanner({ lines, gate, onEnableEditing }: { lines: number; gate: GateResult; onEnableEditing: () => void }) {
  return (
    <Banner
      variant="warning"
      icon={<TriangleAlert aria-hidden="true" />}
      title={`Large file · ${lines.toLocaleString("en-US")} lines`}
      action={
        <GatedButton variant="secondary" size="sm" gate={gate} onClick={onEnableEditing}>
          Enable editing
        </GatedButton>
      }
    >
      Opened read-only first so the page stays fast.
    </Banner>
  );
}

/** The strip under the head while the file is being edited. */
export function EditingStrip({ lane, file }: { lane: string; file: string }) {
  return (
    <div className="flex h-9 flex-none items-center gap-2 border-y border-border bg-muted px-6">
      <span className="font-semibold">Editing NFS</span>
      <span className="text-muted-foreground">
        <SafeText text={`${lane} · ${file}`} />
      </span>
      <span className="ml-auto text-xs text-muted-foreground">
        Saving writes straight to the {lane} NFS share. Git changes only through a PR.
      </span>
    </div>
  );
}

/** The file changed on NFS while it is open for editing: said before the save is refused, not after. */
export function ChangedWhileEditingBanner() {
  return (
    <Banner variant="warning" title="This file changed on NFS while you were editing">
      Someone else saved a newer version. Saving will be refused, and you will be asked what to do with your changes.
    </Banner>
  );
}

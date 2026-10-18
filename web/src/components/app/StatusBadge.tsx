import type { LucideIcon } from "lucide-react";
import { badgeBase } from "@/components/ui/badge";
import { ArrowDownToLine, ArrowUpFromLine, Bell, Check, CircleDashed, CircleHelp, Clock, Hourglass, Lock, Pin, Radio, Server, Split } from "@/icons";
import { cn } from "@/lib/utils";

// Every status is colour plus icon plus label, never colour alone, and the fill weight differs too so the states stay
// distinct in greyscale: Conflict is solid, Untracked is a dashed outline, Pending approval is an outline.
//
// Deviation (design/DEVIATIONS.md): the handoff's "Pending restart" (variant restart) is gone. Since D85 a service has
// four pickup states, so the pickup variants replace it. The mapping from the API's enums (DriftState, PickupState)
// to these variants is written in T1 over the generated types; this file has no API type in it on purpose.
export type StatusVariant =
  | "sync"
  | "git"
  | "nfs"
  | "conflict"
  | "intentional"
  | "unknown"
  | "untracked"
  | "locked"
  | "approval"
  | "live"
  | "live-by"
  | "needs-notify-or-restart"
  | "needs-config-server-restart";

type VariantSpec = {
  label: string;
  /** Handoff icon file name (assets/icons/<name>.svg). */
  icon: string;
  Icon: LucideIcon;
  /** The token family: --status-<tokens>-fg, -bg, -border. */
  tokens: "sync" | "git" | "nfs" | "conflict" | "intentional" | "unknown" | "untracked" | "restart" | "locked" | "approval";
  dashed: boolean;
};

export const STATUS_VARIANTS: Record<StatusVariant, VariantSpec> = {
  // Handoff drift and operational statuses.
  sync: { label: "In sync", icon: "check", Icon: Check, tokens: "sync", dashed: false },
  git: { label: "Git ahead", icon: "arrow-down-to-line", Icon: ArrowDownToLine, tokens: "git", dashed: false },
  nfs: { label: "NFS ahead", icon: "arrow-up-from-line", Icon: ArrowUpFromLine, tokens: "nfs", dashed: false },
  conflict: { label: "Conflict", icon: "split", Icon: Split, tokens: "conflict", dashed: false },
  intentional: { label: "Intentional", icon: "pin", Icon: Pin, tokens: "intentional", dashed: false },
  unknown: { label: "Unknown", icon: "circle-help", Icon: CircleHelp, tokens: "unknown", dashed: false },
  untracked: { label: "Untracked", icon: "circle-dashed", Icon: CircleDashed, tokens: "untracked", dashed: true },
  locked: { label: "Locked", icon: "lock", Icon: Lock, tokens: "locked", dashed: false },
  approval: { label: "Pending approval", icon: "hourglass", Icon: Hourglass, tokens: "approval", dashed: false },
  // The four pickup states (D85). No handoff frame exists for them yet, so these are an interim design: the
  // handoff's "restart" colours for the two that need action, the "sync" colours for the two that are live.
  live: { label: "Live", icon: "radio", Icon: Radio, tokens: "sync", dashed: false },
  "live-by": { label: "Live by", icon: "clock", Icon: Clock, tokens: "sync", dashed: false },
  "needs-notify-or-restart": { label: "Needs notify or restart", icon: "bell", Icon: Bell, tokens: "restart", dashed: false },
  "needs-config-server-restart": { label: "Needs config-server restart", icon: "server", Icon: Server, tokens: "restart", dashed: false },
};

// Literal class names, so Tailwind finds them. The token families are listed once here.
const TOKEN_CLASSES: Record<VariantSpec["tokens"], string> = {
  sync: "text-status-sync-fg bg-status-sync-bg border-status-sync-border",
  git: "text-status-git-fg bg-status-git-bg border-status-git-border",
  nfs: "text-status-nfs-fg bg-status-nfs-bg border-status-nfs-border",
  conflict: "text-status-conflict-fg bg-status-conflict-bg border-status-conflict-border",
  intentional: "text-status-intentional-fg bg-status-intentional-bg border-status-intentional-border",
  unknown: "text-status-unknown-fg bg-status-unknown-bg border-status-unknown-border",
  untracked: "text-status-untracked-fg bg-status-untracked-bg border-status-untracked-border",
  restart: "text-status-restart-fg bg-status-restart-bg border-status-restart-border",
  locked: "text-status-locked-fg bg-status-locked-bg border-status-locked-border",
  approval: "text-status-approval-fg bg-status-approval-bg border-status-approval-border",
};

type StatusBadgeProps = {
  variant: StatusVariant;
  /** A count next to the label (4 Git ahead). */
  count?: number;
  /** Another label for the same look: "PR open" reuses git, "Adopted" reuses sync. */
  label?: string;
  /** For live-by: the time, so the badge reads "Live by 14:32". */
  detail?: string;
  /** Icon and count only, for dense rows. The label stays as the accessible name and the title. */
  compact?: boolean;
  className?: string;
};

export function StatusBadge({ variant, count, label, detail, compact = false, className }: StatusBadgeProps) {
  const spec = STATUS_VARIANTS[variant];
  const base = label ?? spec.label;
  const text = variant === "live-by" && detail && label === undefined ? `${base} ${detail}` : base;
  // `max-w-full` and the truncating label: where the room is short the label ends in an ellipsis, it does not run under its neighbour.
  const classes = cn(badgeBase, "max-w-full", TOKEN_CLASSES[spec.tokens], spec.dashed && "border-dashed", className);

  if (compact) {
    const name = count === undefined ? text : `${count} ${text}`;
    return (
      <span data-slot="status-badge" data-status={variant} role="img" aria-label={name} title={text} className={classes}>
        <spec.Icon aria-hidden="true" />
        {count === undefined ? null : <span className="tabular-nums">{count}</span>}
      </span>
    );
  }
  return (
    <span data-slot="status-badge" data-status={variant} className={classes}>
      <spec.Icon aria-hidden="true" />
      {count === undefined ? null : <span className="tabular-nums">{count}</span>}
      <span className="min-w-0 truncate">{text}</span>
    </span>
  );
}

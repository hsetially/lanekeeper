import type { LucideIcon } from "lucide-react";
import type { ReactNode } from "react";
import { badgeBase } from "@/components/ui/badge";
import { cn } from "@/lib/utils";
import type { ActionTone, StatusTone } from "./model";

// The design's chips (`bd-*`): an icon, a word, and a colour that is never the only sign. Solid dark is Cursor, dashed is
// something that is not in force (expired, a draft). The tone classes are literal so that Tailwind finds them.

export type Tone = ActionTone | StatusTone | "sync" | "detected" | "unknown";

const TONE: Record<Tone, string> = {
  ui: "border-input bg-transparent text-foreground",
  cursor: "border-foreground bg-foreground text-background",
  delete: "border-status-conflict-border bg-status-conflict-bg text-status-conflict-fg",
  restart: "border-status-restart-border bg-status-restart-bg text-status-restart-fg",
  pr: "border-info-border bg-info-bg text-info-fg",
  neutral: "border-border bg-muted text-muted-foreground",
  approval: "border-status-approval-border bg-status-approval-bg text-status-approval-fg",
  stale: "border-warning-border bg-warning-bg text-warning-fg",
  expired: "border-dashed border-input bg-transparent text-muted-foreground",
  ok: "border-success-border bg-success-bg text-success-fg",
  rejected: "border-danger-border bg-danger-bg text-danger-fg",
  draft: "border-dashed border-border bg-muted text-muted-foreground",
  sync: "border-status-sync-border bg-status-sync-bg text-status-sync-fg",
  detected: "border-dashed border-warning-border bg-warning-bg text-warning-fg",
  unknown: "border-dashed border-status-unknown-border bg-status-unknown-bg text-status-unknown-fg italic",
};

export function Chip({ tone, Icon, children, className }: { tone: Tone; Icon?: LucideIcon; children: ReactNode; className?: string }) {
  return (
    <span data-slot="tone-chip" data-tone={tone} className={cn(badgeBase, "max-w-full", TONE[tone], className)}>
      {Icon ? <Icon aria-hidden="true" /> : null}
      <span className="min-w-0 truncate">{children}</span>
    </span>
  );
}

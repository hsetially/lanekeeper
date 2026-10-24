import type { ReactNode } from "react";
import type { DiffHunk, Proposal, SettingChange } from "@/api/generated/model";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { HashChip, PathChip, SettingChip } from "@/components/app/chips";
import { SafeText } from "@/components/app/SafeText";
import { DiffView } from "@/components/app/diff/DiffView";
import { CircleCheck, CircleX, Info, Lock, TriangleAlert } from "@/icons";
import { formatRelative, formatTime } from "@/lib/time";
import { cn } from "@/lib/utils";
import { AppLink } from "@/routes/AppLink";
import type { Banner, BannerVariant } from "./banner";
import { expiresIn } from "./model";
import type { People } from "./people";

// What the drawer says about a proposal, top to bottom: the banner (where it stands), who proposed it and when, what approving
// does, then the proposal itself as the server sent it: the diff of the content, the settings that changed, the files it names,
// the hash it expects to find and the hash it would leave. Every piece of text from a file or a person goes through SafeText.

const ZERO_HASH = /^0{64}$/;
const MOST_ROWS = 20;
const ALERT_ICON = { lock: Lock, info: Info, success: CircleCheck, warning: TriangleAlert, danger: CircleX } as const;
const ALERT_VARIANT: Record<BannerVariant, "info" | "success" | "warning" | "danger"> = { lock: "info", info: "info", success: "success", warning: "warning", danger: "danger" };

export function BannerView({ banner }: { banner: Banner }) {
  const Icon = ALERT_ICON[banner.variant];
  return (
    <Alert variant={ALERT_VARIANT[banner.variant]} icon={<Icon aria-hidden="true" />} data-banner={banner.variant}>
      <AlertTitle>
        <SafeText text={banner.title} />
      </AlertTitle>
      {banner.body !== null ? (
        <AlertDescription>
          <SafeText text={banner.body} />
        </AlertDescription>
      ) : null}
      {banner.request !== null ? (
        <AlertDescription>
          <AppLink route="approval" params={{ id: String(banner.request) }} className="text-link underline">
            View request
          </AppLink>
        </AlertDescription>
      ) : null}
    </Alert>
  );
}

export function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-1.5">
      <h3 className="flex items-center gap-2 text-xs font-medium text-muted-foreground after:h-px after:flex-1 after:bg-border">{title}</h3>
      {children}
    </section>
  );
}

// The key and value rows of a drawer (this one and the audit event's): a two-column list with the keys at 132 px.
export function Rows({ children }: { children: ReactNode }) {
  return <dl className="grid grid-cols-[132px_minmax(0,1fr)] gap-x-3 gap-y-1.5 rounded-sm border border-border px-3 py-2 text-xs">{children}</dl>;
}
export const K = ({ children }: { children: ReactNode }) => <dt className="text-muted-foreground">{children}</dt>;
export const V = ({ children, mono }: { children: ReactNode; mono?: boolean }) => <dd className={cn("min-w-0 [overflow-wrap:anywhere]", mono && "font-mono")}>{children}</dd>;

export function Meta({ p, people, now }: { p: Proposal; people: People; now: number }) {
  const e = expiresIn(p, now);
  const status = p.status;
  const decidedAt = p.decided_at ?? null;
  const end = status === "pending" || status === "stale" ? `Expires in ${e.text}` : status === "expired" ? `Expired ${e.text}` : decidedAt !== null ? `Decided ${formatRelative(decidedAt, now)}` : null;
  return (
    <div className="flex flex-wrap gap-x-3.5 gap-y-0.5 text-xs leading-[18px] text-muted-foreground">
      <span>
        Proposed by{" "}
        <b className="font-medium text-foreground">
          <SafeText text={people.name(p.author)} />
        </b>
        {p.via === "mcp" ? " via Cursor" : ""}
      </span>
      <span title={formatTime(p.created_at)}>Proposed {formatRelative(p.created_at, now)}</span>
      {end !== null ? <span title={formatTime(p.expires_at)}>{end}</span> : null}
    </div>
  );
}

function countLines(hunks: readonly DiffHunk[]): { added: number; removed: number } {
  let added = 0;
  let removed = 0;
  for (const hunk of hunks) {
    for (const line of hunk.lines) {
      if (line.kind === "added") added += 1;
      else if (line.kind === "removed") removed += 1;
    }
  }
  return { added, removed };
}

const short = (hash: string): string => hash.slice(0, 7);

/** The diff of the content, in a box with the file, the hash it expected and the hash it leaves. */
export function ChangeBlock({ p }: { p: Proposal }) {
  const { added, removed } = countLines(p.hunks);
  const rows = p.hunks.reduce((sum, hunk) => sum + hunk.lines.length + 1, 0);
  const height = Math.max(96, Math.min(360, rows * 20 + 8));
  const empty = ZERO_HASH.test(p.new_hash);
  return (
    <div className="overflow-hidden rounded-sm border border-border">
      <div className="flex min-h-6 flex-wrap items-center gap-x-1.5 border-b border-border bg-muted px-2 py-0.5 text-xs leading-4 text-muted-foreground">
        <SafeText text={p.swimlane} className="font-mono text-foreground" />
        <span aria-hidden="true">·</span>
        <SafeText text={p.path} className="font-mono [overflow-wrap:anywhere]" />
        <span aria-hidden="true">·</span>
        <span className="sr-only">expects </span>
        <span className="font-mono">{p.expected.state === "hash" ? short(p.expected.hash) : "new file"}</span>
        <span aria-hidden="true">→</span>
        <span className="sr-only">leaves </span>
        <span className="font-mono">{empty ? "none" : short(p.new_hash)}</span>
        <span className="tabular-nums">
          +{added} −{removed}
        </span>
      </div>
      {/* The height is a CSSOM style, set from the rows the server sent; the diff view scrolls inside it. */}
      <div style={{ height }}>
        <DiffView label="Proposed change" hunks={p.hunks} mode="unified" className="h-full" empty="The server sent no lines for this change." />
      </div>
    </div>
  );
}

function kindLabel(kind: SettingChange["kind"]): string {
  return kind === "type_changed" ? "type changed" : kind;
}

export function SettingsBlock({ changes }: { changes: readonly SettingChange[] }) {
  const shown = changes.slice(0, MOST_ROWS);
  return (
    <ul className="flex flex-col divide-y divide-border rounded-sm border border-border">
      {shown.map((c, index) => (
        // A setting appears once per proposal, but a path is untrusted text: the position keeps two equal paths apart.
        <li key={`${c.path}:${index}`} className="grid grid-cols-[minmax(0,1fr)_auto] gap-x-3 gap-y-0.5 px-3 py-2 text-xs">
          <SettingChip path={c.path} copy={false} />
          <span className="text-muted-foreground">
            {kindLabel(c.kind)}
            {c.severity !== null ? ` · ${c.severity} severity` : ""}
          </span>
          <span className="col-span-2 flex min-w-0 flex-wrap items-center gap-1.5 font-mono">
            {c.left !== null ? <SafeText text={c.left.text} className="max-w-full truncate text-diff-del-fg" /> : <span className="text-muted-foreground">none</span>}
            <span aria-hidden="true" className="text-muted-foreground">
              →
            </span>
            {c.right !== null ? <SafeText text={c.right.text} className="max-w-full truncate text-diff-add-fg" /> : <span className="text-muted-foreground">none</span>}
          </span>
        </li>
      ))}
      {changes.length > shown.length ? <li className="px-3 py-2 text-xs text-muted-foreground">{changes.length - shown.length} more settings changed.</li> : null}
    </ul>
  );
}

/** The files or services the proposal names (a restart's services, a pull request's files, an upload's target). */
export function PathsBlock({ p }: { p: Proposal }) {
  const paths = p.paths ?? [p.path];
  const shown = paths.slice(0, MOST_ROWS);
  return (
    <ul className="flex flex-col gap-1">
      {shown.map((path, index) => (
        <li key={`${path}:${index}`}>
          <PathChip path={path} maxChars={64} />
        </li>
      ))}
      {paths.length > shown.length ? <li className="text-xs text-muted-foreground">{paths.length - shown.length} more.</li> : null}
    </ul>
  );
}

export function Facts({ p }: { p: Proposal }) {
  return (
    <Rows>
      <K>Swimlane</K>
      <dd>
        <SafeText text={p.swimlane} className="font-mono" />
      </dd>
      <K>Expects</K>
      <dd>{p.expected.state === "hash" ? <HashChip hash={p.expected.hash} /> : "The file does not exist yet"}</dd>
      <K>Leaves</K>
      <dd>{ZERO_HASH.test(p.new_hash) ? "No content (it writes none)" : <HashChip hash={p.new_hash} />}</dd>
    </Rows>
  );
}

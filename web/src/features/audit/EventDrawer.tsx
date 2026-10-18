import { useEffect, useRef, type ReactNode, type RefObject } from "react";
import { useRouter } from "@tanstack/react-router";
import type { AuditEvent } from "@/api/generated/model";
import { CopyButton } from "@/components/app/CopyButton";
import { CopyValue } from "@/components/app/CopyValue";
import { SafeText } from "@/components/app/SafeText";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Chip } from "@/features/approvals/chips";
import type { People } from "@/features/approvals/people";
import { Check, CircleHelp, ShieldAlert, TriangleAlert, X, Zap } from "@/icons";
import { formatTime } from "@/lib/time";
import { cn } from "@/lib/utils";
import { actionView, confidenceView, diffLines, sourceLabel, viaView, whoOf, type Break, type DiffKind } from "./model";
import { FOCUSABLE_SCROLLER } from "@/components/app/scroll-region";

// The event drawer (Audit page, 600 px): who, how sure, the diff the hub kept with the event, the evidence of the attribution, and
// the links of the chain. It shows metadata only. The diff is the text the hub stored, already masked by the hub (a flagged line
// is its marker); it is shown as text, line by line, never as markup, and at most MAX_LINES of it.

const MAX_LINES = 400;
const n = (value: number): string => value.toLocaleString("en-US");
const text = (value: string | null | undefined): value is string => typeof value === "string" && value !== "";

const LINE: Record<DiffKind, string> = {
  meta: "bg-muted text-muted-foreground",
  hunk: "bg-diff-hunk-bg text-diff-hunk-fg",
  added: "bg-diff-add-bg text-diff-add-fg",
  removed: "bg-diff-del-bg text-diff-del-fg",
  context: "",
};

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-1.5">
      <h3 className="flex items-center gap-2 text-xs font-medium text-muted-foreground after:h-px after:flex-1 after:bg-border">{title}</h3>
      {children}
    </section>
  );
}

function Rows({ children }: { children: ReactNode }) {
  return <dl className="grid grid-cols-[132px_minmax(0,1fr)] gap-x-3 gap-y-1.5 rounded-sm border border-border px-3 py-2 text-xs">{children}</dl>;
}
const K = ({ children }: { children: ReactNode }) => <dt className="text-muted-foreground">{children}</dt>;
const V = ({ children, mono }: { children: ReactNode; mono?: boolean }) => <dd className={cn("min-w-0 [overflow-wrap:anywhere]", mono && "font-mono")}>{children}</dd>;

function Diff({ text }: { text: string }) {
  const { lines, more } = diffLines(text, MAX_LINES);
  return (
    <div className="overflow-hidden rounded-sm border border-border">
      <div role="group" aria-label="Diff the hub kept" {...FOCUSABLE_SCROLLER} className="max-h-80 overflow-auto font-mono text-xs leading-[18px]">
        {lines.map((line, index) => (
          // The same line can appear twice in a diff, so the position is its identity; the list never reorders.
          <div key={index} className={cn("min-w-max px-3 whitespace-pre", LINE[line.kind])}>
            <SafeText text={line.text === "" ? " " : line.text} />
          </div>
        ))}
      </div>
      {more > 0 ? <p className="border-t border-border bg-muted px-3 py-1 text-xs text-muted-foreground">{n(more)} more lines are not shown.</p> : null}
    </div>
  );
}

function useFocusOnOpen(target: RefObject<HTMLElement | null>): void {
  const router = useRouter();
  useEffect(() => {
    target.current?.focus({ preventScroll: true });
    const off = router.subscribe("onRendered", () => {
      target.current?.focus({ preventScroll: true });
    });
    const stop = setTimeout(off, 1500);
    return () => {
      clearTimeout(stop);
      off();
    };
  }, [router, target]);
}

type Props = {
  event: AuditEvent;
  /** The event before this one in the chain, when it is loaded. */
  previous: AuditEvent | undefined;
  broken: Break | undefined;
  people: People;
  onClose: () => void;
};

export function EventDrawer({ event, previous, broken, people, onClose }: Props) {
  const heading = useRef<HTMLHeadingElement>(null);
  useFocusOnOpen(heading);
  const action = actionView(event.action);
  const via = viaView(event);
  const who = whoOf(event, people.name);
  const a = event.attribution;
  const c = a === null ? null : confidenceView(a.confidence);
  const title = event.target === null ? action.label : `${action.label} ${event.target}`;
  const login = a?.evidence.login_identity;
  const exe = a?.evidence.executable;
  const seen = a?.evidence.observed_at;
  const linked = previous !== undefined && previous.seq === event.seq - 1 && broken === undefined;

  return (
    <div role="dialog" aria-label="Event detail" className="absolute inset-y-0 right-0 z-30 flex w-[600px] max-w-full flex-col border-l border-border bg-popover text-popover-foreground shadow-popover">
      <div className="flex flex-none items-start gap-2 border-b border-border py-3 pr-3 pl-4">
        <div className="flex min-w-0 flex-1 flex-col gap-1.5">
          <div className="flex flex-wrap items-center gap-1.5">
            <Chip tone={via.tone} Icon={via.Icon}>
              {via.label}
            </Chip>
            <span className="inline-flex h-5 items-center rounded-sm border border-border px-1.5 font-mono text-xs">Event {n(event.seq)}</span>
            {broken !== undefined ? (
              <Chip tone="rejected" Icon={ShieldAlert} className="border-danger-fg bg-danger-fg text-background">
                Chain break
              </Chip>
            ) : null}
          </div>
          <h2 ref={heading} tabIndex={-1} className="text-base leading-[22px] font-semibold tracking-[-0.01em] outline-none">
            <SafeText text={title} />
          </h2>
          <div className="flex flex-wrap items-center gap-x-2 font-mono text-xs leading-[18px] text-muted-foreground">
            {event.swimlane === null ? null : <SafeText text={event.swimlane} className="text-foreground" />}
            <span>{formatTime(event.at, { seconds: true })}</span>
          </div>
        </div>
        <Button variant="ghost" size="icon-sm" aria-label="Close" onClick={onClose}>
          <X />
        </Button>
      </div>
      <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto px-4 py-3.5">
        {broken !== undefined ? (
          <Alert variant="danger" icon={<ShieldAlert aria-hidden="true" />}>
            <AlertTitle>The chain breaks at this event</AlertTitle>
            <AlertDescription>The previous hash stored here is not the hash of event {n(broken.previous)}, which is loaded. This compares the hashes the hub sent.</AlertDescription>
          </Alert>
        ) : who.kind === "unknown" ? (
          <Alert variant="warning" icon={<TriangleAlert aria-hidden="true" />}>
            <AlertTitle>Changed outside Lanekeeper, author unknown</AlertTitle>
            <AlertDescription>Lanekeeper saw the file change but could not tie it to a login.</AlertDescription>
          </Alert>
        ) : event.via === "mcp" && who.kind === "named" ? (
          <Alert variant="info" icon={<Zap aria-hidden="true" />}>
            <AlertTitle>
              <SafeText text={`Made by a Cursor agent for ${who.name}`} />
            </AlertTitle>
            <AlertDescription>It came in through the MCP server, on behalf of that person.</AlertDescription>
          </Alert>
        ) : null}

        <Section title="Who">
          <Rows>
            <K>User</K>
            <V>
              {who.kind === "unknown" ? (
                <Chip tone="unknown" Icon={CircleHelp}>
                  unknown author
                </Chip>
              ) : who.kind === "system" ? (
                "Lanekeeper"
              ) : (
                <>
                  <SafeText text={who.name} />
                  {who.probable ? <span className="ml-1.5 text-muted-foreground">probable</span> : null}
                </>
              )}
            </V>
            <K>Attribution</K>
            <V>{a === null || c === null ? <span className="text-muted-foreground">None recorded</span> : `${sourceLabel(a.source)} · ${c.label} confidence`}</V>
            {event.actor !== null ? (
              <>
                <K>Recorded by</K>
                <V>
                  <SafeText text={people.name(event.actor)} />
                </V>
              </>
            ) : null}
            {event.request_id === null ? null : (
              <>
                <K>Request ID</K>
                <V mono>
                  <CopyValue value={event.request_id} what="request ID" />
                </V>
              </>
            )}
          </Rows>
        </Section>

        {event.diff !== null && event.diff !== "" ? (
          <Section title="Change">
            <Diff text={event.diff} />
          </Section>
        ) : null}

        {text(login) || text(exe) || typeof seen === "number" ? (
          <Section title="Attribution evidence">
            <Rows>
              {text(login) ? (
                <>
                  <K>OS Login user</K>
                  <V mono>
                    <SafeText text={login} />
                  </V>
                </>
              ) : null}
              {text(exe) ? (
                <>
                  <K>Command (exe)</K>
                  <V mono>
                    <SafeText text={exe} />
                  </V>
                </>
              ) : null}
              {typeof seen === "number" ? (
                <>
                  <K>Seen</K>
                  <V mono>{formatTime(seen, { seconds: true })}</V>
                </>
              ) : null}
            </Rows>
          </Section>
        ) : null}

        <Section title="Chain">
          <Rows>
            <K>Status</K>
            <V>
              {broken !== undefined ? (
                <Chip tone="rejected" Icon={ShieldAlert} className="border-danger-fg bg-danger-fg text-background">
                  Chain break
                </Chip>
              ) : linked ? (
                <Chip tone="ok" Icon={Check}>
                  Linked to event {n(event.seq - 1)}
                </Chip>
              ) : (
                <Chip tone="neutral">Not compared: the previous event is not loaded</Chip>
              )}
            </V>
            <K>Chain hash</K>
            <V mono>{event.hash}</V>
            <K>Previous · {n(event.seq - 1)}</K>
            <V mono>
              <CopyValue value={event.prev_hash} what="previous hash" valueClassName={cn(broken !== undefined && "font-semibold text-danger-fg")} />
            </V>
            {broken === undefined ? null : (
              <>
                <K>Hash of {n(broken.previous)}</K>
                <V mono>
                  <CopyValue value={broken.actual} what={`hash of event ${n(broken.previous)}`} />
                </V>
              </>
            )}
          </Rows>
          <p className="text-xs text-muted-foreground">The chain hash is the SHA-256 of the previous chain hash and this event's record. Changing any old event changes every hash after it.</p>
        </Section>
      </div>
      <div className="flex flex-none items-center gap-2 border-t border-border px-4 py-2.5">
        <CopyButton value={event.hash} what="chain hash" showLabel />
      </div>
    </div>
  );
}

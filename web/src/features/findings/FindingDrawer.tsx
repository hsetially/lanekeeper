import { useRef, useState, type ReactNode } from "react";
import { Dialog as DialogPrimitive } from "radix-ui";
import type { Finding } from "@/api/generated/model";
import { CopyValue } from "@/components/app/CopyValue";
import { SafeText } from "@/components/app/SafeText";
import { Button } from "@/components/ui/button";
import { X } from "@/icons";
import { formatRelative, formatTime } from "@/lib/time";
import { AppLink } from "@/routes/AppLink";
import { useNow } from "@/live/use-now";
import { CodeChip } from "./cells";
import { KIND_INFO } from "./kinds";
import { lineLabel } from "./lines";
import { SeverityBadge } from "./SeverityBadge";

// The drawer: the evidence for one finding and what to do next, in the design's three parts (what this means, the evidence, the
// recommended next steps) and the actions that open the file. It is a non-modal dialog inside the page's box, as the design
// draws it: the table stays in view and a click on another row shows that finding, and Escape closes it.
//
// What it says about THIS finding is what the server sent (`Finding`): the check's own sentence, the file, the tenant and base
// file, when it was found, the rules that rated it, and the places in the file. What it says about the KIND (what it means, what
// to do) is words (./kinds.ts). It computes nothing: no diff, no commits, no hashes; the diff and the history are the file view's.
//
// The design's two actions on a finding, "Mark intentional" and "Restore lost version", are not here: the contract has no
// operation for either (CG6). The step that would use them points at the file's history, which has the version.

const FIRST_PLACES = 20;

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-1.5">
      <h3 className="flex items-center gap-2 text-xs font-medium text-muted-foreground after:h-px after:flex-1 after:bg-border">{title}</h3>
      {children}
    </section>
  );
}

function Row({ name, children }: { name: string; children: ReactNode }) {
  return (
    <div className="grid grid-cols-[112px_minmax(0,1fr)] items-baseline gap-3 border-b border-border px-3 py-2 last:border-b-0">
      <dt className="text-muted-foreground">{name}</dt>
      <dd className="m-0 flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">{children}</dd>
    </div>
  );
}

function Places({ finding }: { finding: Finding }) {
  const [all, setAll] = useState(false);
  const places = all ? finding.locations : finding.locations.slice(0, FIRST_PLACES);
  return (
    <>
      <ul className="m-0 flex list-none flex-col p-0">
        {places.map((place, i) => {
          const label = lineLabel(place.lines);
          return (
            // The place in the list is its identity: the same setting can be named at several lines, and the list never reorders.
            <li key={i} className="flex items-center justify-between gap-3 border-b border-border px-3 py-1.5 last:border-b-0">
              <SafeText text={place.path} className="min-w-0 truncate font-mono text-xs" />
              {finding.path === null ? (
                <span className="font-mono text-xs">{label}</span>
              ) : (
                <AppLink route="files" params={{ id: finding.swimlane }} search={{ path: finding.path, line: String(place.lines.start) }} aria-label={`Open at ${label}`} className="font-mono text-xs whitespace-nowrap">
                  {label}
                </AppLink>
              )}
            </li>
          );
        })}
      </ul>
      {all || finding.locations.length <= FIRST_PLACES ? null : (
        <Button
          variant="secondary"
          size="sm"
          className="self-start"
          onClick={() => {
            setAll(true);
          }}
        >
          Show all {finding.locations.length} places
        </Button>
      )}
    </>
  );
}

function Body({ finding }: { finding: Finding }) {
  const info = KIND_INFO[finding.kind];
  const now = useNow(true, 30_000);
  const first = finding.locations[0];
  const line: { line?: string } = first === undefined ? {} : { line: String(first.lines.start) };
  const baseFile = finding.logical_file ?? null;
  const tenant = finding.tenant ?? null;
  return (
    <>
      <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-x-hidden overflow-y-auto px-4 py-3.5">
        <p className="m-0 text-sm leading-5 font-medium text-pretty">{info.meaning}</p>
        <Section title="Evidence">
          <div className="rounded-sm border border-border">
            <dl className="m-0 flex flex-col">
              <Row name="The check says">
                <SafeText text={finding.message} />
              </Row>
              {tenant === null ? null : (
                <Row name="Tenant">
                  <SafeText text={tenant} className="font-mono text-xs" />
                </Row>
              )}
              {baseFile === null || baseFile === finding.path ? null : (
                <Row name="Base file">
                  <CopyValue value={baseFile} what="base file path" valueClassName="font-mono text-xs" />
                </Row>
              )}
              <Row name="Found">
                <span>{formatTime(finding.detected_at)}</span>
                <span className="text-muted-foreground">{formatRelative(finding.detected_at, now)}</span>
              </Row>
              {finding.rule_ids.length === 0 ? null : (
                <Row name="Rated by">
                  {finding.rule_ids.map((id) => (
                    <SafeText key={id} text={id} className="rounded-sm border border-border px-1 font-mono text-xs" />
                  ))}
                </Row>
              )}
            </dl>
          </div>
          {finding.locations.length === 0 ? null : <Places finding={finding} />}
        </Section>
        <Section title="Recommended next steps">
          <ol className="m-0 flex list-none flex-col gap-1.5 p-0 [counter-reset:step]">
            {info.steps.map((step) => (
              <li key={step} className="grid grid-cols-[20px_minmax(0,1fr)] gap-2 text-pretty [counter-increment:step] before:flex before:size-5 before:items-center before:justify-center before:rounded-full before:border before:border-input before:text-xs before:font-semibold before:tabular-nums before:content-[counter(step)] first:before:border-foreground first:before:bg-foreground first:before:text-background">
                <span>{step}</span>
              </li>
            ))}
          </ol>
        </Section>
      </div>
      <div className="flex flex-none flex-wrap items-center gap-2 border-t border-border px-4 pt-2.5 pb-3">
        {finding.path === null ? (
          <p className="m-0 text-xs text-muted-foreground">This finding is about the swimlane, not about one file.</p>
        ) : (
          <>
            <Button asChild>
              <AppLink route="files" params={{ id: finding.swimlane }} search={{ path: finding.path, ...line }}>
                Open file
              </AppLink>
            </Button>
            {info.opens.includes("diff") ? (
              <Button asChild variant="secondary">
                <AppLink route="files" params={{ id: finding.swimlane }} search={{ path: finding.path, tab: "diff", diff: "git" }}>
                  Open diff
                </AppLink>
              </Button>
            ) : null}
            {info.opens.includes("history") ? (
              <Button asChild variant="secondary">
                <AppLink route="files" params={{ id: finding.swimlane }} search={{ path: finding.path, panel: "history", ...line }}>
                  Open history
                </AppLink>
              </Button>
            ) : null}
          </>
        )}
      </div>
    </>
  );
}

type DrawerProps = {
  /** The finding to show, or `null` for one that is no longer in the list (it was resolved, or it changed). */
  finding: Finding | null;
  onClose: () => void;
  /** Where the keyboard goes when the drawer closes. */
  onCloseFocus: () => void;
};

export function FindingDrawer({ finding, onClose, onCloseFocus }: DrawerProps) {
  const info = finding === null ? null : KIND_INFO[finding.kind];
  const close = useRef<HTMLButtonElement>(null);
  return (
    <DialogPrimitive.Root
      open
      modal={false}
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <DialogPrimitive.Content
        aria-describedby={undefined}
        // The table beside it stays in use: a click on another row shows that finding, and a menu above it is not "outside".
        onInteractOutside={(event) => {
          event.preventDefault();
        }}
        // Focus starts on Close, as before the path got a copy button: a button that appears on hover is never where a drawer opens.
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          close.current?.focus();
        }}
        onCloseAutoFocus={(event) => {
          event.preventDefault();
          onCloseFocus();
        }}
        className="absolute inset-y-0 right-0 z-20 flex w-[540px] max-w-full flex-col border-l border-border bg-popover text-popover-foreground shadow-popover"
      >
        <div className="flex flex-none items-start gap-2 border-b border-border py-3 pr-3 pl-4">
          <div className="flex min-w-0 flex-1 flex-col gap-1.5">
            {finding === null || info === null ? null : (
              <div className="flex flex-wrap items-center gap-1.5">
                <SeverityBadge severity={finding.severity} long />
                <CodeChip code={info.code} />
              </div>
            )}
            <DialogPrimitive.Title className="m-0 text-base leading-[22px] font-semibold tracking-[-0.01em]">{info === null ? "This finding is no longer open" : info.name}</DialogPrimitive.Title>
            {finding === null ? null : (
              <div className="flex flex-wrap items-center gap-x-2 gap-y-0.5 font-mono text-xs text-muted-foreground">
                <SafeText text={finding.swimlane} className="text-foreground" />
                {finding.path === null ? null : <CopyValue value={finding.path} what="path" valueClassName="break-all" />}
              </div>
            )}
          </div>
          <DialogPrimitive.Close asChild>
            <Button ref={close} variant="ghost" size="icon-sm" aria-label="Close">
              <X />
            </Button>
          </DialogPrimitive.Close>
        </div>
        {finding === null ? (
          <p className="m-0 px-4 py-4 text-sm text-muted-foreground">The check no longer reports it: the file changed, or someone fixed it. Close this to go back to the list.</p>
        ) : (
          <Body key={finding.id} finding={finding} />
        )}
      </DialogPrimitive.Content>
    </DialogPrimitive.Root>
  );
}

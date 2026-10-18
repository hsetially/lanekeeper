import type { FileFormat, ValidationIssue } from "@/api/generated/model";
import { GatedButton } from "@/components/app/GatedButton";
import { SafeText } from "@/components/app/SafeText";
import type { GateResult } from "@/components/app/write-gate";
import { Button } from "@/components/ui/button";
import { CircleX, Hourglass, ShieldAlert, TriangleAlert, Upload } from "@/icons";
import { languageOf, type Language, eolLabel, encodingLabel, lineCount } from "../model";
import { EditSurface, type EditorMarker } from "./EditSurface";
import { issueLine } from "./outcome";

// Edit mode (File, 1g to 1i): the editor, a Problems panel when the server refused the content, the status bar and the bar with
// Cancel and Save. There is no validation in the browser: the engine validates on save, and what it found comes back as
// lines and rule names (never the text of a line), which are marked in the editor and listed here in the app's own words.
// Save is never silent: it asks the page to open the review.

export type EditPaneProps = {
  name: string;
  lane: string;
  /** The text the editor starts with. */
  initialText: string;
  /** The text now, for the line count. */
  text: string;
  format: FileFormat;
  dirty: boolean;
  /** What the server found the last time it refused the content. Cleared by the page as soon as the person types. */
  issues: readonly ValidationIssue[];
  /** The person's writes need an Admin's approval: Save becomes "Submit for approval". */
  approval: boolean;
  /** Who may save and when (the role, the live stream, the agent). */
  saveGate: GateResult;
  /** The hash this edit was based on before the person chose to re-apply on top of a newer version, or null. */
  rebasedFrom: string | null;
  onText: (text: string) => void;
  onCancel: () => void;
  onSave: () => void;
  load?: () => Promise<import("@/editor/loader").MonacoApi>;
};

const LANGUAGE: Record<Language, string> = { yaml: "YAML", xml: "XML", ini: "Properties", json: "JSON", plaintext: "Plain text" };

function Problems({ issues }: { issues: readonly ValidationIssue[] }) {
  const lines = issues.map(issueLine).sort((a, b) => (a.severity === b.severity ? 0 : a.severity === "error" ? -1 : 1));
  return (
    <div role="region" aria-label="Problems" className="flex-none border-t border-border bg-muted">
      <div className="flex h-7 items-center gap-2 border-b border-border px-3 font-semibold">
        Problems
        <span className="inline-flex min-w-[18px] items-center justify-center rounded-sm border border-border bg-background px-1.5 text-xs font-normal text-muted-foreground">{lines.length}</span>
      </div>
      <ul className="m-0 list-none p-0">
        {lines.map((line, i) => {
          const Icon = line.severity === "error" ? CircleX : TriangleAlert;
          return (
            <li key={i} className="flex h-7 items-center gap-2 border-b border-border px-3">
              <Icon aria-hidden="true" className={line.severity === "error" ? "size-3.5 shrink-0 text-danger-fg" : "size-3.5 shrink-0 text-warning-fg"} />
              <span className="font-medium">{line.severity === "error" ? "Error" : "Warning"}</span>
              <span className="text-muted-foreground">
                <SafeText text={line.message} />
              </span>
              {line.line === null ? null : <span className="ml-auto font-mono text-xs text-muted-foreground">L{line.line}</span>}
            </li>
          );
        })}
      </ul>
    </div>
  );
}

export function EditPane({ name, lane, initialText, text, format, dirty, issues, approval, saveGate, rebasedFrom, onText, onCancel, onSave, load }: EditPaneProps) {
  const language = languageOf(name);
  const markers: EditorMarker[] = issues.map((issue) => {
    const line = issueLine(issue);
    return { line: line.line ?? 1, severity: line.severity, message: line.message };
  });
  const secret = issues.find((issue) => issue.kind === "secret_match");
  const gate: GateResult = !saveGate.allowed ? saveGate : dirty ? saveGate : { allowed: false, reason: "No changes to save" };
  const lines = lineCount(text);
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <EditSurface label={name} initialText={initialText} language={language} markers={markers} onTextChange={onText} {...(load ? { load } : {})} />
      {issues.length > 0 ? <Problems issues={issues} /> : null}
      <div role="group" aria-label="File format" className="flex h-6 flex-none items-center gap-4 border-t border-border bg-muted px-3 font-mono text-xs text-muted-foreground">
        <span className="font-medium text-foreground">{eolLabel(format.eol)}</span>
        <span className="font-medium text-foreground">{encodingLabel(format.encoding, format.bom)}</span>
        <span>{LANGUAGE[language]}</span>
        <span>
          {lines.toLocaleString("en-US")} {lines === 1 ? "line" : "lines"}
        </span>
      </div>
      <div className="flex flex-none flex-col border-t border-border bg-background">
        {secret ? (
          <div role="alert" className="flex items-center gap-2 border-b border-danger-border bg-danger-bg px-4 py-2 text-danger-fg">
            <ShieldAlert aria-hidden="true" className="size-3.5 shrink-0" />
            <strong className="font-semibold">
              Save blocked: line {secret.line ?? "?"} looks like a secret{secret.rule === undefined ? "" : ` (${secret.rule})`}
            </strong>
            <span className="text-foreground">Remove it and keep secrets in the secret store. Lanekeeper never shows the flagged text.</span>
          </div>
        ) : null}
        <div className="flex h-[52px] items-center gap-3 px-4">
          <span>{dirty ? "Unsaved changes" : "No changes yet"}</span>
          {approval ? (
            <span className="inline-flex h-5 items-center gap-1 rounded-sm border border-status-approval-border bg-status-approval-bg px-1.5 text-xs font-medium text-status-approval-fg">
              <Hourglass aria-hidden="true" className="size-3" />
              Needs approval from an Admin
            </span>
          ) : null}
          {rebasedFrom === null ? null : <span className="text-muted-foreground">Based on the version that is on NFS now. Saving replaces it with yours.</span>}
          <Button variant="ghost" className="ml-auto" onClick={onCancel}>
            Cancel
          </Button>
          <GatedButton gate={gate} onClick={onSave}>
            {approval ? <Hourglass aria-hidden="true" /> : <Upload aria-hidden="true" />}
            {approval ? "Submit for approval" : `Save to ${lane} NFS`}
          </GatedButton>
        </div>
      </div>
    </div>
  );
}

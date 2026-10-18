import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState, type ChangeEvent, type DragEvent } from "react";
import type { DocSummary } from "@/api/generated/model";
import { ConsequenceDialog } from "@/components/app/ConsequenceDialog";
import { SafeText } from "@/components/app/SafeText";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Field, FieldControl, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Upload } from "@/icons";
import { formatRelative } from "@/lib/time";
import { cn } from "@/lib/utils";
import { docWriteError } from "./errors";
import { docMutations } from "./mutations";
import { formatBytes, readUpload, type UploadReading } from "./upload";

// Upload a doc, or replace an uploaded one (Editor and above; the design's frames 1p to 1t). A drop zone for one Markdown file,
// the three states it can be in, the consequences before the button, and a write that is never optimistic: the dialog stays
// open on a failure, with the file and the title the person chose, and says what happened with the request id.
//
// A replace carries the version it was made against (`expected_version`); a 409 means someone saved first.

export type UploadMode = { kind: "create" } | { kind: "replace"; doc: DocSummary };

type Props = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  mode: UploadMode;
  /** The doc as the server saved it. Called after the dialog's own state is settled, before it closes. */
  onDone?: (doc: DocSummary) => void;
};

const MAX_TITLE = 256;

export function UploadDocDialog({ open, onOpenChange, mode, onDone }: Props) {
  const client = useQueryClient();
  const create = useMutation(docMutations.create(client));
  const update = useMutation(docMutations.update(client));
  const replacing = mode.kind === "replace";
  const write = replacing ? update : create;
  const [reading, setReading] = useState<UploadReading | null>(null);
  const [title, setTitle] = useState("");
  const [over, setOver] = useState(false);
  const input = useRef<HTMLInputElement>(null);
  // The latest file chosen, so that a slow read of an earlier one cannot replace it.
  const latest = useRef(0);

  const { reset: resetCreate } = create;
  const { reset: resetUpdate } = update;
  useEffect(() => {
    if (open) return;
    latest.current += 1;
    setReading(null);
    setTitle("");
    setOver(false);
    resetCreate();
    resetUpdate();
  }, [open, resetCreate, resetUpdate]);

  async function choose(file: File): Promise<void> {
    const mine = ++latest.current;
    const next = await readUpload(file);
    if (mine !== latest.current) return;
    write.reset();
    setReading(next);
    if (next.ok && !replacing) setTitle(next.title);
  }

  function onPick(event: ChangeEvent<HTMLInputElement>): void {
    const file = event.target.files?.[0];
    event.target.value = "";
    if (file) void choose(file);
  }

  function onDrop(event: DragEvent<HTMLDivElement>): void {
    event.preventDefault();
    setOver(false);
    const file = event.dataTransfer.files[0];
    if (file) void choose(file);
  }

  const trimmed = title.trim();
  const ready = reading?.ok === true && (replacing || trimmed !== "");
  const working = write.isPending;

  function confirm(): void {
    if (reading?.ok !== true || !ready) return;
    const done = (doc: DocSummary): void => {
      onDone?.(doc);
      onOpenChange(false);
    };
    if (mode.kind === "replace") update.mutate({ id: mode.doc.id, body: { markdown: reading.markdown, expected_version: mode.doc.version } }, { onSuccess: done });
    else create.mutate({ title: trimmed, markdown: reading.markdown }, { onSuccess: done });
  }

  const error = write.error;
  const verb = replacing ? "replace" : "upload";
  const consequence = replacing ? `Replaces the current text of this doc, version ${mode.doc.version}, saved ${formatRelative(mode.doc.updated_at, Date.now())}.` : "Adds a new doc to the docs. Everyone who can open Docs can read it.";

  return (
    <ConsequenceDialog
      initialFocus="first-field"
      open={open}
      onOpenChange={onOpenChange}
      title={replacing ? `Replace “${mode.doc.title}”` : "Upload a doc"}
      consequence={consequence}
      actionLabel={replacing ? "Replace doc" : "Upload doc"}
      workingLabel={replacing ? "Replacing…" : "Uploading…"}
      onConfirm={confirm}
      working={working}
      canConfirm={ready}
      error={error ? docWriteError(error, "save", { replacing }) : null}
    >
      <div
        data-testid="drop-zone"
        onDragOver={(event) => {
          event.preventDefault();
          setOver(true);
        }}
        onDragLeave={() => setOver(false)}
        onDrop={onDrop}
        className={cn("flex flex-col items-center gap-1.5 rounded-md border border-dashed border-input bg-muted text-center", reading ? "p-3" : "p-7", over && "border-ring bg-selection")}
      >
        <Upload aria-hidden="true" className="size-5 text-muted-foreground" />
        <strong className="font-semibold">{reading ? "Drop a different file" : "Drop a Markdown file here"}</strong>
        <span className="text-xs text-muted-foreground">Markdown (.md) only, up to 2 MB</span>
        <Button type="button" variant="secondary" disabled={working} onClick={() => input.current?.click()} className="mt-1">
          Choose a file
        </Button>
        <input ref={input} type="file" accept=".md,text/markdown" tabIndex={-1} aria-hidden="true" onChange={onPick} className="sr-only" />
      </div>

      {reading ? <FileStatus reading={reading} verb={verb} /> : null}

      {reading?.ok === true && !replacing ? (
        <Field>
          <FieldLabel>Title</FieldLabel>
          <FieldControl>
            <Input value={title} maxLength={MAX_TITLE} disabled={working} onChange={(event) => setTitle(event.target.value)} autoComplete="off" />
          </FieldControl>
        </Field>
      ) : null}

      {reading?.ok === true ? <Consequences replacing={replacing} version={replacing ? mode.doc.version : 0} /> : null}
    </ConsequenceDialog>
  );
}

function FileStatus({ reading, verb }: { reading: UploadReading; verb: string }) {
  const head = (
    <AlertTitle className="flex items-center gap-2">
      <span className="font-mono text-xs">
        <SafeText text={reading.name} />
      </span>
      <span className="ml-auto text-xs font-normal tabular-nums">{formatBytes(reading.size)}</span>
    </AlertTitle>
  );
  if (!reading.ok) {
    return (
      <Alert variant="danger">
        {head}
        <AlertDescription>
          <strong className="font-semibold">{reading.head}</strong> {reading.message}
        </AlertDescription>
      </Alert>
    );
  }
  return (
    <Alert variant="success">
      {head}
      <AlertDescription>
        <strong className="font-semibold">{`Ready to ${verb}.`}</strong> Read as UTF-8 text.
      </AlertDescription>
    </Alert>
  );
}

function Consequences({ replacing, version }: { replacing: boolean; version: number }) {
  return (
    <ul className="m-0 flex list-disc flex-col gap-0.5 pl-5 text-sm text-muted-foreground">
      {replacing ? (
        <>
          <li>
            Saves as version <span className="font-mono text-xs text-foreground">{version + 1}</span>.
          </li>
          <li>Everyone who opens the doc after this will see the new text.</li>
          <li>The doc’s headings are read again from the new text, so a link to a heading may stop working if you rename or move it.</li>
        </>
      ) : (
        <>
          <li>The doc is saved as it is in the file. Its headings are read from the file.</li>
          <li>You can replace it later from the reader.</li>
        </>
      )}
    </ul>
  );
}

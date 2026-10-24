import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import type { RetentionInput, RetentionPreview, RetentionSettings } from "@/api/generated/model";
import { ConsequenceDialog } from "@/components/app/ConsequenceDialog";
import { DestructiveConfirm } from "@/components/app/DestructiveConfirm";
import { GatedButton } from "@/components/app/GatedButton";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { SafeText } from "@/components/app/SafeText";
import { WriteError, type WriteErrorInfo } from "@/components/app/WriteError";
import { useWriteGate, type GateResult } from "@/components/app/write-gate";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Field, FieldControl, FieldError, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { Spinner } from "@/components/ui/spinner";
import { adminMutations } from "@/features/admin/mutations";
import { adminQueries } from "@/features/admin/queries";
import { SavedNotice } from "@/features/admin/rules";
import { adminWriteError } from "@/features/admin/write-error";
import { TriangleAlert } from "@/icons";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { useQueryState } from "@/lib/query/use-query-state";
import { formatTime } from "@/lib/time";
import { cn } from "@/lib/utils";
import { CONFIRM_PHRASE, formatBytes, formatCount, parseRetention, policySentence, sameValues, wouldDelete, type RetentionErrors } from "./retention-model";

// Retention: how long stored file versions are kept. THE RETENTION JOB DELETES (D118): a version that the policy does not keep, and
// that nothing refers to, is removed for good, and Lanekeeper cannot restore it. So this page says that first; asks the hub what a
// policy would delete (the dry run, `PUT ?dry_run=true`, which changes nothing) and makes the person look at the answer before the
// policy can be saved; and for a policy that would delete, asks for words to be typed. What the API does not give is not drawn: the
// job's own dry-run switch (a setting of the hub that the API does not report), the size of a run, the time of the next purge and the
// history of purges (design/DEVIATIONS.md). Nothing here estimates what a policy deletes: that is the hub's answer.

const CONFLICT = "Someone else changed the policy at the same time. Reload the page, check the policy and try again.";
const DRY_MESSAGES: Readonly<Partial<Record<number, string>>> = {
  0: "The hub could not be reached. Nothing was changed; try again.",
  403: "The hub refused the dry run. Your role may have changed since this page loaded; reload the page and try again.",
  409: CONFLICT,
  422: "The hub did not accept these values for a dry run. Check them and try again.",
  429: "Too many requests in a short time. Wait a moment and try again.",
  500: "The hub could not run the dry run. Nothing was changed; try again.",
  502: "The hub could not run the dry run. Nothing was changed; try again.",
  503: "The hub could not run the dry run. Nothing was changed; try again.",
  504: "The hub could not run the dry run. Nothing was changed; try again.",
};

type Shown = { readonly kind: "none" } | { readonly kind: "missing"; readonly values: RetentionInput } | { readonly kind: "ok"; readonly values: RetentionInput; readonly result: RetentionPreview };

const valuesOf = (settings: RetentionSettings): RetentionInput => ({ keep_days: settings.keep_days, keep_last_versions: settings.keep_last_versions });
const policyLabel = (values: RetentionInput): string => `${values.keep_days} days, last ${values.keep_last_versions} versions`;

function DeletesNotice() {
  return (
    <div className="flex flex-col gap-2">
      <Alert variant="warning">
        <AlertTitle>Retention permanently deletes stored versions</AlertTitle>
        <AlertDescription>
          The retention job deletes by default: it removes the stored versions that the policy below does not keep, and a deleted version cannot be restored. A dry run only shows what a policy would delete. Baselines and versions referenced by audit events, proposals, drafts or PR links are always kept (D78).
        </AlertDescription>
      </Alert>
      <p className="max-w-[680px] text-xs text-muted-foreground">
        The hub's API does not report whether this hub's retention job is deleting or only counting: that is a setting of the hub, not of this page. Read every policy here as one that deletes. The job removes versions in bounded batches, so a large backlog can take several runs.
      </p>
    </div>
  );
}

function Stat({ term, value, tone }: { term: string; value: string; tone?: "warn" }) {
  return (
    <div className="flex flex-col gap-0.5">
      <dt className="text-xs text-muted-foreground">{term}</dt>
      <dd className={cn("m-0 font-mono text-lg font-semibold tabular-nums [overflow-wrap:anywhere]", tone === "warn" && "text-warning-fg")}>{value}</dd>
    </div>
  );
}

function DryRunPanel({ shown, savedPreview, saved, running, stale, error }: { shown: Shown; savedPreview: RetentionPreview | null; saved: RetentionInput; running: boolean; stale: boolean; error: WriteErrorInfo | null }) {
  const draft = shown.kind === "ok" ? shown : null;
  const result = draft?.result ?? savedPreview;
  const heading = draft === null ? `The saved policy: ${policyLabel(saved)}` : `This policy: ${policyLabel(draft.values)}`;
  return (
    <section aria-label="Dry run" className="flex flex-col gap-3 rounded-md border border-border bg-muted p-3">
      <h4 className="text-sm font-semibold">Dry run · nothing is deleted</h4>
      {running ? (
        <p role="status" className="flex items-center gap-2 text-sm text-muted-foreground">
          <Spinner />
          Running the dry run…
        </p>
      ) : null}
      {error === null ? null : <WriteError {...error} />}
      {shown.kind === "missing" ? (
        <Alert variant="warning">
          <AlertTitle>The hub sent no preview for this policy</AlertTitle>
          <AlertDescription>Run the dry run again. A policy cannot be saved without one.</AlertDescription>
        </Alert>
      ) : null}
      {result === null ? (
        <p className="text-sm text-muted-foreground">Run the dry run to see what a policy would delete.</p>
      ) : (
        <>
          <p className="text-xs text-muted-foreground">{heading}</p>
          {/* As many columns as the room allows, each at least 9rem: a number of ten or more digits does not fit a third of a narrow panel. */}
          <dl className="m-0 grid grid-cols-[repeat(auto-fit,minmax(9rem,1fr))] gap-3">
            <Stat term="Versions that would be deleted" value={formatCount(result.versions_to_delete)} {...(result.versions_to_delete > 0 ? { tone: "warn" as const } : {})} />
            <Stat term="Space that would be freed" value={formatBytes(result.bytes_to_free)} />
            <Stat term="Versions kept because something refers to them" value={formatCount(result.versions_kept_by_reference)} />
          </dl>
        </>
      )}
      {stale ? <p className="text-xs text-warning-fg">You changed the values after this dry run. Run it again before you save.</p> : null}
    </section>
  );
}

function RetentionBody({ saved }: { saved: RetentionSettings }) {
  const client = useQueryClient();
  const baseGate = useWriteGate("admin");
  const dry = useMutation(adminMutations.previewRetention());
  const save = useMutation(adminMutations.putRetention(client));
  const { reset: resetDry } = dry;
  const { reset: resetSave } = save;
  const users = useCursorPages({ ...adminQueries.users(), autoLoad: true });
  const savedValues = valuesOf(saved);
  const [draft, setDraft] = useState<{ days: string; last: string } | null>(null);
  const [shown, setShown] = useState<Shown>({ kind: "none" });
  const [errors, setErrors] = useState<RetentionErrors>({});
  const [confirming, setConfirming] = useState(false);
  const [notice, setNotice] = useState(false);

  const days = draft?.days ?? String(saved.keep_days);
  const last = draft?.last ?? String(saved.keep_last_versions);
  const parsed = parseRetention(days, last);
  const changed = draft !== null && (!parsed.ok || !sameValues(parsed.value, savedValues));
  const fresh = shown.kind === "ok" && parsed.ok && sameValues(shown.values, parsed.value);
  const stale = shown.kind === "ok" && !fresh;
  const author = useMemo(() => users.items.find((u) => u.id === saved.updated_by), [users.items, saved.updated_by]);

  const edit = (next: { days?: string; last?: string }): void => {
    setNotice(false);
    setErrors({});
    setDraft({ days, last, ...next });
  };

  const run = (): void => {
    if (!parsed.ok) {
      setErrors(parsed.errors);
      return;
    }
    const values = parsed.value;
    setErrors({});
    setNotice(false);
    resetDry();
    dry.mutate(values, { onSuccess: (answer) => setShown(answer.dry_run_preview === null ? { kind: "missing", values } : { kind: "ok", values, result: answer.dry_run_preview }) });
  };

  const revert = (): void => {
    setDraft(null);
    setShown({ kind: "none" });
    setErrors({});
    resetDry();
  };

  const reason = !parsed.ok ? "Fix the values first" : !changed ? "Nothing has changed" : !fresh ? "Run the dry run for these values first" : null;
  const saveGate: GateResult = !baseGate.allowed || reason === null ? baseGate : { allowed: false, reason };

  const confirm = (): void => {
    if (shown.kind !== "ok") return;
    save.mutate(
      { body: shown.values },
      {
        onSuccess: () => {
          setConfirming(false);
          setDraft(null);
          setShown({ kind: "none" });
          setNotice(true);
        },
      },
    );
  };
  const close = (): void => {
    resetSave();
    setConfirming(false);
  };
  const saveError = save.error === null ? null : adminWriteError(save.error, "The policy could not be saved", { 409: CONFLICT });
  const result = shown.kind === "ok" ? shown.result : null;

  return (
    <div className="flex flex-col gap-4">
      {notice ? (
        <SavedNotice
          title="Retention policy saved"
          onDismiss={() => {
            setNotice(false);
          }}
        >
          The retention job uses it from its next run.
        </SavedNotice>
      ) : null}
      <section aria-label="Current policy" className="text-sm">
        <p>
          {saved.updated_at === null ? "Never changed: the defaults apply. " : ""}Keeps {policySentence(savedValues)}.
          {saved.updated_at === null ? null : (
            <>
              {" "}
              Last changed {formatTime(saved.updated_at)}
              {saved.updated_by === null ? null : (
                <>
                  {" by "}
                  {author === undefined ? <span>a user</span> : <SafeText text={author.display_name} />}
                </>
              )}
              .
            </>
          )}
        </p>
      </section>

      <div className="grid grid-cols-[minmax(0,1fr)_minmax(0,1.1fr)] gap-4 rounded-md border border-border bg-card p-4 max-md:grid-cols-1">
        <div className="flex flex-col gap-3">
          <h3 className="text-sm font-semibold">Versions kept</h3>
          <Field invalid={errors.days !== undefined}>
            <FieldLabel>Days to keep versions</FieldLabel>
            <FieldControl>
              <Input
                value={days}
                inputMode="numeric"
                autoComplete="off"
                spellCheck={false}
                className="font-mono"
                onChange={(event) => {
                  edit({ days: event.target.value });
                }}
              />
            </FieldControl>
            <FieldError>{errors.days}</FieldError>
          </Field>
          <Field invalid={errors.last !== undefined}>
            <FieldLabel>Versions to always keep per file</FieldLabel>
            <FieldControl>
              <Input
                value={last}
                inputMode="numeric"
                autoComplete="off"
                spellCheck={false}
                className="font-mono"
                onChange={(event) => {
                  edit({ last: event.target.value });
                }}
              />
            </FieldControl>
            <FieldError>{errors.last}</FieldError>
          </Field>
          <p className="text-xs text-muted-foreground">
            A version is deleted only when it is older than the day limit and is not one of the last versions of its file. Baselines and versions referenced by audit events, proposals, drafts or PR links are always kept.
          </p>
        </div>
        <DryRunPanel
          shown={shown}
          savedPreview={saved.dry_run_preview}
          saved={savedValues}
          running={dry.isPending}
          stale={stale}
          error={dry.error === null ? null : adminWriteError(dry.error, "The dry run could not be run", DRY_MESSAGES)}
        />
        <div className="col-span-full flex flex-wrap items-center justify-end gap-2">
          <Button variant="ghost" disabled={draft === null} onClick={revert}>
            Revert
          </Button>
          <GatedButton minRole="admin" variant="secondary" onClick={run}>
            Run dry run
          </GatedButton>
          <GatedButton
            gate={saveGate}
            onClick={() => {
              resetSave();
              setConfirming(true);
            }}
          >
            Save policy
          </GatedButton>
        </div>
      </div>

      {result !== null && shown.kind === "ok" && wouldDelete(result) ? (
        <DestructiveConfirm
          open={confirming}
          onOpenChange={(open) => {
            if (!open) close();
          }}
          title="Save a policy that deletes versions"
          consequence={`This policy keeps ${policySentence(shown.values)}. The dry run found ${formatCount(result.versions_to_delete)} versions to delete, freeing about ${formatBytes(result.bytes_to_free)}. A deleted version cannot be restored.`}
          confirmValue={CONFIRM_PHRASE}
          actionLabel="Save and delete"
          workingLabel="Saving…"
          titleIcon={<TriangleAlert />}
          showMatchHint
          working={save.isPending}
          error={saveError}
          onConfirm={confirm}
        >
          <p className="text-xs text-muted-foreground">The retention job removes them in bounded batches from its next run, so a large number can take several runs. Versions that something refers to are kept.</p>
        </DestructiveConfirm>
      ) : null}
      {result !== null && shown.kind === "ok" && !wouldDelete(result) ? (
        <ConsequenceDialog
          open={confirming}
          onOpenChange={(open) => {
            if (!open) close();
          }}
          title="Save the retention policy"
          consequence={`The retention job will keep ${policySentence(shown.values)}. The dry run found nothing that this policy would delete now.`}
          actionLabel="Save policy"
          workingLabel="Saving…"
          working={save.isPending}
          error={saveError}
          onConfirm={confirm}
        />
      ) : null}
    </div>
  );
}

/** The shape of the form and of the dry run beside it, so that nothing moves when the policy arrives. */
function RetentionSkeleton() {
  return (
    <div data-slot="retention-loading" className="grid grid-cols-[minmax(0,1fr)_minmax(0,1.1fr)] gap-4 rounded-md border border-border bg-card p-4 max-md:grid-cols-1">
      <div className="flex flex-col gap-3">
        <Skeleton className="h-4 w-28" />
        <Skeleton className="h-7 w-full" />
        <Skeleton className="h-7 w-full" />
        <Skeleton className="h-8 w-3/4" />
      </div>
      <Skeleton className="h-32 w-full" />
    </div>
  );
}

export function RetentionSection() {
  const query = useQuery(adminQueries.retention());
  const state = useQueryState(query);
  return (
    <div className="flex flex-col gap-4">
      <DeletesNotice />
      <QueryBoundaryView state={state} noun="the retention policy" source="the retention policy" loading={<RetentionSkeleton />}>
        {(saved) => <RetentionBody saved={saved} />}
      </QueryBoundaryView>
    </div>
  );
}

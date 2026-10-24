import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo, useState } from "react";
import type { FileEntry, PrJob, PrLink, PrTarget } from "@/api/generated/model";
import { ApiProblem } from "@/api/problem";
import { SafeText } from "@/components/app/SafeText";
import { WriteError, type WriteErrorInfo } from "@/components/app/WriteError";
import { DuplicatePr, OpenPrButton } from "@/components/pr/DuplicatePr";
import { PrJobProgress } from "@/components/pr/PrJobProgress";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { swimlaneListQuery } from "@/features/swimlanes/queries";
import { duplicatePrOf } from "@/features/prs/duplicate";
import { prJobQuery } from "@/features/prs/queries";
import { prMutations } from "@/features/prs/mutations";
import { BranchChip } from "@/components/app/chips";
import { GitPullRequest } from "@/icons";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { cn } from "@/lib/utils";
import { fromProblem, type WriteOutcome } from "../edit/outcome";
import { effectiveStem, splitPath } from "../model";
import { MyTokenLink, PushStep, TokenStep, doneAction } from "./JobStep";
import { defaultBody, defaultTitle, tokenTrouble } from "./model";
import { TargetStep, type Target } from "./TargetStep";
import { usePrPreview } from "./usePrPreview";

// Raise a pull request (Consequence dialogs, group 1). Step 1 chooses where it goes, with the repository, the branch and the path
// each target writes taken from the server's preview before anything exists; step 2 is the title and the description; then the
// job, which is the server's to report (the shared job view, kept fresh by `pr_job_progress`): nothing here says "created" before
// the job is completed. A second PR for a change that has an open one is refused by the server and shown as the shared card. GitHub's refusals have their own views: an expired or unusable
// token (a link to My token), and no push access (the other target).
//
// Not kept across a reload: the job runs on the server and is not lost, but the design's "resumed" job needs a place to keep
// its id that the app is not allowed (no web storage outside the preferences).

export type PrDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  lane: string;
  path: string;
  entry: FileEntry | null;
  /** The swimlane's tenant set. */
  tenants: readonly string[];
  /** The hash of the file on NFS now. */
  hash: string;
  /** An answer that is a dialog of the page's own: sent for approval, or locked. */
  onSettled: (outcome: WriteOutcome) => void;
  /** GitHub said the token expired: Git-dependent actions stay off until a new one is saved (W32). */
  onTokenExpired: () => void;
};

type View =
  | { kind: "target" }
  | { kind: "form" }
  | { kind: "exists"; link: PrLink }
  | { kind: "job"; job: PrJob }
  | { kind: "err-token"; why: "expired" | "unusable"; requestId: string | undefined }
  | { kind: "err-push"; requestId: string | undefined; alternative: Target | null };

/** The dialog's title for each view. A function and not a table, so that no module-level value is typed with the API's types. */
function titleOf(kind: View["kind"]): string {
  switch (kind) {
    case "target":
    case "form":
    case "exists":
      return "Raise pull request";
    case "job":
      return "Creating pull request";
    case "err-token":
    case "err-push":
      return "Pull request not created";
  }
}

export function PrDialog({ open, onOpenChange, lane, path, entry, tenants, hash, onSettled, onTokenExpired }: PrDialogProps) {
  const client = useQueryClient();
  const fileTenant = entry?.tenant ?? null;
  const branches = useMemo(() => (fileTenant !== null ? [fileTenant] : [...tenants]), [fileTenant, tenants]);
  const [branch, setBranch] = useState<string | null>(branches[0] ?? null);
  const [target, setTarget] = useState<Target>(fileTenant !== null ? "tenant" : "base");
  const [view, setView] = useState<View>({ kind: "target" });
  const [title, setTitle] = useState<string | null>(null);
  const [body, setBody] = useState<string | null>(null);
  const [error, setError] = useState<WriteErrorInfo | null>(null);
  const create = useMutation(prMutations.create(client));
  const { reset } = create;

  // Closing forgets everything the dialog held. `reset` is the one stable member of the mutation (the object itself is new on every
  // render), and the view is only replaced when it is not already the first one, so a closed dialog settles and stays settled.
  useEffect(() => {
    if (open) return;
    setView((current) => (current.kind === "target" ? current : { kind: "target" }));
    setError(null);
    setTitle(null);
    setBody(null);
    reset();
  }, [open, reset]);

  // The target that is previewed for each card. A branch the swimlane does not have (a stale choice) is not previewed.
  const tenantTarget: PrTarget | null = branch !== null && branches.includes(branch) ? { kind: "tenant", branch } : null;
  const tenantPreview = usePrPreview(open, lane, path, tenantTarget);
  const basePreview = usePrPreview(open, lane, path, { kind: "base" });
  const selected = target === "tenant" ? tenantPreview : basePreview;
  const preview = selected.status === "ready" ? selected.preview : null;
  const file = preview?.files.find((f) => f.nfs_path === path) ?? preview?.files[0];
  const nothing = preview !== null && preview.files.length === 0 && preview.unchanged.includes(path);
  const fresh = tenantPreview.status === "ready" && tenantPreview.preview.files.some((f) => !f.exists_on_branch) && tenantPreview.preview.files.length > 0;

  const lanes = useCursorPages({ ...swimlaneListQuery(), autoLoad: true, enabled: open });
  const affected = useMemo(() => (branch === null ? [] : lanes.items.filter((s) => s.tenants.includes(branch)).map((s) => s.id)), [lanes.items, branch]);

  const stem = effectiveStem(entry?.logical_file ?? path);
  const defaults = useMemo(
    () => ({
      title: defaultTitle({ target, stem, branch: target === "tenant" ? branch : fileTenant, lane }),
      body: preview === null ? "" : defaultBody({ lane, path, repo: preview.repo, base: preview.base_branch, writes: file?.repo_path ?? "", hash }),
    }),
    [target, stem, branch, fileTenant, lane, preview, file?.repo_path, path, hash],
  );
  const titleValue = title ?? defaults.title;
  const bodyValue = body ?? defaults.body;

  const openJob = view.kind === "job" ? view.job : null;
  const live = useQuery({ ...prJobQuery(openJob?.id ?? 0), enabled: openJob !== null });
  const job = live.data ?? openJob;

  const prTarget: PrTarget = target === "base" || branch === null ? { kind: "base" } : { kind: "tenant", branch };
  const { dir, name: fileName } = splitPath(path);

  const submit = async (): Promise<void> => {
    setError(null);
    try {
      const result = await create.mutateAsync({ swimlane: lane, paths: [path], title: titleValue, ...(bodyValue === "" ? {} : { body: bodyValue }), target: prTarget });
      if ("outcome" in result) {
        onOpenChange(false);
        onSettled({ kind: "sent", proposal: result.id });
        return;
      }
      setView({ kind: "job", job: result });
    } catch (thrown) {
      if (!(thrown instanceof ApiProblem)) {
        setError({ title: "The request failed", detail: "Nothing was created. Try again." });
        return;
      }
      const duplicate = duplicatePrOf(thrown);
      if (duplicate !== null) {
        setView({ kind: "exists", link: duplicate });
        return;
      }
      const trouble = tokenTrouble(thrown);
      if (trouble === "expired" || trouble === "unusable") {
        if (trouble === "expired") onTokenExpired();
        setView({ kind: "err-token", why: trouble, requestId: thrown.requestId });
        return;
      }
      if (trouble === "no_access") {
        const baseOk = thrown.body["base"] === true;
        const tenantOk = thrown.body["tenant"] === true;
        setView({ kind: "err-push", requestId: thrown.requestId, alternative: target === "base" ? (tenantOk ? "tenant" : null) : baseOk ? "base" : null });
        return;
      }
      const outcome = fromProblem(thrown);
      if (outcome.kind === "locked" || outcome.kind === "conflict") {
        onOpenChange(false);
        onSettled(outcome);
        return;
      }
      const withId = thrown.requestId === undefined ? {} : { requestId: thrown.requestId };
      const info: WriteErrorInfo =
        outcome.kind === "failed"
          ? { title: outcome.title, detail: "Nothing was created. Try again, or send the request ID to the platform team.", ...withId }
          : { title: "The pull request was refused", detail: "Nothing was created.", ...withId };
      setError(info);
    }
  };

  const goOn = (): void => {
    if (preview?.open_pr) {
      setView({ kind: "exists", link: preview.open_pr });
      return;
    }
    setView({ kind: "form" });
  };

  const heading = job?.state === "completed" ? "Pull request created" : job?.state === "failed" ? "Pull request not created" : titleOf(view.kind);
  const step = view.kind === "target" ? 1 : view.kind === "form" ? 2 : null;
  const repoOf = (t: Target) => (t === "tenant" ? tenantPreview : basePreview);
  const repoName = (() => {
    const s = repoOf(target);
    return s.status === "ready" ? s.preview.repo : "the repository";
  })();

  let content: React.ReactNode;
  let footer: React.ReactNode;
  if (view.kind === "target") {
    content = (
      <TargetStep
        lane={lane}
        target={target}
        onTarget={setTarget}
        branch={branch !== null && branches.includes(branch) ? branch : null}
        branches={branches}
        pickable={fileTenant === null && branches.length > 1}
        onBranch={setBranch}
        tenant={tenantPreview}
        base={basePreview}
        affected={affected}
        fresh={fresh}
        nothing={nothing}
      />
    );
    footer = (
      <>
        <span className="mr-auto text-xs text-muted-foreground">Step 1 of 2</span>
        <Button variant="ghost" onClick={() => onOpenChange(false)}>
          Cancel
        </Button>
        <Button {...(preview === null || nothing ? { "aria-disabled": true } : {})} onClick={() => preview !== null && !nothing && goOn()}>
          Continue
        </Button>
      </>
    );
  } else if (view.kind === "form") {
    content = (
      <div className="flex flex-col gap-3">
        <div className="flex flex-col gap-1.5 rounded-md border border-border bg-muted p-3 text-sm">
          <div className="flex flex-wrap items-center gap-2">
            <span className="w-14 text-muted-foreground">Target</span>
            <span className="font-mono text-xs">
              <SafeText text={repoName} />
            </span>
            {preview === null ? null : <BranchChip branch={preview.base_branch} copy={false} />}
            <Button variant="ghost" size="sm" className="ml-auto" onClick={() => setView({ kind: "target" })}>
              Change
            </Button>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <span className="w-14 text-muted-foreground">Writes</span>
            <span className="font-mono text-xs">
              <SafeText text={file?.repo_path ?? ""} />
            </span>
          </div>
        </div>
        <div className="flex flex-col gap-1">
          <Label htmlFor="pr-title">Title</Label>
          <Input id="pr-title" value={titleValue} maxLength={256} onChange={(event) => setTitle(event.target.value)} />
        </div>
        <div className="flex flex-col gap-1">
          <Label htmlFor="pr-body">Description</Label>
          <Textarea id="pr-body" value={bodyValue} onChange={(event) => setBody(event.target.value)} className="min-h-32 font-mono text-xs" />
        </div>
        {error ? <WriteError {...error} /> : null}
      </div>
    );
    footer = (
      <>
        <span className="mr-auto text-xs text-muted-foreground">Step 2 of 2</span>
        <Button variant="ghost" onClick={() => setView({ kind: "target" })}>
          Back
        </Button>
        <Button variant="ghost" onClick={() => onOpenChange(false)}>
          Cancel
        </Button>
        <Button loading={create.isPending} {...(titleValue.trim() === "" ? { "aria-disabled": true } : {})} onClick={() => titleValue.trim() !== "" && void submit()}>
          <GitPullRequest aria-hidden="true" />
          Create PR
        </Button>
      </>
    );
  } else if (view.kind === "exists") {
    content = <DuplicatePr link={view.link} swimlane={lane} />;
    footer = (
      <>
        <Button variant="ghost" onClick={() => onOpenChange(false)}>
          Close
        </Button>
        <OpenPrButton link={view.link} />
      </>
    );
  } else if (view.kind === "job" && job !== null) {
    // What the job means for the person, said when it is completed: the file stays NFS ahead until the PR merges and syncs.
    const effect = target === "tenant" ? `The file stays NFS ahead until the PR merges and ${lane} syncs.` : "Base changes reach every swimlane on its next sync after the PR merges.";
    content = <PrJobProgress jobId={view.job.id} initial={view.job} title={titleValue} effect={effect} />;
    if (job.state === "completed") {
      footer = (
        <>
          <Button variant="ghost" className="mr-auto" onClick={() => setView({ kind: "target" })}>
            Start over
          </Button>
          <Button variant="secondary" onClick={() => onOpenChange(false)}>
            Close
          </Button>
          {doneAction(job)}
        </>
      );
    } else if (job.state === "failed") {
      footer = (
        <>
          <Button variant="ghost" className="mr-auto" onClick={() => onOpenChange(false)}>
            Close
          </Button>
          <Button variant="secondary" onClick={() => setView({ kind: "form" })}>
            Try again
          </Button>
        </>
      );
    } else {
      footer = (
        <>
          <span className="mr-auto text-xs text-muted-foreground">Esc closes this dialog, not the job</span>
          <Button variant="secondary" onClick={() => onOpenChange(false)}>
            Close
          </Button>
        </>
      );
    }
  } else if (view.kind === "err-token") {
    content = <TokenStep why={view.why} requestId={view.requestId} repo={repoName === "the repository" ? undefined : repoName} />;
    footer = (
      <>
        <Button variant="ghost" className="mr-auto" onClick={() => onOpenChange(false)}>
          Close
        </Button>
        <Button variant="secondary" onClick={() => setView({ kind: "form" })}>
          Retry
        </Button>
        <MyTokenLink />
      </>
    );
  } else if (view.kind === "err-push") {
    const { alternative } = view;
    content = <PushStep repo={repoName} requestId={view.requestId} alternative={alternative === null ? null : alternative === "tenant" ? "Or choose the tenant branch." : "Or choose base."} />;
    footer = (
      <>
        <Button variant="ghost" className="mr-auto" onClick={() => onOpenChange(false)}>
          Close
        </Button>
        <Button variant="secondary" onClick={() => setView({ kind: "target" })}>
          Choose another target
        </Button>
        <Button onClick={() => setView({ kind: "form" })}>Retry</Button>
      </>
    );
  } else {
    content = null;
    footer = null;
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent size="lg" className="gap-4">
        <DialogHeader>
          <div className="flex items-start justify-between gap-3">
            <div className="flex min-w-0 flex-col gap-0.5">
              <DialogTitle>{heading}</DialogTitle>
              <DialogDescription className="font-mono text-xs">
                <SafeText text={[lane, dir, fileName].filter((part) => part !== "").join(" / ")} />
              </DialogDescription>
            </div>
            {step === null ? null : (
              <span className="mt-1 flex items-center gap-1.5 text-xs text-muted-foreground" aria-hidden="true">
                <span className="flex gap-0.5">
                  <span className="h-1 w-5 rounded-sm bg-primary" />
                  <span className={cn("h-1 w-5 rounded-sm", step === 2 ? "bg-primary" : "bg-border")} />
                </span>
              </span>
            )}
          </div>
        </DialogHeader>
        {content}
        <DialogFooter>{footer}</DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

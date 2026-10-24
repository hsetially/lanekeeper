import type { ReactNode } from "react";
import { BranchChip } from "@/components/app/chips";
import { SafeText } from "@/components/app/SafeText";
import { Alert } from "@/components/ui/alert";
import { NativeSelect } from "@/components/ui/native-select";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { Skeleton } from "@/components/ui/skeleton";
import { Layers } from "@/icons";
import { cn } from "@/lib/utils";
import type { PrPreviewState } from "./usePrPreview";

// Step 1 of the pull-request dialog (1b to 1d): where it goes. Two cards, the tenant's branch and base, each with the repository,
// the branch and the path it writes, from the server's preview before anything is created, and who it affects. The choice is a
// radio group: the arrow keys move it, as the design's cards do.

export type Target = "tenant" | "base";

type CardProps = {
  value: Target;
  selected: boolean;
  ariaName: string;
  title: string;
  hint: string;
  state: PrPreviewState;
  /** The tenant card for a base file: which branch of the swimlane's tenants. */
  picker?: ReactNode;
  children?: ReactNode;
};

function Row({ term, children }: { term: string; children: ReactNode }) {
  return (
    <>
      <dt className="text-muted-foreground">{term}</dt>
      <dd className="m-0 min-w-0 [overflow-wrap:anywhere]">{children}</dd>
    </>
  );
}

function Card({ value, selected, ariaName, title, hint, state, picker, children }: CardProps) {
  const preview = state.status === "ready" ? state.preview : null;
  const writes = preview?.files.map((f) => f.repo_path) ?? [];
  return (
    <div data-slot="target-card" data-selected={selected} className={cn("relative flex min-w-0 cursor-pointer flex-col gap-2 rounded-md border p-3", selected ? "border-primary bg-selection" : "border-border bg-card")}>
      <div className="flex items-start gap-2">
        {/* The radio's hit area is stretched over the whole card, so a click anywhere on it chooses; the picker sits above it. */}
        <RadioGroupItem value={value} aria-label={ariaName} className="static mt-0.5 after:inset-0" />
        <div className="flex min-w-0 flex-col">
          <b className="font-semibold">{title}</b>
          <span className="text-xs text-muted-foreground">{hint}</span>
        </div>
      </div>
      {picker}
      {state.status === "loading" || state.status === "idle" ? (
        <div role="status" aria-busy="true" aria-label="Checking where this goes" className="flex flex-col gap-1.5">
          <Skeleton className="h-4 w-4/5" />
          <Skeleton className="h-4 w-3/5" />
        </div>
      ) : state.status === "error" ? (
        <p className="m-0 text-xs text-danger-fg">This target could not be checked. Nothing was created. Choose the other target, or try again later.</p>
      ) : (
        <dl className="m-0 grid grid-cols-[52px_minmax(0,1fr)] gap-x-2 gap-y-1.5 text-xs leading-4">
          <Row term="Repo">
            <span className="font-mono">
              <SafeText text={state.preview.repo} />
            </span>
          </Row>
          <Row term="Branch">
            <BranchChip branch={state.preview.base_branch} copy={false} />
          </Row>
          <Row term="Writes">
            {writes.map((w) => (
              <span key={w} className="block font-mono">
                <SafeText text={w} />
              </span>
            ))}
          </Row>
        </dl>
      )}
      {preview?.warnings.map((w, i) => (
        <Alert key={i} variant="warning">
          <SafeText text={w} />
        </Alert>
      ))}
      {children}
    </div>
  );
}

export type TargetStepProps = {
  lane: string;
  target: Target;
  onTarget: (target: Target) => void;
  /** The tenant branch the tenant card is for, or null when the swimlane has none (then only base is offered). */
  branch: string | null;
  /** The tenant branches to choose from, for a base file whose swimlane has more than one. */
  branches: readonly string[];
  pickable: boolean;
  onBranch: (branch: string) => void;
  tenant: PrPreviewState;
  base: PrPreviewState;
  /** Swimlanes that run the tenant branch. */
  affected: readonly string[];
  /** The file is not on the tenant branch yet: the PR adds it and forks it from base. */
  fresh: boolean;
  /** The branch already has this content: there is nothing to raise. */
  nothing: boolean;
};

function effectOf(target: Target, branch: string | null, lane: string, fresh: boolean): { tone: "info" | "warning"; title: string; text: string } {
  const on = branch ?? "the branch";
  if (target === "base") return { tone: "warning", title: "Every swimlane gets this change on its next sync.", text: "Swimlanes with their own tenant file for it keep that file." };
  if (fresh) return { tone: "warning", title: `This PR adds a new file on ${on} and forks it from base.`, text: `Future base changes won't reach ${on} for this file.` };
  return { tone: "info", title: `This PR changes ${on} only.`, text: `${lane} picks it up after the PR merges and the swimlane syncs.` };
}

export function TargetStep({ lane, target, onTarget, branch, branches, pickable, onBranch, tenant, base, affected, fresh, nothing }: TargetStepProps) {
  const effect = effectOf(target, branch, lane, fresh);
  return (
    <div className="flex flex-col gap-3">
      <RadioGroup value={target} onValueChange={(value) => onTarget(value === "base" ? "base" : "tenant")} aria-label="Target of the pull request" className="grid grid-cols-2 items-start gap-3">
        {branch === null ? null : (
          <Card
            value="tenant"
            selected={target === "tenant"}
            ariaName={`Tenant branch ${branch}`}
            title={pickable ? "Tenant branch" : `Tenant branch ${branch}`}
            hint="(this branch only)"
            state={tenant}
            {...(pickable
              ? {
                  picker: (
                    <div className="relative z-10">
                      <NativeSelect aria-label="Tenant branch" value={branch} onChange={(event) => onBranch(event.target.value)}>
                        {branches.map((b) => (
                          <option key={b} value={b}>
                            {b}
                          </option>
                        ))}
                      </NativeSelect>
                    </div>
                  ),
                }
              : {})}
          >
            <p className="m-0 flex items-start gap-1.5 text-xs leading-4">
              <Layers aria-hidden="true" className="mt-0.5 size-3 shrink-0" />
              <span>
                Affects swimlanes running {branch}: <strong className="font-mono">{affected.length > 0 ? affected.join(", ") : lane}</strong>
              </span>
            </p>
            {fresh && tenant.status === "ready" ? <Alert variant="warning">New file on {branch}. Future base changes won't reach {branch} for this file.</Alert> : null}
          </Card>
        )}
        <Card value="base" selected={target === "base"} ariaName="Base (all swimlanes)" title="Base" hint="(all swimlanes)" state={base}>
          <Alert variant="warning">This copies one tenant's content into base and affects every swimlane on its next sync.</Alert>
        </Card>
      </RadioGroup>
      {nothing ? (
        <Alert variant="info">
          <b>The branch already has this content.</b>
          <span>There is nothing new to raise as a pull request for this target.</span>
        </Alert>
      ) : (
        <Alert variant={effect.tone}>
          <b>{effect.title}</b>
          <span className="text-muted-foreground">{effect.text}</span>
        </Alert>
      )}
    </div>
  );
}

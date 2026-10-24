import { useMutation, useQueryClient } from "@tanstack/react-query";
import { visibleText } from "@/components/app/safe-text";
import { useId, useState } from "react";
import type { PathMappingRule } from "@/api/generated/model";
import { ConsequenceDialog } from "@/components/app/ConsequenceDialog";
import { SafeText } from "@/components/app/SafeText";
import { Checkbox } from "@/components/ui/checkbox";
import { Field, FieldControl, FieldDescription, FieldError, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { adminMutations } from "@/features/admin/mutations";
import { checkConfigKey, checkRepoRoot, omitKey } from "@/features/admin/validate";
import { adminWriteError } from "@/features/admin/write-error";
import { swimlaneListQuery } from "@/features/swimlanes/queries";
import { useCursorPages } from "@/lib/query/cursor-pages";

// One path-mapping rule: where the base files live in the base repository, where the tenant files live in the tenant repository, and
// for which swimlanes (none chosen means every swimlane). The tenant's name is added before the extension of a tenant file by the
// engine, not by a setting. Both roots are relative paths inside their repository.

type Errors = { id?: string; base?: string; tenant?: string; swimlanes?: string };

export function PathRuleDialog({ rule, onClose, onSaved }: { rule: PathMappingRule | null; onClose: () => void; onSaved: (id: string) => void }) {
  const client = useQueryClient();
  const save = useMutation(adminMutations.putPathMapping(client));
  const lanes = useCursorPages({ ...swimlaneListQuery(), autoLoad: true });
  const uid = useId();
  const editing = rule !== null;
  const [id, setId] = useState(rule?.id ?? "");
  const [base, setBase] = useState(rule?.base_root ?? "");
  const [tenant, setTenant] = useState(rule?.tenant_root ?? "");
  const [scope, setScope] = useState<"all" | "some">(rule === null || rule.swimlanes.length === 0 ? "all" : "some");
  const [chosen, setChosen] = useState<string[]>(rule?.swimlanes ?? []);
  const [errors, setErrors] = useState<Errors>({});

  const submit = (): void => {
    const found: Errors = {};
    if (!editing) {
      const bad = checkConfigKey(id.trim(), "Enter a rule id");
      if (bad !== null) found.id = bad;
    }
    const badBase = checkRepoRoot(base.trim());
    if (badBase !== null) found.base = badBase;
    const badTenant = checkRepoRoot(tenant.trim());
    if (badTenant !== null) found.tenant = badTenant;
    if (scope === "some" && chosen.length === 0) found.swimlanes = "Choose at least one swimlane";
    setErrors(found);
    if (Object.keys(found).length > 0) return;
    const ruleId = editing ? rule.id : id.trim();
    save.mutate(
      { id: ruleId, body: { base_root: base.trim(), tenant_root: tenant.trim(), swimlanes: scope === "all" ? [] : chosen } },
      {
        onSuccess: () => {
          onSaved(ruleId);
          onClose();
        },
      },
    );
  };

  return (
    <ConsequenceDialog
      initialFocus="first-field"
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
      title={editing ? `Edit rule ${visibleText(rule.id)}` : "Add a path mapping"}
      consequence={
        editing
          ? "Saving remaps every file this rule covers. Files that no longer match a rule may become Untracked until a rule covers them."
          : "Says where files live in the two repositories, for the swimlanes chosen. It takes effect on the next sync."
      }
      actionLabel="Save rule"
      workingLabel="Saving…"
      size="lg"
      working={save.isPending}
      error={save.error === null ? null : adminWriteError(save.error, "The rule could not be saved")}
      onConfirm={submit}
    >
      <Field invalid={errors.id !== undefined}>
        <FieldLabel>Rule id</FieldLabel>
        <FieldControl>
          <Input value={id} readOnly={editing} autoComplete="off" spellCheck={false} placeholder="default" className="font-mono" onChange={(event) => setId(event.target.value)} />
        </FieldControl>
        <FieldError>{errors.id}</FieldError>
        {editing ? <FieldDescription>The id cannot change. Delete the rule and add another to rename it.</FieldDescription> : null}
      </Field>
      <div className="grid grid-cols-2 gap-3 max-sm:grid-cols-1">
        <Field invalid={errors.base !== undefined}>
          <FieldLabel>Base repo path</FieldLabel>
          <FieldControl>
            <Input value={base} autoComplete="off" spellCheck={false} placeholder="config" className="font-mono" onChange={(event) => setBase(event.target.value)} />
          </FieldControl>
          <FieldError>{errors.base}</FieldError>
          <FieldDescription>Where the base files are, inside the base repository.</FieldDescription>
        </Field>
        <Field invalid={errors.tenant !== undefined}>
          <FieldLabel>Tenant repo path</FieldLabel>
          <FieldControl>
            <Input value={tenant} autoComplete="off" spellCheck={false} placeholder="data/config" className="font-mono" onChange={(event) => setTenant(event.target.value)} />
          </FieldControl>
          <FieldError>{errors.tenant}</FieldError>
          <FieldDescription>Where a tenant's files are, inside the tenant repository.</FieldDescription>
        </Field>
      </div>
      <p className="text-xs text-muted-foreground">A tenant file sits beside its base file: the tenant's name is added before the extension, so tx-infinity-core.yml for tenant sit1 is tx-infinity-core-sit1.yml.</p>
      <fieldset className="m-0 flex min-w-0 flex-col gap-2 border-0 p-0">
        <legend className="mb-1 text-sm font-medium">Applies to</legend>
        <RadioGroup
          value={scope}
          onValueChange={(next) => {
            setScope(next === "some" ? "some" : "all");
            setErrors(omitKey(errors, "swimlanes"));
          }}
          aria-label="Applies to"
        >
          <div className="flex items-center gap-2">
            <RadioGroupItem id={`${uid}-all`} value="all" />
            <Label htmlFor={`${uid}-all`}>Every swimlane</Label>
          </div>
          <div className="flex items-center gap-2">
            <RadioGroupItem id={`${uid}-some`} value="some" />
            <Label htmlFor={`${uid}-some`}>Only some swimlanes</Label>
          </div>
        </RadioGroup>
        {scope === "some" ? (
          <ul aria-label="Swimlanes" className="m-0 grid max-h-40 list-none grid-cols-3 gap-x-4 gap-y-1.5 overflow-auto rounded-md border border-border p-2 max-sm:grid-cols-2">
            {lanes.items.map((lane) => (
              <li key={lane.id} className="flex min-w-0 items-center gap-2">
                <Checkbox
                  id={`${uid}-lane-${lane.id}`}
                  checked={chosen.includes(lane.id)}
                  onCheckedChange={(checked) => {
                    setChosen(checked === true ? [...chosen, lane.id] : chosen.filter((x) => x !== lane.id));
                    setErrors(omitKey(errors, "swimlanes"));
                  }}
                />
                <Label htmlFor={`${uid}-lane-${lane.id}`} className="min-w-0 truncate font-mono text-xs font-normal">
                  <SafeText text={lane.id} />
                </Label>
              </li>
            ))}
          </ul>
        ) : null}
        {errors.swimlanes === undefined ? null : (
          <p role="alert" className="text-xs text-danger-fg">
            {errors.swimlanes}
          </p>
        )}
      </fieldset>
    </ConsequenceDialog>
  );
}

import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useId, useState } from "react";
import { Severity, type ChangeKind, type FindingKind, type SeverityRule } from "@/api/generated/model";
import { ConsequenceDialog } from "@/components/app/ConsequenceDialog";
import { SafeText } from "@/components/app/SafeText";
import { visibleText } from "@/components/app/safe-text";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Checkbox } from "@/components/ui/checkbox";
import { Field, FieldControl, FieldDescription, FieldError, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeSelect } from "@/components/ui/native-select";
import { Switch } from "@/components/ui/switch";
import { adminMutations } from "@/features/admin/mutations";
import { checkConfigKey, checkName, omitKey } from "@/features/admin/validate";
import { adminWriteError } from "@/features/admin/write-error";
import {
  buildMatch,
  CHANGE_CHOICES,
  changeLabel,
  checkRuleGlob,
  DEFAULT_PRIORITY,
  FINDING_CHOICES,
  findingLabel,
  matchesEverything,
  matchParts,
  notRatedYet,
  SEVERITY_CHOICES,
  severityLabel,
} from "./severity-model";

// One severity rule of the admin's own. The built-in rules are not here and cannot be changed from here: this rule is added to them,
// and the highest severity of the rules that match wins, so it can raise a severity and never lower one (D103). What it matches is
// said back to the person as they set it, from the form, in words; whether anything matches is the registry's. The priority of the
// contract is not shown: the hub does not use it (the highest wins whatever the order), so a new rule sends a fixed one and an
// existing rule keeps its own.

type Errors = { id?: string; name?: string; path?: string; setting?: string };

const CONFLICT = "Someone else changed the rules at the same time. Reload the list, check what is there now and try again.";

const toggle = <T,>(list: readonly T[], item: T, on: boolean): T[] => (on ? [...list, item] : list.filter((x) => x !== item));

export function SeverityRuleDialog({ rule, existing, onClose, onSaved }: { rule: SeverityRule | null; existing: ReadonlySet<string>; onClose: () => void; onSaved: (id: string) => void }) {
  const client = useQueryClient();
  const save = useMutation(adminMutations.putSeverityRule(client));
  const uid = useId();
  const editing = rule !== null;
  const [id, setId] = useState(rule?.id ?? "");
  const [name, setName] = useState(rule?.name ?? "");
  const [severity, setSeverity] = useState<Severity>(rule?.severity ?? Severity.high);
  const [findingKinds, setFindingKinds] = useState<FindingKind[]>(rule?.match.finding_kinds ?? []);
  const [changeKinds, setChangeKinds] = useState<ChangeKind[]>(rule?.match.change_kinds ?? []);
  const [pathGlob, setPathGlob] = useState(rule?.match.path_glob ?? "");
  const [settingGlob, setSettingGlob] = useState(rule?.match.setting_glob ?? "");
  const [enabled, setEnabled] = useState(rule?.enabled ?? true);
  const [errors, setErrors] = useState<Errors>({});

  const match = buildMatch({ findingKinds, changeKinds, pathGlob, settingGlob });
  const parts = matchParts(match);

  const submit = (): void => {
    const found: Errors = {};
    if (!editing) {
      const bad = checkConfigKey(id.trim(), "Enter a rule id");
      if (bad !== null) found.id = bad;
      else if (existing.has(id.trim())) found.id = "A rule with this id exists already. Use Edit on its row to change it.";
    }
    const badName = checkName(name);
    if (badName !== null) found.name = badName;
    const badPath = checkRuleGlob(pathGlob);
    if (badPath !== null) found.path = badPath;
    const badSetting = checkRuleGlob(settingGlob);
    if (badSetting !== null) found.setting = badSetting;
    setErrors(found);
    if (Object.keys(found).length > 0) return;
    const ruleId = editing ? rule.id : id.trim();
    save.mutate(
      { id: ruleId, body: { name: name.trim(), match, severity, priority: rule?.priority ?? DEFAULT_PRIORITY, enabled } },
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
      title={editing ? `Edit rule ${visibleText(rule.id)}` : "Add a severity rule"}
      consequence="Pending changes and open findings that match are rated again with this rule. Changes already resolved keep the severity they had."
      actionLabel="Save rule"
      workingLabel="Saving…"
      size="lg"
      working={save.isPending}
      error={save.error === null ? null : adminWriteError(save.error, "The rule could not be saved", { 409: CONFLICT })}
      onConfirm={submit}
    >
      <div className="grid grid-cols-2 gap-3 max-sm:grid-cols-1">
        <Field invalid={errors.id !== undefined}>
          <FieldLabel>Rule id</FieldLabel>
          <FieldControl>
            <Input value={id} readOnly={editing} autoComplete="off" spellCheck={false} placeholder="ssl-critical" className="font-mono" onChange={(event) => setId(event.target.value)} />
          </FieldControl>
          <FieldError>{errors.id}</FieldError>
          {editing ? <FieldDescription>The id cannot change. Delete the rule and add another to rename it.</FieldDescription> : null}
        </Field>
        <Field invalid={errors.name !== undefined}>
          <FieldLabel>Name</FieldLabel>
          <FieldControl>
            <Input
              value={name}
              autoComplete="off"
              placeholder="SSL config is Critical"
              onChange={(event) => {
                setName(event.target.value);
                setErrors(omitKey(errors, "name"));
              }}
            />
          </FieldControl>
          <FieldError>{errors.name}</FieldError>
        </Field>
      </div>

      <div className="grid grid-cols-2 items-start gap-3 max-sm:grid-cols-1">
        <Field>
          <FieldLabel>Severity</FieldLabel>
          <FieldControl>
            <NativeSelect
              value={severity}
              onChange={(event) => {
                setSeverity(SEVERITY_CHOICES.find((s) => s === event.target.value) ?? Severity.high);
              }}
            >
              {SEVERITY_CHOICES.map((s) => (
                <option key={s} value={s}>
                  {severityLabel(s)}
                </option>
              ))}
            </NativeSelect>
          </FieldControl>
          <FieldDescription>What a change or a finding that this rule matches is raised to, if the rules that already match say less.</FieldDescription>
        </Field>
        <div className="flex flex-col gap-1">
          <div className="flex h-7 items-center gap-2">
            <Switch id={`${uid}-enabled`} checked={enabled} onCheckedChange={setEnabled} />
            <Label htmlFor={`${uid}-enabled`} className="font-normal">
              This rule is on
            </Label>
          </div>
          <p className="text-xs text-muted-foreground">Turn it off to stop this rule from raising anything. This switches off this rule only: the built-in rules stay on.</p>
        </div>
      </div>

      <fieldset className="m-0 flex min-w-0 flex-col gap-2 border-0 p-0">
        <legend className="mb-1 text-sm font-medium">Finding types</legend>
        <ul className="m-0 grid list-none grid-cols-2 gap-x-4 gap-y-1.5 p-0 max-sm:grid-cols-1">
          {FINDING_CHOICES.map((kind) => (
            <li key={kind} className="flex items-center gap-2">
              <Checkbox id={`${uid}-f-${kind}`} checked={findingKinds.includes(kind)} onCheckedChange={(checked) => setFindingKinds(toggle(findingKinds, kind, checked === true))} />
              <Label htmlFor={`${uid}-f-${kind}`} className="font-normal">
                {findingLabel(kind)}
              </Label>
            </li>
          ))}
        </ul>
        <p className="text-xs text-muted-foreground">Check none to match a change or a finding of any type.</p>
      </fieldset>

      <fieldset className="m-0 flex min-w-0 flex-col gap-2 border-0 p-0">
        <legend className="mb-1 text-sm font-medium">Change types</legend>
        <ul className="m-0 grid list-none grid-cols-2 gap-x-4 gap-y-1.5 p-0 max-sm:grid-cols-1">
          {CHANGE_CHOICES.map((kind) => (
            <li key={kind} className="flex items-center gap-2">
              <Checkbox id={`${uid}-c-${kind}`} checked={changeKinds.includes(kind)} onCheckedChange={(checked) => setChangeKinds(toggle(changeKinds, kind, checked === true))} />
              <Label htmlFor={`${uid}-c-${kind}`} className="font-normal">
                {changeLabel(kind)}
              </Label>
            </li>
          ))}
        </ul>
      </fieldset>

      <div className="grid grid-cols-2 gap-3 max-sm:grid-cols-1">
        <Field invalid={errors.path !== undefined}>
          <FieldLabel>Path glob</FieldLabel>
          <FieldControl>
            <Input
              value={pathGlob}
              autoComplete="off"
              spellCheck={false}
              placeholder="security/**"
              className="font-mono"
              onChange={(event) => {
                setPathGlob(event.target.value);
                setErrors(omitKey(errors, "path"));
              }}
            />
          </FieldControl>
          <FieldError>{errors.path}</FieldError>
          <FieldDescription>Matches the whole path from the config root, with upper and lower case told apart. * also crosses /, so security/** is every file below security. Leave empty to match any path.</FieldDescription>
        </Field>
        <Field invalid={errors.setting !== undefined}>
          <FieldLabel>Setting glob</FieldLabel>
          <FieldControl>
            <Input
              value={settingGlob}
              autoComplete="off"
              spellCheck={false}
              placeholder="spring.datasource.*"
              className="font-mono"
              onChange={(event) => {
                setSettingGlob(event.target.value);
                setErrors(omitKey(errors, "setting"));
              }}
            />
          </FieldControl>
          <FieldError>{errors.setting}</FieldError>
          <FieldDescription>Matches the path of a setting that changed. Leave empty to match any setting.</FieldDescription>
        </Field>
      </div>

      <p role="status" aria-label="What this rule matches" className="text-sm">
        {matchesEverything(match) ? (
          "This rule matches every change and every finding."
        ) : (
          <>
            This rule matches:{" "}
            {parts.map((part, index) => (
              <span key={part.label}>
                {index === 0 ? "" : " · "}
                {part.label} {part.glob ? <SafeText text={part.value} className="font-mono text-xs" /> : part.value}
              </span>
            ))}
          </>
        )}
      </p>
      {matchesEverything(match) ? (
        <Alert variant="warning">
          <AlertTitle>With no condition, this rule raises every change and every finding to its severity</AlertTitle>
          <AlertDescription>Add a path, a finding type or a change type to narrow it.</AlertDescription>
        </Alert>
      ) : null}
      {notRatedYet(match) ? (
        <Alert variant="info">
          <AlertTitle>Not applied yet</AlertTitle>
          <AlertDescription>
            The hub does not look at the individual setting changes of a file when it rates it, so a change type or a setting glob has nothing to match today (decision D103). You can still save the rule.
          </AlertDescription>
        </Alert>
      ) : null}
    </ConsequenceDialog>
  );
}

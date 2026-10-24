import { useEffect, useRef, useState, type SyntheticEvent } from "react";
import { visibleText } from "@/components/app/safe-text";
import { Button } from "@/components/ui/button";
import { Field, FieldControl, FieldError, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { NativeSelect } from "@/components/ui/native-select";
import { Spinner } from "@/components/ui/spinner";
import { cn } from "@/lib/utils";
import { problemsOf, selectionFrom, SERVED_FIELDS, type ServedDraft, type ServedField, type ServedProblems, type ServedSelection } from "./model";

// The pickers of the "As served" view: the file, the application, the tenant and the channel, and the button that asks. The
// contract has no list of applications or channels (an application is the name a service asks the config-server for, and no
// operation lists them), so those two are typed and checked against their patterns (model.ts); the tenant is chosen from the
// swimlane's own when they are known, and typed when they are not.
//
// Nothing is asked of the config-server until the button is pressed: each request is a round trip through the swimlane's agent
// (limited on the hub and at the agent), so typing and choosing send nothing. A form that cannot be a request sends nothing either,
// and says what is wrong next to the field, never repeating what was typed.

type Props = {
  /** What the address holds. When it changes (Back, a link) the fields take it; until then they keep what the person typed. */
  draft: ServedDraft;
  /** What is wrong with a value the address GAVE, shown before anything is pressed. */
  problems: ServedProblems;
  /** The swimlane's tenants, when known. With none, the tenant is typed. */
  tenants: readonly string[];
  /** A request is in flight. */
  busy: boolean;
  onShow: (selection: ServedSelection) => void;
};

const keyOf = (draft: ServedDraft): string => SERVED_FIELDS.map((field) => draft[field]).join("\u0000");

export function SelectionForm({ draft, problems, tenants, busy, onShow }: Props) {
  const [values, setValues] = useState<ServedDraft>(draft);
  const [checked, setChecked] = useState(false);
  const seen = useRef(keyOf(draft));
  const refs = useRef<Partial<Record<ServedField, HTMLElement | null>>>({});

  useEffect(() => {
    const key = keyOf(draft);
    if (key === seen.current) return;
    seen.current = key;
    setValues(draft);
    setChecked(false);
  }, [draft]);

  // Before the first press, the problems are those of the address; after it, those of what is in the fields now.
  const shown: ServedProblems = checked ? problemsOf(values) : problems;
  const set = (field: ServedField, value: string): void => {
    setValues((current) => ({ ...current, [field]: value }));
  };
  const bind = (field: ServedField) => (element: HTMLElement | null): void => {
    refs.current[field] = element;
  };

  const submit = (event: SyntheticEvent): void => {
    event.preventDefault();
    const selection = selectionFrom(values);
    if (selection === null) {
      setChecked(true);
      const first = SERVED_FIELDS.find((field) => problemsOf(values)[field] !== undefined);
      if (first !== undefined) refs.current[first]?.focus();
      return;
    }
    onShow(selection);
  };

  const wanted = values.tenant.trim();
  const unknownTenant = wanted !== "" && !tenants.includes(wanted);

  return (
    <form aria-label="What to ask the config-server for" noValidate onSubmit={submit} className="flex flex-none flex-wrap items-start gap-x-3 gap-y-2 border-b border-border bg-muted px-3 py-2">
      <Field invalid={shown.file !== undefined} className="min-w-64 flex-[3_1_16rem]">
        <FieldLabel className="text-xs text-muted-foreground">File</FieldLabel>
        <FieldControl>
          <Input
            ref={bind("file")}
            value={values.file}
            onChange={(event) => {
              set("file", event.target.value);
            }}
            placeholder="tx-infinity-api/connection-pool.yml"
            aria-required="true"
            autoComplete="off"
            spellCheck={false}
            className="font-mono text-xs"
          />
        </FieldControl>
        <FieldError>{shown.file}</FieldError>
      </Field>
      <Field invalid={shown.application !== undefined} className="min-w-44 flex-[2_1_11rem]">
        <FieldLabel className="text-xs text-muted-foreground">Application</FieldLabel>
        <FieldControl>
          <Input
            ref={bind("application")}
            value={values.application}
            onChange={(event) => {
              set("application", event.target.value);
            }}
            placeholder="tx-infinity-core"
            aria-required="true"
            autoComplete="off"
            spellCheck={false}
            className="font-mono text-xs"
          />
        </FieldControl>
        <FieldError>{shown.application}</FieldError>
      </Field>
      <Field invalid={shown.tenant !== undefined} className="w-40 flex-none">
        <FieldLabel className="text-xs text-muted-foreground">Tenant</FieldLabel>
        <FieldControl>
          {tenants.length > 0 ? (
            <NativeSelect
              ref={bind("tenant")}
              value={values.tenant}
              onChange={(event) => {
                set("tenant", event.target.value);
              }}
              aria-required="true"
              className="font-mono text-xs"
            >
              <option value="">Choose a tenant</option>
              {tenants.map((tenant) => (
                <option key={tenant} value={tenant}>
                  {visibleText(tenant)}
                </option>
              ))}
              {unknownTenant ? <option value={values.tenant}>{`${visibleText(wanted)} (not a tenant of this swimlane)`}</option> : null}
            </NativeSelect>
          ) : (
            <Input
              ref={bind("tenant")}
              value={values.tenant}
              onChange={(event) => {
                set("tenant", event.target.value);
              }}
              placeholder="sit7"
              aria-required="true"
              autoComplete="off"
              spellCheck={false}
              className="font-mono text-xs"
            />
          )}
        </FieldControl>
        <FieldError>{shown.tenant}</FieldError>
      </Field>
      <Field invalid={shown.channel !== undefined} className="w-40 flex-none">
        <FieldLabel className="text-xs text-muted-foreground">Channel</FieldLabel>
        <FieldControl>
          <Input
            ref={bind("channel")}
            value={values.channel}
            onChange={(event) => {
              set("channel", event.target.value);
            }}
            placeholder="None"
            autoComplete="off"
            spellCheck={false}
            className="font-mono text-xs"
          />
        </FieldControl>
        <FieldError>{shown.channel}</FieldError>
      </Field>
      <div className="flex flex-none flex-col gap-1">
        <span aria-hidden="true" className="h-4" />
        {/* Not the Button's own `loading`: it puts a spinner in front of the label while a request is out, and the button grows by it. The
            spinner and the longer label are in the button all the time, in one grid cell with the other label: the button is as wide as the
            widest, so the fields beside it do not move when a request goes out (they reflowed by 45 px). The label that is not said is invisible
            and out of the accessibility tree: the name is the one said. A press while busy is ignored. */}
        <Button
          type="submit"
          {...(busy ? { "aria-busy": true, "aria-disabled": true } : {})}
          onClick={(event) => {
            if (busy) event.preventDefault();
          }}
        >
          <span className="grid justify-items-center">
            <span aria-hidden={busy ? "true" : undefined} className={cn("col-start-1 row-start-1", busy && "invisible")}>
              Show served text
            </span>
            <span aria-hidden={busy ? undefined : "true"} className={cn("col-start-1 row-start-1 inline-flex items-center gap-1.5", !busy && "invisible")}>
              <Spinner />
              Asking the config-server…
            </span>
          </span>
        </Button>
      </div>
      <p className="m-0 basis-full text-xs text-muted-foreground">
        The application is the name a service asks the config-server for. Leave the channel empty for a request without one. The hub asks the config-server in this swimlane&apos;s cluster
        and shows its answer as it is; nothing is guessed.
      </p>
    </form>
  );
}

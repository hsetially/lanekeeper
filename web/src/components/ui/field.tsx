import { createContext, useCallback, useContext, useEffect, useId, useMemo, useState, type ComponentProps } from "react";
import { Slot } from "radix-ui";
import { cn } from "@/lib/utils";
import { Label } from "./label";

type FieldContextValue = {
  controlId: string;
  invalid: boolean;
  disabled: boolean;
  describedBy: string | undefined;
  register: (id: string) => () => void;
};

const FieldContext = createContext<FieldContextValue | null>(null);

function useField(part: string): FieldContextValue {
  const ctx = useContext(FieldContext);
  if (!ctx) throw new Error(`${part} must be rendered inside <Field>`);
  return ctx;
}

type FieldProps = ComponentProps<"div"> & { invalid?: boolean; disabled?: boolean };

/**
 * Label, control, description and error as one unit. FieldControl gives the control its id, aria-invalid
 * and aria-describedby, so the validation wiring the handoff asks for cannot be forgotten.
 */
export function Field({ invalid = false, disabled = false, className, children, ...props }: FieldProps) {
  const controlId = useId();
  const [ids, setIds] = useState<readonly string[]>([]);
  const register = useCallback((id: string) => {
    setIds((prev) => (prev.includes(id) ? prev : [...prev, id]));
    return () => setIds((prev) => prev.filter((x) => x !== id));
  }, []);
  const value = useMemo<FieldContextValue>(
    () => ({ controlId, invalid, disabled, describedBy: ids.length > 0 ? ids.join(" ") : undefined, register }),
    [controlId, invalid, disabled, ids, register],
  );
  return (
    <FieldContext.Provider value={value}>
      <div data-slot="field" data-invalid={invalid || undefined} data-disabled={disabled || undefined} className={cn("flex flex-col gap-1", className)} {...props}>
        {children}
      </div>
    </FieldContext.Provider>
  );
}

export function FieldGroup({ className, ...props }: ComponentProps<"div">) {
  return <div data-slot="field-group" className={cn("flex flex-col gap-3", className)} {...props} />;
}

export function FieldLabel({ className, ...props }: ComponentProps<typeof Label>) {
  const { controlId } = useField("FieldLabel");
  return <Label htmlFor={controlId} className={className} {...props} />;
}

export function FieldControl({ children }: { children: React.ReactNode }) {
  const { controlId, invalid, disabled, describedBy } = useField("FieldControl");
  return (
    <Slot.Root id={controlId} {...(invalid ? { "aria-invalid": true } : {})} {...(describedBy ? { "aria-describedby": describedBy } : {})} {...(disabled ? { disabled: true } : {})}>
      {children}
    </Slot.Root>
  );
}

function useRegistered(part: string, idProp: string | undefined): string {
  const { register } = useField(part);
  const generated = useId();
  const id = idProp ?? generated;
  useEffect(() => register(id), [register, id]);
  return id;
}

export function FieldDescription({ className, id: idProp, ...props }: ComponentProps<"p">) {
  const id = useRegistered("FieldDescription", idProp);
  return <p data-slot="field-description" id={id} className={cn("text-xs text-muted-foreground", className)} {...props} />;
}

function VisibleError({ className, id: idProp, ...props }: ComponentProps<"p">) {
  const id = useRegistered("FieldError", idProp);
  return <p data-slot="field-error" id={id} role="alert" className={cn("text-xs text-danger-fg", className)} {...props} />;
}

/** Shown only while the field is invalid, and then announced. */
export function FieldError(props: ComponentProps<"p">) {
  const { invalid } = useField("FieldError");
  return invalid ? <VisibleError {...props} /> : null;
}

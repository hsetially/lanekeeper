import type { ComponentProps, MouseEvent } from "react";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";

// The read-only field that shows a one-time secret. Whoever cannot use the copy button next to it can still take the whole token:
// focus, a click and a double click each select all of it (the browser's own double click stops at a hyphen or a dot, and a token
// has both). It is not completed, not spell-checked and set in mono. It is one of the secret displays (secret/README in the
// imports test): only they import it.

type SecretFieldProps = Omit<ComponentProps<typeof Input>, "readOnly" | "type" | "autoComplete" | "spellCheck" | "defaultValue" | "value" | "onChange"> & {
  /** The secret, for as long as it is shown. */
  value: string;
};

export function SecretField({ className, onFocus, onClick, onDoubleClick, value, ...props }: SecretFieldProps) {
  const selectAll = (event: { currentTarget: HTMLInputElement }): void => {
    event.currentTarget.select();
  };
  return (
    <Input
      readOnly
      autoComplete="off"
      spellCheck={false}
      value={value}
      className={cn("font-mono", className)}
      onFocus={(event) => {
        selectAll(event);
        onFocus?.(event);
      }}
      onClick={(event: MouseEvent<HTMLInputElement>) => {
        selectAll(event);
        onClick?.(event);
      }}
      onDoubleClick={(event: MouseEvent<HTMLInputElement>) => {
        selectAll(event);
        onDoubleClick?.(event);
      }}
      {...props}
    />
  );
}

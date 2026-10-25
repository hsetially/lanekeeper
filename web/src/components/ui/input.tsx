import type { ComponentProps } from "react";
import { Search } from "@/icons";
import { cn } from "@/lib/utils";
import { Kbd } from "./kbd";

// 28px, 4px radius, a 1px control-boundary border (3:1). Hover darkens the border; focus adds a 1px inner ring
// on top of the global 2px focus outline; invalid is a destructive border with aria-invalid.
const controlClasses =
  "h-7 w-full min-w-0 rounded-sm border border-input bg-background px-2 text-sm text-foreground transition-colors placeholder:text-muted-foreground hover:border-muted-foreground focus-visible:border-ring focus-visible:shadow-[0_0_0_1px_var(--ring)] disabled:cursor-not-allowed disabled:bg-muted disabled:opacity-55 aria-invalid:border-destructive aria-invalid:shadow-[0_0_0_1px_var(--destructive)]";

export function Input({ className, type = "text", ...props }: ComponentProps<"input">) {
  return <input data-slot="input" type={type} className={cn(controlClasses, className)} {...props} />;
}

type Toggle = {
  id: string;
  /** Accessible name and tooltip: "Match case". */
  label: string;
  /** What the 24x22 button shows: "Aa", ".*". */
  short: string;
  pressed: boolean;
  onPressedChange: (pressed: boolean) => void;
};

type SearchInputProps = Omit<ComponentProps<"input">, "type"> & {
  /** Key hint at the end ("/"), decorative. */
  kbd?: string;
  toggles?: readonly Toggle[];
  /** Classes for the outer box. */
  boxClassName?: string;
};

/** Input with a leading search icon, an optional key hint and toggle buttons (Match case, Regex). */
export function SearchInput({ className, boxClassName, kbd, toggles = [], ...props }: SearchInputProps) {
  return (
    <div
      data-slot="search-input"
      className={cn(
        "flex h-7 w-full min-w-0 items-center gap-1.5 rounded-sm border border-input bg-background transition-colors hover:border-muted-foreground has-[input:focus-visible]:border-ring has-[input:focus-visible]:shadow-[0_0_0_1px_var(--ring)] has-[input:focus-visible]:outline-2 has-[input:focus-visible]:outline-offset-2 has-[input:focus-visible]:outline-ring has-[input:disabled]:cursor-not-allowed has-[input:disabled]:bg-muted has-[input:disabled]:opacity-55",
        // The design's search field pads 4px (not 8) after a key hint or a toggle, which have a padding of their own.
        kbd || toggles.length > 0 ? "pr-1 pl-2" : "px-2",
        boxClassName,
      )}
    >
      <Search aria-hidden="true" className="size-3.5 shrink-0 text-muted-foreground" />
      <input type="search" className={cn("h-full min-w-0 flex-1 appearance-none bg-transparent text-sm text-foreground outline-none placeholder:text-muted-foreground focus-visible:outline-none", className)} {...props} />
      {toggles.map((t) => (
        <button
          key={t.id}
          type="button"
          aria-label={t.label}
          title={t.label}
          aria-pressed={t.pressed}
          onClick={() => t.onPressedChange(!t.pressed)}
          className="inline-flex h-[22px] w-6 shrink-0 cursor-pointer items-center justify-center rounded-sm border border-transparent bg-transparent font-mono text-xs font-medium text-muted-foreground transition-colors hover:bg-accent aria-pressed:border-input aria-pressed:bg-selection aria-pressed:text-foreground"
        >
          {t.short}
        </button>
      ))}
      {kbd ? <Kbd aria-hidden="true">{kbd}</Kbd> : null}
    </div>
  );
}

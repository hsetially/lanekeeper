import { LiveIndicator } from "@/components/app/LiveIndicator";
import { Kbd } from "@/components/ui/kbd";
import { Search } from "@/icons";
import { Breadcrumb } from "./Breadcrumb";
import { useShell } from "./ShellContext";
import { SwimlaneSwitcher } from "./SwimlaneSwitcher";

// The header (the Shell): the breadcrumb, the swimlane switcher, the search button with its key, and the live pill. The
// design also draws a notifications bell; the contract has no notifications to put behind it (CG14), so there is none
// (design/DEVIATIONS.md). The search button narrows below 1400 px, as the design's 1280 px frames do, and the breadcrumb gives
// way before anything else does.

export function Header() {
  const { openPalette, preloadPalette } = useShell();
  return (
    <header data-slot="header" className="relative z-[6] flex h-11 flex-none items-center gap-3 border-b border-border bg-background px-4">
      <Breadcrumb />
      <SwimlaneSwitcher />
      <button
        type="button"
        aria-keyshortcuts="Control+K Meta+K"
        onClick={openPalette}
        onPointerEnter={preloadPalette}
        onFocus={preloadPalette}
        className="inline-flex h-7 w-[168px] flex-none cursor-pointer items-center gap-2 rounded-sm border border-input bg-background py-0 pr-1.5 pl-2 text-left text-muted-foreground hover:bg-accent min-[1400px]:w-60"
      >
        <Search aria-hidden="true" className="size-3.5 shrink-0" />
        <span className="min-w-0 flex-1 truncate">Search or jump to…</span>
        <Kbd>⌘K</Kbd>
      </button>
      <LiveIndicator />
    </header>
  );
}

/** The header while the session loads: the same height and border, so that nothing moves when it arrives. */
export function HeaderSkeleton() {
  return <header aria-hidden="true" data-slot="header" className="relative z-[6] h-11 flex-none border-b border-border bg-background" />;
}

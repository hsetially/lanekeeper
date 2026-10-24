import { Button } from "@/components/ui/button";
import { ShieldAlert } from "@/icons";
import type { Chain } from "./model";

// "Integrity" (the design's panel above the log). The design shows the result of the hub's own verification run: when it ran, the
// signed checkpoint, the key. The contract has no operation for any of that, so this panel never says the log is fine: nothing
// here verifies it, and an "all clear" would read as if something had.
//
// What it does say is a finding, when there is one. The browser compared the previous hash each loaded event stores with the hash of
// the event right before it, for events that follow each other. A link that was cut is a fact worth showing, and the panel names it
// and jumps to it; it also says that this compares hashes the hub sent, and is not the hub's own verification. With no broken link
// the panel is not there.

type Props = {
  chain: Chain;
  onShow: (id: number) => void;
};

const n = (value: number): string => value.toLocaleString("en-US");

export function IntegrityPanel({ chain, onShow }: Props) {
  // The first break in chain order: the lowest number is where the chain first fails.
  const first = [...chain.breaks.entries()].map(([id, b]) => ({ id, ...b })).sort((a, b) => a.previous - b.previous)[0];
  if (first === undefined) return null;
  return (
    <section aria-label="Integrity" className="mb-3 flex flex-none flex-col overflow-hidden rounded-md border border-danger-border bg-card">
      <div className="flex min-h-11 flex-wrap items-center gap-3 bg-danger-bg py-1.5 pr-2 pl-3 text-danger-fg">
        <ShieldAlert aria-hidden="true" className="size-4 shrink-0" />
        <div className="flex min-w-0 flex-col">
          <b className="text-[13px] leading-[18px] font-semibold">Chain break at event {n(first.previous + 1)}</b>
          <span className="text-xs leading-4">It stores a previous hash that is not the hash of event {n(first.previous)}. This compares hashes the hub sent; it is not the hub's own verification.</span>
        </div>
        <Button
          variant="secondary"
          size="sm"
          className="ml-auto"
          onClick={() => {
            onShow(first.id);
          }}
        >
          Show event {n(first.previous + 1)}
        </Button>
      </div>
    </section>
  );
}

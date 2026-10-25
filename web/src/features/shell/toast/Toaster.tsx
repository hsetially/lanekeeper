import { useEffect, useState, useSyncExternalStore } from "react";
import { Button } from "@/components/ui/button";
import { CircleCheck, CircleX, Info, TriangleAlert, X } from "@/icons";
import { cn } from "@/lib/utils";
import { isPlainClick, useNavigateTo } from "@/routes/AppLink";
import { toasts, type ToastItem, type ToastKind, type ToastStore } from "./store";

// The toaster (the design's Toast: 400 px, bottom right, a popover surface, an icon, a title, one link, Dismiss). It is our own
// component, not `sonner`, which injects a <style> that the CSP refuses (S12; design/DEVIATIONS.md).
//
// The region is persistent and polite, so a toast that is inserted is announced; each toast has the kind in words for
// assistive technology as well as in its icon and colour. A toast stays while the pointer is over it or focus is inside it,
// and goes when its time is up; an error has no time.

const KIND: Record<ToastKind, { Icon: typeof Info; word: string; tone: string }> = {
  info: { Icon: Info, word: "Notice", tone: "text-info-fg" },
  success: { Icon: CircleCheck, word: "Success", tone: "text-success-fg" },
  warning: { Icon: TriangleAlert, word: "Warning", tone: "text-warning-fg" },
  danger: { Icon: CircleX, word: "Error", tone: "text-danger-fg" },
};

function ToastCard({ item, store, onNavigate }: { item: ToastItem; store: ToastStore; onNavigate: ((href: string) => void) | undefined }) {
  const [held, setHeld] = useState(false);
  const { Icon, word, tone } = KIND[item.kind];
  const Shown = item.icon ?? Icon;

  useEffect(() => {
    if (held || item.durationMs === null) return undefined;
    const timer = setTimeout(() => {
      store.dismiss(item.id);
    }, item.durationMs);
    return () => {
      clearTimeout(timer);
    };
  }, [held, item.durationMs, item.id, store]);

  const { action } = item;
  return (
    <div
      data-slot="toast"
      data-kind={item.kind}
      onMouseEnter={() => {
        setHeld(true);
      }}
      onMouseLeave={() => {
        setHeld(false);
      }}
      onFocus={() => {
        setHeld(true);
      }}
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget)) setHeld(false);
      }}
      className="pointer-events-auto flex w-[400px] max-w-[calc(100vw-2rem)] items-start gap-2 rounded-md border border-border bg-popover px-3 py-2.5 text-popover-foreground shadow-popover animate-lk-pop-in"
    >
      <Shown aria-hidden="true" className={cn("mt-0.5 size-3.5 shrink-0", tone)} />
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <span className="sr-only">{word}: </span>
        <strong className="min-w-0 font-semibold break-words">{item.title}</strong>
        {item.detail ? <span className="text-xs text-muted-foreground">{item.detail}</span> : null}
        {action && "href" in action ? (
          <a
            href={action.href}
            className="w-fit text-xs font-medium"
            onClick={(event) => {
              // A plain click is a route change inside the app; anything else (a new tab) is the browser's.
              if (onNavigate && isPlainClick(event)) {
                event.preventDefault();
                onNavigate(action.href);
              }
              store.dismiss(item.id);
            }}
          >
            {action.label}
          </a>
        ) : action ? (
          <button
            type="button"
            className="w-fit cursor-pointer text-xs font-medium text-link underline-offset-2 hover:underline"
            onClick={() => {
              action.onClick();
              store.dismiss(item.id);
            }}
          >
            {action.label}
          </button>
        ) : null}
      </div>
      <Button
        variant="ghost"
        size="icon-sm"
        aria-label="Dismiss"
        className="size-5"
        onClick={() => {
          store.dismiss(item.id);
        }}
      >
        <X />
      </Button>
    </div>
  );
}

/** The toaster over a store (a test passes its own). */
export function ToasterView({ store, onNavigate }: { store: ToastStore; onNavigate?: (href: string) => void }) {
  const items = useSyncExternalStore(store.subscribe, store.getSnapshot, store.getSnapshot);
  return (
    <div role="region" aria-label="Notifications" data-slot="toaster" className="pointer-events-none fixed right-4 bottom-4 z-[45] flex flex-col items-end gap-2">
      <div aria-live="polite" className="flex flex-col items-end gap-2">
        {items.map((item) => (
          <ToastCard key={item.id} item={item} store={store} onNavigate={onNavigate} />
        ))}
      </div>
    </div>
  );
}

/** The app's toaster. Mount once, in the shell. */
export function Toaster() {
  return <ToasterView store={toasts} onNavigate={useNavigateTo()} />;
}

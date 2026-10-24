import { useLayoutEffect, useSyncExternalStore } from "react";
import { hasDirty, subscribeDirty } from "@/auth/dirty";
import { Banner } from "@/components/app/Banner";
import { Button } from "@/components/ui/button";
import { claimNewVersionScreen, useNewVersionScreenShown, useStale } from "@/lib/deploy/stale";

// The prompt a stale tab shows after a deploy. "Reload" fetches the new build; it is held back while a screen has unsaved
// work (the dirty registry, src/auth/dirty.ts), because a reload would throw the work away. The button stays focusable and
// says why (aria-disabled, not `disabled`), as every gated control of the app does.

const reloadPage = (): void => {
  window.location.reload();
};

function useDirty(): boolean {
  return useSyncExternalStore(subscribeDirty, hasDirty, () => false);
}

function ReloadButton({ reload, dirty }: { reload: () => void; dirty: boolean }) {
  return (
    <Button
      aria-disabled={dirty || undefined}
      onClick={() => {
        if (!dirty) reload();
      }}
    >
      Reload
    </Button>
  );
}

const WHY_HELD = "You have unsaved changes. Save or copy them first, then reload.";

/** Shown when a chunk failed to preload. Nothing while the build is current. */
export function NewVersionBanner({ reload = reloadPage }: { reload?: () => void }) {
  const stale = useStale();
  const screenShown = useNewVersionScreenShown();
  const dirty = useDirty();
  // The full-page screen has the same words and the same button.
  if (!stale || screenShown) return null;
  return (
    <Banner variant="info" title="A new version is available" action={<ReloadButton reload={reload} dirty={dirty} />}>
      {dirty ? WHY_HELD : "Reload to get it. This page may not work until you do."}
    </Banner>
  );
}

/** The whole page, for a route whose chunk could not be fetched. */
export function NewVersionScreen({ reload = reloadPage }: { reload?: () => void }) {
  const dirty = useDirty();
  // Before the first paint, so that the banner is never seen beside the screen.
  useLayoutEffect(() => claimNewVersionScreen(), []);
  return (
    <div className="flex flex-col gap-3 p-6">
      <h1 className="text-lg font-semibold">A new version is available</h1>
      <p className="text-muted-foreground">{dirty ? WHY_HELD : "This page needs files that the new version replaced. Reload to get them."}</p>
      <div>
        <ReloadButton reload={reload} dirty={dirty} />
      </div>
    </div>
  );
}

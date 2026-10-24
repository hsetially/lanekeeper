import { useCallback, useEffect, useState } from "react";
import { Banner } from "@/components/app/Banner";
import { CodeView } from "@/components/app/virtual/CodeView";
import { Button } from "@/components/ui/button";
import { loadEditor, type MonacoApi } from "./loader";

// What Edit mode shows when the editor's chunk cannot be fetched (a deploy removed it, or the network dropped): the file,
// read-only, in the same CodeView the file view uses, and an alert that says why. Never a blank pane. The page-wide banner
// says "A new version is available" when the cause is a deploy (src/lib/deploy/stale.ts); this says what it means here.

export type EditorModule = { state: "loading" } | { state: "ready"; monaco: MonacoApi } | { state: "failed"; retry: () => void };

/** Load the editor chunk; `load` is the lazy loader, replaceable in a test. */
export function useEditorModule(load: () => Promise<MonacoApi> = loadEditor): EditorModule {
  const [attempt, setAttempt] = useState(0);
  const [result, setResult] = useState<{ attempt: number; monaco: MonacoApi } | { attempt: number; failed: true } | null>(null);
  const retry = useCallback(() => setAttempt((n) => n + 1), []);

  useEffect(() => {
    let current = true;
    load().then(
      (monaco) => {
        if (current) setResult({ attempt, monaco });
      },
      () => {
        if (current) setResult({ attempt, failed: true });
      },
    );
    return () => {
      current = false;
    };
  }, [attempt, load]);

  if (result === null || result.attempt !== attempt) return { state: "loading" };
  return "monaco" in result ? { state: "ready", monaco: result.monaco } : { state: "failed", retry };
}

export function EditorFallback({ label, text, onRetry }: { label: string; text: string; onRetry: () => void }) {
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <Banner variant="danger" title="The editor could not be loaded" action={<Button onClick={onRetry}>Try again</Button>}>
        The file is shown read-only. Your changes cannot be made here until the editor loads.
      </Banner>
      <CodeView label={label} text={text} className="flex-1" />
    </div>
  );
}

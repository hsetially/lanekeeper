import { useEffect, useRef, useState } from "react";
import { EditorFallback, useEditorModule } from "@/editor/fallback";
import { EDITOR_OPTIONS, type MonacoApi } from "@/editor/loader";
import { Skeleton } from "@/components/ui/skeleton";
import { Spinner } from "@/components/ui/spinner";
import type { Language } from "../model";

/** The dark theme of the editor: `vs-dark` with a comment colour that reads. */
const DARK_THEME = "lk-dark";

// Edit mode's surface: Monaco, which is only ever loaded through src/editor (a lazy chunk, option (e) of the Monaco decision),
// made into an editor on a box the page sizes. It reports the text and nothing else leaves it. If the chunk cannot be fetched
// (a deploy removed it, the network dropped) the file is shown read-only with an alert and a retry, never a blank pane.
//
// The editor is made once for a file and kept: a re-render with new markers marks lines, it does not rebuild the editor, and
// what the person typed is the model's own, so nothing a re-render does can lose it.

export type EditorMarker = { line: number; severity: "error" | "warning"; message: string };

export type EditSurfaceProps = {
  /** The file's name: the fallback table's name, and the editor's accessible name. */
  label: string;
  /** The text the editor starts with. Read once, when the editor is made. */
  initialText: string;
  language: Language;
  markers: readonly EditorMarker[];
  onTextChange: (text: string) => void;
  /** The loader of the editor chunk: a test's own. */
  load?: () => Promise<MonacoApi>;
};

const OWNER = "lanekeeper";

/** Whether the app is in its dark theme (the class the theme switch puts on the document). */
function useIsDark(): boolean {
  const [dark, setDark] = useState(() => document.documentElement.classList.contains("dark"));
  useEffect(() => {
    const observer = new MutationObserver(() => setDark(document.documentElement.classList.contains("dark")));
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ["class"] });
    return () => observer.disconnect();
  }, []);
  return dark;
}

type Editor = ReturnType<MonacoApi["editor"]["create"]>;
type Model = ReturnType<MonacoApi["editor"]["createModel"]>;

function Host({ monaco, label, initialText, language, markers, onTextChange }: Omit<EditSurfaceProps, "load"> & { monaco: MonacoApi }) {
  const box = useRef<HTMLDivElement>(null);
  const [made, setMade] = useState<{ editor: Editor; model: Model } | null>(null);
  const start = useRef(initialText);
  const latest = useRef(onTextChange);
  useEffect(() => {
    latest.current = onTextChange;
  });
  const dark = useIsDark();

  useEffect(() => {
    const node = box.current;
    if (!node) return undefined;
    const model = monaco.editor.createModel(start.current, language);
    const editor = monaco.editor.create(node, { ...EDITOR_OPTIONS, model, readOnly: false, ariaLabel: `Editor for ${label}` });
    const typed = model.onDidChangeContent(() => latest.current(model.getValue()));
    // The options turn automatic layout off (it would poll the box); the box's own size is watched instead.
    const resize = new ResizeObserver(() => editor.layout());
    resize.observe(node);
    editor.layout();
    editor.focus();
    setMade({ editor, model });
    return () => {
      resize.disconnect();
      typed.dispose();
      editor.dispose();
      model.dispose();
      setMade(null);
    };
  }, [monaco, language, label]);

  useEffect(() => {
    // Monaco's own dark theme draws comments in #608B4E on #1E1E1E: 4.2:1, under the 4.5:1 that text needs (axe `color-contrast`). The
    // green of VS Code's Dark+ is 5.0:1. Everything else stays as `vs-dark` has it.
    monaco.editor.defineTheme(DARK_THEME, { base: "vs-dark", inherit: true, rules: [{ token: "comment", foreground: "6A9955" }], colors: {} });
    monaco.editor.setTheme(dark ? DARK_THEME : "vs");
  }, [monaco, dark, made]);

  useEffect(() => {
    if (!made) return;
    const { model } = made;
    const lines = model.getLineCount();
    // Only the app's own words are given to the editor, never the text of a line. A line past the end is marked on the last one.
    monaco.editor.setModelMarkers(
      model,
      OWNER,
      markers.map((m) => {
        const line = Math.min(Math.max(1, m.line), lines);
        return {
          severity: m.severity === "error" ? monaco.MarkerSeverity.Error : monaco.MarkerSeverity.Warning,
          startLineNumber: line,
          startColumn: 1,
          endLineNumber: line,
          endColumn: model.getLineMaxColumn(line),
          message: m.message,
        };
      }),
    );
  }, [monaco, made, markers]);

  return <div ref={box} className="min-h-0 flex-1 overflow-hidden" />;
}

export function EditSurface({ load, ...props }: EditSurfaceProps) {
  const module = useEditorModule(load);
  if (module.state === "loading") {
    return (
      <div role="status" aria-busy="true" className="flex min-h-0 flex-1 flex-col gap-2 p-4">
        <span className="flex items-center gap-2 text-sm text-muted-foreground">
          <Spinner />
          Loading the editor…
        </span>
        <Skeleton className="h-3 w-2/5" />
        <Skeleton className="h-3 w-3/5" />
        <Skeleton className="h-3 w-1/2" />
      </div>
    );
  }
  if (module.state === "failed") return <EditorFallback label={props.label} text={props.initialText} onRetry={module.retry} />;
  return <Host monaco={module.monaco} {...props} />;
}

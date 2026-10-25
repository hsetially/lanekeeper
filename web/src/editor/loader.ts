import { installEditorEnvironment } from "./environment";
import { EDITOR_OPTIONS } from "./options";

// The one way in to Monaco. A dynamic import, so the editor (about 4 MB, 1 MB gzipped) is a chunk that loads only when a
// screen asks for it (Edit mode, T4), never in the initial JavaScript (P10). T2 adds the fetch-only warm-up for editors.

export type MonacoApi = typeof import("./entry");

let loading: Promise<MonacoApi> | undefined;

/**
 * Load Monaco, with the environment installed first. Safe to call from several places: one load, one answer.
 * A failed load is not remembered, so a retry can succeed (a stale chunk after a deploy, a network blip).
 */
export function loadEditor(): Promise<MonacoApi> {
  loading ??= (async () => {
    installEditorEnvironment();
    const monaco = await import("./entry");
    // JSON highlighting needs no language service (and its worker): tokens only.
    monaco.json.jsonDefaults.setModeConfiguration({ documentFormattingEdits: false, documentRangeFormattingEdits: false, completionItems: false, hovers: false, documentSymbols: false, tokens: true, colors: false, foldingRanges: false, diagnostics: false, selectionRanges: false });
    monaco.json.jsonDefaults.setDiagnosticsOptions({ validate: false });
    return monaco;
  })().catch((error: unknown) => {
    loading = undefined;
    throw error;
  });
  return loading;
}

export { EDITOR_OPTIONS };

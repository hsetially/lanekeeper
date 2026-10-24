// The Monaco entry: the editor core and the contributions Lanekeeper uses, and nothing else. Loaded only through
// src/editor/loader.ts, after src/editor/environment.ts has run (so the Trusted Types hook is in place before any Monaco
// module is evaluated).
//
// Derived from monaco-editor 0.57.0's esm/vs/editor/editor.main.js, which imports every contribution and 80 languages. Here
// only YAML, XML, `ini` (Java properties) and the JSON language feature are kept; plain text is built in.
//
// LEFT OUT ON PURPOSE (decision of 2026-10-17, plan W3 option (e)): every contribution that renders Markdown, because the
// Markdown renderer asks for a Trusted Types policy of its own (`dompurify`), which the hub's CSP does not allow, and parses
// HTML with DOMParser, which Trusted Types refuses. Hover, suggest and its inline completions, parameter hints, inline
// completions, the colour picker, links, code lens, inlay hints, and the go-to-symbol peek (go to definition and
// references, the symbol quick access). The editor options in options.ts also turn the features off, but turning an option
// off does not remove the command (Ctrl+Space still opened the suggest widget in G0): leaving the module out does.
//
// `e2e/editor/csp.spec.ts` measures this entry under the production CSP on every Monaco upgrade. A new Monaco version can add
// a contribution that renders Markdown or writes a style attribute some other way; the spec is what finds out.
import * as json from "monaco-editor/languages/features/json/register.js";
export { json };
import "monaco-editor/languages/definitions/ini/register.js";
import "monaco-editor/languages/definitions/xml/register.js";
import "monaco-editor/languages/definitions/yaml/register.js";
import "monaco-editor/editor/contrib/anchorSelect/browser/anchorSelect.js";
import "monaco-editor/editor/contrib/bracketMatching/browser/bracketMatching.js";
import "monaco-editor/editor/contrib/caretOperations/browser/transpose.js";
import "monaco-editor/editor/contrib/clipboard/browser/clipboard.js";
import "monaco-editor/editor/contrib/codeAction/browser/codeActionContributions.js";
import "monaco-editor/editor/browser/widget/codeEditor/codeEditorWidget.js";
import "../../node_modules/monaco-editor/esm/vs/base/browser/ui/codicons/codicon/codicon.css";
import "monaco-editor/editor/contrib/comment/browser/comment.js";
import "monaco-editor/editor/contrib/contextmenu/browser/contextmenu.js";
import "monaco-editor/editor/contrib/cursorUndo/browser/cursorUndo.js";
import "monaco-editor/editor/browser/widget/diffEditor/diffEditor.contribution.js";
import "monaco-editor/editor/contrib/diffEditorBreadcrumbs/browser/contribution.js";
import "monaco-editor/editor/contrib/dnd/browser/dnd.js";
import "monaco-editor/editor/contrib/documentSymbols/browser/documentSymbols.js";
import "monaco-editor/editor/contrib/dropOrPasteInto/browser/dropIntoEditorContribution.js";
import "monaco-editor/features/find/register.js";
import "monaco-editor/editor/contrib/floatingMenu/browser/floatingMenu.contribution.js";
import "monaco-editor/editor/contrib/folding/browser/folding.js";
import "monaco-editor/editor/contrib/fontZoom/browser/fontZoom.js";
import "monaco-editor/editor/contrib/format/browser/formatActions.js";
import "monaco-editor/editor/contrib/gotoError/browser/gotoError.js";
import "monaco-editor/editor/standalone/browser/quickAccess/standaloneGotoLineQuickAccess.js";
import "monaco-editor/editor/contrib/gpu/browser/gpuActions.js";
import "monaco-editor/editor/contrib/indentation/browser/indentation.js";
import "monaco-editor/editor/contrib/inlineProgress/browser/inlineProgress.js";
import "monaco-editor/editor/contrib/inPlaceReplace/browser/inPlaceReplace.js";
import "monaco-editor/editor/contrib/insertFinalNewLine/browser/insertFinalNewLine.js";
import "monaco-editor/editor/standalone/browser/inspectTokens/inspectTokens.js";
import "monaco-editor/editor/standalone/browser/iPadShowKeyboard/iPadShowKeyboard.js";
import "monaco-editor/editor/contrib/lineSelection/browser/lineSelection.js";
import "monaco-editor/editor/contrib/linesOperations/browser/linesOperations.js";
import "monaco-editor/editor/contrib/linkedEditing/browser/linkedEditing.js";
import "monaco-editor/editor/contrib/longLinesHelper/browser/longLinesHelper.js";
import "monaco-editor/editor/contrib/middleScroll/browser/middleScroll.contribution.js";
import "monaco-editor/editor/contrib/multicursor/browser/multicursor.js";
import "monaco-editor/editor/contrib/placeholderText/browser/placeholderText.contribution.js";
import "monaco-editor/editor/standalone/browser/quickAccess/standaloneCommandsQuickAccess.js";
import "monaco-editor/editor/standalone/browser/quickAccess/standaloneHelpQuickAccess.js";
import "monaco-editor/editor/contrib/readOnlyMessage/browser/contribution.js";
import "monaco-editor/editor/contrib/rename/browser/rename.js";
import "monaco-editor/editor/contrib/sectionHeaders/browser/sectionHeaders.js";
import "monaco-editor/editor/contrib/semanticTokens/browser/viewportSemanticTokens.js";
import "monaco-editor/editor/contrib/smartSelect/browser/smartSelect.js";
import "monaco-editor/editor/contrib/snippet/browser/snippetController2.js";
import "monaco-editor/editor/contrib/stickyScroll/browser/stickyScrollContribution.js";
import "monaco-editor/editor/standalone/browser/toggleHighContrast/toggleHighContrast.js";
import "monaco-editor/editor/contrib/toggleTabFocusMode/browser/toggleTabFocusMode.js";
import "monaco-editor/editor/contrib/tokenization/browser/tokenization.js";
import "monaco-editor/editor/contrib/unicodeHighlighter/browser/unicodeHighlighter.js";
import "monaco-editor/editor/contrib/unusualLineTerminators/browser/unusualLineTerminators.js";
import "monaco-editor/editor/contrib/wordHighlighter/browser/wordHighlighter.js";
import "monaco-editor/editor/contrib/wordOperations/browser/wordOperations.js";
import "monaco-editor/editor/contrib/wordPartOperations/browser/wordPartOperations.js";
import "monaco-editor/editor/browser/coreCommands.js";
import "monaco-editor/editor/contrib/caretOperations/browser/caretOperations.js";
import "monaco-editor/editor/contrib/dropOrPasteInto/browser/copyPasteContribution.js";
import "monaco-editor/editor/contrib/find/browser/findController.js";
import "monaco-editor/editor/contrib/gotoError/browser/markerSelectionStatus.js";
import "monaco-editor/editor/contrib/semanticTokens/browser/documentSemanticTokens.js";
import "monaco-editor/editor/common/standaloneStrings.js";
import "../../node_modules/monaco-editor/esm/vs/base/browser/ui/codicons/codicon/codicon-modifiers.css";
export { CancellationTokenSource, Emitter, KeyCode, KeyMod, MarkerSeverity, MarkerTag, Position, Range, Selection, SelectionDirection, Token, Uri, editor, languages } from "monaco-editor/editor/editor.api.js";

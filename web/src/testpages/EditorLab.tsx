// TEST PAGE, never in the production dist (its route exists only in `vite build --mode perf`). It loads the editor exactly as the
// app will (src/editor/loader.ts) and creates the editors that e2e/editor/csp.spec.ts measures under the production CSP:
// a plain editor, a diff editor (side by side and inline) and one editor per language. It exposes what the spec needs on
// `window.__lab`. The box of the editor is sized through CSSOM below, never with a style attribute (the CSP refuses it).
import { useEffect, useRef } from "react";
import { installEditorEnvironment } from "@/editor/environment";
import { EDITOR_OPTIONS, loadEditor, type MonacoApi } from "@/editor/loader";

type Lab = {
  ready: boolean;
  error: string | null;
  monaco?: MonacoApi;
  editor?: ReturnType<MonacoApi["editor"]["create"]>;
  diff?: ReturnType<MonacoApi["editor"]["createDiffEditor"]>;
  askedNames: () => readonly string[];
  setLanguage?: (language: string) => string;
  samples: Record<string, string>;
};

const yaml = (): string => {
  const lines = ["# synthetic config, not a real tenant", "spring:", "  application:", "    name: demo-service", "  profiles:", "    active: dev"];
  for (let i = 0; i < 70; i++) lines.push(`key${i}: value-${i}  # line ${i + 7}`);
  // A zero width space, a right-to-left override and a Cyrillic letter that looks like a Latin one (S8).
  lines.splice(10, 0, 'zero_width: "a\u200bb"   # U+200B inside the quotes', 'bidi: "x\u202ey"          # U+202E');
  lines.push('cyrillic_a: "\u0430pple"       # Cyrillic a');
  return `${lines.join("\n")}\n`;
};
const YAML = yaml();
const YAML_B = YAML.replace("name: demo-service", "name: demo-service-2").replace("key3: value-3", "key3: changed").replace("key20: value-20", "key20: value-20\nadded: yes");
const samples: Record<string, string> = {
  yaml: YAML,
  json: '{\n  "name": "demo",\n  "list": [1, 2, 3],\n  "nested": { "a": true, "b": null }\n}\n',
  xml: '<?xml version="1.0"?>\n<config>\n  <entry key="a">1</entry>\n  <!-- note -->\n</config>\n',
  ini: "# properties\nserver.port=8080\nspring.application.name=demo\n[section]\nkey = value\n",
  plaintext: "plain text line one\nplain text line two\n",
};

const lab: Lab = { ready: false, error: null, askedNames: () => installEditorEnvironment().askedNames, samples };
(window as unknown as { __lab: Lab }).__lab = lab;

async function run(host: HTMLElement, say: (text: string) => void): Promise<void> {
  const params = new URLSearchParams(location.search);
  const kind = params.get("k") ?? "editor";
  const monaco = await loadEditor();
  lab.monaco = monaco;
  host.style.width = "1000px";
  host.style.height = "560px";
  if (kind === "editor") {
    const model = monaco.editor.createModel(params.get("c") === "1" ? corrupt() : YAML, "yaml");
    lab.editor = monaco.editor.create(host, { ...EDITOR_OPTIONS, model });
    lab.editor.layout({ width: 1000, height: 560 });
  } else if (kind === "diff") {
    const original = monaco.editor.createModel(YAML, "yaml");
    const modified = monaco.editor.createModel(YAML_B, "yaml");
    const diff = monaco.editor.createDiffEditor(host, { ...EDITOR_OPTIONS, renderSideBySide: params.get("m") !== "inline", originalEditable: false, readOnly: true });
    diff.setModel({ original, modified });
    diff.layout({ width: 1000, height: 560 });
    lab.diff = diff;
    lab.editor = diff.getModifiedEditor();
  } else if (kind === "langs") {
    const editor = monaco.editor.create(host, { ...EDITOR_OPTIONS, value: "" });
    editor.layout({ width: 1000, height: 560 });
    lab.editor = editor;
    lab.setLanguage = (language: string) => {
      const model = monaco.editor.createModel(samples[language] ?? "", language);
      editor.setModel(model);
      return model.getLanguageId();
    };
    lab.setLanguage("yaml");
  } else throw new Error(`unknown kind ${kind}`);
  lab.ready = true;
  say("editor created");
}

/** Model lines whose text contains `style="..."`: the editor must show them as they are (a regex rewrite changed 4 of 5 in G0). */
function corrupt(): string {
  return ['a: <div style="color:red">hi</div>', "b: x style='top:5px' y", 'c: width style = "width: 3px" end', "d: plain line", 'e: <span class="k" style="top:1px;left:2px">t</span> style="z"'].join("\n");
}

export function EditorLab() {
  const host = useRef<HTMLDivElement>(null);
  const status = useRef<HTMLParagraphElement>(null);
  const started = useRef(false);
  useEffect(() => {
    if (started.current || !host.current) return;
    started.current = true;
    const say = (text: string): void => {
      if (status.current) status.current.textContent = text;
    };
    run(host.current, say).catch((error: unknown) => {
      lab.error = error instanceof Error ? (error.stack ?? error.message) : String(error);
      say(`FAILED ${error instanceof Error ? error.message : String(error)}`);
    });
  }, []);
  return (
    <main>
      <h1 className="sr-only">Editor test page</h1>
      <p ref={status}>loading</p>
      <div ref={host} />
    </main>
  );
}

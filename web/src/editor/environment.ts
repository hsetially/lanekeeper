// Everything that must be in place BEFORE a Monaco module is evaluated (F7 of the plan): Monaco creates its Trusted Types
// policies at module evaluation, through `MonacoEnvironment.createTrustedTypesPolicy` if it is set. This module sets it, so
// that every one of Monaco's policy names (there are 11 and more) gets the ONE policy `lanekeeper-monaco`, which the hub's
// CSP allows (`trusted-types lanekeeper-docs lanekeeper-monaco`). It also installs the global patches
// (src/editor/global-patches.ts) and makes the editor's web worker from a same-origin /assets URL through the policy.
//
// src/editor/loader.ts calls `installEditorEnvironment()` first and only then imports ./entry (a dynamic import, so the
// order is the order of execution; a static import above the call would be hoisted and Monaco would create its policies
// directly, which the browser refuses).
import editorWorkerUrl from "monaco-editor/editor/editor.worker.js?worker&url";
import { readNonce } from "@/lib/csp/nonce";
import { createMonacoPolicy } from "@/lib/csp/trusted-types";
import { installGlobalPatches } from "./global-patches";
import { checkWorkerUrl, rewriteStyleAttrsInTags } from "./policy";
import { createStyleAttrName } from "./style-apply";

type Installed = { askedNames: string[] };
let installed: Installed | undefined;

/** Idempotent. Returns the names Monaco has asked the hook for so far (for the CSP spec and the tests). */
export function installEditorEnvironment(): Installed {
  if (installed) return installed;
  const styleAttr = createStyleAttrName();
  const policy = createMonacoPolicy({
    createHTML: (html) => rewriteStyleAttrsInTags(html, styleAttr),
    createScriptURL: (url) => checkWorkerUrl(url, location.origin),
  });
  installGlobalPatches({ nonce: readNonce() ?? "", styleAttr });
  const asked: string[] = [];
  globalThis.MonacoEnvironment = {
    createTrustedTypesPolicy(name: string) {
      asked.push(name);
      return policy;
    },
    getWorker(_workerId: string, label: string) {
      // One worker script, for every label (JSON is tokens-only and needs none of its own).
      return new Worker(policy.createScriptURL(editorWorkerUrl) as string | URL, { name: label });
    },
  };
  installed = { askedNames: asked };
  return installed;
}

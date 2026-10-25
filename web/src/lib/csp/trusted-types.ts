// Trusted Types (S12). The hub's CSP says `require-trusted-types-for 'script'` and `trusted-types lanekeeper-docs
// lanekeeper-monaco`: the page may create exactly these two policies, once each, and every HTML or script URL sink needs a
// value one of them made. THIS IS THE ONLY MODULE THAT CALLS `trustedTypes.createPolicy` (the lint enforces it).
//
//  - `lanekeeper-docs` makes the one HTML the app inserts: `RenderedDoc.html`, which the server sanitised (CG11). Its
//    createHTML is a pass-through of a branded value. It is safe because of three layers, not because of this function:
//    the server's allowlist, the CSP (no inline script, no off-site anything), and the fact that only
//    components/app/SanitisedDocHtml.tsx can obtain the brand.
//  - `lanekeeper-monaco` is the editor's, created in src/editor/ through createMonacoPolicy below, with the rules it passes.
//
// Where `window.trustedTypes` does not exist (not Chromium, or a test) the same functions return plain strings; the CSP of
// the hub is what protects those browsers, and the app must not crash in them.

import type { RenderedDoc } from "@/api/generated/model";

export const DOCS_POLICY_NAME = "lanekeeper-docs";
export const MONACO_POLICY_NAME = "lanekeeper-monaco";

declare const sanitisedBrand: unique symbol;
/** HTML that the server sanitised. Only `brandRenderedDoc` makes one. */
export type SanitisedHtml = string & { readonly [sanitisedBrand]: true };

/** The HTML of a server-rendered doc, branded. Takes the generated type, so no other string can be passed off as one. */
export function brandRenderedDoc(doc: Pick<RenderedDoc, "html">): SanitisedHtml {
  return doc.html as SanitisedHtml;
}

type Host = { trustedTypes?: TrustedTypePolicyFactory } | undefined;

function host(): Host {
  return typeof window === "undefined" ? undefined : window;
}

export class PolicyError extends Error {
  override readonly name = "PolicyError";
}

type Created = { name: string; createHTML(input: string): TrustedHTML | string; createScriptURL(input: string): TrustedScriptURL | string };
const created = new Map<string, Created>();

function make(name: string, rules: Required<Pick<TrustedTypePolicyOptions, "createHTML" | "createScriptURL">>): Created {
  const existing = created.get(name);
  if (existing) return existing;
  const factory = host()?.trustedTypes;
  let policy: Created;
  if (factory === undefined) {
    policy = { name, createHTML: (input) => rules.createHTML(input), createScriptURL: (input) => rules.createScriptURL(input) };
  } else {
    try {
      policy = factory.createPolicy(name, rules);
    } catch {
      // The name is not in `trusted-types`, or it was created before. Never fall back to a string: that would hide the
      // misconfiguration and then fail at the sink instead.
      throw new PolicyError(`The Trusted Types policy ${name} could not be created (is it in LK_TRUSTED_TYPES?)`);
    }
  }
  created.set(name, policy);
  return policy;
}

const refuseScriptUrl = (): never => {
  throw new PolicyError("This policy does not make script URLs");
};

/** The HTML to hand to the one sink for docs. Creates `lanekeeper-docs` on first use. */
export function docsHtml(html: SanitisedHtml): TrustedHTML | string {
  return make(DOCS_POLICY_NAME, { createHTML: (input) => input, createScriptURL: refuseScriptUrl }).createHTML(html);
}

/**
 * The editor's policy: one policy for every name Monaco asks for. The rules come from src/editor/ (the rewrite of style
 * attributes and the same-origin check on worker URLs); the creation, which must happen once, is here.
 */
export function createMonacoPolicy(rules: { createHTML: (input: string) => string; createScriptURL: (input: string) => string }): Created {
  return make(MONACO_POLICY_NAME, rules);
}

/** For tests: forget what was created. A real page creates each policy once and never resets. */
export function resetPoliciesForTests(): void {
  created.clear();
}

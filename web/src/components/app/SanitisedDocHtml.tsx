import { useMemo } from "react";
import type { RenderedDoc } from "@/api/generated/model";
import { brandRenderedDoc, docsHtml } from "@/lib/csp/trusted-types";
import { cn } from "@/lib/utils";

// THE ONE HTML SINK OF THE APP (S12). It renders `RenderedDoc.html`, the page the server rendered with pulldown-cmark and
// cleaned with its allowlist (CG11), through the Trusted Types policy `lanekeeper-docs`. Nothing else in the app may set
// inner HTML: the lint bans it everywhere except this file, and SanitisedDocHtml.test.tsx proves this is the only
// component that does and the only importer of the policy.
//
// Safety does not rest on this component. It rests on the server's allowlist (no `style`, no `class`, no event handler,
// links only https or same-origin), on the CSP (no inline script, no off-site anything) and on Trusted Types. This
// component only refuses to take anything but a `RenderedDoc`.
//
// A link in the page is the server's `href`, handed to the browser as it came. The app must never percent-decode it or
// remove its dot segments: `/%2F%2Fevil.example/` is a path on this origin until something decodes it
// (docs/interfaces.md, open item 6).
export function SanitisedDocHtml({ doc, className }: { doc: RenderedDoc; className?: string }) {
  const html = useMemo(() => docsHtml(brandRenderedDoc(doc)), [doc]);
  return <div className={cn("doc-prose", className)} data-content-hash={doc.content_hash} dangerouslySetInnerHTML={{ __html: html }} />;
}

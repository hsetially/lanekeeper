import type { RefObject } from "react";
import { CopyButton } from "./CopyButton";
import { absoluteLink } from "./copy-link";

/** A copy button for the permalink of a page of the app: `<CopyLink href={linkTo("files", { id }, { path, line })} what="link to this line" />`. Renders nothing for a path that is not the app's. */
export function CopyLink({ href, fragment, what, source, className }: { href: string; /** A heading id, for a link to a place in the page. */ fragment?: string; what: string; source?: RefObject<HTMLElement | null>; className?: string }) {
  const link = absoluteLink(href, fragment);
  if (link === null) return null;
  return <CopyButton value={link} what={what} {...(source ? { source } : {})} {...(className ? { className } : {})} />;
}

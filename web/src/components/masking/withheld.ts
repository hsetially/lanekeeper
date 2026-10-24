import type { FileClass } from "@/api/generated/model";

// Whether the server withholds the content of a file (D79, D104). A denied file (`*.jks`, `*.p12`, `*.pem`, `*.key`, ...) is known by
// name, size and hash only: its content is never read beyond hashing, never sent and never written. The server says so twice, on
// the listing entry (`FileEntry.content_withheld`) and on the content (`FileContent.content_withheld`, with `text` null), and the
// class is `denied` on both. Either signal alone is enough to withhold, so a screen that sees one never shows an action for the file.

export function isContentWithheld(file: { content_withheld?: boolean | undefined; class?: FileClass | undefined } | null | undefined): boolean {
  return file !== null && file !== undefined && (file.content_withheld === true || file.class === "denied");
}

export type WithheldReason = "denied" | "unchecked";

/**
 * Why the content is withheld, or null when it is not. A file of the denied class is one the hub never reads (a keystore, a key). A
 * file of another class whose content is withheld is one the hub could not check for secrets: the contract fails closed (S14b) and
 * sends no text, so the reason a person is told is not "sensitive file type".
 */
export function withheldReason(file: { content_withheld?: boolean | undefined; class?: FileClass | undefined } | null | undefined): WithheldReason | null {
  if (!isContentWithheld(file)) return null;
  return file?.class === "denied" ? "denied" : "unchecked";
}

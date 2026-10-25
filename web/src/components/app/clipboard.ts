// The one place the app writes to the clipboard. It never throws, never logs and never keeps what it was given: the text goes to
// `navigator.clipboard.writeText` and the answer is one of three words. A page that is not secure has no Clipboard API, a
// Permissions-Policy or the browser can refuse the write (no user gesture, no focus), and a policy can make the call throw
// before it returns a promise; all three are "can't", and the button says so (copy-feedback.ts).
//
// Only a click handler may call it: the generic button (CopyButton) and the secret one (secret/SecretCopyButton). Nothing
// copies on mount, on a timer or on a live event.

export type ClipboardResult = "ok" | "unavailable" | "denied";

export async function writeClipboard(text: string): Promise<ClipboardResult> {
  const clipboard = (navigator as { clipboard?: Clipboard }).clipboard;
  if (typeof clipboard?.writeText !== "function") return "unavailable";
  try {
    await clipboard.writeText(text);
    return "ok";
  } catch {
    return "denied";
  }
}

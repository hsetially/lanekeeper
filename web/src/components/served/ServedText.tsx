import type { ServedFile } from "@/api/generated/model";
import { EmptyState } from "@/components/app/EmptyState";
import { CodeView } from "@/components/app/virtual/CodeView";
import { MaskedTextView } from "@/components/masking/MaskedTextView";
import { isContentWithheld } from "@/components/masking/withheld";
import { lineCount } from "@/features/file/model";
import { FileImage, Lock } from "@/icons";
import { cn } from "@/lib/utils";

// The text of a served file (D86): what the config-server answered, as the hub received it, read-only. The web masks nothing (M1): where
// the hub masked a value the text holds the marker and `masks` says where, and it is drawn as a hidden value (components/masking), which
// fails closed when a mask cannot be trusted. A served mask names no setting, because the served text is not a file on NFS, so there is
// no Reveal here and this view never passes the slot that would draw one.
//
// Three answers have no text to draw, and the first wins over everything else the answer holds:
//  - the hub WITHHOLDS the file (a key, a keystore, or a scan that could not finish): `content_withheld`. An answer that says so and
//    still carries a text is a fault of the server, and the text is not drawn;
//  - the bytes are not text (`text` is null and the file is not withheld): a binary file, an encoding the hub cannot read, a size over its limit;
//  - the file is empty.
// `ServedFile` has no file class, so the view cannot tell a denied type from a scan that could not finish, and says both.

type Props = {
  served: ServedFile;
  /** The accessible name of the lines. */
  label: string;
  className?: string;
};

function StatusBar({ text }: { text: string }) {
  const lines = lineCount(text);
  return (
    <div role="group" aria-label="About this text" className="flex h-6 flex-none items-center gap-4 border-t border-border bg-muted px-3 font-mono text-xs text-muted-foreground">
      <span>
        {lines.toLocaleString("en-US")} {lines === 1 ? "line" : "lines"}
      </span>
      <span className="font-sans">Read-only view of the config-server&apos;s answer</span>
    </div>
  );
}

export function ServedText({ served, label, className }: Props) {
  if (isContentWithheld(served)) {
    return (
      <EmptyState icon={Lock} title="Content withheld" {...(className === undefined ? {} : { className })}>
        The hub does not send the text of this file. It is a type the hub never shows (a key, a keystore or a certificate), or it could not be checked for secrets, and the hub then sends nothing.
      </EmptyState>
    );
  }
  const text = served.text;
  if (text === null) {
    return (
      <EmptyState icon={FileImage} title="Not shown as text" {...(className === undefined ? {} : { className })}>
        The config-server&apos;s answer is not text the hub can show: a binary file, an encoding it cannot read, or a size over its limit.
      </EmptyState>
    );
  }
  if (text === "") {
    return (
      <EmptyState title="The config-server served an empty file" {...(className === undefined ? {} : { className })}>
        The file exists, but the answer has 0 bytes.
      </EmptyState>
    );
  }
  const masks = served.masks ?? [];
  return (
    // `min-h-48`, not `min-h-0`: the facts, the placeholders and the sources round it can take every pixel there is (the answer scrolls as one,
    // ServedResult), and a text of 0 px is not a text. This floor is what stays when they do.
    <div data-slot="served-text" className={cn("flex min-h-48 flex-1 flex-col", className)}>
      <div className="min-h-0 flex-1 bg-card pt-1">
        {masks.length > 0 ? <MaskedTextView label={label} text={text} masks={masks} className="h-full" /> : <CodeView label={label} text={text} className="h-full" />}
      </div>
      <StatusBar text={text} />
    </div>
  );
}

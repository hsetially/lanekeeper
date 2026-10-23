import type { ReactNode } from "react";
import { getGetBlobUrl } from "@/api/generated/endpoints/content-store/content-store";
import type { FileContent, FileEntry } from "@/api/generated/model";
import { EmptyState } from "@/components/app/EmptyState";
import { HashChip } from "@/components/app/chips";
import { SafeText } from "@/components/app/SafeText";
import { CodeView } from "@/components/app/virtual/CodeView";
import { AttributionChip } from "@/components/attribution";
import { MaskedTextView } from "@/components/masking/MaskedTextView";
import { isContentWithheld } from "@/components/masking/withheld";
import { Button } from "@/components/ui/button";
import { useActorNames } from "@/features/attribution";
import { RevealableMaskedText } from "@/features/masking/RevealableMaskedText";
import { FileImage } from "@/icons";
import { formatTime } from "@/lib/time";
import { eolLabel, encodingLabel, formatSize, imageKind, languageOf, lineCount, type Language } from "../model";

// What a file looks like on the NFS, Git and Baseline tabs: its lines (masked where the server masked them, failing closed when a
// mask cannot be checked: see components/masking/plan.ts), a note and a way forward for an empty file, the picture or the download
// for a binary one, and for a file whose content the server withholds only what the server does show (name, size, hash, when it
// changed and by whom). Everything from the file is data: SafeText.

const LANGUAGE: Record<Language, string> = { yaml: "YAML", xml: "XML", ini: "Properties", json: "JSON", plaintext: "Plain text" };
const HASH = /^[0-9a-f]{64}$/;

export type ContentViewProps = {
  content: FileContent;
  /** The file's name, for the labels. */
  name: string;
  /** Where the content was read from, as it reads in "is empty on NFS": "NFS", "Git", "the baseline". */
  where: string;
  highlightLine: number | null;
  /** The listing entry, for what a withheld file shows. */
  entry: FileEntry | null;
  /** The way forward from an empty file, when a comparison with Git can be made. */
  onCompareWithGit: (() => void) | null;
  /**
   * The swimlane whose NFS file this is. Given, a masked value that names its setting has a Reveal control (`revealValue` reads the
   * file on NFS, so only the NFS tab passes it). Absent, a masked value is a token and nothing else: the Git and Baseline tabs.
   */
  lane?: string;
};

function StatusBar({ content, name }: { content: FileContent; name: string }) {
  const lines = content.text === null ? 0 : lineCount(content.text);
  return (
    <div role="group" aria-label="File format" className="flex h-6 flex-none items-center gap-4 border-t border-border bg-muted px-3 font-mono text-xs text-muted-foreground">
      <span className="font-medium text-foreground">{eolLabel(content.format.eol)}</span>
      <span className="font-medium text-foreground">{encodingLabel(content.format.encoding, content.format.bom)}</span>
      <span>{LANGUAGE[languageOf(name)]}</span>
      <span>
        {lines.toLocaleString("en-US")} {lines === 1 ? "line" : "lines"}
      </span>
    </div>
  );
}

function BinaryView({ content, name }: { content: FileContent; name: string }) {
  const href = HASH.test(content.hash) ? getGetBlobUrl(content.hash) : null;
  const kind = imageKind(name);
  return (
    <div className="flex flex-col gap-4 p-6">
      {href !== null && kind !== null ? (
        <img src={href} alt={name} className="max-h-96 max-w-full self-start rounded-md border border-border bg-muted object-contain" />
      ) : (
        <EmptyState icon={FileImage} title="No preview for this kind of file." className="items-start px-0 py-2 text-left" />
      )}
      <div className="flex flex-wrap items-center gap-3 text-sm text-muted-foreground">
        {kind === null ? null : <span>{kind}</span>}
        <span>{formatSize(content.size)}</span>
        <HashChip hash={content.hash} />
        {href === null ? null : (
          <Button asChild variant="secondary" size="sm">
            <a href={href} download>
              Download
            </a>
          </Button>
        )}
      </div>
    </div>
  );
}

function Row({ term, children }: { term: string; children: ReactNode }) {
  return (
    <>
      <dt className="border-b border-border px-3 py-2 text-muted-foreground group-last:border-b-0">{term}</dt>
      <dd className="m-0 flex flex-wrap items-center gap-2 border-b border-border px-3 py-2 text-sm group-last:border-b-0">{children}</dd>
    </>
  );
}

function WithheldCard({ content, name, entry }: { content: FileContent; name: string; entry: FileEntry | null }) {
  const { nameOf } = useActorNames();
  const attribution = entry?.attribution ?? null;
  const observed = attribution?.evidence.observed_at ?? null;
  return (
    <div className="flex flex-1 items-start justify-center px-6 py-10">
      <dl className="grid w-[560px] max-w-full grid-cols-[140px_minmax(0,1fr)] overflow-hidden rounded-md border border-border bg-card">
        <Row term="Name">
          <SafeText text={name} className="font-mono text-xs" />
        </Row>
        <Row term="Size">{formatSize(content.size)}</Row>
        <Row term="Hash">
          {/* A fact about a file the page will not show: nothing in it is worth pasting elsewhere. */}
          <HashChip hash={content.hash} copy={false} />
        </Row>
        <Row term="Last changed">{observed === null ? "Not available" : formatTime(observed)}</Row>
        <Row term="Changed by">
          <AttributionChip attribution={attribution} nameOf={nameOf} emptyLabel="Not available" />
        </Row>
      </dl>
    </div>
  );
}

export function ContentView({ content, name, where, highlightLine, entry, onCompareWithGit, lane }: ContentViewProps) {
  if (isContentWithheld(content)) return <WithheldCard content={content} name={name} entry={entry} />;
  if (content.class === "binary" || content.text === null) return <BinaryView content={content} name={name} />;
  if (content.text === "") {
    return (
      <EmptyState
        title={`${name} is empty on ${where}`}
        action={
          onCompareWithGit === null ? undefined : (
            <Button variant="secondary" onClick={onCompareWithGit}>
              Compare with Git
            </Button>
          )
        }
      >
        The file exists but has 0 bytes. Compare it with Git, or restore the Git version.
      </EmptyState>
    );
  }
  const masks = content.masks ?? [];
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="min-h-0 flex-1 bg-card pt-1">
        {masks.length > 0 ? (
          lane !== undefined ? (
            <RevealableMaskedText swimlane={lane} path={content.path} label={name} text={content.text} masks={masks} highlightLine={highlightLine} className="h-full" />
          ) : (
            <MaskedTextView label={name} text={content.text} masks={masks} highlightLine={highlightLine} className="h-full" />
          )
        ) : (
          <CodeView label={name} text={content.text} highlightLine={highlightLine} className="h-full" />
        )}
      </div>
      <StatusBar content={content} name={name} />
    </div>
  );
}

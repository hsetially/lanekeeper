import { getGetBlobUrl } from "@/api/generated/endpoints/content-store/content-store";
import type { BinaryCompare as BinaryCompareData } from "@/api/generated/model";
import { HashChip } from "@/components/app/chips";
import { SafeText } from "@/components/app/SafeText";
import { StatusBadge } from "@/components/app/StatusBadge";
import { formatSize, imageKind } from "../model";

// Two versions of a binary file side by side (Diff, 1m). The pictures come from the blob store by hash (the same bytes
// forever, so the browser may keep them), and only an image the browser can show safely is drawn.

const HASH = /^[0-9a-f]{64}$/;

function Side({ name, label, hash, size }: { name: string; label: string; hash: string | null; size: number | null }) {
  const href = hash !== null && HASH.test(hash) ? getGetBlobUrl(hash) : null;
  const drawable = href !== null && imageKind(name) !== null;
  return (
    <div className="flex min-w-0 flex-col gap-2">
      <div className="flex items-center gap-2">
        <span className="font-semibold">
          <SafeText text={label} />
        </span>
        {hash === null ? null : <HashChip hash={hash} />}
        {size === null ? null : <span className="ml-auto text-xs text-muted-foreground">{formatSize(size)}</span>}
      </div>
      <div className="flex h-60 items-center justify-center overflow-hidden rounded-md border border-border bg-muted">
        {hash === null ? (
          <span className="font-mono text-xs text-muted-foreground">
            <SafeText text={`Not in ${label}`} />
          </span>
        ) : drawable ? (
          <img src={href} alt={`${name} from ${label}`} className="max-h-full max-w-full object-contain" />
        ) : (
          <span className="font-mono text-xs text-muted-foreground">No preview for this kind of file.</span>
        )}
      </div>
    </div>
  );
}

export function BinaryCompare({ name, leftLabel, rightLabel, binary }: { name: string; leftLabel: string; rightLabel: string; binary: BinaryCompareData }) {
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-4 overflow-auto px-6 py-5">
      <div className="flex items-center gap-2">
        <StatusBadge variant="nfs" label="Different content" />
        <span className="text-muted-foreground">Same name, different hash. Binary files compare by image, not by line.</span>
      </div>
      <div className="grid grid-cols-2 gap-4">
        <Side name={name} label={leftLabel} hash={binary.left} size={binary.left_size} />
        <Side name={name} label={rightLabel} hash={binary.right} size={binary.right_size} />
      </div>
    </div>
  );
}

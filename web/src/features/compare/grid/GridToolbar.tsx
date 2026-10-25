import type { ReactNode, Ref } from "react";
import { Button } from "@/components/ui/button";
import { SearchInput } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeSelect } from "@/components/ui/native-select";
import { Switch } from "@/components/ui/switch";
import { ChevronDown, Server } from "@/icons";
import { cn } from "@/lib/utils";
import { FilePicker } from "./FilePicker";

// The bar above the grid (the design's toolbar): the logical file, the swimlanes, "Only differing rows", the channel, the
// search over the rows that are loaded, and on the right what the grid is doing. While the rows load, or when the swimlanes
// are too many to show, the controls that need rows are off and the ones that fix the problem are not.

type Props = {
  file: string | null;
  /** The first two swimlanes: the picker lists the files that differ between them. */
  sides: readonly string[];
  filePickerOpen: boolean;
  onFilePickerOpen: (open: boolean) => void;
  onFile: (file: string) => void;
  laneCount: number;
  limit: number;
  onChooseLanes: () => void;
  onlyDiff: boolean;
  onOnlyDiff: (on: boolean) => void;
  channel: string | null;
  channels: readonly string[];
  onChannel: (channel: string | null) => void;
  q: string;
  onQ: (q: string) => void;
  searchRef: Ref<HTMLInputElement>;
  /** Controls that need rows. */
  rowsDisabled: boolean;
  status: ReactNode;
};

export function GridToolbar(props: Props) {
  const { file, sides, filePickerOpen, onFilePickerOpen, onFile, laneCount, limit, onChooseLanes, onlyDiff, onOnlyDiff, channel, channels, onChannel, q, onQ, searchRef, rowsDisabled, status } = props;
  const over = laneCount > limit;
  const options = channel !== null && !channels.includes(channel) ? [...channels, channel] : channels;
  return (
    <div role="group" aria-label="Grid options" className="relative z-[6] flex min-h-10 flex-none flex-wrap items-center gap-2 border-b border-border bg-muted px-2 py-1.5">
      <FilePicker file={file} sides={sides} open={filePickerOpen} onOpenChange={onFilePickerOpen} onChoose={onFile} />
      <Button variant="secondary" aria-haspopup="dialog" onClick={onChooseLanes} className={cn("bg-background", over && "border-danger-border bg-danger-bg text-danger-fg")}>
        <Server data-icon="inline-start" />
        {over ? `${laneCount} swimlanes · limit ${limit}` : `${laneCount} ${laneCount === 1 ? "swimlane" : "swimlanes"}`}
        <ChevronDown data-icon="inline-end" />
      </Button>
      <span aria-hidden="true" className="h-5 w-px bg-border" />
      <Label className="inline-flex cursor-pointer items-center gap-2 text-sm font-normal">
        <Switch checked={onlyDiff} onCheckedChange={onOnlyDiff} disabled={rowsDisabled} />
        Only differing rows
      </Label>
      <span className="inline-flex items-center gap-1">
        <label htmlFor="compare-channel" className="px-1 text-xs text-muted-foreground">
          Channel
        </label>
        <NativeSelect id="compare-channel" value={channel ?? ""} disabled={rowsDisabled} onChange={(event) => onChannel(event.target.value === "" ? null : event.target.value)} className="w-40 font-mono text-xs">
          <option value="">All channels</option>
          {options.map((name) => (
            <option key={name} value={name}>
              {name}
            </option>
          ))}
        </NativeSelect>
      </span>
      <SearchInput
        ref={searchRef}
        aria-label="Search settings"
        placeholder="Search settings in this file…"
        kbd="/"
        value={q}
        disabled={rowsDisabled}
        onChange={(event) => onQ(event.target.value)}
        boxClassName="w-60 flex-none bg-background"
        className="font-mono text-xs"
      />
      {/* The count's width changes as pages arrive ("50+" to "550+", "Reading…" to a number). If the bar wrapped by what it said, it would grow
          a line under the rows being read and push the grid down. So the box is as wide as it asks to be (14rem) whatever it says, takes the
          room that is left, and wraps inside itself, two lines at most, in the height of a button. */}
      <span className="ml-auto flex h-7 min-w-[14rem] flex-1 basis-0 items-center justify-end gap-1.5 text-right text-xs leading-[14px] text-muted-foreground tabular-nums">{status}</span>
    </div>
  );
}

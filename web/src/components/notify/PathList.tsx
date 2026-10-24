import { useId } from "react";
import { SafeText } from "@/components/app/SafeText";
import { FOCUSABLE_SCROLLER } from "@/components/app/scroll-region";

// The files a notification is about to announce. A request holds at most 100, so the list is drawn whole: the person confirms
// exactly what will be sent. A path is untrusted text (it came from the file system), so it goes through SafeText. `label` is
// what the list is, with its count ("2 files to announce"): it is shown above the list and it names the region.

export function PathList({ label, paths }: { label: string; paths: readonly string[] }) {
  const id = useId();
  if (paths.length === 0) return null;
  return (
    <div className="flex flex-col gap-1">
      <p id={id} className="text-xs font-medium text-muted-foreground">
        {label}
      </p>
      <div role="region" aria-labelledby={id} {...FOCUSABLE_SCROLLER} className="max-h-32 overflow-auto rounded-md border border-border bg-muted px-2 py-1.5">
        <ul className="flex flex-col gap-0.5">
          {paths.map((path) => (
            <li key={path} className="font-mono text-xs break-all">
              <SafeText text={path} />
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}

import { Banner } from "@/components/app/Banner";
import { SafeText } from "@/components/app/SafeText";
import { FOCUSABLE_SCROLLER } from "@/components/app/scroll-region";
import { placeholdersShown } from "@/features/served/model";

// The placeholders that stayed in a served text: `${KEY}` where the config-server's property view had no such key, so it left the
// placeholder as written. The engine finds them in the text it was given (`unresolved_placeholders`, at most 10,000 keys), so the
// list says WHICH, and the text shows WHERE. A key is untrusted text (it comes from a file) and goes through SafeText.
//
// The names are in a box of their own that holds a few lines and scrolls (a keyboard reaches it): twelve long keys on lines of their own
// took 300 px at 1024 wide, and with the sources under the text left the text no room at all.

/** The keys named in the banner; the rest are counted. */
export const MAX_NAMED = 12;

type Props = { placeholders: readonly string[] };

export function UnresolvedPlaceholders({ placeholders }: Props) {
  const { shown, more } = placeholdersShown(placeholders, MAX_NAMED);
  const total = shown.length + more;
  if (total === 0) return null;
  return (
    <Banner variant="warning" title={`${total.toLocaleString("en-US")} ${total === 1 ? "placeholder" : "placeholders"} left as written`}>
      <div role="region" aria-label="Placeholders left as written" {...FOCUSABLE_SCROLLER} className="flex max-h-24 flex-wrap items-center gap-x-2 gap-y-1 overflow-y-auto">
        <span>The config-server had no value for</span>
        {shown.map((key) => (
          <span key={key} data-slot="placeholder" className="font-mono text-xs">
            <SafeText text={`\${${key}}`} />
          </span>
        ))}
        {more > 0 ? <span>{`and ${more.toLocaleString("en-US")} more`}</span> : null}
      </div>
    </Banner>
  );
}
